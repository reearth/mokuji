// A Mokuji writer for browsers, workers and Node: the Rust crate's
// `mokuji::write` and `mokuji::meta`, with no dependencies.
//
// SPEC.md §3.4 asks a writer for a byte-identical file from identical input
// and parameters. This one is held to more than that: stored raw, its output
// is the Rust writer's byte for byte, and the tests rebuild the conformance
// fixtures to show it. Compressed, it is not, and cannot be: it compresses
// with the runtime's own CompressionStream, so that a page needs no
// compressor of its own, and no specification of deflate fixes which of the
// valid encodings a compressor emits. That is what `generator.name` and
// `generator.version` in the metadata are for: a rebuild is checkable against
// the writer that made it — and, here, on the runtime that ran it.
import { SPLITTERS } from "./tokenizers.js";
/** What this writer calls itself in `generator.name`. */
export const GENERATOR_NAME = "mokuji-js";
/**
 * What this package records as `generator.version`. A release sets it with
 * the other packages' versions (RELEASING.md), and publishing checks that they
 * agree.
 */
export const VERSION = "0.1.1";
/** The first section identifier an application may use (SPEC.md §5.2). */
export const SECTION_APPLICATION_MIN = 0x8000;
const SPEC_MINOR = 1;
const HEADER_LEN = 64;
const SECTION_ENTRY_LEN = 24;
const FLAG_HAS_PAYLOAD = 1 << 1;
const FLAG_HAS_FREQS = 1 << 2;
const FLAG_HAS_POSITIONS = 1 << 3;
const INLINE = 1;
const CODEC_RAW = 0;
const CODEC_DEFLATE = 1;
const SECTION = {
    META: 0x0001,
    TERM_INDEX: 0x0010,
    TERM_BLOCKS: 0x0011,
    TERM_BLOCK_OFFSETS: 0x0012,
    COMPLETIONS: 0x0013,
    POSTINGS: 0x0020,
    PAYLOAD: 0x0030,
    PAYLOAD_OFFSETS: 0x0031,
    DOC_LENGTHS: 0x0032,
    DOC_ATTRIBUTES: 0x0033,
    POSITIONS: 0x0060,
};
function resolve(options) {
    return {
        termsPerBlock: options.termsPerBlock ?? 1024,
        docsPerPayloadBlock: options.docsPerPayloadBlock ?? 64,
        hasFreqs: options.hasFreqs ?? true,
        ints: options.ints ?? null,
        positions: options.positions ?? false,
        docLengths: options.docLengths ?? false,
        compress: options.compress ?? true,
        completions: options.completions
            ? { minTerms: 0, maxSpan: 0, ...options.completions }
            : null,
        inlinePostings: options.inlinePostings ?? 8,
    };
}
/**
 * The options as `mokuji build` records them in `generator.params`, and
 * whether the build compressed. A caller with parameters of its own, such as
 * how the documents were ordered, adds them.
 */
