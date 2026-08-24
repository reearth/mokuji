#!/usr/bin/env python3
"""Build the `tiny` conformance fixture directly from SPEC.md.

This is deliberately an independent implementation. If the fixture were emitted
by `colex-write`, a mistake in the writer would simply be mirrored by the
reader that was developed against it, and the pair would agree with each other
rather than with the specification. Written twice, in two languages, from the
prose alone, a disagreement is visible.

It also serves as the first check that the specification is implementable by
someone holding nothing but the document.

Usage: python3 build_tiny.py [--check]
"""

import argparse
import hashlib
import pathlib
import sys

HERE = pathlib.Path(__file__).parent

MAGIC = b"COLEXv00"
SPEC_MAJOR, SPEC_MINOR = 0, 1
HEADER_LEN = 64
SECTION_ENTRY_LEN = 24

FLAG_TERMDICT_MONOLITHIC = 1 << 0
FLAG_HAS_PAYLOAD = 1 << 1
FLAG_HAS_FREQS = 1 << 2

SEC_META = 0x0001
SEC_TERM_INDEX = 0x0010
SEC_TERM_BLOCKS = 0x0011
SEC_TERM_BLOCK_OFFSETS = 0x0012
SEC_POSTINGS = 0x0020
SEC_PAYLOAD = 0x0030
SEC_PAYLOAD_OFFSETS = 0x0031

# Small enough that every structure in the format appears more than once, which
# is what makes a fixture worth having: a single-block dictionary would never
# exercise the block search, and a single-document posting list would never
# exercise a gap.
TERMS_PER_BLOCK = 2
DOCS_PER_PAYLOAD_BLOCK = 2

# The corpus. Deliberately shaped so that the dictionary has a shared prefix to
# front-code ("chiyoda" / "chuo"), a term in every document, and a term in one.
DOCUMENTS = [
    ("tokyo chiyoda", "35.694,139.754"),
    ("tokyo chuo", "35.671,139.772"),
    ("tokyo shibuya", "35.664,139.698"),
    ("osaka chuo", "34.681,135.513"),
]


# --- primitives ------------------------------------------------------------


def varint(value):
    assert value >= 0
    out = bytearray()
    while True:
        byte = value & 0x7F
        value >>= 7
        out.append(byte | 0x80 if value else byte)
        if not value:
            return bytes(out)


def read_varint(buf, pos):
    value = shift = 0
    while True:
        byte = buf[pos]
        pos += 1
        value |= (byte & 0x7F) << shift
        if not byte & 0x80:
            return value, pos
        shift += 7


def front_coded(key, previous):
    shared = 0
    limit = min(len(key), len(previous))
    while shared < limit and key[shared] == previous[shared]:
        shared += 1
    suffix = key[shared:]
    return varint(shared) + varint(len(suffix)) + suffix


def read_front_coded(buf, pos, previous):
    shared, pos = read_varint(buf, pos)
    length, pos = read_varint(buf, pos)
    assert shared <= len(previous), "shared_len exceeds the preceding key"
    return previous[:shared] + buf[pos : pos + length], pos + length


def pack_group(values):
    """One bit-packed group: minimal width, least significant bit first."""
    width = max((v.bit_length() for v in values), default=0)
    acc = 0
    for i, v in enumerate(values):
        assert v.bit_length() <= width
        acc |= v << (i * width)
    nbytes = (len(values) * width + 7) // 8
    return bytes([width]) + acc.to_bytes(nbytes, "little")


def unpack_group(buf, pos, count):
    width = buf[pos]
    pos += 1
    nbytes = (count * width + 7) // 8
    acc = int.from_bytes(buf[pos : pos + nbytes], "little")
    mask = (1 << width) - 1
    values = [(acc >> (i * width)) & mask for i in range(count)]
    assert acc >> (count * width) == 0, "padding bits are not zero"
    return values, pos + nbytes


def pack_values(values):
    return b"".join(pack_group(values[i : i + 128]) for i in range(0, len(values), 128))


def unpack_values(buf, pos, count):
    out = []
    while len(out) < count:
        group, pos = unpack_group(buf, pos, min(128, count - len(out)))
        out.extend(group)
    return out, pos


