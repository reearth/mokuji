// A Mokuji reader for browsers, workers and Node, with no dependencies.
//
// This is the third implementation of SPEC.md, after the Rust one and the
// Python fixture builder, and the first written against a transport the format
// was designed for rather than a file already in memory. If it reads what the
// others wrote, the specification survives contact with a third reader.
//
// It bundles no decompressor. The required codec is raw deflate precisely so
// that a page can use DecompressionStream and stay this small.
import { SPLITTERS, TOKENIZERS } from "./tokenizers.js";
const MAGIC = "MOKUJIv0";
const SECTION_ENTRY_LEN = 24;
const FLAG_HAS_PAYLOAD = 1 << 1;
const FLAG_HAS_FREQS = 1 << 2;
const FLAG_HAS_POSITIONS = 1 << 3;
// A dictionary entry's `postings_len` when it states its postings itself
// (SPEC.md §5.3.4). No run is this short.
const IMPLIED = 0;
const INLINE = 1;
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
const CODEC_RAW = 0;
const CODEC_DEFLATE = 1;
const PAYLOAD_OPAQUE = 0;
const PAYLOAD_DELTA_INTS = 1;
/** Whether `source` is a Blob, in a runtime that has them (every current one). */
function isBlob(source) {
    return typeof Blob !== "undefined" && source instanceof Blob;
}
// --- primitives -----------------------------------------------------------
function unzigzag(v) {
    return v % 2 === 0 ? v / 2 : -(v + 1) / 2;
}
/** Writes `value * 10^exponent` in full, keeping the zeros the file claims. */
function scaled(value, exponent) {
    if (exponent >= 0)
        return String(value) + "0".repeat(exponent);
    const digits = -exponent;
    const unit = 10 ** digits;
    const sign = value < 0 ? "-" : "";
    const magnitude = Math.abs(value);
    const whole = Math.floor(magnitude / unit);
    return `${sign}${whole}.${String(magnitude % unit).padStart(digits, "0")}`;
}
// How much to ask for when opening. The resident set is sized to be fetched in
// one request; this is a reader's guess at how large that is, not a property of
// the format.
const OPEN_PREFIX = 256 * 1024;
const utf8 = new TextDecoder();
const encoder = new TextEncoder();
// Just enough CBOR to read the metadata section (SPEC.md §6): unsigned
// integers, text, arrays, and maps. Unlike the Rust reader this does not check
// that the encoding is canonical; a page wants to know where a file came from,
// and checking that it was written correctly is `mokuji verify`'s job.
function cbor(bytes, pos = 0) {
    const view = new DataView(bytes.buffer, bytes.byteOffset);
    const initial = bytes[pos];
    const major = initial >> 5;
    const small = initial & 0x1f;
    let argument;
    if (small < 24)
        [argument, pos] = [small, pos + 1];
    else if (small === 24)
        [argument, pos] = [bytes[pos + 1], pos + 2];
    else if (small === 25)
        [argument, pos] = [view.getUint16(pos + 1), pos + 3];
    else if (small === 26)
        [argument, pos] = [view.getUint32(pos + 1), pos + 5];
    else if (small === 27)
        [argument, pos] = [Number(view.getBigUint64(pos + 1)), pos + 9];
    else
        throw new Error("metadata uses an indefinite or reserved length");
    switch (major) {
        case 0:
            return [argument, pos];
        case 3:
            return [utf8.decode(bytes.subarray(pos, pos + argument)), pos + argument];
        case 4: {
            const items = [];
            for (let i = 0; i < argument; i++) {
                const [item, next] = cbor(bytes, pos);
                items.push(item);
                pos = next;
            }
            return [items, pos];
        }
        case 5: {
            const map = {};
            for (let i = 0; i < argument; i++) {
                const [key, afterKey] = cbor(bytes, pos);
                const [value, afterValue] = cbor(bytes, afterKey);
                map[String(key)] = value;
                pos = afterValue;
            }
            return [map, pos];
        }
        default:
            throw new Error(`metadata uses CBOR major type ${major}`);
    }
}
class Cursor {
    bytes;
    pos;
    constructor(bytes, pos = 0) {
        this.bytes = bytes;
        this.pos = pos;
    }
    varint() {
        let value = 0;
        let shift = 1;
        for (;;) {
            const byte = this.bytes[this.pos++];
            if (byte === undefined)
                throw new Error("truncated varint");
            value += (byte & 0x7f) * shift;
            if ((byte & 0x80) === 0)
                return value;
            shift *= 128;
        }
    }
    // A front-coded key, decoded against the previous one (SPEC.md §5.3.1).
    key(previous) {
        const shared = this.varint();
        const length = this.varint();
        if (shared > previous.length)
            throw new Error("shared prefix too long");
        const out = new Uint8Array(shared + length);
        out.set(previous.subarray(0, shared));
        out.set(this.bytes.subarray(this.pos, this.pos + length), shared);
        this.pos += length;
        return out;
    }
    // One bit-packed group: minimal width, least significant bit first.
    group(count) {
        const width = this.bytes[this.pos++];
        if (width > 53) {
            // Values wider than a double can hold exactly would need BigInt. No
            // Mokuji file in practice has them, and pretending otherwise would be
            // worse than saying so.
            throw new Error(`bit width ${width} exceeds what this reader supports`);
        }
        const out = new Array(count);
        for (let i = 0; i < count; i++) {
            let value = 0;
            let scale = 1;
            for (let bit = 0; bit < width; bit++) {
                const at = i * width + bit;
                if (this.bytes[this.pos + (at >> 3)] & (1 << (at & 7)))
                    value += scale;
                scale *= 2;
            }
            out[i] = value;
        }
        this.pos += Math.ceil((count * width) / 8);
        return out;
    }
    values(count) {
        const out = [];
        while (out.length < count) {
            out.push(...this.group(Math.min(128, count - out.length)));
        }
        return out;
    }
}
function compare(a, b) {
    const n = Math.min(a.length, b.length);
    for (let i = 0; i < n; i++) {
        if (a[i] !== b[i])
            return a[i] - b[i];
    }
    return a.length - b.length;
}
function startsWith(key, prefix) {
    if (key.length < prefix.length)
        return false;
    for (let i = 0; i < prefix.length; i++) {
        if (key[i] !== prefix[i])
            return false;
    }
    return true;
}
async function inflate(unit) {
    const cursor = new Cursor(unit);
    const length = cursor.varint();
    // A view of a plain ArrayBuffer: what DecompressionStream is given is never
    // a SharedArrayBuffer, since every unit comes from a fetch or a slice of one.
    const stream = new Blob([unit.subarray(cursor.pos)])
        .stream()
        .pipeThrough(new DecompressionStream("deflate-raw"));
    const out = new Uint8Array(await new Response(stream).arrayBuffer());
    if (out.length !== length)
        throw new Error("unit does not match its declared length");
    return out;
}
/** Bytes as a string, one character per byte, to key a Map by. */
const byteKey = (bytes) => String.fromCharCode(...bytes);
/** The completions section (SPEC.md §5.3.5), checked as a reader must. */
function decodeCompletions(bytes, numDocs) {
    const cursor = new Cursor(bytes);
    const minTerms = cursor.varint();
    const maxSpan = cursor.varint();
    const docsPerEntry = cursor.varint();
    const count = cursor.varint();
    if (docsPerEntry === 0)
        throw new Error("a completion entry lists no documents");
    const entries = new Map();
    let previous = new Uint8Array(0);
    for (let i = 0; i < count; i++) {
        const key = cursor.key(previous);
        if (key.length === 0 || (i > 0 && compare(key, previous) <= 0)) {
            throw new Error("completion prefixes are empty or out of order");
        }
        previous = key;
        const n = cursor.varint();
        if (n === 0 || n > docsPerEntry)
            throw new Error("a completion lists too few or too many");
        const docs = [];
        const seen = new Set();
        let last = 0;
        for (let j = 0; j < n; j++) {
            last += unzigzag(cursor.varint());
            if (last < 0 || last >= numDocs || seen.has(last)) {
                throw new Error("a completion lists a document outside the index, or twice");
            }
            seen.add(last);
            docs.push(last);
        }
        entries.set(byteKey(key), docs);
    }
    if (cursor.pos !== bytes.length)
        throw new Error("trailing bytes in the completions section");
    return { minTerms, maxSpan, docsPerEntry, entries };
}
/** Named columns of per-document numbers (SPEC.md §5.8). */
function decodeColumns(bytes, numDocs) {
    const cursor = new Cursor(bytes);
    const count = cursor.varint();
    const headers = [];
    for (let i = 0; i < count; i++) {
        const length = cursor.varint();
        const name = utf8.decode(bytes.subarray(cursor.pos, cursor.pos + length));
        cursor.pos += length;
        const raw = bytes[cursor.pos++];
        if (headers.some(([taken]) => taken === name)) {
            throw new Error("two attribute columns share a name");
        }
        headers.push([name, raw > 127 ? raw - 256 : raw]);
    }
    const columns = new Map();
    for (const [name, scaleExponent] of headers) {
        columns.set(name, { name, scaleExponent, values: cursor.values(numDocs).map(unzigzag) });
    }
    if (cursor.pos !== bytes.length) {
        throw new Error("trailing bytes in the document attributes section");
    }
    return columns;
}
const text = (value) => typeof value === "string" ? value : null;
const field = (value, key) => value !== null && typeof value === "object" && !Array.isArray(value) ? value[key] : undefined;
// --- a file ---------------------------------------------------------------
/** One open Mokuji file: its resident set held, everything else read on demand. */
export class MokujiIndex {
    /** Everything a query costs, so the page can show it rather than claim it. */
    stats = { requests: 0, bytes: 0 };
    /** Whether neighbouring blocks are read in one request (SPEC.md §4 allows
     * it: the count can only fall). On by default; the conformance suite turns
     * it off to check the count the cases state, block by block. */
    coalesce = true;
    /** Called with every range request, so a page can show the requests
     * themselves rather than a count of them. */
    onRequest = null;
    /** The file's length, once a response has said. */
    fileLength;
    flags;
    numDocs;
    sumDoclen;
    sections;
    /** The metadata section, decoded (SPEC.md §6). */
    meta;
    tokenizerId;
    /** The index's license, as an SPDX expression, if it states one. */
    license;
    /** The credit its results carry, if it states one. */
    attribution;
    /** What it was derived from, and when (SPEC.md §6). */
    sources;
    /** What a query goes through before it is looked up. */
    normalize;
    docLengths;
    columns;
    completions;
    termIndex;
    termBlockOffsets;
    payloadOffsets;
    blockCount;
    boundaries;
    /** A URL, or what the bytes are read from; a `Blob` is read as a `Source`. */
    source;
    /**
     * @param source a URL, a `Blob`, or a `Source` returning bytes — which is
     * what lets the same reader be tested against a file on disk.
     */
    constructor(source) {
        if (isBlob(source)) {
            // A slice of a file-backed Blob reads that range and no more, so a
            // file of gigabytes costs what a query touches, as over HTTP.
            this.source = {
                read: async (offset, length) => new Uint8Array(await source.slice(offset, offset + length).arrayBuffer()),
            };
            this.fileLength = source.size;
        }
        else {
            this.source = source;
        }
    }
    resetStats() {
        this.stats = { requests: 0, bytes: 0 };
    }
    async read(offset, length) {
        if (length === 0)
            return new Uint8Array(0);
        if (typeof this.source !== "string") {
            const bytes = await this.source.read(offset, length);
            this.stats.requests += 1;
            this.stats.bytes += bytes.length;
            if (this.onRequest)
                this.onRequest({ offset, length: bytes.length });
            return bytes;
        }
        const response = await fetch(this.source, {
            headers: { Range: `bytes=${offset}-${offset + length - 1}` },
        });
        if (response.status !== 206) {
            throw new Error(`the server ignored the Range header (status ${response.status}); ` +
                `Mokuji needs range requests to be honoured`);
        }
        const range = response.headers.get("Content-Range");
        if (range && this.fileLength === undefined) {
            this.fileLength = Number(range.split("/")[1]);
        }
        const bytes = new Uint8Array(await response.arrayBuffer());
        this.stats.requests += 1;
        this.stats.bytes += bytes.length;
        if (this.onRequest)
            this.onRequest({ offset, length: bytes.length });
        return bytes;
    }
    async unit(bytes, codec) {
        if (codec === CODEC_RAW)
            return bytes;
        if (codec === CODEC_DEFLATE)
            return inflate(bytes);
        throw new Error(`codec ${codec} is registered but not implemented here`);
    }
    section(id) {
        const section = this.sections.get(id);
        if (!section)
            throw new Error(`section ${id} is missing`);
        return section;
    }
    static async open(source) {
        const index = new MokujiIndex(source);
        // One request for the resident set, optimistically. SPEC.md §5 recommends
        // a layout that puts all of it at the head of the file for exactly this.
        const prefix = await index.read(0, OPEN_PREFIX);
        const view = new DataView(prefix.buffer, prefix.byteOffset);
        if (utf8.decode(prefix.subarray(0, 8)) !== MAGIC)
            throw new Error("not a Mokuji file");
        if (prefix[8] !== 0)
            throw new Error(`unsupported major version ${prefix[8]}`);
        index.flags = view.getUint16(10, true);
        index.numDocs = Number(view.getBigUint64(16, true));
        index.sumDoclen = Number(view.getBigUint64(24, true));
        const tableOffset = Number(view.getBigUint64(32, true));
        const tableLength = view.getUint32(40, true);
        const count = view.getUint32(12, true);
        const table = tableOffset + tableLength <= prefix.length
            ? prefix.subarray(tableOffset, tableOffset + tableLength)
            : await index.read(tableOffset, tableLength);
        index.sections = new Map();
        const tableView = new DataView(table.buffer, table.byteOffset);
        for (let i = 0; i < count; i++) {
            const at = i * SECTION_ENTRY_LEN;
            index.sections.set(tableView.getUint16(at, true), {
                codec: table[at + 2],
                offset: Number(tableView.getBigUint64(at + 8, true)),
                length: Number(tableView.getBigUint64(at + 16, true)),
            });
        }
        const wanted = [SECTION.META, SECTION.TERM_INDEX, SECTION.TERM_BLOCK_OFFSETS];
        if (index.flags & FLAG_HAS_PAYLOAD)
            wanted.push(SECTION.PAYLOAD_OFFSETS);
        // Both are read in bulk before any payload is, which is what makes them
        // worth having as sections of their own (SPEC.md §5.7, §5.8). A reader that
        // fetched them per query would pay for the separation twice.
        // Completions too: whether a prefix has an entry decides what it costs,
        // and has to be known before anything is read for it (SPEC.md §4.1.1).
        for (const id of [SECTION.COMPLETIONS, SECTION.DOC_LENGTHS, SECTION.DOC_ATTRIBUTES]) {
            if (index.sections.has(id))
                wanted.push(id);
        }
        for (const id of wanted)
            index.section(id);
        // The recommended layout puts everything resident at the head of the file,
        // so when the opening prefix does not reach far enough the remedy is one
        // wider read rather than one read per section.
        const spanEnd = Math.max(...wanted.map((id) => {
            const s = index.section(id);
            return s.offset + s.length;
        }));
        let span = prefix;
        if (spanEnd > prefix.length) {
            // Only the part not already in hand. Re-reading the prefix would double
            // the bytes for no reason, and the opening cost is on screen.
            const tail = await index.read(prefix.length, spanEnd - prefix.length);
            span = new Uint8Array(prefix.length + tail.length);
            span.set(prefix);
            span.set(tail, prefix.length);
        }
        const resident = async (id) => {
            const section = index.section(id);
            return index.unit(span.subarray(section.offset, section.offset + section.length), section.codec);
        };
        index.meta = cbor(await resident(SECTION.META))[0];
        index.tokenizerId = text(field(field(index.meta, "tokenizer"), "id")) ?? "identity";
        // The index's license and the credit its results carry (SPEC.md §6). A
        // page that shows results owes the credit, so it is kept at hand.
        index.license = text(field(index.meta, "license"));
        index.attribution = text(field(index.meta, "attribution"));
        const sources = field(index.meta, "source");
        index.sources = (Array.isArray(sources) ? sources : []).map((entry) => ({
            contentHash: text(field(entry, "content_hash")) ?? "",
            uri: text(field(entry, "uri")),
            retrieved: text(field(entry, "retrieved")),
            modified: text(field(entry, "modified")),
        }));
        const normalize = TOKENIZERS[index.tokenizerId];
        if (normalize) {
            index.normalize = normalize;
        }
        else {
            // Readable, just not reachable by an unnormalized query. Saying so beats
            // returning nothing and looking broken.
            console.warn(`unknown tokenizer '${index.tokenizerId}'; queries used as typed`);
            index.normalize = (q) => q;
        }
        index.docLengths = index.sections.has(SECTION.DOC_LENGTHS)
            ? new Cursor(await resident(SECTION.DOC_LENGTHS)).values(index.numDocs)
            : null;
        index.columns = index.sections.has(SECTION.DOC_ATTRIBUTES)
            ? decodeColumns(await resident(SECTION.DOC_ATTRIBUTES), index.numDocs)
            : new Map();
        index.completions = index.sections.has(SECTION.COMPLETIONS)
            ? decodeCompletions(await resident(SECTION.COMPLETIONS), index.numDocs)
            : null;
        index.termIndex = await resident(SECTION.TERM_INDEX);
        index.termBlockOffsets = await resident(SECTION.TERM_BLOCK_OFFSETS);
        if (index.flags & FLAG_HAS_PAYLOAD) {
            index.payloadOffsets = await resident(SECTION.PAYLOAD_OFFSETS);
        }
        // Boundary keys, decoded once. They are what lets a prefix scan know
        // whether the next block can extend the range without fetching it.
        const cursor = new Cursor(index.termIndex, 4);
        index.blockCount = new DataView(index.termIndex.buffer, index.termIndex.byteOffset).getUint32(0, true);
        index.boundaries = [];
        let previous = new Uint8Array(0);
        for (let i = 0; i < index.blockCount; i++) {
            previous = cursor.key(previous);
            index.boundaries.push(previous);
        }
        return index;
    }
    offsetPair(table, base, i) {
        const view = new DataView(table.buffer, table.byteOffset);
        const start = Number(view.getBigUint64(base + i * 8, true));
        const end = Number(view.getBigUint64(base + i * 8 + 8, true));
        return [start, end - start];
    }
    /** Blocks of one section by ordinal, a request for each run of neighbours.
     *
     * Blocks are laid end to end, so ordinals that follow one another are one
     * contiguous range of the file, and reading them as one costs a single round
     * trip where reading them apart costs one each. The runs are independent of
     * each other and fetched at once. Returns each block as stored, before its
     * codec is undone.
     */
    async blockRuns(ordinals, locate) {
        const sorted = [...new Set(ordinals)].sort((a, b) => a - b);
        const runs = [];
        for (const ordinal of sorted) {
            const run = runs[runs.length - 1];
            if (run && this.coalesce && run[run.length - 1] === ordinal - 1)
                run.push(ordinal);
            else
                runs.push([ordinal]);
        }
        const out = new Map();
        await Promise.all(runs.map(async (run) => {
            const [start] = locate(run[0]);
            const [lastOffset, lastLength] = locate(run[run.length - 1]);
            const bytes = await this.read(start, lastOffset + lastLength - start);
            for (const ordinal of run) {
                const [offset, length] = locate(ordinal);
                out.set(ordinal, bytes.subarray(offset - start, offset - start + length));
            }
        }));
        return out;
    }
    /** Several dictionary blocks, decoded, a request for each run of neighbours. */
    async termBlocks(ordinals) {
        const raw = await this.blockRuns(ordinals, (i) => this.offsetPair(this.termBlockOffsets, 0, i));
        const out = new Map();
        for (const [ordinal, bytes] of raw)
            out.set(ordinal, await this.decodeTermBlock(bytes));
        return out;
    }
    async termBlock(ordinal) {
        const [offset, length] = this.offsetPair(this.termBlockOffsets, 0, ordinal);
        return this.decodeTermBlock(await this.read(offset, length));
    }
    async decodeTermBlock(raw) {
        const block = await this.unit(raw, this.section(SECTION.TERM_BLOCKS).codec);
        const cursor = new Cursor(block);
        const entryCount = cursor.varint();
        const firstTermId = cursor.varint();
        const hasPositions = Boolean(this.flags & FLAG_HAS_POSITIONS);
        const hasFreqs = Boolean(this.flags & FLAG_HAS_FREQS);
        const entries = [];
        let key = new Uint8Array(0);
        let postingsOffset = null;
        let positionsOffset = null;
        // One more than the last document an implied or inline entry named: what
        // an inline entry's first document is written against (SPEC.md §5.3.4).
        let expected = firstTermId;
        for (let i = 0; i < entryCount; i++) {
            key = cursor.key(key);
            const termId = firstTermId + i;
            const docFreq = cursor.varint();
            const delta = cursor.varint();
            postingsOffset = postingsOffset === null ? delta : postingsOffset + delta;
            const stored = cursor.varint();
            const entry = {
                term: key,
                termId,
                docFreq,
                postingsOffset,
                postingsLength: stored === INLINE ? 0 : stored,
            };
            if (stored === IMPLIED) {
                expected = termId + 1;
            }
            else if (stored === INLINE) {
                if (docFreq === 0)
                    throw new Error("inline postings name no document");
                const docs = [expected + unzigzag(cursor.varint())];
                for (let j = 1; j < docFreq; j++)
                    docs.push(docs[j - 1] + cursor.varint() + 1);
                if (docs[0] < 0 || docs[docs.length - 1] >= this.numDocs) {
                    throw new Error("inline postings name a missing document");
                }
                const freqs = docs.map(() => (hasFreqs ? cursor.varint() + 1 : 1));
                entry.inline = { docs, freqs };
                expected = docs[docs.length - 1] + 1;
            }
            if (hasPositions) {
                // Two further varints an entry carries only when the file has
                // positions, which is why the flag is read before any entry is.
                const gap = cursor.varint();
                positionsOffset = positionsOffset === null ? gap : positionsOffset + gap;
                entry.positionsOffset = positionsOffset;
                entry.positionsLength = cursor.varint();
            }
            entries.push(entry);
        }
        return entries;
    }
    /** One named column of per-document numbers, or undefined.
     *
     * Resident, so this costs no read: the point of the section is that a scorer
     * can weigh many documents before deciding which payloads are worth fetching.
     */
    column(name) {
        return this.columns.get(name);
    }
    /** Where a term occurs in each document its postings name.
     *
     * One read, and only for a caller that asks. The run is decoded from the
     * front and the frequencies say how many values belong to each document, so
     * the postings have to be in hand first.
     */
    async positions(entry, postings) {
        if (!(this.flags & FLAG_HAS_POSITIONS) || !entry.positionsLength)
            return [];
        const run = await this.read(entry.positionsOffset ?? 0, entry.positionsLength);
        const cursor = new Cursor(run);
        const out = [];
        for (const freq of postings.freqs) {
            let current = null;
            out.push(cursor.values(freq).map((value) => {
                // The first is a position; the rest are gaps less one, so that
                // adjacent occurrences pack to nothing.
                current = current === null ? value : current + value + 1;
                return current;
            }));
        }
        if (cursor.pos !== run.length)
            throw new Error("trailing bytes in positions run");
        return out;
    }
    /** The dictionary block a term would be in, from the resident index alone. */
    blockFor(term) {
        let start = 0;
        for (let i = 0; i < this.boundaries.length; i++) {
            if (compare(this.boundaries[i], term) <= 0)
                start = i;
        }
        return start;
    }
    /** One term, exactly, or null.
     *
     * A prefix scan would answer this, and would cost more: `xn76ur` is a prefix
     * of every cell inside it, so completing it fetches postings for all of them
     * when the caller wanted one. Exactly one block is read, always.
     */
    async exact(termText) {
        const term = encoder.encode(termText);
        for (const entry of await this.termBlock(this.blockFor(term))) {
            const order = compare(entry.term, term);
            if (order === 0)
                return entry;
            if (order > 0)
                break;
        }
        return null;
    }
    /** Several terms at once, each exactly, in one read per dictionary block.
     *
     * Cells one geohash precision apart are neighbours in the dictionary, so the
     * lookups a point needs usually land in the same block. Asking for them
     * together is the difference between reading it once and reading it twice.
     */
    async exactMany(termTexts) {
        const wanted = termTexts.map((t) => {
            const term = encoder.encode(t);
            return { term, block: this.blockFor(term) };
        });
        const blocks = await this.termBlocks(wanted.map((w) => w.block));
        return wanted.map(({ term, block }) => {
            for (const entry of blocks.get(block)) {
                const order = compare(entry.term, term);
                if (order === 0)
                    return entry;
                if (order > 0)
                    break;
            }
            return null;
        });
    }
    /** Terms beginning with `prefix`, at most `limit` of them. */
    async prefix(prefixText, limit = 20) {
        const prefix = encoder.encode(prefixText);
        let start = 0;
        for (let i = 0; i < this.boundaries.length; i++) {
            if (compare(this.boundaries[i], prefix) <= 0)
                start = i;
        }
        const found = [];
        for (let ordinal = start; ordinal < this.blockCount; ordinal++) {
            // Whether the range continues is answerable from the resident index
            // alone, so only the first block is fetched on faith.
            if (ordinal > start && !startsWith(this.boundaries[ordinal], prefix))
                break;
            let done = false;
            for (const entry of await this.termBlock(ordinal)) {
                if (startsWith(entry.term, prefix)) {
                    found.push(entry);
                    if (found.length === limit)
                        return found;
                }
                else if (compare(entry.term, prefix) > 0 && found.length > 0) {
                    done = true;
                    break;
                }
            }
            if (done)
                break;
        }
        return found;
    }
    /** Terms beginning with each of several prefixes, every one of them.
     *
     * Which blocks each prefix spans is answerable from the resident index, so
     * all of them are known before any is read, and they are fetched together:
     * a request for each run of neighbouring blocks, however many prefixes fall
     * in it. This is what a query over an area needs — the cells covering it
     * are many short prefixes, mostly in blocks next to one another — and what
     * `prefix`, which walks one prefix a block at a time, would make a request
     * per block per prefix.
     */
    async prefixMany(prefixTexts) {
        const prefixes = prefixTexts.map((t) => encoder.encode(t));
        const spans = prefixes.map((prefix) => {
            const start = this.blockFor(prefix);
            let end = start;
            while (end + 1 < this.blockCount && startsWith(this.boundaries[end + 1], prefix))
                end++;
            return [start, end];
        });
        const ordinals = [];
        for (const [start, end] of spans)
            for (let i = start; i <= end; i++)
                ordinals.push(i);
        const blocks = await this.termBlocks(ordinals);
        return prefixes.map((prefix, at) => {
            const [start, end] = spans[at];
            const found = [];
            for (let i = start; i <= end; i++) {
                for (const entry of blocks.get(i))
                    if (startsWith(entry.term, prefix))
                        found.push(entry);
            }
            return found;
        });
    }
    /** Terms from `low` to `high`, both inclusive, at most `limit` of them.
     *
     * The same walk as `prefix`, with a different test for where it stops: the
     * dictionary is sorted, so a range of keys is a run of neighbouring entries,
     * and whether the run continues past a block is answerable from the resident
     * index alone.
     */
    async range(lowText, highText, limit = Infinity) {
        const low = encoder.encode(lowText);
        const high = encoder.encode(highText);
        const start = this.blockFor(low);
        const found = [];
        for (let ordinal = start; ordinal < this.blockCount; ordinal++) {
            if (ordinal > start && compare(this.boundaries[ordinal], high) > 0)
                break;
            for (const entry of await this.termBlock(ordinal)) {
                if (compare(entry.term, high) > 0)
                    return found;
                if (compare(entry.term, low) >= 0) {
                    found.push(entry);
                    if (found.length === limit)
                        return found;
                }
            }
        }
        return found;
    }
    // The postings a dictionary entry states itself (SPEC.md §5.3.4): inline,
    // or implied — the term names one document, whose ID is the term's own, once.
    implied(entry) {
        if (entry.inline)
            return entry.inline;
        if (entry.docFreq !== 1)
            throw new Error("implied postings need a doc_freq of 1");
        if (entry.termId >= this.numDocs)
            throw new Error("implied postings name a missing document");
        return { docs: [entry.termId], freqs: [1] };
    }
    /** Postings for a contiguous range of entries, in one request — or none. */
    async postingsSpan(entries) {
        if (entries.length === 0)
            return [];
        // Entries whose postings are implied or inline contribute no bytes, so an
        // index of a dictionary spends no request here at all.
        const stored = entries.filter((e) => e.postingsLength > 0);
        let start = 0;
        let span = null;
        if (stored.length > 0) {
            const first = stored[0];
            const last = stored[stored.length - 1];
            start = first.postingsOffset;
            span = await this.read(start, last.postingsOffset + last.postingsLength - start);
        }
        const hasFreqs = Boolean(this.flags & FLAG_HAS_FREQS);
        return entries.map((entry) => {
            if (entry.postingsLength === 0)
                return this.implied(entry);
            const at = entry.postingsOffset - start;
            const cursor = new Cursor(span.subarray(at, at + entry.postingsLength));
            if (cursor.bytes[cursor.pos++] !== 0)
                throw new Error("unsupported postings codec");
            const count = cursor.varint();
            const docs = [];
            let current = null;
            // The first value is a document ID; the rest are gaps less one.
            for (const value of cursor.values(count)) {
                current = current === null ? value : current + value + 1;
                docs.push(current);
            }
            const freqs = hasFreqs ? cursor.values(count).map((f) => f + 1) : docs.map(() => 1);
            return { docs, freqs };
        });
    }
    /** Payload for documents, as text: opaque bytes decoded, or integers written
     * out against the scale the file declares. */
    async payloadSpan(docs) {
        if (!(this.flags & FLAG_HAS_PAYLOAD) || !this.payloadOffsets)
            return docs.map(() => "");
        const header = this.payloadOffsets;
        const view = new DataView(header.buffer, header.byteOffset);
        const codec = header[0];
        const valuesPerDoc = header[1];
        // A signed byte: the stored integers stand for value * 10^exponent.
        const exponent = (header[2] << 24) >> 24;
        const perBlock = view.getUint32(4, true);
        // Every block the documents fall in, fetched at once. They are independent
        // of each other, so reading them one after another would cost a round
        // trip each for nothing: measured over the network, ten results in eight
        // blocks took 273 ms in sequence against one round trip in parallel.
        // Neighbouring blocks are one range of the file and read as one.
        const raw = await this.blockRuns(docs.map((doc) => Math.floor(doc / perBlock)), (i) => this.offsetPair(header, 12, i));
        const blocks = new Map();
        for (const [ordinal, bytes] of raw) {
            blocks.set(ordinal, await this.unit(bytes, this.section(SECTION.PAYLOAD).codec));
        }
        const out = [];
        for (const doc of docs) {
            const ordinal = Math.floor(doc / perBlock);
            const cached = blocks.get(ordinal);
            const cursor = new Cursor(cached);
            const count = cursor.varint();
            const index = doc % perBlock;
            if (index >= count)
                throw new Error("payload block is shorter than expected");
            if (codec === PAYLOAD_OPAQUE) {
                for (let i = 0; i < count; i++) {
                    const length = cursor.varint();
                    if (i === index) {
                        out.push(utf8.decode(cached.subarray(cursor.pos, cursor.pos + length)));
                        break;
                    }
                    cursor.pos += length;
                }
            }
            else if (codec === PAYLOAD_DELTA_INTS) {
                // Every entry up to the wanted one has to be decoded, because each is a
                // difference from the one before it. A block is what gets fetched
                // anyway, and it is sized so that this is cheap.
                const values = new Array(valuesPerDoc).fill(0);
                for (let entry = 0; entry <= index; entry++) {
                    for (let i = 0; i < valuesPerDoc; i++) {
                        const delta = unzigzag(cursor.varint());
                        values[i] = entry === 0 ? delta : values[i] + delta;
                    }
                }
                out.push(values.map((v) => scaled(v, exponent)).join(","));
            }
            else {
                throw new Error(`payload codec ${codec} is not implemented here`);
            }
        }
        return out;
    }
    /** Documents ranked by BM25 over the terms the query becomes.
     *
     * The same ranking as `mokuji search`: Robertson–Spärck Jones idf, `k1` of
     * 1.2, `b` from `lengthNorm`, a union scored over the terms a document
     * actually has, and ties broken by document ID. Without document lengths the
     * order is by document ID and the scores are null.
     *
     * One request per run of dictionary blocks the terms fall in, then one per
     * term whose postings are stored as a run, then the payload of the first
     * `limit`.
     */
    async search(query, options = {}) {
        const { limit = 10, any = false, probe, lengthNorm = 0.75, boost } = options;
        const split = SPLITTERS[this.tokenizerId] ?? ((q) => [q]);
        const terms = split(query);
        const column = boost === undefined ? undefined : this.column(boost);
        if (boost !== undefined && !column) {
            throw new Error(`this index carries no attribute column called '${boost}'`);
        }
        const none = { terms, probed: [], skipped: 0, matched: 0, results: [] };
        if (terms.length === 0)
            return none;
        const found = await this.exactMany(terms);
        let present = [];
        for (let i = 0; i < terms.length; i++) {
            const entry = found[i];
            if (entry)
                present.push({ term: terms[i], entry });
            // A term nobody wrote down cannot be in an intersection.
            else if (!any)
                return none;
        }
        if (present.length === 0)
            return none;
        // Rarest first: a term matching few documents says the most about which
        // ones, and costs the least to say it. Stable, as the CLI's sort is.
        let skipped = 0;
        if (probe !== undefined) {
            present = present
                .map((p, i) => ({ p, i }))
                .sort((a, b) => a.p.entry.docFreq - b.p.entry.docFreq || a.i - b.i)
                .map(({ p }) => p);
            skipped = Math.max(0, present.length - probe);
            present = present.slice(0, probe);
        }
        const runs = await Promise.all(present.map(async ({ entry }) => (await this.postingsSpan([entry]))[0]));
        const probed = present.map(({ term, entry }) => ({ term, docFreq: entry.docFreq }));
        let matched;
        if (any) {
            matched = [...new Set(runs.flatMap((r) => r.docs))].sort((a, b) => a - b);
        }
        else {
            const rest = runs.slice(1).map((r) => new Set(r.docs));
            matched = runs[0].docs.filter((doc) => rest.every((s) => s.has(doc)));
        }
        let ranked;
        if (!this.docLengths) {
            ranked = matched.map((doc) => ({ doc, score: null }));
        }
        else {
            const lengths = this.docLengths;
            const n = this.numDocs;
            const avgdl = Math.max(n === 0 ? 1 : this.sumDoclen / n, 1);
            const K1 = 1.2;
            const b = lengthNorm;
            const at = runs.map((r) => new Map(r.docs.map((doc, i) => [doc, r.freqs[i]])));
            ranked = matched.map((doc) => {
                const dl = lengths[doc] ?? 0;
                let score = 0;
                present.forEach(({ entry }, i) => {
                    const tf = at[i].get(doc);
                    if (tf === undefined)
                        return;
                    const df = entry.docFreq;
                    const idf = Math.log((n - df + 0.5) / (df + 0.5) + 1);
                    score += (idf * (tf * (K1 + 1))) / (tf + K1 * (1 - b + (b * dl) / avgdl));
                });
                if (column)
                    score += Math.log(1 + Math.max(column.values[doc] ?? 0, 0));
                return { doc, score };
            });
            ranked.sort((x, y) => y.score - x.score || x.doc - y.doc);
        }
        const shown = ranked.slice(0, limit);
        const payloads = await this.payloadSpan(shown.map((r) => r.doc).sort((a, b) => a - b));
        const byDoc = new Map(shown.map((r) => r.doc).sort((a, b) => a - b).map((doc, i) => [doc, payloads[i]]));
        return {
            terms,
            probed,
            skipped,
            matched: matched.length,
            results: shown.map((r) => ({ ...r, payload: byDoc.get(r.doc) })),
        };
    }
    /** The first `limit` documents for what has been typed.
     *
     * From the completions section when the prefix has an entry that reaches
     * the limit — or lists fewer than it may, so lists them all — which costs no
     * request before the payload. Otherwise every term the prefix matches, whose
     * runs are united and returned in document order.
     */
    async complete(typed, limit = 10) {
        const normalized = this.normalize(typed);
        const { docs, fromTable } = await this.completeDocs(normalized, limit);
        const requestsBeforePayload = this.stats.requests;
        const payloads = await this.payloadSpan(docs);
        return { normalized, fromTable, docs, payloads, requestsBeforePayload };
    }
    /** `complete` for a prefix already normalized, without the payload. */
    async completeDocs(normalized, limit) {
        const listed = this.completions?.entries.get(byteKey(encoder.encode(normalized)));
        if (listed && this.completions && (limit <= listed.length || listed.length < this.completions.docsPerEntry)) {
            return { docs: listed.slice(0, limit), fromTable: true };
        }
        const entries = await this.prefix(normalized, Infinity);
        const postings = await this.postingsSpan(entries);
        const docs = [...new Set(postings.flatMap((p) => p.docs))].sort((a, b) => a - b).slice(0, limit);
        return { docs, fromTable: false };
    }
    /** Complete a prefix, returning every matching document with its payload.
     *
     * The query goes through the index's own tokenizer first. Without that, an
     * index built from `一丁目` is unreachable by anyone typing `一丁目`.
     */
    async lookup(prefixText, limit = 20) {
        const normalized = this.normalize(prefixText);
        const entries = await this.prefix(normalized, limit);
        const postings = await this.postingsSpan(entries);
        // A term may name more than one document — in a dictionary it never does,
        // but `tiny` is not a dictionary, and returning only the first would drop
        // results silently. Documents are collected in ascending order so that the
        // payload reads share blocks.
        const wanted = [...new Set(postings.flatMap((p) => p.docs))].sort((a, b) => a - b);
        const payloads = new Map();
        const fetched = await this.payloadSpan(wanted);
        wanted.forEach((doc, i) => payloads.set(doc, fetched[i]));
        const results = [];
        entries.forEach((entry, i) => {
            for (const doc of postings[i].docs) {
                results.push({ term: utf8.decode(entry.term), doc, payload: payloads.get(doc) });
            }
        });
        return { normalized, results };
    }
}
/**
 * A collection: segments in a manifest's order, less their deletes
 * (MANIFEST.md). Every segment is an ordinary Mokuji file, opened on its own.
 */
