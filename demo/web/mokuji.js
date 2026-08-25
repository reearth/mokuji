// A Mokuji reader in the browser, in plain JavaScript.
//
// This is the third implementation of SPEC.md, after the Rust one and the
// Python fixture builder, and the first written against a transport the format
// was designed for rather than a file already in memory. If it reads what the
// others wrote, the specification survives contact with a third reader.
//
// It bundles no decompressor. The required codec is raw deflate precisely so
// that a page can use DecompressionStream and stay this small.

const MAGIC = "MOKUJIv0";
const HEADER_LEN = 64;
const SECTION_ENTRY_LEN = 24;

const FLAG_HAS_PAYLOAD = 1 << 1;
const FLAG_HAS_FREQS = 1 << 2;
const FLAG_HAS_POSITIONS = 1 << 3;

const SECTION = {
  META: 0x0001,
  TERM_INDEX: 0x0010,
  TERM_BLOCKS: 0x0011,
  TERM_BLOCK_OFFSETS: 0x0012,
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

function unzigzag(v) {
  return v % 2 === 0 ? v / 2 : -(v + 1) / 2;
}

/** Writes `value * 10^exponent` in full, keeping the zeros the file claims. */
function scaled(value, exponent) {
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

// --- metadata -------------------------------------------------------------

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
  if (small < 24) [argument, pos] = [small, pos + 1];
  else if (small === 24) [argument, pos] = [bytes[pos + 1], pos + 2];
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
        map[key] = value;
        pos = afterValue;
      }
      return [map, pos];
    }
    default:
      throw new Error(`metadata uses CBOR major type ${major}`);
  }
}

// --- ja-address -----------------------------------------------------------

const JA_NUMERALS = new Map([
  ["〇", 0], ["零", 0], ["一", 1], ["二", 2], ["三", 3], ["四", 4], ["五", 5],
  ["六", 6], ["七", 7], ["八", 8], ["九", 9], ["十", 10], ["百", 100], ["千", 1000],
]);

// See TOKENIZERS.md. 十 百 千 multiply the digit before them, or one when there
// is none; two bare digits in sequence are positional notation, which this rule
// does not claim to read.
function parseNumeral(run) {
  let total = 0;
  let digit = null;
  for (const c of run) {
    const value = JA_NUMERALS.get(c);
    if (value >= 10) {
      total += (digit ?? 1) * value;
      digit = null;
    } else {
      if (digit !== null) return null;
      digit = value;
    }
  }
  return total + (digit ?? 0);
}

// Characters a Japanese place name is written with in more than one way. See
// TOKENIZERS.md: `丸ノ内` and `丸の内` are the same place and the registry
// spells both, so both have to fold onto one before anything is compared.
const JA_VARIANTS = new Map([
  ["ノ", "の"],
  ["ケ", "ヶ"],
  ["ヵ", "ヶ"],
]);

const isKatakana = (c) => c !== undefined && c >= "\u30A0" && c <= "\u30FF";

/** Folds the variant spellings, but only where they are connectives.
 *
 * Inside a katakana word these characters are letters: `マルノウチ` is not
 * `マルのウチ`, and `ケーキ` is not `ヶーキ`.
 */
function foldVariants(chars) {
  return chars.map((c, i) => {
    // A kanji is never part of a katakana word, so it needs no guard.
    if (c === "之") return "の";
    const folded = JA_VARIANTS.get(c);
    if (folded === undefined) return c;
    if (isKatakana(chars[i - 1]) || isKatakana(chars[i + 1])) return c;
    return folded;
  });
}

/** The `ja-address` tokenizer, applied to a query. */
export function jaAddress(text) {
  const chars = foldVariants([...text.normalize("NFKC").trim()]);
  let out = "";
  let i = 0;
  while (i < chars.length) {
    const start = i;
    while (i < chars.length && JA_NUMERALS.has(chars[i])) i++;
    if (i === start) {
      out += chars[i++];
      continue;
    }
    const run = chars.slice(start, i).join("");
    const value = chars[i] === "丁" && chars[i + 1] === "目" ? parseNumeral(run) : null;
    out += value === null ? run : String(value);
  }
  return out;
}

const TOKENIZERS = {
  "space-split": (q) => q,
  identity: (q) => q,
  bigram: (q) => q,
  "ja-address": jaAddress,
  // The same normalization as `ja-address`; only the token boundary differs,
  // and a prefix is not split in either.
  "ja-address-bigram": jaAddress,
};

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

class Cursor {
  constructor(bytes, pos = 0) {
    this.bytes = bytes;
    this.pos = pos;
  }