export function writeParams(options = {}) {
    const o = resolve(options);
    const params = {
        terms_per_block: o.termsPerBlock,
        docs_per_payload_block: o.docsPerPayloadBlock,
        has_freqs: o.hasFreqs ? 1 : 0,
        payload: o.ints ? `ints:${-o.ints.scaleExponent}` : "opaque",
        doc_lengths: o.docLengths ? 1 : 0,
        positions: o.positions ? 1 : 0,
        inline_postings: o.inlinePostings,
        compress: o.compress ? 1 : 0,
    };
    const c = o.completions;
    if (c)
        params.completions = `${c.minTerms},${c.maxSpan},${c.docsPerEntry} by document order`;
    return params;
}
// --- encoders ---------------------------------------------------------------
const encoder = new TextEncoder();
const MASK64 = (1n << 64n) - 1n;
const bytesOf = (value) => typeof value === "string" ? encoder.encode(value) : value;
/** Bytes, growing as they are appended to. */
class Out {
    bytes = new Uint8Array(64);
    length = 0;
    reserve(n) {
        if (this.length + n <= this.bytes.length)
            return;
        let size = this.bytes.length * 2;
        while (size < this.length + n)
            size *= 2;
        const grown = new Uint8Array(size);
        grown.set(this.bytes.subarray(0, this.length));
        this.bytes = grown;
    }
    byte(b) {
        this.reserve(1);
        this.bytes[this.length++] = b;
    }
    push(bytes) {
        this.reserve(bytes.length);
        this.bytes.set(bytes, this.length);
        this.length += bytes.length;
    }
    varint(value) {
        if (typeof value === "number" && value < 2 ** 31) {
            let v = value;
            while (v >= 0x80) {
                this.byte((v & 0x7f) | 0x80);
                v >>>= 7;
            }
            this.byte(v);
            return;
        }
        let v = BigInt(value);
        while (v >= 0x80n) {
            this.byte(Number(v & 0x7fn) | 0x80);
            v >>= 7n;
        }
        this.byte(Number(v));
    }
    u32(v) {
        const b = new Uint8Array(4);
        new DataView(b.buffer).setUint32(0, v, true);
        this.push(b);
    }
    u64(v) {
        const b = new Uint8Array(8);
        new DataView(b.buffer).setBigUint64(0, BigInt(v), true);
        this.push(b);
    }
    /** Appends `key`, front-coded against `previous` (SPEC.md §5.3.1). */
    key(key, previous) {
        let shared = 0;
        while (shared < key.length && shared < previous.length && key[shared] === previous[shared])
            shared++;
        this.varint(shared);
        this.varint(key.length - shared);
        this.push(key.subarray(shared));
    }
    /** Appends values in bit-packed groups of 128 (SPEC.md §5.4.1). */
    values(values) {
        for (let start = 0; start < values.length; start += 128) {
            const group = values.slice(start, start + 128).map(BigInt);
            let width = 0;
            for (const v of group)
                width = Math.max(width, v === 0n ? 0 : v.toString(2).length);
            this.byte(width);
            if (width === 0)
                continue;
            const packed = new Uint8Array(Math.ceil((group.length * width) / 8));
            group.forEach((v, i) => {
                for (let j = 0; j < width; j++) {
                    if ((v >> BigInt(j)) & 1n) {
                        const bit = i * width + j;
                        packed[bit >> 3] |= 1 << (bit & 7);
                    }
                }
            });
            this.push(packed);
        }
    }
    done() {
        return this.bytes.slice(0, this.length);
    }
}
/** An i64 as the unsigned integer it zigzags to. */
const zigzag = (v) => BigInt.asUintN(64, (v << 1n) ^ (v >> 63n));
function compareBytes(a, b) {
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) {
        if (a[i] !== b[i])
            return a[i] - b[i];
    }
    return a.length - b.length;
}
function startsWith(key, prefix) {
    return key.length >= prefix.length && compareBytes(key.subarray(0, prefix.length), prefix) === 0;
}
/** The deflate codec: the length, then raw deflate from the runtime's CompressionStream. */
async function compress(unit) {
    const out = new Out();
    out.varint(unit.length);
    const stream = new Blob([unit]).stream().pipeThrough(new CompressionStream("deflate-raw"));
    out.push(new Uint8Array(await new Response(stream).arrayBuffer()));
    return out.done();
}
function cborHead(out, major, value) {
    const v = BigInt(value);
    const m = major << 5;
    if (v < 24n)
        out.byte(m | Number(v));
    else if (v <= 0xffn)
        out.push(Uint8Array.of(m | 24, Number(v)));
    else if (v <= 0xffffn)
        out.push(Uint8Array.of(m | 25, Number(v >> 8n), Number(v & 0xffn)));
    else {
        const size = v <= 0xffffffffn ? 4 : 8;
        out.byte(m | (size === 4 ? 26 : 27));
        for (let shift = BigInt((size - 1) * 8); shift >= 0n; shift -= 8n)
            out.byte(Number((v >> shift) & 0xffn));
    }
}
/**
 * One value as canonical CBOR (RFC 8949 §4.2): definite lengths, shortest-form
 * integers, and map keys sorted by their encoded bytes.
 */