# --- canonical CBOR (RFC 8949 section 4.2) ---------------------------------


def cbor_head(major, value):
    if value < 24:
        return bytes([major << 5 | value])
    for extra, size in ((24, 1), (25, 2), (26, 4), (27, 8)):
        if value < 1 << (size * 8):
            return bytes([major << 5 | extra]) + value.to_bytes(size, "big")
    raise ValueError(value)


def cbor(value):
    if isinstance(value, bool):
        raise TypeError("no booleans in this fixture")
    if isinstance(value, int):
        return cbor_head(0, value)
    if isinstance(value, str):
        raw = value.encode()
        return cbor_head(3, len(raw)) + raw
    if isinstance(value, list):
        return cbor_head(4, len(value)) + b"".join(cbor(v) for v in value)
    if isinstance(value, dict):
        # Deterministic encoding orders keys by their encoded bytes.
        items = sorted(((cbor(k), cbor(v)) for k, v in value.items()))
        return cbor_head(5, len(items)) + b"".join(k + v for k, v in items)
    raise TypeError(type(value))


# --- index construction ----------------------------------------------------


def tokenize(text):
    """The fixture's tokenizer: split on spaces. Recorded, not specified."""
    return text.split(" ")


def build_index():
    postings = {}
    for doc_id, (text, _) in enumerate(DOCUMENTS):
        for token in tokenize(text):
            postings.setdefault(token, {})
            postings[token][doc_id] = postings[token].get(doc_id, 0) + 1
    return {term: postings[term] for term in sorted(postings)}


def encode_postings(docs):
    doc_ids = sorted(docs)
    values = [doc_ids[0]] + [
        doc_ids[i] - doc_ids[i - 1] - 1 for i in range(1, len(doc_ids))
    ]
    freqs = [docs[d] - 1 for d in doc_ids]
    return bytes([0]) + varint(len(doc_ids)) + pack_values(values) + pack_values(freqs)


def encode_term_block(entries, first_term_id, postings_offsets):
    out = bytearray(varint(len(entries)) + varint(first_term_id))
    previous_key = b""
    previous_offset = None
    for (term, doc_freq, length), offset in zip(entries, postings_offsets):
        out += front_coded(term.encode(), previous_key)
        out += varint(doc_freq)
        out += varint(offset if previous_offset is None else offset - previous_offset)
        out += varint(length)
        previous_key = term.encode()
        previous_offset = offset
    return bytes(out)


def encode_payload_block(values):
    out = bytearray(varint(len(values)))
    for value in values:
        raw = value.encode()
        out += varint(len(raw)) + raw
    return bytes(out)


def chunk(items, size):
    return [items[i : i + size] for i in range(0, len(items), size)]