  varint() {
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
  key(previous) {
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
        if (this.bytes[this.pos + (at >> 3)] & (1 << (at & 7))) value += scale;
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
    if (a[i] !== b[i]) return a[i] - b[i];
  }
  return a.length - b.length;
}

function startsWith(key, prefix) {
  if (key.length < prefix.length) return false;
  for (let i = 0; i < prefix.length; i++) {
    if (key[i] !== prefix[i]) return false;
  }
  return true;
}

async function inflate(unit) {
  const cursor = new Cursor(unit);
  const length = cursor.varint();
  const stream = new Blob([unit.subarray(cursor.pos)])
    .stream()
    .pipeThrough(new DecompressionStream("deflate-raw"));
  const out = new Uint8Array(await new Response(stream).arrayBuffer());
  if (out.length !== length) throw new Error("unit does not match its declared length");
  return out;
}

export class MokujiIndex {
  /**
   * @param source a URL, or `{ read(offset, length) }` returning bytes — which
   * is what lets the same reader be tested against a file on disk.
   */
  constructor(source) {
    this.source = source;
    // Everything a query costs, so the page can show it rather than claim it.
    this.stats = { requests: 0, bytes: 0 };
    // Called with every range request, so a page can show the requests
    // themselves rather than a count of them.
    this.onRequest = null;
  }

  resetStats() {
    this.stats = { requests: 0, bytes: 0 };
  }

  async read(offset, length) {
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
          `Mokuji needs range requests to be honoured`
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

  async unit(bytes, codec) {
    if (codec === CODEC_RAW) return bytes;
    if (codec === CODEC_DEFLATE) return inflate(bytes);
    throw new Error(`codec ${codec} is registered but not implemented here`);
  }

  static async open(source) {
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
        codec: table[at + 2],
        offset: Number(tableView.getBigUint64(at + 8, true)),
        length: Number(tableView.getBigUint64(at + 16, true)),
      });
    }

    const wanted = [SECTION.META, SECTION.TERM_INDEX, SECTION.TERM_BLOCK_OFFSETS];
    if (index.flags & FLAG_HAS_PAYLOAD) wanted.push(SECTION.PAYLOAD_OFFSETS);
    // Both are read in bulk before any payload is, which is what makes them
    // worth having as sections of their own (SPEC.md §5.7, §5.8). A reader that
    // fetched them per query would pay for the separation twice.
    for (const id of [SECTION.DOC_LENGTHS, SECTION.DOC_ATTRIBUTES]) {
      if (index.sections.has(id)) wanted.push(id);
    }
    for (const id of wanted) {
      if (!index.sections.has(id)) throw new Error(`section ${id} is missing`);
    }