export function encodeCbor(value) {
    const out = new Out();
    writeCbor(value, out);
    return out.done();
}
function writeCbor(value, out) {
    if (typeof value === "boolean")
        value = value ? 1 : 0;
    if (typeof value === "number" || typeof value === "bigint") {
        if (typeof value === "number" && !Number.isSafeInteger(value)) {
            throw new Error("metadata holds only integers, and a number past 2^53 as a bigint");
        }
        const v = BigInt(value);
        if (v < 0n || v > MASK64)
            throw new Error("metadata holds only non-negative integers of 64 bits");
        cborHead(out, 0, v);
    }
    else if (typeof value === "string") {
        const raw = encoder.encode(value);
        cborHead(out, 3, raw.length);
        out.push(raw);
    }
    else if (Array.isArray(value)) {
        cborHead(out, 4, value.length);
        for (const item of value)
            writeCbor(item, out);
    }
    else if (value !== null && typeof value === "object" && Object.getPrototypeOf(value) === Object.prototype) {
        const pairs = Object.entries(value).map(([k, v]) => [encodeCbor(k), encodeCbor(v)]);
        pairs.sort((a, b) => compareBytes(a[0], b[0]));
        cborHead(out, 5, pairs.length);
        for (const [k, v] of pairs) {
            out.push(k);
            out.push(v);
        }
    }
    else {
        throw new Error(`metadata cannot hold ${String(value)}`);
    }
}
/** The metadata section, as canonical CBOR. */
export function encodeMetadata(meta) {
    const root = {
        generation: meta.generation ?? 1,
        source: meta.sources.map((s) => {
            if (!s.contentHash)
                throw new Error("a source has no content hash");
            const entry = { content_hash: s.contentHash };
            if (s.uri)
                entry.uri = s.uri;
            if (s.retrieved)
                entry.retrieved = s.retrieved;
            if (s.modified)
                entry.modified = s.modified;
            return entry;
        }),
        generator: {
            name: meta.generatorName ?? GENERATOR_NAME,
            version: meta.generatorVersion ?? VERSION,
            params: meta.generatorParams ?? {},
        },
        tokenizer: { id: meta.tokenizerId, params: meta.tokenizerParams ?? {} },
    };
    if (meta.attrs != null)
        root.attrs = meta.attrs;
    if (meta.license != null)
        root.license = meta.license;
    if (meta.attribution != null)
        root.attribution = meta.attribution;
    return encodeCbor(root);
}
/**
 * The terms a document's text becomes under a tokenizer from TOKENIZERS.md,
 * as a writer builds them. A key tokenizer, `identity` or `ja-address`, makes
 * the whole text one term.
 */
export function split(tokenizerId, text) {
    const splitter = SPLITTERS[tokenizerId];
    if (!splitter)
        throw new Error(`unknown tokenizer '${tokenizerId}'`);
    return splitter(text);
}
const EMPTY = new Uint8Array(0);
/**
 * Builds a Mokuji file from documents and a sorted stream of postings.
 *
 * Documents are added first, in document ID order; postings are then pushed
 * in term order (by UTF-8 bytes, which is not JavaScript's string order) and
 * within a term in document order. Both orderings are checked rather than
 * assumed. Sorting stays with the caller.
 */