def build():
    index = build_index()
    terms = list(index)
    runs = {term: encode_postings(index[term]) for term in terms}
    term_chunks = chunk(terms, TERMS_PER_BLOCK)

    payload_blocks = [
        encode_payload_block([p for _, p in group])
        for group in chunk(DOCUMENTS, DOCS_PER_PAYLOAD_BLOCK)
    ]

    term_index = bytearray(len(term_chunks).to_bytes(4, "little"))
    previous = b""
    for group in term_chunks:
        term_index += front_coded(group[0].encode(), previous)
        previous = group[0].encode()
    term_index = bytes(term_index)

    corpus = (HERE / "tiny.corpus.txt").read_bytes()
    meta = cbor(
        {
            "generation": 1,
            "source": [
                {
                    "content_hash": "sha2-256:" + hashlib.sha256(corpus).hexdigest(),
                    "uri": "conformance/fixtures/tiny.corpus.txt",
                }
            ],
            "generator": {
                "name": "build_tiny.py",
                "version": "0.1",
                "params": {
                    "terms_per_block": TERMS_PER_BLOCK,
                    "docs_per_payload_block": DOCS_PER_PAYLOAD_BLOCK,
                },
            },
            "tokenizer": {"id": "space-split", "params": {}},
        }
    )

    # Postings are placed before the term blocks that point into them, so a
    # postings offset is known before any term block is built. With the
    # opposite order each depends on the size of the other, which has no fixed
    # point once blocks are compressed.
    order = [
        SEC_META,
        SEC_TERM_INDEX,
        SEC_TERM_BLOCK_OFFSETS,
        SEC_PAYLOAD_OFFSETS,
        SEC_POSTINGS,
        SEC_TERM_BLOCKS,
        SEC_PAYLOAD,
    ]
    head = HEADER_LEN + SECTION_ENTRY_LEN * len(order)
    postings_at = (
        head
        + len(meta)
        + len(term_index)
        + 8 * (len(term_chunks) + 1)
        + 8
        + 8 * (len(payload_blocks) + 1)
    )

    offsets, cursor = {}, postings_at
    for term in terms:
        offsets[term] = cursor
        cursor += len(runs[term])

    blocks, term_id = [], 0
    for group in term_chunks:
        entries = [(t, len(index[t]), len(runs[t])) for t in group]
        blocks.append(encode_term_block(entries, term_id, [offsets[t] for t in group]))
        term_id += len(group)

    sizes = {
        SEC_META: len(meta),
        SEC_TERM_INDEX: len(term_index),
        SEC_TERM_BLOCK_OFFSETS: 8 * (len(term_chunks) + 1),
        SEC_PAYLOAD_OFFSETS: 8 + 8 * (len(payload_blocks) + 1),
        SEC_POSTINGS: sum(len(runs[t]) for t in terms),
        SEC_TERM_BLOCKS: sum(len(b) for b in blocks),
        SEC_PAYLOAD: sum(len(b) for b in payload_blocks),
    }
    placed, cursor = {}, head
    for sid in order:
        placed[sid] = cursor
        cursor += sizes[sid]
    assert placed[SEC_POSTINGS] == postings_at

    term_block_offsets = b"".join(
        (placed[SEC_TERM_BLOCKS] + o).to_bytes(8, "little")
        for o in running_offsets(blocks)
    )
    payload_offsets = (
        DOCS_PER_PAYLOAD_BLOCK.to_bytes(4, "little")
        + len(payload_blocks).to_bytes(4, "little")
        + b"".join(
            (placed[SEC_PAYLOAD] + o).to_bytes(8, "little")
            for o in running_offsets(payload_blocks)
        )
    )

    bodies = {
        SEC_META: meta,
        SEC_TERM_INDEX: term_index,
        SEC_TERM_BLOCK_OFFSETS: term_block_offsets,
        SEC_PAYLOAD_OFFSETS: payload_offsets,
        SEC_TERM_BLOCKS: b"".join(blocks),
        SEC_POSTINGS: b"".join(runs[t] for t in terms),
        SEC_PAYLOAD: b"".join(payload_blocks),
    }
    for sid, body in bodies.items():
        assert len(body) == sizes[sid], sid

    header = bytearray(HEADER_LEN)
    header[0:8] = MAGIC
    header[8] = SPEC_MAJOR
    header[9] = SPEC_MINOR
    header[10:12] = (FLAG_HAS_PAYLOAD | FLAG_HAS_FREQS).to_bytes(2, "little")
    header[12:16] = len(order).to_bytes(4, "little")
    header[16:24] = len(DOCUMENTS).to_bytes(8, "little")
    header[24:32] = sum(len(tokenize(t)) for t, _ in DOCUMENTS).to_bytes(8, "little")
    header[32:40] = HEADER_LEN.to_bytes(8, "little")
    header[40:44] = (SECTION_ENTRY_LEN * len(order)).to_bytes(4, "little")

    table = bytearray()
    regions = [(0, HEADER_LEN, "header")]
    for sid in sorted(bodies):
        entry = bytearray(SECTION_ENTRY_LEN)
        entry[0:2] = sid.to_bytes(2, "little")
        entry[2] = 0  # raw
        entry[8:16] = placed[sid].to_bytes(8, "little")
        entry[16:24] = sizes[sid].to_bytes(8, "little")
        table += entry
    regions.append((HEADER_LEN, len(table), "section table"))

    out = bytearray(header + table)
    for sid in order:
        assert len(out) == placed[sid]
        regions.append((placed[sid], sizes[sid], section_name(sid)))
        out += bodies[sid]
    return bytes(out), regions


