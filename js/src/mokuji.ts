// A Mokuji reader for browsers, workers and Node, with no dependencies.
//
// This is the third implementation of SPEC.md, after the Rust one and the
// Python fixture builder, and the first written against a transport the format
// was designed for rather than a file already in memory. If it reads what the
// others wrote, the specification survives contact with a third reader.
//
// It bundles no decompressor. The required codec is raw deflate precisely so
// that a page can use DecompressionStream and stay this small.

import { TOKENIZERS, type Normalize } from "./tokenizers.js";

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
} as const;

const CODEC_RAW = 0;
const CODEC_DEFLATE = 1;

const PAYLOAD_OPAQUE = 0;
const PAYLOAD_DELTA_INTS = 1;

// --- types ----------------------------------------------------------------

/**
 * Where a file's bytes come from: anything that returns a range of them.
 *
 * A URL string makes the reader fetch ranges itself. An object lets the caller
 * decide — a file on disk in a test, an R2 binding and an edge cache in a
 * worker — while the reader counts what was asked for either way.
 */
export interface Source {
  read(offset: number, length: number): Promise<Uint8Array> | Uint8Array;
}

/** What a query cost, so a page can show it rather than claim it. */
export interface Stats {
  requests: number;
  bytes: number;
}

/** One range request, as reported to `onRequest`. */
export interface RequestInfo {
  offset: number;
  length: number;
}

/** A value the metadata section can hold (SPEC.md §6). */
export type CborValue = number | string | CborValue[] | { [key: string]: CborValue };

/** One dictionary entry (SPEC.md §5.3.4). */
export interface TermEntry {
  term: Uint8Array;
  termId: number;
  docFreq: number;
  postingsOffset: number;
  /** Bytes of the postings section the term's run occupies: zero when the
   * entry states its postings itself, implied or inline. */
  postingsLength: number;
  /** The postings, when the entry carries them inline. */
  inline?: Postings;
  positionsOffset?: number;
  positionsLength?: number;
}

/** One term's postings: ascending document IDs and their frequencies. */
export interface Postings {
  docs: number[];
  freqs: number[];
}

/** One artifact an index was derived from, as its metadata records it (SPEC.md §6). */
export interface Provenance {
  /** `<algorithm>:<lowercase hex>`. */
  contentHash: string;
  /** Where it can be obtained. */
  uri: string | null;
  /** When the builder obtained it, RFC 3339. */
  retrieved: string | null;
  /** When its publisher last changed it, RFC 3339 — how current the data is. */
  modified: string | null;
}

/** One named column of per-document numbers (SPEC.md §5.8). */
export interface Column {
  name: string;
  scaleExponent: number;
  values: number[];
}

/** The completions section (SPEC.md §5.3.5), keyed by prefix bytes. */
export interface Completions {
  minTerms: number;
  maxSpan: number;
  docsPerEntry: number;
  entries: Map<string, number[]>;
}

interface Section {
  codec: number;
  offset: number;
  length: number;
}

// --- primitives -----------------------------------------------------------

function unzigzag(v: number): number {
  return v % 2 === 0 ? v / 2 : -(v + 1) / 2;
}