export class MokujiBuilder {
    options;
    payloads = [];
    rows = [];
    columns = [];
    sumDocLen = 0;
    lengths = [];
    terms = [];
    open = null;
    sections = new Map();
    constructor(options = {}) {
        const o = resolve(options);
        if (!(o.termsPerBlock >= 1) || !(o.docsPerPayloadBlock >= 1))
            throw new Error("block sizes must be non-zero");
        // A positions run says nothing about where one document's positions end
        // and the next begin; the frequency is that boundary.
        if (o.positions && !o.hasFreqs) {
            throw new Error("positions need frequencies, which say how many belong to each document");
        }
        if (o.inlinePostings < 0)
            throw new Error("an inline postings bound cannot be negative");
        if (o.ints) {
            const { valuesPerDoc, scaleExponent } = o.ints;
            if (!(valuesPerDoc >= 0 && valuesPerDoc <= 255 && scaleExponent >= -128 && scaleExponent <= 127)) {
                throw new Error("ints are a byte of values per document and a signed byte of scale");
            }
        }
        const c = o.completions;
        if (c) {
            if (!(c.docsPerEntry >= 1))
                throw new Error("a completion entry lists at least one document");
            // Both zero would be a table with no rule for what goes in it.
            if (c.minTerms === 0 && c.maxSpan === 0) {
                throw new Error("completions need a term count or a postings size to cover");
            }
        }
        this.options = o;
    }
    /** How many documents have been added. */
    get documents() {
        return this.options.ints ? this.rows.length : this.payloads.length;
    }
    /** Appends a document whose payload is opaque bytes, or text written as UTF-8, returning its ID. */
    addDocument(payload) {
        if (this.options.ints)
            throw new Error("this index encodes payload as integers");
        this.payloads.push(typeof payload === "string" ? encoder.encode(payload) : payload.slice());
        return this.documents - 1;
    }
    /** Appends a document whose payload is a fixed row of integers, returning its ID. */
    addDocumentValues(values) {
        if (!this.options.ints)
            throw new Error("this index encodes payload as opaque bytes");
        if (values.length !== this.options.ints.valuesPerDoc) {
            throw new Error("a document carries the wrong number of values");
        }
        this.rows.push(values.map(toI64));
        return this.documents - 1;
    }
    /** Adds a column of per-document numbers (SPEC.md §5.8), checked at `finish` against the document count. */
    addColumn(column) {
        if (!(column.scaleExponent >= -128 && column.scaleExponent <= 127)) {
            throw new Error("a column's scale exponent does not fit a signed byte");
        }
        if (this.columns.some((c) => c.name === column.name)) {
            throw new Error("two document attribute columns share a name");
        }
        column.values.forEach(toI64);
        this.columns.push({ ...column, values: column.values.slice() });
    }
    /**
     * Records that `term` occurs `freq` times in `doc`. Terms ascend by their
     * UTF-8 bytes; within a term, documents ascend.
     */
    addPosting(term, doc, freq = 1) {
        const key = bytesOf(term);
        if (!(Number.isSafeInteger(freq) && freq >= 1))
            throw new Error("a posting with zero frequency is not a posting");
        if (!(Number.isInteger(doc) && doc >= 0 && doc < this.documents)) {
            throw new Error("posting refers to a document that was never added");
        }
        const open = this.open;
        const order = open ? compareBytes(key, open.term) : 1;
        if (open && order === 0) {
            if (doc <= open.docs[open.docs.length - 1])
                throw new Error("documents within a term are not ascending");
            open.docs.push(doc);
            open.freqs.push(freq);
            open.positions.push(null);
        }
        else if (open && order < 0) {
            throw new Error("terms are not ascending");
        }
        else {
            this.closeTerm();
            this.open = { term: key.slice(), docs: [doc], freqs: [freq], positions: [null] };
        }
        this.sumDocLen += freq;
        if (this.options.docLengths) {
            while (this.lengths.length <= doc)
                this.lengths.push(0);
            this.lengths[doc] += freq;
        }
    }
    /** Records that `term` occurs in `doc` at exactly these ascending positions. */
    addPostingAt(term, doc, positions) {
        if (positions.length === 0)
            throw new Error("a posting with no positions is not a posting");
        for (let i = 1; i < positions.length; i++) {
            if (positions[i] <= positions[i - 1])
                throw new Error("positions within a document are not ascending");
        }
        this.addPosting(term, doc, positions.length);
        this.open.positions[this.open.positions.length - 1] = positions.slice();
    }
    /**
     * Attaches an application-defined section (SPEC.md §5.2), stored
     * uncompressed and placed last, so that it moves nothing a reader looks for.
     */
    addSection(id, bytes) {
        if (!(Number.isInteger(id) && id >= SECTION_APPLICATION_MIN && id <= 0xffff)) {
            throw new Error("a section this specification defines cannot be supplied by an application");
        }
        this.sections.set(id, bytes.slice());
    }
    closeTerm() {
        const open = this.open;
        if (!open)
            return;
        this.open = null;
        const { docs, freqs } = open;
        // Positions, in the same document order the postings run uses.
        const positions = new Out();
        if (this.options.positions) {
            open.positions.forEach((row, i) => {
                const values = (row ?? []).map((at, j, all) => (j === 0 ? at : at - all[j - 1] - 1));
                // A posting recorded without positions still needs its share of the
                // run, or the frequencies stop lining up.
                while (values.length < freqs[i])
                    values.push(0);
                positions.values(values);
            });
        }
        const c = this.options.completions;
        const head = c ? docs.slice(0, c.docsPerEntry) : [];
        // The dictionary can state this term's postings by itself: it names one
        // document, whose ID is the ID this term is about to be given, once
        // (SPEC.md §5.3.4).
        const termId = this.terms.length;
        if (docs.length === 1 && docs[0] === termId && freqs[0] === 1) {
            this.terms.push({ term: open.term, docFreq: 1, run: EMPTY, inline: null, short: null, positions: positions.done(), head });
            return;
        }
        const run = new Out();
        run.byte(0); // codec 0
        run.varint(docs.length);
        // The first value is a document ID and the rest are gaps less one, so
        // that consecutive documents become zeros and pack to nothing.
        run.values(docs.map((d, i) => (i === 0 ? d : d - docs[i - 1] - 1)));
        if (this.options.hasFreqs)
            run.values(freqs.map((f) => f - 1));
        // Every inline value costs at least a byte, so a term with more documents
        // than the bound allows bytes can never be inlined.
        this.terms.push({
            term: open.term,
            docFreq: docs.length,
            run: run.done(),
            inline: null,
            short: docs.length <= this.options.inlinePostings ? { docs, freqs } : null,
            positions: positions.done(),
            head,
        });
    }
    /**
     * Moves short postings into their dictionary entries (SPEC.md §5.3.4), once
     * every term is known: a term's first document is written against the
     * entries before it in its block.
     */
    inlinePostings() {
        const bound = this.options.inlinePostings;
        const per = this.options.termsPerBlock;
        for (let start = 0; start < this.terms.length; start += per) {
            let expected = start;
            for (let i = start; i < Math.min(start + per, this.terms.length); i++) {
                const term = this.terms[i];
                const short = term.short;
                if (!short) {
                    // Implied: the document is the term's own ID.
                    if (term.run.length === 0)
                        expected = i + 1;
                    continue;
                }
                term.short = null;
                if (bound === 0)
                    continue;
                const inline = new Out();
                inline.varint(zigzag(BigInt(short.docs[0] - expected)));
                for (let j = 1; j < short.docs.length; j++)
                    inline.varint(short.docs[j] - short.docs[j - 1] - 1);
                if (this.options.hasFreqs)
                    for (const f of short.freqs)
                        inline.varint(f - 1);
                if (inline.length <= bound && inline.length <= term.run.length) {
                    expected = short.docs[short.docs.length - 1] + 1;
                    term.inline = inline.done();
                    term.run = EMPTY;
                }
            }
        }
    }
    /** Compresses each unit, or leaves them all raw if that is no smaller; decided per section. */
    async pack(units) {
        if (!this.options.compress)
            return [units, CODEC_RAW];
        const packed = await Promise.all(units.map(compress));
        const size = (list) => list.reduce((n, u) => n + u.length, 0);
        return size(packed) < size(units) ? [packed, CODEC_DEFLATE] : [units, CODEC_RAW];
    }
    /**
     * The file, with `meta` as the metadata section: canonical CBOR, such as
     * `encodeMetadata` makes. It carries the provenance of a source the writer
     * never sees, so composing it is the caller's job.
     */
    async finish(meta) {
        this.closeTerm();
        if (this.terms.length === 0)
            throw new Error("an index with no terms cannot be built");
        this.inlinePostings();
        const o = this.options;
        const chunks = [];
        for (let i = 0; i < this.terms.length; i += o.termsPerBlock)
            chunks.push(this.terms.slice(i, i + o.termsPerBlock));
        // Payload blocks. Deltas never cross a block boundary: a block is
        // addressed on its own and must decode on its own.
        const rawPayload = [];
        for (let start = 0; start < this.documents; start += o.docsPerPayloadBlock) {
            const end = Math.min(start + o.docsPerPayloadBlock, this.documents);
            const block = new Out();
            block.varint(end - start);
            for (let doc = start; doc < end; doc++) {
                if (!o.ints) {
                    block.varint(this.payloads[doc].length);
                    block.push(this.payloads[doc]);
                    continue;
                }
                this.rows[doc].forEach((v, i) => {
                    const delta = doc > start ? BigInt.asIntN(64, v - this.rows[doc - 1][i]) : v;
                    block.varint(zigzag(delta));
                });
            }
            rawPayload.push(block.done());
        }
        const [payloadBlocks, payloadCodec] = await this.pack(rawPayload);
        const index = new Out();
        index.u32(chunks.length);
        let previous = EMPTY;
        for (const chunk of chunks) {
            index.key(chunk[0].term, previous);
            previous = chunk[0].term;
        }
        const [[termIndex], termIndexCodec] = await this.pack([index.done()]);
        // Like the term index, completions depend on the terms and their runs and
        // on no offset, so they can be sized before anything is placed.
        let completions = null;
        let completionsCodec = CODEC_RAW;
        if (o.completions) {
            const [[packed], codec] = await this.pack([completionsSection(this.terms, o.completions)]);
            completions = packed;
            completionsCodec = codec;
        }
        let docLengths = null;
        if (o.docLengths) {
            const out = new Out();
            out.values(Array.from({ length: this.documents }, (_, i) => this.lengths[i] ?? 0));
            docLengths = out.done();
        }
        let attributes = null;
        if (this.columns.length > 0) {
            const out = new Out();
            out.varint(this.columns.length);
            for (const column of this.columns) {
                if (column.values.length !== this.documents) {
                    throw new Error("a document attribute column does not have one value per document");
                }
                const name = encoder.encode(column.name);
                out.varint(name.length);
                out.push(name);
                out.byte(column.scaleExponent & 0xff);
            }
            for (const column of this.columns)
                out.values(column.values.map((v) => zigzag(toI64(v))));
            attributes = out.done();
        }
        // The order the sections are written in. The resident ones go first, so
        // that a cold start fetches them in one read; postings and positions go
        // before the term blocks that point into them, so that an offset is known
        // before any block is built.
        const order = [SECTION.META, SECTION.TERM_INDEX];
        if (completions)
            order.push(SECTION.COMPLETIONS);
        order.push(SECTION.TERM_BLOCK_OFFSETS, SECTION.PAYLOAD_OFFSETS);
        if (docLengths)
            order.push(SECTION.DOC_LENGTHS);
        if (attributes)
            order.push(SECTION.DOC_ATTRIBUTES);
        order.push(SECTION.POSTINGS);
        if (o.positions)
            order.push(SECTION.POSITIONS);
        order.push(SECTION.TERM_BLOCKS, SECTION.PAYLOAD);
        // Application sections go last, behind everything the format defines.
        order.push(...[...this.sections.keys()].sort((a, b) => a - b));
        const sum = (list) => list.reduce((n, u) => n + u.length, 0);
        const tableLen = SECTION_ENTRY_LEN * order.length;
        const postingsLen = sum(this.terms.map((t) => t.run));
        const positionsLen = sum(this.terms.map((t) => t.positions));
        const termBlockOffsetsLen = 8 * (chunks.length + 1);
        const payloadOffsetsLen = 12 + 8 * (payloadBlocks.length + 1);
        const postingsAt = HEADER_LEN + tableLen + meta.length + termIndex.length + (completions?.length ?? 0) +
            termBlockOffsetsLen + payloadOffsetsLen + (docLengths?.length ?? 0) + (attributes?.length ?? 0);
        const [blocks, termBlocksCodec] = await this.pack(this.termBlocks(chunks, postingsAt, postingsAt + postingsLen));
        const lengthOf = new Map([
            [SECTION.META, meta.length],
            [SECTION.TERM_INDEX, termIndex.length],
            [SECTION.COMPLETIONS, completions?.length ?? 0],
            [SECTION.TERM_BLOCK_OFFSETS, termBlockOffsetsLen],
            [SECTION.PAYLOAD_OFFSETS, payloadOffsetsLen],
            [SECTION.DOC_LENGTHS, docLengths?.length ?? 0],
            [SECTION.DOC_ATTRIBUTES, attributes?.length ?? 0],
            [SECTION.POSTINGS, postingsLen],
            [SECTION.POSITIONS, positionsLen],
            [SECTION.TERM_BLOCKS, sum(blocks)],
            [SECTION.PAYLOAD, sum(payloadBlocks)],
        ]);
        for (const [id, data] of this.sections)
            lengthOf.set(id, data.length);
        const offsetOf = new Map();
        let cursor = HEADER_LEN + tableLen;
        for (const id of order) {
            offsetOf.set(id, cursor);
            cursor += lengthOf.get(id);
        }
        // Metadata stays readable without a decompressor, and the offset tables
        // stay raw because their size feeds back into the placement they describe.
        const codecOf = new Map([
            [SECTION.TERM_INDEX, termIndexCodec],
            [SECTION.COMPLETIONS, completionsCodec],
            [SECTION.TERM_BLOCKS, termBlocksCodec],
            [SECTION.PAYLOAD, payloadCodec],
        ]);
        const out = new Out();
        out.push(encoder.encode("MOKUJI\u0000\u0000"));
        out.byte(0);
        out.byte(SPEC_MINOR);
        let flags = FLAG_HAS_PAYLOAD;
        if (o.hasFreqs)
            flags |= FLAG_HAS_FREQS;
        if (o.positions)
            flags |= FLAG_HAS_POSITIONS;
        out.push(Uint8Array.of(flags & 0xff, flags >> 8));
        out.u32(order.length);
        out.u64(this.documents);
        out.u64(this.sumDocLen);
        out.u64(HEADER_LEN);
        out.u32(tableLen);
        out.push(new Uint8Array(HEADER_LEN - out.length));
        // The table is sorted by identifier, which is not the order the sections
        // are written in.
        for (const id of [...order].sort((a, b) => a - b)) {
            out.push(Uint8Array.of(id & 0xff, id >> 8, codecOf.get(id) ?? CODEC_RAW, 0, 0, 0, 0, 0));
            out.u64(offsetOf.get(id));
            out.u64(lengthOf.get(id));
        }
        for (const id of order) {
            switch (id) {
                case SECTION.META:
                    out.push(meta);
                    break;
                case SECTION.TERM_INDEX:
                    out.push(termIndex);
                    break;
                case SECTION.COMPLETIONS:
                    out.push(completions);
                    break;
                case SECTION.TERM_BLOCK_OFFSETS:
                    offsetTable(out, offsetOf.get(SECTION.TERM_BLOCKS), blocks);
                    break;
                case SECTION.PAYLOAD_OFFSETS: {
                    const ints = o.ints;
                    out.push(Uint8Array.of(ints ? 1 : 0, ints?.valuesPerDoc ?? 0, (ints?.scaleExponent ?? 0) & 0xff, 0));
                    out.u32(o.docsPerPayloadBlock);
                    out.u32(payloadBlocks.length);
                    offsetTable(out, offsetOf.get(SECTION.PAYLOAD), payloadBlocks);
                    break;
                }
                case SECTION.DOC_LENGTHS:
                    out.push(docLengths);
                    break;
                case SECTION.DOC_ATTRIBUTES:
                    out.push(attributes);
                    break;
                case SECTION.POSTINGS:
                    for (const t of this.terms)
                        out.push(t.run);
                    break;
                case SECTION.POSITIONS:
                    for (const t of this.terms)
                        out.push(t.positions);
                    break;
                case SECTION.TERM_BLOCKS:
                    for (const b of blocks)
                        out.push(b);
                    break;
                case SECTION.PAYLOAD:
                    for (const b of payloadBlocks)
                        out.push(b);
                    break;
                default:
                    out.push(this.sections.get(id));
            }
        }
        return out.done();
    }
    termBlocks(chunks, postingsAt, positionsAt) {
        let termId = 0;
        return chunks.map((chunk) => {
            const block = new Out();
            block.varint(chunk.length);
            block.varint(termId);
            let previous = EMPTY;
            chunk.forEach((term, i) => {
                block.key(term.term, previous);
                block.varint(term.docFreq);
                // The first entry's offsets are absolute; the rest are the distance
                // from the entry before, which is that entry's length.
                block.varint(i === 0 ? postingsAt : chunk[i - 1].run.length);
                if (term.inline) {
                    block.varint(INLINE);
                    block.push(term.inline);
                }
                else {
                    // Zero for implied postings, which have no run.
                    block.varint(term.run.length);
                }
                if (this.options.positions) {
                    block.varint(i === 0 ? positionsAt : chunk[i - 1].positions.length);
                    block.varint(term.positions.length);
                    positionsAt += term.positions.length;
                }
                previous = term.term;
                postingsAt += term.run.length;
            });
            termId += chunk.length;
            return block.done();
        });
    }
}
/** An integer as an i64, checked: a number must be safe, a bigint within 64 bits. */
function toI64(value) {
    if (typeof value === "number" && !Number.isSafeInteger(value)) {
        throw new Error("a value is not an integer; one past 2^53 is given as a bigint");
    }
    const v = BigInt(value);
    if (BigInt.asIntN(64, v) !== v)
        throw new Error("a value does not fit 64 bits");
    return v;
}
function offsetTable(out, base, blocks) {
    let cursor = base;
    for (const block of blocks) {
        out.u64(cursor);
        cursor += block.length;
    }
    out.u64(cursor);
}
/**
 * The completions section, uncompressed (SPEC.md §5.3.5).
 *
 * Every prefix of every term is a candidate, and most are cheap, so this does
 * not enumerate them. It walks down from one byte: the terms sharing a prefix
 * are a contiguous run of the sorted dictionary, a prefix that is not
 * expensive has no expensive extension, and so only the runs that were
 * expensive at one length are split at the next.
 */