def running_offsets(blocks):
    offsets, cursor = [], 0
    for block in blocks:
        offsets.append(cursor)
        cursor += len(block)
    offsets.append(cursor)
    return offsets


def section_name(sid):
    return {
        SEC_META: "meta",
        SEC_TERM_INDEX: "term index",
        SEC_TERM_BLOCKS: "term blocks",
        SEC_TERM_BLOCK_OFFSETS: "term block offsets",
        SEC_POSTINGS: "postings",
        SEC_PAYLOAD: "payload",
        SEC_PAYLOAD_OFFSETS: "payload offsets",
    }[sid]


# --- reading back ----------------------------------------------------------


class CountingFile:
    """Reads ranges and counts them, the way a conformance reader does."""

    def __init__(self, data):
        self.data = data
        self.reads = 0

    def read(self, offset, length):
        assert offset + length <= len(self.data), "range outside the file"
        self.reads += 1
        return self.data[offset : offset + length]


def load_resident(f):
    header = f.read(0, HEADER_LEN)
    assert header[0:8] == MAGIC
    assert header[8] == SPEC_MAJOR
    count = int.from_bytes(header[12:16], "little")
    table_offset = int.from_bytes(header[32:40], "little")
    table_len = int.from_bytes(header[40:44], "little")
    table = f.read(table_offset, table_len)

    sections, previous_id = {}, None
    for i in range(count):
        entry = table[i * SECTION_ENTRY_LEN : (i + 1) * SECTION_ENTRY_LEN]
        sid = int.from_bytes(entry[0:2], "little")
        assert previous_id is None or sid > previous_id, "table is not sorted"
        previous_id = sid
        sections[sid] = (
            int.from_bytes(entry[8:16], "little"),
            int.from_bytes(entry[16:24], "little"),
        )

    resident = {sid: f.read(*sections[sid]) for sid in sorted(sections) if sid in
                (SEC_META, SEC_TERM_INDEX, SEC_TERM_BLOCK_OFFSETS, SEC_PAYLOAD_OFFSETS)}
    return header, sections, resident


def find_block(term_index, term):
    """The greatest boundary key not exceeding `term`; None if it precedes all."""
    count = int.from_bytes(term_index[0:4], "little")
    pos, previous, found = 4, b"", None
    for ordinal in range(count):
        key, pos = read_front_coded(term_index, pos, previous)
        assert key > previous or ordinal == 0, "boundary keys are not increasing"
        if key <= term:
            found = ordinal
        previous = key
    return found


def lookup(f, resident, ordinal, term):
    offsets = resident[SEC_TERM_BLOCK_OFFSETS]
    start = int.from_bytes(offsets[ordinal * 8 : ordinal * 8 + 8], "little")
    end = int.from_bytes(offsets[ordinal * 8 + 8 : ordinal * 8 + 16], "little")
    block = f.read(start, end - start)

    count, pos = read_varint(block, 0)
    first_term_id, pos = read_varint(block, pos)
    key, offset = b"", None
    for i in range(count):
        key, pos = read_front_coded(block, pos, key)
        doc_freq, pos = read_varint(block, pos)
        delta, pos = read_varint(block, pos)
        offset = delta if offset is None else offset + delta
        length, pos = read_varint(block, pos)
        if key == term:
            return first_term_id + i, doc_freq, offset, length
    return None


def decode_postings(run, has_freqs):
    assert run[0] == 0, "unexpected postings codec"
    count, pos = read_varint(run, 1)
    values, pos = unpack_values(run, pos, count)
    doc_ids, current = [], None
    for v in values:
        current = v if current is None else current + v + 1
        doc_ids.append(current)
    freqs = [1] * count
    if has_freqs:
        raw, pos = unpack_values(run, pos, count)
        freqs = [v + 1 for v in raw]
    assert pos == len(run), "trailing bytes in postings run"
    return dict(zip(doc_ids, freqs))