/** Writes `value * 10^exponent` in full, keeping the zeros the file claims. */
function scaled(value: number, exponent: number): string {
  if (exponent >= 0) return String(value) + "0".repeat(exponent);
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
function cbor(bytes: Uint8Array, pos = 0): [CborValue, number] {
  const view = new DataView(bytes.buffer, bytes.byteOffset);
  const initial = bytes[pos]!;
  const major = initial >> 5;
  const small = initial & 0x1f;

  let argument: number;
  if (small < 24) [argument, pos] = [small, pos + 1];
  else if (small === 24) [argument, pos] = [bytes[pos + 1]!, pos + 2];
  else if (small === 25) [argument, pos] = [view.getUint16(pos + 1), pos + 3];
  else if (small === 26) [argument, pos] = [view.getUint32(pos + 1), pos + 5];
  else if (small === 27) [argument, pos] = [Number(view.getBigUint64(pos + 1)), pos + 9];
  else throw new Error("metadata uses an indefinite or reserved length");

  switch (major) {
    case 0:
      return [argument, pos];
    case 3:
      return [utf8.decode(bytes.subarray(pos, pos + argument)), pos + argument];
    case 4: {
      const items: CborValue[] = [];
      for (let i = 0; i < argument; i++) {
        const [item, next] = cbor(bytes, pos);
        items.push(item);
        pos = next;
      }
      return [items, pos];
    }
    case 5: {
      const map: { [key: string]: CborValue } = {};
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
  constructor(
    public bytes: Uint8Array,
    public pos = 0,
  ) {}

  varint(): number {
    let value = 0;
    let shift = 1;
    for (;;) {
      const byte = this.bytes[this.pos++];
      if (byte === undefined) throw new Error("truncated varint");
      value += (byte & 0x7f) * shift;
      if ((byte & 0x80) === 0) return value;
      shift *= 128;
    }
  }

  // A front-coded key, decoded against the previous one (SPEC.md §5.3.1).
  key(previous: Uint8Array): Uint8Array {
    const shared = this.varint();
    const length = this.varint();
    if (shared > previous.length) throw new Error("shared prefix too long");
    const out = new Uint8Array(shared + length);
    out.set(previous.subarray(0, shared));
    out.set(this.bytes.subarray(this.pos, this.pos + length), shared);
    this.pos += length;
    return out;
  }

  // One bit-packed group: minimal width, least significant bit first.
  group(count: number): number[] {
    const width = this.bytes[this.pos++]!;
    if (width > 53) {
      // Values wider than a double can hold exactly would need BigInt. No
      // Mokuji file in practice has them, and pretending otherwise would be
      // worse than saying so.
      throw new Error(`bit width ${width} exceeds what this reader supports`);
    }
    const out = new Array<number>(count);
    for (let i = 0; i < count; i++) {
      let value = 0;
      let scale = 1;
      for (let bit = 0; bit < width; bit++) {
        const at = i * width + bit;
        if (this.bytes[this.pos + (at >> 3)]! & (1 << (at & 7))) value += scale;
        scale *= 2;
      }
      out[i] = value;
    }
    this.pos += Math.ceil((count * width) / 8);
    return out;
  }

  values(count: number): number[] {
    const out: number[] = [];
    while (out.length < count) {
      out.push(...this.group(Math.min(128, count - out.length)));
    }
    return out;
  }
}

function compare(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length);
  for (let i = 0; i < n; i++) {
    if (a[i] !== b[i]) return a[i]! - b[i]!;
  }
  return a.length - b.length;
}

function startsWith(key: Uint8Array, prefix: Uint8Array): boolean {
  if (key.length < prefix.length) return false;
  for (let i = 0; i < prefix.length; i++) {
    if (key[i] !== prefix[i]) return false;
  }
  return true;
}

async function inflate(unit: Uint8Array): Promise<Uint8Array> {
  const cursor = new Cursor(unit);
  const length = cursor.varint();
  // A view of a plain ArrayBuffer: what DecompressionStream is given is never
  // a SharedArrayBuffer, since every unit comes from a fetch or a slice of one.
  const stream = new Blob([unit.subarray(cursor.pos) as Uint8Array<ArrayBuffer>])
    .stream()
    .pipeThrough(new DecompressionStream("deflate-raw"));
  const out = new Uint8Array(await new Response(stream).arrayBuffer());
  if (out.length !== length) throw new Error("unit does not match its declared length");
  return out;
}

/** Bytes as a string, one character per byte, to key a Map by. */
const byteKey = (bytes: Uint8Array): string => String.fromCharCode(...bytes);

/** The completions section (SPEC.md §5.3.5), checked as a reader must. */
function decodeCompletions(bytes: Uint8Array, numDocs: number): Completions {
  const cursor = new Cursor(bytes);
  const minTerms = cursor.varint();
  const maxSpan = cursor.varint();
  const docsPerEntry = cursor.varint();
  const count = cursor.varint();
  if (docsPerEntry === 0) throw new Error("a completion entry lists no documents");
  const entries = new Map<string, number[]>();
  let previous: Uint8Array = new Uint8Array(0);
  for (let i = 0; i < count; i++) {
    const key = cursor.key(previous);
    if (key.length === 0 || (i > 0 && compare(key, previous) <= 0)) {
      throw new Error("completion prefixes are empty or out of order");
    }
    previous = key;
    const n = cursor.varint();
    if (n === 0 || n > docsPerEntry) throw new Error("a completion lists too few or too many");
    const docs: number[] = [];
    const seen = new Set<number>();
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
  if (cursor.pos !== bytes.length) throw new Error("trailing bytes in the completions section");
  return { minTerms, maxSpan, docsPerEntry, entries };
}

/** Named columns of per-document numbers (SPEC.md §5.8). */
function decodeColumns(bytes: Uint8Array, numDocs: number): Map<string, Column> {
  const cursor = new Cursor(bytes);
  const count = cursor.varint();
  const headers: [string, number][] = [];
  for (let i = 0; i < count; i++) {
    const length = cursor.varint();
    const name = utf8.decode(bytes.subarray(cursor.pos, cursor.pos + length));
    cursor.pos += length;
    const raw = bytes[cursor.pos++]!;
    if (headers.some(([taken]) => taken === name)) {
      throw new Error("two attribute columns share a name");
    }
    headers.push([name, raw > 127 ? raw - 256 : raw]);
  }
  const columns = new Map<string, Column>();
  for (const [name, scaleExponent] of headers) {
    columns.set(name, { name, scaleExponent, values: cursor.values(numDocs).map(unzigzag) });
  }
  if (cursor.pos !== bytes.length) {
    throw new Error("trailing bytes in the document attributes section");
  }
  return columns;
}

const text = (value: CborValue | undefined): string | null =>
  typeof value === "string" ? value : null;

const field = (value: CborValue | undefined, key: string): CborValue | undefined =>
  value !== null && typeof value === "object" && !Array.isArray(value) ? value[key] : undefined;

// --- a file ---------------------------------------------------------------

/** One open Mokuji file: its resident set held, everything else read on demand. */
export class MokujiIndex {
  /** Everything a query costs, so the page can show it rather than claim it. */
  stats: Stats = { requests: 0, bytes: 0 };
  /** Called with every range request, so a page can show the requests
   * themselves rather than a count of them. */
  onRequest: ((request: RequestInfo) => void) | null = null;
  /** The file's length, once a response has said. */
  fileLength: number | undefined;

  flags!: number;
  numDocs!: number;
  sumDoclen!: number;
  sections!: Map<number, Section>;
  /** The metadata section, decoded (SPEC.md §6). */
  meta!: CborValue;
  tokenizerId!: string;
  /** The index's license, as an SPDX expression, if it states one. */
  license!: string | null;
  /** The credit its results carry, if it states one. */
  attribution!: string | null;
  /** What it was derived from, and when (SPEC.md §6). */
  sources!: Provenance[];
  /** What a query goes through before it is looked up. */
  normalize!: Normalize;
  docLengths!: number[] | null;
  columns!: Map<string, Column>;
  completions!: Completions | null;
  termIndex!: Uint8Array;
  termBlockOffsets!: Uint8Array;
  payloadOffsets: Uint8Array | undefined;
  blockCount!: number;
  boundaries!: Uint8Array[];

  /**
   * @param source a URL, or a `Source` returning bytes — which is what lets
   * the same reader be tested against a file on disk.
   */
  constructor(readonly source: string | Source) {}

  resetStats(): void {
    this.stats = { requests: 0, bytes: 0 };
  }

  async read(offset: number, length: number): Promise<Uint8Array> {
    if (length === 0) return new Uint8Array(0);
    if (typeof this.source !== "string") {
      const bytes = await this.source.read(offset, length);
      this.stats.requests += 1;
      this.stats.bytes += bytes.length;
      if (this.onRequest) this.onRequest({ offset, length: bytes.length });
      return bytes;
    }

    const response = await fetch(this.source, {
      headers: { Range: `bytes=${offset}-${offset + length - 1}` },
    });
    if (response.status !== 206) {
      throw new Error(
        `the server ignored the Range header (status ${response.status}); ` +
          `Mokuji needs range requests to be honoured`,
      );
    }
    const range = response.headers.get("Content-Range");
    if (range && this.fileLength === undefined) {
      this.fileLength = Number(range.split("/")[1]);
    }
    const bytes = new Uint8Array(await response.arrayBuffer());
    this.stats.requests += 1;
    this.stats.bytes += bytes.length;
    if (this.onRequest) this.onRequest({ offset, length: bytes.length });
    return bytes;
  }

  async unit(bytes: Uint8Array, codec: number): Promise<Uint8Array> {
    if (codec === CODEC_RAW) return bytes;
    if (codec === CODEC_DEFLATE) return inflate(bytes);
    throw new Error(`codec ${codec} is registered but not implemented here`);
  }

  private section(id: number): Section {
    const section = this.sections.get(id);
    if (!section) throw new Error(`section ${id} is missing`);
    return section;
  }

  static async open(source: string | Source): Promise<MokujiIndex> {
    const index = new MokujiIndex(source);
    // One request for the resident set, optimistically. SPEC.md §5 recommends
    // a layout that puts all of it at the head of the file for exactly this.
    const prefix = await index.read(0, OPEN_PREFIX);

    const view = new DataView(prefix.buffer, prefix.byteOffset);
    if (utf8.decode(prefix.subarray(0, 8)) !== MAGIC) throw new Error("not a Mokuji file");
    if (prefix[8] !== 0) throw new Error(`unsupported major version ${prefix[8]}`);

    index.flags = view.getUint16(10, true);
    index.numDocs = Number(view.getBigUint64(16, true));
    index.sumDoclen = Number(view.getBigUint64(24, true));

    const tableOffset = Number(view.getBigUint64(32, true));
    const tableLength = view.getUint32(40, true);
    const count = view.getUint32(12, true);
    const table =
      tableOffset + tableLength <= prefix.length
        ? prefix.subarray(tableOffset, tableOffset + tableLength)
        : await index.read(tableOffset, tableLength);

    index.sections = new Map();
    const tableView = new DataView(table.buffer, table.byteOffset);
    for (let i = 0; i < count; i++) {
      const at = i * SECTION_ENTRY_LEN;
      index.sections.set(tableView.getUint16(at, true), {
        codec: table[at + 2]!,
        offset: Number(tableView.getBigUint64(at + 8, true)),
        length: Number(tableView.getBigUint64(at + 16, true)),
      });
    }

    const wanted: number[] = [SECTION.META, SECTION.TERM_INDEX, SECTION.TERM_BLOCK_OFFSETS];
    if (index.flags & FLAG_HAS_PAYLOAD) wanted.push(SECTION.PAYLOAD_OFFSETS);
    // Both are read in bulk before any payload is, which is what makes them
    // worth having as sections of their own (SPEC.md §5.7, §5.8). A reader that
    // fetched them per query would pay for the separation twice.
    // Completions too: whether a prefix has an entry decides what it costs,
    // and has to be known before anything is read for it (SPEC.md §4.1.1).
    for (const id of [SECTION.COMPLETIONS, SECTION.DOC_LENGTHS, SECTION.DOC_ATTRIBUTES]) {
      if (index.sections.has(id)) wanted.push(id);
    }
    for (const id of wanted) index.section(id);

    // The recommended layout puts everything resident at the head of the file,
    // so when the opening prefix does not reach far enough the remedy is one
    // wider read rather than one read per section.
    const spanEnd = Math.max(
      ...wanted.map((id) => {
        const s = index.section(id);
        return s.offset + s.length;
      }),
    );
    let span = prefix;
    if (spanEnd > prefix.length) {
      // Only the part not already in hand. Re-reading the prefix would double
      // the bytes for no reason, and the opening cost is on screen.
      const tail = await index.read(prefix.length, spanEnd - prefix.length);
      span = new Uint8Array(prefix.length + tail.length);
      span.set(prefix);
      span.set(tail, prefix.length);
    }

    const resident = async (id: number): Promise<Uint8Array> => {
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
    } else {
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
    let previous: Uint8Array = new Uint8Array(0);
    for (let i = 0; i < index.blockCount; i++) {
      previous = cursor.key(previous);
      index.boundaries.push(previous);
    }
    return index;
  }

  private offsetPair(table: Uint8Array, base: number, i: number): [number, number] {
    const view = new DataView(table.buffer, table.byteOffset);
    const start = Number(view.getBigUint64(base + i * 8, true));
    const end = Number(view.getBigUint64(base + i * 8 + 8, true));
    return [start, end - start];
  }

  async termBlock(ordinal: number): Promise<TermEntry[]> {
    const [offset, length] = this.offsetPair(this.termBlockOffsets, 0, ordinal);
    const raw = await this.read(offset, length);
    const block = await this.unit(raw, this.section(SECTION.TERM_BLOCKS).codec);

    const cursor = new Cursor(block);
    const entryCount = cursor.varint();
    const firstTermId = cursor.varint();
    const hasPositions = Boolean(this.flags & FLAG_HAS_POSITIONS);
    const hasFreqs = Boolean(this.flags & FLAG_HAS_FREQS);
    const entries: TermEntry[] = [];
    let key: Uint8Array = new Uint8Array(0);
    let postingsOffset: number | null = null;
    let positionsOffset: number | null = null;
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
      const entry: TermEntry = {
        term: key,
        termId,
        docFreq,
        postingsOffset,
        postingsLength: stored === INLINE ? 0 : stored,
      };
      if (stored === IMPLIED) {
        expected = termId + 1;
      } else if (stored === INLINE) {
        if (docFreq === 0) throw new Error("inline postings name no document");
        const docs = [expected + unzigzag(cursor.varint())];
        for (let j = 1; j < docFreq; j++) docs.push(docs[j - 1]! + cursor.varint() + 1);
        if (docs[0]! < 0 || docs[docs.length - 1]! >= this.numDocs) {
          throw new Error("inline postings name a missing document");
        }
        const freqs = docs.map(() => (hasFreqs ? cursor.varint() + 1 : 1));
        entry.inline = { docs, freqs };
        expected = docs[docs.length - 1]! + 1;
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
  column(name: string): Column | undefined {
    return this.columns.get(name);
  }

  /** Where a term occurs in each document its postings name.
   *
   * One read, and only for a caller that asks. The run is decoded from the
   * front and the frequencies say how many values belong to each document, so
   * the postings have to be in hand first.
   */
  async positions(entry: TermEntry, postings: Postings): Promise<number[][]> {
    if (!(this.flags & FLAG_HAS_POSITIONS) || !entry.positionsLength) return [];
    const run = await this.read(entry.positionsOffset ?? 0, entry.positionsLength);
    const cursor = new Cursor(run);
    const out: number[][] = [];
    for (const freq of postings.freqs) {
      let current: number | null = null;
      out.push(
        cursor.values(freq).map((value) => {
          // The first is a position; the rest are gaps less one, so that
          // adjacent occurrences pack to nothing.
          current = current === null ? value : current + value + 1;
          return current;
        }),
      );
    }
    if (cursor.pos !== run.length) throw new Error("trailing bytes in positions run");
    return out;
  }

  /** The dictionary block a term would be in, from the resident index alone. */
  blockFor(term: Uint8Array): number {
    let start = 0;
    for (let i = 0; i < this.boundaries.length; i++) {
      if (compare(this.boundaries[i]!, term) <= 0) start = i;
    }
    return start;
  }

  /** One term, exactly, or null.
   *
   * A prefix scan would answer this, and would cost more: `xn76ur` is a prefix
   * of every cell inside it, so completing it fetches postings for all of them
   * when the caller wanted one. Exactly one block is read, always.
   */
  async exact(termText: string): Promise<TermEntry | null> {
    const term = encoder.encode(termText);
    for (const entry of await this.termBlock(this.blockFor(term))) {
      const order = compare(entry.term, term);
      if (order === 0) return entry;
      if (order > 0) break;
    }
    return null;
  }

  /** Several terms at once, each exactly, in one read per dictionary block.
   *
   * Cells one geohash precision apart are neighbours in the dictionary, so the
   * lookups a point needs usually land in the same block. Asking for them
   * together is the difference between reading it once and reading it twice.
   */
  async exactMany(termTexts: string[]): Promise<(TermEntry | null)[]> {
    const wanted = termTexts.map((t) => {
      const term = encoder.encode(t);
      return { term, block: this.blockFor(term) };
    });
    const blocks = new Map<number, TermEntry[]>();
    for (const { block } of wanted) {
      if (!blocks.has(block)) blocks.set(block, await this.termBlock(block));
    }
    return wanted.map(({ term, block }) => {
      for (const entry of blocks.get(block)!) {
        const order = compare(entry.term, term);
        if (order === 0) return entry;
        if (order > 0) break;
      }
      return null;
    });
  }

  /** Terms beginning with `prefix`, at most `limit` of them. */
  async prefix(prefixText: string, limit = 20): Promise<TermEntry[]> {
    const prefix = encoder.encode(prefixText);
    let start = 0;
    for (let i = 0; i < this.boundaries.length; i++) {
      if (compare(this.boundaries[i]!, prefix) <= 0) start = i;
    }

    const found: TermEntry[] = [];
    for (let ordinal = start; ordinal < this.blockCount; ordinal++) {
      // Whether the range continues is answerable from the resident index
      // alone, so only the first block is fetched on faith.
      if (ordinal > start && !startsWith(this.boundaries[ordinal]!, prefix)) break;

      let done = false;
      for (const entry of await this.termBlock(ordinal)) {
        if (startsWith(entry.term, prefix)) {
          found.push(entry);
          if (found.length === limit) return found;
        } else if (compare(entry.term, prefix) > 0 && found.length > 0) {
          done = true;
          break;
        }
      }
      if (done) break;
    }
    return found;
  }

  /** Terms from `low` to `high`, both inclusive, at most `limit` of them.
   *
   * The same walk as `prefix`, with a different test for where it stops: the
   * dictionary is sorted, so a range of keys is a run of neighbouring entries,
   * and whether the run continues past a block is answerable from the resident
   * index alone.
   */
  async range(lowText: string, highText: string, limit = Infinity): Promise<TermEntry[]> {
    const low = encoder.encode(lowText);
    const high = encoder.encode(highText);
    const start = this.blockFor(low);

    const found: TermEntry[] = [];
    for (let ordinal = start; ordinal < this.blockCount; ordinal++) {
      if (ordinal > start && compare(this.boundaries[ordinal]!, high) > 0) break;
      for (const entry of await this.termBlock(ordinal)) {
        if (compare(entry.term, high) > 0) return found;
        if (compare(entry.term, low) >= 0) {
          found.push(entry);
          if (found.length === limit) return found;
        }
      }
    }
    return found;
  }

  // The postings a dictionary entry states itself (SPEC.md §5.3.4): inline,
  // or implied — the term names one document, whose ID is the term's own, once.
  implied(entry: TermEntry): Postings {
    if (entry.inline) return entry.inline;
    if (entry.docFreq !== 1) throw new Error("implied postings need a doc_freq of 1");
    if (entry.termId >= this.numDocs) throw new Error("implied postings name a missing document");
    return { docs: [entry.termId], freqs: [1] };
  }

  /** Postings for a contiguous range of entries, in one request — or none. */
  async postingsSpan(entries: TermEntry[]): Promise<Postings[]> {
    if (entries.length === 0) return [];
    // Entries whose postings are implied or inline contribute no bytes, so an
    // index of a dictionary spends no request here at all.
    const stored = entries.filter((e) => e.postingsLength > 0);
    let start = 0;
    let span: Uint8Array | null = null;
    if (stored.length > 0) {
      const first = stored[0]!;
      const last = stored[stored.length - 1]!;
      start = first.postingsOffset;
      span = await this.read(start, last.postingsOffset + last.postingsLength - start);
    }

    const hasFreqs = Boolean(this.flags & FLAG_HAS_FREQS);
    return entries.map((entry) => {
      if (entry.postingsLength === 0) return this.implied(entry);
      const at = entry.postingsOffset - start;
      const cursor = new Cursor(span!.subarray(at, at + entry.postingsLength));
      if (cursor.bytes[cursor.pos++] !== 0) throw new Error("unsupported postings codec");
      const count = cursor.varint();
      const docs: number[] = [];
      let current: number | null = null;
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
  async payloadSpan(docs: number[]): Promise<string[]> {
    if (!(this.flags & FLAG_HAS_PAYLOAD) || !this.payloadOffsets) return docs.map(() => "");
    const header = this.payloadOffsets;
    const view = new DataView(header.buffer, header.byteOffset);
    const codec = header[0];
    const valuesPerDoc = header[1]!;
    // A signed byte: the stored integers stand for value * 10^exponent.
    const exponent = (header[2]! << 24) >> 24;
    const perBlock = view.getUint32(4, true);

    // Every block the documents fall in, fetched at once. They are independent
    // of each other, so reading them one after another would cost a round
    // trip each for nothing: measured over the network, ten results in eight
    // blocks took 273 ms in sequence against one round trip in parallel.
    const ordinals = [...new Set(docs.map((doc) => Math.floor(doc / perBlock)))];
    const blocks = new Map<number, Uint8Array>(
      await Promise.all(
        ordinals.map(async (ordinal): Promise<[number, Uint8Array]> => {
          const [offset, length] = this.offsetPair(header, 12, ordinal);
          const raw = await this.read(offset, length);
          return [ordinal, await this.unit(raw, this.section(SECTION.PAYLOAD).codec)];
        }),
      ),
    );

    const out: string[] = [];
    for (const doc of docs) {
      const ordinal = Math.floor(doc / perBlock);
      const cached = blocks.get(ordinal)!;

      const cursor = new Cursor(cached);
      const count = cursor.varint();
      const index = doc % perBlock;
      if (index >= count) throw new Error("payload block is shorter than expected");

      if (codec === PAYLOAD_OPAQUE) {
        for (let i = 0; i < count; i++) {
          const length = cursor.varint();
          if (i === index) {
            out.push(utf8.decode(cached.subarray(cursor.pos, cursor.pos + length)));
            break;
          }
          cursor.pos += length;
        }
      } else if (codec === PAYLOAD_DELTA_INTS) {
        // Every entry up to the wanted one has to be decoded, because each is a
        // difference from the one before it. A block is what gets fetched
        // anyway, and it is sized so that this is cheap.
        const values = new Array<number>(valuesPerDoc).fill(0);
        for (let entry = 0; entry <= index; entry++) {
          for (let i = 0; i < valuesPerDoc; i++) {
            const delta = unzigzag(cursor.varint());
            values[i] = entry === 0 ? delta : values[i]! + delta;
          }
        }
        out.push(values.map((v) => scaled(v, exponent)).join(","));
      } else {
        throw new Error(`payload codec ${codec} is not implemented here`);
      }
    }
    return out;
  }

  /** The first `limit` documents for what has been typed.
   *
   * From the completions section when the prefix has an entry that reaches
   * the limit — or lists fewer than it may, so lists them all — which costs no
   * request before the payload. Otherwise every term the prefix matches, whose
   * runs are united and returned in document order.
   */
  async complete(typed: string, limit = 10): Promise<Completion> {
    const normalized = this.normalize(typed);
    const { docs, fromTable } = await this.completeDocs(normalized, limit);
    const requestsBeforePayload = this.stats.requests;
    const payloads = await this.payloadSpan(docs);
    return { normalized, fromTable, docs, payloads, requestsBeforePayload };
  }

  /** `complete` for a prefix already normalized, without the payload. */
  async completeDocs(normalized: string, limit: number): Promise<{ docs: number[]; fromTable: boolean }> {
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
  async lookup(
    prefixText: string,
    limit = 20,
  ): Promise<{ normalized: string; results: { term: string; doc: number; payload: string }[] }> {
    const normalized = this.normalize(prefixText);
    const entries = await this.prefix(normalized, limit);
    const postings = await this.postingsSpan(entries);

    // A term may name more than one document — in a dictionary it never does,
    // but `tiny` is not a dictionary, and returning only the first would drop
    // results silently. Documents are collected in ascending order so that the
    // payload reads share blocks.
    const wanted = [...new Set(postings.flatMap((p) => p.docs))].sort((a, b) => a - b);
    const payloads = new Map<number, string>();
    const fetched = await this.payloadSpan(wanted);
    wanted.forEach((doc, i) => payloads.set(doc, fetched[i]!));

    const results: { term: string; doc: number; payload: string }[] = [];
    entries.forEach((entry, i) => {
      for (const doc of postings[i]!.docs) {
        results.push({ term: utf8.decode(entry.term), doc, payload: payloads.get(doc)! });
      }
    });
    return { normalized, results };
  }
}

/** What `MokujiIndex.complete` found, and how. */
export interface Completion {
  normalized: string;
  /** Whether the completions section answered, at no read before the payload. */
  fromTable: boolean;
  docs: number[];
  payloads: string[];
  requestsBeforePayload: number;
}

// --- a collection ---------------------------------------------------------

/** One segment of a collection, as the manifest lists it (MANIFEST.md §2). */
export interface ManifestSegment {
  path: string;
  content_hash: string;
  length: number;
  deletes?: number[];
  region?: { countries: string[] };
}

/** A collection's manifest (MANIFEST.md §2). */
export interface Manifest {
  mokuji_manifest: string;
  generation: number;
  segments: ManifestSegment[];
}

/** One document of a collection: its segment, and its ID there. */
export interface Hit {
  segment: number;
  doc: number;
}

/** What a collection's completion found, and how. */
export interface CollectionCompletion {
  normalized: string;
  hits: Hit[];
  consulted: { segment: number; fromTable: boolean }[];
  payloads: string[];
  requestsBeforePayload: number;
}

/**
 * A collection: segments in a manifest's order, less their deletes
 * (MANIFEST.md). Every segment is an ordinary Mokuji file, opened on its own.
 */
export class MokujiCollection {
  readonly deletes: Set<number>[];
  /** How many segments the manifest lists, before a region left some out. */
  manifestSegments = 0;

  constructor(
    readonly manifest: Manifest,
    readonly segments: MokujiIndex[],
  ) {
    this.deletes = manifest.segments.map((s) => new Set(s.deletes ?? []));
  }

  /**
   * Opens the manifest at `url` and every segment it names, at once: they are
   * independent, so opening them costs one round trip however many there are.
   * `sourceFor(url)` may supply a byte source instead of fetching.
   */
  static async open(
    url: string | Manifest,
    sourceFor: (url: string) => string | Source = (u) => u,
    { countries = null }: { countries?: string[] | null } = {},
  ): Promise<MokujiCollection> {
    let manifest: Manifest =
      typeof url === "string" ? ((await (await fetch(url, { cache: "no-store" })).json()) as Manifest) : url;
    const listed = manifest.segments.length;
    // About a place: only the segments whose region includes it, and those
    // with none (MANIFEST.md §7). The others are never opened.
    if (countries) {
      manifest = {
        ...manifest,
        segments: manifest.segments.filter(
          (s) => !s.region || s.region.countries.some((c) => countries.includes(c)),
        ),
      };
    }
    if (String(manifest.mokuji_manifest ?? "").split(".")[0] !== "0") {
      throw new Error(`manifest version ${manifest.mokuji_manifest} is not one this reader implements`);
    }
    const here = (globalThis as { location?: { href: string } }).location?.href;
    const base = typeof url === "string" ? new URL(url, here) : null;
    const segments = await Promise.all(
      manifest.segments.map(async (s) => {
        const index = await MokujiIndex.open(sourceFor(base ? new URL(s.path, base).href : s.path));
        if (index.fileLength !== undefined && index.fileLength !== s.length) {
          throw new Error(`${s.path} is ${index.fileLength} bytes, not the ${s.length} the manifest says`);
        }
        const deletes = s.deletes ?? [];
        if (deletes.some((d, i) => d >= index.numDocs || (i > 0 && d <= deletes[i - 1]!))) {
          throw new Error(`${s.path} has deletes that are out of order or out of range`);
        }
        return index;
      }),
    );
    const tokenizer = JSON.stringify(field(segments[0]?.meta, "tokenizer"));
    for (const [i, index] of segments.entries()) {
      if (JSON.stringify(field(index.meta, "tokenizer")) !== tokenizer) {
        throw new Error(`${manifest.segments[i]!.path} was tokenized differently from the first segment`);
      }
    }
    const collection = new MokujiCollection(manifest, segments);
    collection.manifestSegments = listed;
    return collection;
  }

  resetStats(): void {
    for (const index of this.segments) index.resetStats();
  }

  /** Requests and bytes, summed; and the most requests any one segment made. */
  get stats(): Stats & { roundTrips: number } {
    const per = this.segments.map((s) => s.stats);
    return {
      requests: per.reduce((a, s) => a + s.requests, 0),
      bytes: per.reduce((a, s) => a + s.bytes, 0),
      roundTrips: Math.max(0, ...per.map((s) => s.requests)),
    };
  }

  normalize(typed: string): string {
    return this.segments[0]!.normalize(typed);
  }

  /** One segment's first `need` live documents: the entry, then all it
   * lists, then as many again as the segment has deletes (MANIFEST.md §3.2). */
  async liveIn(at: number, normalized: string, need: number): Promise<{ live: number[]; fromTable: boolean }> {
    const index = this.segments[at]!;
    const deleted = this.deletes[at]!;
    const listed = index.completions?.docsPerEntry ?? 0;
    let fromTable = true;
    let live: number[] = [];
    for (const ask of [need, Math.max(listed, need), need + deleted.size]) {
      const found = await index.completeDocs(normalized, ask);
      fromTable &&= found.fromTable;
      live = found.docs.filter((d) => !deleted.has(d));
      if (live.length >= need || found.docs.length < ask) break;
    }
    return { live: live.slice(0, need), fromTable };
  }

  /** The first `limit` live documents, in collection order. */
  async complete(typed: string, limit = 10): Promise<CollectionCompletion> {
    const normalized = this.normalize(typed);
    const hits: Hit[] = [];
    const consulted: { segment: number; fromTable: boolean }[] = [];
    for (let at = 0; at < this.segments.length && hits.length < limit; at++) {
      const { live, fromTable } = await this.liveIn(at, normalized, limit - hits.length);
      consulted.push({ segment: at, fromTable });
      for (const doc of live) hits.push({ segment: at, doc });
    }
    const requestsBeforePayload = this.stats.requests;
    return { normalized, hits, consulted, payloads: await this.payloads(hits), requestsBeforePayload };
  }

  /** Every segment's own first `limit`, asked at once, for a caller that
   * ranks across segments by a score of its own (MANIFEST.md §3.3). */
  async candidates(typed: string, limit = 10): Promise<CollectionCompletion> {
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
  async payloads(hits: Hit[]): Promise<string[]> {
    const out = new Array<string>(hits.length);
    await Promise.all(
      this.segments.map(async (index, at) => {
        const mine = hits.map((h, i): [Hit, number] => [h, i]).filter(([h]) => h.segment === at);
        if (mine.length === 0) return;
        mine.sort(([a], [b]) => a.doc - b.doc);
        const payloads = await index.payloadSpan(mine.map(([h]) => h.doc));
        mine.forEach(([, i], j) => (out[i] = payloads[j]!));
      }),
    );
    return out;
  }

  /** Each segment's credit, once each, for a page that shows results. */
  get attributions(): string[] {
    return [...new Set(this.segments.map((s) => s.attribution).filter((a): a is string => Boolean(a)))];
  }
}