function completionsSection(terms, c) {
    const entries = [];
    const pending = [[0, terms.length, 1]];
    while (pending.length > 0) {
        const [start, end, length] = pending.pop();
        // Split the run by the byte at length - 1. A term too short to have one
        // is the shared prefix itself, and sorts first.
        const groups = [];
        let at = start;
        while (at < end && terms[at].term.length < length)
            at++;
        while (at < end) {
            const key = terms[at].term.subarray(0, length);
            let next = at + 1;
            while (next < end && startsWith(terms[next].term, key))
                next++;
            groups.push([at, next]);
            at = next;
        }
        for (const [from, to] of groups) {
            const span = terms.slice(from, to).reduce((n, t) => n + t.run.length, 0);
            const expensive = (c.minTerms !== 0 && to - from >= c.minTerms) || (c.maxSpan !== 0 && span >= c.maxSpan);
            if (!expensive)
                continue;
            pending.push([from, to, length + 1]);
            const best = [...new Set(terms.slice(from, to).flatMap((t) => t.head))].sort((a, b) => a - b);
            entries.push([terms[from].term.subarray(0, length), best.slice(0, c.docsPerEntry)]);
        }
    }
    // The walk visits prefixes depth-first from a stack; the section wants them
    // in byte order.
    entries.sort((a, b) => compareBytes(a[0], b[0]));
    const out = new Out();
    out.varint(c.minTerms);
    out.varint(c.maxSpan);
    out.varint(c.docsPerEntry);
    out.varint(entries.length);
    let previous = EMPTY;
    for (const [prefix, docs] of entries) {
        out.key(prefix, previous);
        previous = prefix;
        out.varint(docs.length);
        let last = 0;
        for (const doc of docs) {
            out.varint(zigzag(BigInt(doc - last)));
            last = doc;
        }
    }
    return out.done();
}