def payload_of(f, resident, doc_id, cache):
    section = resident[SEC_PAYLOAD_OFFSETS]
    per_block = int.from_bytes(section[0:4], "little")
    ordinal, index = divmod(doc_id, per_block)
    # Results are visited in document ID order and payload is stored in that
    # order, so documents sharing a block share a read. This is the difference
    # between m and m' in SPEC section 4.
    if ordinal not in cache:
        base = 8 + ordinal * 8
        start = int.from_bytes(section[base : base + 8], "little")
        end = int.from_bytes(section[base + 8 : base + 16], "little")
        cache[ordinal] = f.read(start, end - start)
    block = cache[ordinal]

    count, pos = read_varint(block, 0)
    assert index < count
    for i in range(count):
        length, pos = read_varint(block, pos)
        if i == index:
            return block[pos : pos + length].decode()
        pos += length
    raise AssertionError("unreachable")


def search(data, query):
    """An intersection query, reporting what it cost."""
    f = CountingFile(data)
    header, _, resident = load_resident(f)
    has_freqs = bool(int.from_bytes(header[10:12], "little") & FLAG_HAS_FREQS)
    f.reads = 0  # the resident set is accounted for separately

    results = None
    for term in query:
        ordinal = find_block(resident[SEC_TERM_INDEX], term.encode())
        if ordinal is None:
            return {}, f.reads
        entry = lookup(f, resident, ordinal, term.encode())
        if entry is None:
            return {}, f.reads
        _, doc_freq, offset, length = entry
        docs = decode_postings(f.read(offset, length), has_freqs)
        assert len(docs) == doc_freq, "doc_freq disagrees with the postings run"
        results = docs.keys() if results is None else results & docs.keys()

    cache = {}
    return {d: payload_of(f, resident, d, cache) for d in sorted(results)}, f.reads


# --- output ----------------------------------------------------------------


def hexdump(data, regions):
    lines = []
    for offset, length, label in regions:
        lines.append(f"\n{label} ({length} bytes at {offset})")
        for base in range(offset, offset + length, 16):
            row = data[base : min(base + 16, offset + length)]
            hexpart = " ".join(f"{b:02x}" for b in row)
            text = "".join(chr(b) if 32 <= b < 127 else "." for b in row)
            lines.append(f"  {base:08x}  {hexpart:<47}  |{text}|")
    return "\n".join(lines).lstrip() + "\n"


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="verify, do not write")
    args = parser.parse_args()

    (HERE / "tiny.corpus.txt").write_text(
        "".join(f"{text}\t{payload}\n" for text, payload in DOCUMENTS)
    )
    data, regions = build()

    # Every case in cases.toml, checked here as well, so that a change to the
    # fixture cannot quietly invalidate the expectations the Rust reader is
    # held to.
    expectations = [
        # query, expected result, expected reads
        (["tokyo"], {0: "35.694,139.754", 1: "35.671,139.772", 2: "35.664,139.698"}, 4),
        (["tokyo", "chuo"], {1: "35.671,139.772"}, 5),
        (["chiyoda"], {0: "35.694,139.754"}, 3),
        (["osaka", "tokyo"], {}, 4),
        # An absent term still costs the block read that proves it absent.
        (["kyoto"], {}, 1),
    ]
    for query, expected, expected_reads in expectations:
        got, reads = search(data, query)
        assert got == expected, (query, got, expected)
        bound = 2 * len(query) + len(expected)
        assert reads <= bound, (query, reads, bound)
        assert reads == expected_reads, (query, reads, expected_reads)

    dump = hexdump(data, regions)
    if args.check:
        if (HERE / "tiny.colex").read_bytes() != data:
            sys.exit("tiny.colex is stale; re-run build_tiny.py")
        if (HERE / "tiny.hex").read_text() != dump:
            sys.exit("tiny.hex is stale; re-run build_tiny.py")
        print(f"fixture is current ({len(data)} bytes)")
        return

    (HERE / "tiny.colex").write_bytes(data)
    (HERE / "tiny.hex").write_text(dump)
    print(f"wrote tiny.colex ({len(data)} bytes) and tiny.hex")


if __name__ == "__main__":
    main()