    // The recommended layout puts everything resident at the head of the file,
    // so when the opening prefix does not reach far enough the remedy is one
    // wider read rather than one read per section.
    const spanEnd = Math.max(...wanted.map((id) => {
      const s = index.sections.get(id);
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
      const section = index.sections.get(id);
      return index.unit(
        span.subarray(section.offset, section.offset + section.length),
        section.codec
      );
    };

    index.meta = cbor(await resident(SECTION.META))[0];
    index.tokenizerId = index.meta?.tokenizer?.id ?? "identity";
    index.normalize = TOKENIZERS[index.tokenizerId];
    if (!index.normalize) {
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

    index.termIndex = await resident(SECTION.TERM_INDEX);
    index.termBlockOffsets = await resident(SECTION.TERM_BLOCK_OFFSETS);
    if (index.flags & FLAG_HAS_PAYLOAD) {
      index.payloadOffsets = await resident(SECTION.PAYLOAD_OFFSETS);
    }

    // Boundary keys, decoded once. They are what lets a prefix scan know
    // whether the next block can extend the range without fetching it.
    const cursor = new Cursor(index.termIndex, 4);
    index.blockCount = new DataView(
      index.termIndex.buffer,
      index.termIndex.byteOffset
    ).getUint32(0, true);
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

  async termBlock(ordinal) {
    const [offset, length] = this.offsetPair(this.termBlockOffsets, 0, ordinal);
    const raw = await this.read(offset, length);
    const block = await this.unit(raw, this.sections.get(SECTION.TERM_BLOCKS).codec);

    const cursor = new Cursor(block);
    const entryCount = cursor.varint();
    const firstTermId = cursor.varint();
    const hasPositions = Boolean(this.flags & FLAG_HAS_POSITIONS);
    const entries = [];
    let key = new Uint8Array(0);
    let postingsOffset = null;
    let positionsOffset = null;
    for (let i = 0; i < entryCount; i++) {
      key = cursor.key(key);
      const docFreq = cursor.varint();
      const delta = cursor.varint();
      postingsOffset = postingsOffset === null ? delta : postingsOffset + delta;
      const entry = {
        term: key,
        termId: firstTermId + i,
        docFreq,
        postingsOffset,
        postingsLength: cursor.varint(),
      };
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
    if (!(this.flags & FLAG_HAS_POSITIONS) || entry.positionsLength === 0) return [];
    const run = await this.read(entry.positionsOffset, entry.positionsLength);
    const cursor = new Cursor(run);
    const out = [];
    for (const freq of postings.freqs) {
      let current = null;
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
  blockFor(term) {
    let start = 0;
    for (let i = 0; i < this.boundaries.length; i++) {
      if (compare(this.boundaries[i], term) <= 0) start = i;
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
  async exactMany(termTexts) {
    const wanted = termTexts.map((text) => {
      const term = encoder.encode(text);
      return { term, block: this.blockFor(term) };
    });
    const blocks = new Map();
    for (const { block } of wanted) {
      if (!blocks.has(block)) blocks.set(block, await this.termBlock(block));
    }
    return wanted.map(({ term, block }) => {
      for (const entry of blocks.get(block)) {
        const order = compare(entry.term, term);
        if (order === 0) return entry;
        if (order > 0) break;
      }
      return null;
    });
  }

  /** Terms beginning with `prefix`, at most `limit` of them. */
  async prefix(prefixText, limit = 20) {
    const prefix = encoder.encode(prefixText);
    let start = 0;
    for (let i = 0; i < this.boundaries.length; i++) {
      if (compare(this.boundaries[i], prefix) <= 0) start = i;
    }

    const found = [];
    for (let ordinal = start; ordinal < this.blockCount; ordinal++) {
      // Whether the range continues is answerable from the resident index
      // alone, so only the first block is fetched on faith.
      if (ordinal > start && !startsWith(this.boundaries[ordinal], prefix)) break;

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

  // The postings a dictionary entry implies by carrying no run (SPEC.md
  // §5.3.4): the term names one document, whose ID is the term's own, once.
  implied(entry) {
    if (entry.docFreq !== 1) throw new Error("implied postings need a doc_freq of 1");
    if (entry.termId >= this.numDocs) throw new Error("implied postings name a missing document");
    return { docs: [entry.termId], freqs: [1] };
  }

  /** Postings for a contiguous range of entries, in one request — or none. */
  async postingsSpan(entries) {
    if (entries.length === 0) return [];
    // Entries with implied postings contribute no bytes, so an index of a
    // dictionary spends no request here at all.
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
      if (entry.postingsLength === 0) return this.implied(entry);
      const at = entry.postingsOffset - start;
      const cursor = new Cursor(span.subarray(at, at + entry.postingsLength));
      if (cursor.bytes[cursor.pos++] !== 0) throw new Error("unsupported postings codec");
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

  /** Payload for documents given in ascending order, sharing blocks. */
  async payloadSpan(docs) {
    if (!(this.flags & FLAG_HAS_PAYLOAD)) return docs.map(() => "");
    const header = this.payloadOffsets;
    const view = new DataView(header.buffer, header.byteOffset);
    const codec = header[0];
    const valuesPerDoc = header[1];
    // A signed byte: the stored integers stand for value * 10^exponent.
    const exponent = (header[2] << 24) >> 24;
    const perBlock = view.getUint32(4, true);

    const out = [];
    let cachedOrdinal = null;
    let cached = null;
    for (const doc of docs) {
      const ordinal = Math.floor(doc / perBlock);
      if (ordinal !== cachedOrdinal) {
        const [offset, length] = this.offsetPair(header, 12, ordinal);
        const raw = await this.read(offset, length);
        cached = await this.unit(raw, this.sections.get(SECTION.PAYLOAD).codec);
        cachedOrdinal = ordinal;
      }

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
        const values = new Array(valuesPerDoc).fill(0);
        for (let entry = 0; entry <= index; entry++) {
          for (let i = 0; i < valuesPerDoc; i++) {
            const delta = unzigzag(cursor.varint());
            values[i] = entry === 0 ? delta : values[i] + delta;
          }
        }
        out.push(values.map((v) => scaled(v, exponent)).join(","));
      } else {
        throw new Error(`payload codec ${codec} is not implemented here`);
      }
    }
    return out;
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
