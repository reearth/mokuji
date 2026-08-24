// A COLex reader in the browser, in plain JavaScript.
//
// This is the third implementation of SPEC.md, after the Rust one and the
// Python fixture builder, and the first written against a transport the format
// was designed for rather than a file already in memory. If it reads what the
// others wrote, the specification survives contact with a third reader.
//
// It bundles no decompressor. The required codec is raw deflate precisely so
// that a page can use DecompressionStream and stay this small.

const MAGIC = "COLEXv00";
const HEADER_LEN = 64;
const SECTION_ENTRY_LEN = 24;

const FLAG_TERMDICT_MONOLITHIC = 1 << 0;
const FLAG_HAS_PAYLOAD = 1 << 1;
const FLAG_HAS_FREQS = 1 << 2;

const SECTION = {
  META: 0x0001,
  TERM_INDEX: 0x0010,
  TERM_BLOCKS: 0x0011,
  TERM_BLOCK_OFFSETS: 0x0012,
  POSTINGS: 0x0020,
  PAYLOAD: 0x0030,
  PAYLOAD_OFFSETS: 0x0031,
};

const CODEC_RAW = 0;
const CODEC_DEFLATE = 1;

// How much to ask for when opening. The resident set is sized to be fetched in
// one request; this is a reader's guess at how large that is, not a property of
// the format.
const OPEN_PREFIX = 256 * 1024;

const utf8 = new TextDecoder();
const encoder = new TextEncoder();

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
      // COLex file in practice has them, and pretending otherwise would be
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

export class ColexIndex {
  constructor(url) {
    this.url = url;
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
    const response = await fetch(this.url, {
      headers: { Range: `bytes=${offset}-${offset + length - 1}` },
    });
    if (response.status !== 206) {
      throw new Error(
        `the server ignored the Range header (status ${response.status}); ` +
          `COLex needs range requests to be honoured`
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

  static async open(url) {
    const index = new ColexIndex(url);
    // One request for the resident set, optimistically. SPEC.md §5 recommends
    // a layout that puts all of it at the head of the file for exactly this.
    const prefix = await index.read(0, OPEN_PREFIX);

    const view = new DataView(prefix.buffer, prefix.byteOffset);
    if (utf8.decode(prefix.subarray(0, 8)) !== MAGIC) throw new Error("not a COLex file");
    if (prefix[8] !== 0) throw new Error(`unsupported major version ${prefix[8]}`);

    index.flags = view.getUint16(10, true);
    if (index.flags & FLAG_TERMDICT_MONOLITHIC) {
      throw new Error("a monolithic term dictionary cannot be read over range requests");
    }
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

    const resident = async (id) => {
      const section = index.sections.get(id);
      if (!section) throw new Error(`section ${id} is missing`);
      const raw =
        section.offset + section.length <= prefix.length
          ? prefix.subarray(section.offset, section.offset + section.length)
          : await index.read(section.offset, section.length);
      return index.unit(raw, section.codec);
    };

    index.meta = await resident(SECTION.META);
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
    const entries = [];
    let key = new Uint8Array(0);
    let postingsOffset = null;
    for (let i = 0; i < entryCount; i++) {
      key = cursor.key(key);
      const docFreq = cursor.varint();
      const delta = cursor.varint();
      postingsOffset = postingsOffset === null ? delta : postingsOffset + delta;
      entries.push({
        term: key,
        termId: firstTermId + i,
        docFreq,
        postingsOffset,
        postingsLength: cursor.varint(),
      });
    }
    return entries;
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
    if (!(this.flags & FLAG_HAS_PAYLOAD)) return docs.map(() => new Uint8Array(0));
    const view = new DataView(this.payloadOffsets.buffer, this.payloadOffsets.byteOffset);
    const perBlock = view.getUint32(0, true);

    const out = [];
    let cachedOrdinal = null;
    let cached = null;
    for (const doc of docs) {
      const ordinal = Math.floor(doc / perBlock);
      if (ordinal !== cachedOrdinal) {
        const [offset, length] = this.offsetPair(this.payloadOffsets, 8, ordinal);
        const raw = await this.read(offset, length);
        cached = await this.unit(raw, this.sections.get(SECTION.PAYLOAD).codec);
        cachedOrdinal = ordinal;
      }
      const cursor = new Cursor(cached);
      const count = cursor.varint();
      const index = doc % perBlock;
      if (index >= count) throw new Error("payload block is shorter than expected");
      for (let i = 0; i < count; i++) {
        const length = cursor.varint();
        if (i === index) {
          out.push(cached.subarray(cursor.pos, cursor.pos + length));
          break;
        }
        cursor.pos += length;
      }
    }
    return out;
  }

  /** Complete a prefix, returning terms with their payload. */
  async lookup(prefixText, limit = 20) {
    const entries = await this.prefix(prefixText, limit);
    const postings = await this.postingsSpan(entries);
    const docs = postings.map((p) => p.docs[0]);
    const payloads = await this.payloadSpan(docs);
    return entries.map((entry, i) => ({
      term: utf8.decode(entry.term),
      doc: docs[i],
      payload: utf8.decode(payloads[i]),
    }));
  }
}