export class MokujiCollection {
    manifest;
    segments;
    deletes;
    /** How many segments the manifest lists, before a region left some out. */
    manifestSegments = 0;
    constructor(manifest, segments) {
        this.manifest = manifest;
        this.segments = segments;
        this.deletes = manifest.segments.map((s) => new Set(s.deletes ?? []));
    }
    /**
     * Opens the manifest at `url` and every segment it names, at once: they are
     * independent, so opening them costs one round trip however many there are.
     * `sourceFor(url)` may supply a byte source instead of fetching.
     */
    static async open(url, sourceFor = (u) => u, { countries = null } = {}) {
        let manifest = typeof url === "string" ? (await (await fetch(url, { cache: "no-store" })).json()) : url;
        const listed = manifest.segments.length;
        // About a place: only the segments whose region includes it, and those
        // with none (MANIFEST.md §7). The others are never opened.
        if (countries) {
            manifest = {
                ...manifest,
                segments: manifest.segments.filter((s) => !s.region || s.region.countries.some((c) => countries.includes(c))),
            };
        }
        if (String(manifest.mokuji_manifest ?? "").split(".")[0] !== "0") {
            throw new Error(`manifest version ${manifest.mokuji_manifest} is not one this reader implements`);
        }
        const here = globalThis.location?.href;
        const base = typeof url === "string" ? new URL(url, here) : null;
        const segments = await Promise.all(manifest.segments.map(async (s) => {
            const index = await MokujiIndex.open(sourceFor(base ? new URL(s.path, base).href : s.path));
            if (index.fileLength !== undefined && index.fileLength !== s.length) {
                throw new Error(`${s.path} is ${index.fileLength} bytes, not the ${s.length} the manifest says`);
            }
            const deletes = s.deletes ?? [];
            if (deletes.some((d, i) => d >= index.numDocs || (i > 0 && d <= deletes[i - 1]))) {
                throw new Error(`${s.path} has deletes that are out of order or out of range`);
            }
            return index;
        }));
        const tokenizer = JSON.stringify(field(segments[0]?.meta, "tokenizer"));
        for (const [i, index] of segments.entries()) {
            if (JSON.stringify(field(index.meta, "tokenizer")) !== tokenizer) {
                throw new Error(`${manifest.segments[i].path} was tokenized differently from the first segment`);
            }
        }
        const collection = new MokujiCollection(manifest, segments);
        collection.manifestSegments = listed;
        return collection;
    }
    resetStats() {
        for (const index of this.segments)
            index.resetStats();
    }
    /** Requests and bytes, summed; and the most requests any one segment made. */
    get stats() {
        const per = this.segments.map((s) => s.stats);
        return {
            requests: per.reduce((a, s) => a + s.requests, 0),
            bytes: per.reduce((a, s) => a + s.bytes, 0),
            roundTrips: Math.max(0, ...per.map((s) => s.requests)),
        };
    }
    normalize(typed) {
        return this.segments[0].normalize(typed);
    }
    /** One segment's first `need` live documents: the entry, then all it
     * lists, then as many again as the segment has deletes (MANIFEST.md §3.2). */
    async liveIn(at, normalized, need) {
        const index = this.segments[at];
        const deleted = this.deletes[at];
        const listed = index.completions?.docsPerEntry ?? 0;
        let fromTable = true;
        let live = [];
        for (const ask of [need, Math.max(listed, need), need + deleted.size]) {
            const found = await index.completeDocs(normalized, ask);
            fromTable &&= found.fromTable;
            live = found.docs.filter((d) => !deleted.has(d));
            if (live.length >= need || found.docs.length < ask)
                break;
        }
        return { live: live.slice(0, need), fromTable };
    }
    /** The first `limit` live documents, in collection order. */
    async complete(typed, limit = 10) {
        const normalized = this.normalize(typed);
        const hits = [];
        const consulted = [];
        for (let at = 0; at < this.segments.length && hits.length < limit; at++) {
            const { live, fromTable } = await this.liveIn(at, normalized, limit - hits.length);
            consulted.push({ segment: at, fromTable });
            for (const doc of live)
                hits.push({ segment: at, doc });
        }
        const requestsBeforePayload = this.stats.requests;
        return { normalized, hits, consulted, payloads: await this.payloads(hits), requestsBeforePayload };
    }
    /** Every segment's own first `limit`, asked at once, for a caller that
     * ranks across segments by a score of its own (MANIFEST.md §3.3). */
    async candidates(typed, limit = 10) {
        const normalized = this.normalize(typed);
        const lists = await Promise.all(this.segments.map((_, at) => this.liveIn(at, normalized, limit)));
        const hits = lists.flatMap(({ live }, segment) => live.map((doc) => ({ segment, doc })));
        const requestsBeforePayload = this.stats.requests;
        return {
            normalized,
            hits,
            consulted: lists.map(({ fromTable }, segment) => ({ segment, fromTable })),
            payloads: await this.payloads(hits),
            requestsBeforePayload,
        };
    }
    /** Payload for hits, one span per segment, in the hits' order. */
    async payloads(hits) {
        const out = new Array(hits.length);
        await Promise.all(this.segments.map(async (index, at) => {
            const mine = hits.map((h, i) => [h, i]).filter(([h]) => h.segment === at);
            if (mine.length === 0)
                return;
            mine.sort(([a], [b]) => a.doc - b.doc);
            const payloads = await index.payloadSpan(mine.map(([h]) => h.doc));
            mine.forEach(([, i], j) => (out[i] = payloads[j]));
        }));
        return out;
    }
    /** Each segment's credit, once each, for a page that shows results. */
    get attributions() {
        return [...new Set(this.segments.map((s) => s.attribution).filter((a) => Boolean(a)))];
    }
}
