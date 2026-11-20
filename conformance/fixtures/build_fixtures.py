#!/usr/bin/env python3
"""Build the `tiny` conformance fixture directly from SPEC.md.

This is deliberately an independent implementation. If the fixture were emitted
by `mokuji-write`, a mistake in the writer would simply be mirrored by the
reader that was developed against it, and the pair would agree with each other
rather than with the specification. Written twice, in two languages, from the
prose alone, a disagreement is visible.

It also serves as the first check that the specification is implementable by
someone holding nothing but the document.

Usage: python3 build_fixtures.py [--check]
"""

import argparse
import hashlib
import json
import pathlib
import re
import sys
import unicodedata

HERE = pathlib.Path(__file__).parent

MAGIC = b"MOKUJIv0"
SPEC_MAJOR, SPEC_MINOR = 0, 1
HEADER_LEN = 64
SECTION_ENTRY_LEN = 24

FLAG_TERMDICT_MONOLITHIC = 1 << 0
FLAG_HAS_PAYLOAD = 1 << 1
FLAG_HAS_FREQS = 1 << 2
FLAG_HAS_POSITIONS = 1 << 3

SEC_META = 0x0001
SEC_TERM_INDEX = 0x0010
SEC_TERM_BLOCKS = 0x0011
SEC_TERM_BLOCK_OFFSETS = 0x0012
SEC_COMPLETIONS = 0x0013
SEC_POSTINGS = 0x0020
SEC_PAYLOAD = 0x0030
SEC_PAYLOAD_OFFSETS = 0x0031
SEC_DOC_LENGTHS = 0x0032
SEC_DOC_ATTRIBUTES = 0x0033
SEC_POSITIONS = 0x0060

# Small enough that every structure in the format appears more than once, which
# is what makes a fixture worth having: a single-block dictionary would never
# exercise the block search, and a single-document posting list would never
# exercise a gap.
TERMS_PER_BLOCK = 2
DOCS_PER_PAYLOAD_BLOCK = 2
# The most bytes a term's postings may take written in its dictionary entry
# (SPEC.md 5.3.4), the same bound the reference writer uses by default, so that
# both write the same bytes. A term is inlined only when that is also no longer
# than its run.
INLINE_POSTINGS = 8
INLINE = 1  # the postings_len that means "inline"

PAYLOAD_OPAQUE = 0
PAYLOAD_DELTA_INTS = 1

# The corpus. Deliberately shaped so that the dictionary has a shared prefix to
# front-code ("chiyoda" / "chuo"), a term in every document, and a term in one.
# A dictionary index rather than a document collection: one term names one
# record, the terms are already normalized, and the payload is a pair of
# coordinates rather than bytes. This is the shape an address index has, and
# the one the `tiny` fixture does not exercise.
# In term order, which for a dictionary is the order the documents have to be
# in: a term's postings are implied only when the document it names carries the
# term's own ID (SPEC.md 5.3.4).
DICT_DOCUMENTS = [
    ("大阪府大阪市中央区本町1丁目1番1号", "135.501000,34.682000"),
    ("東京都千代田区丸の内1丁目1番1号", "139.767201,35.681560"),
    ("東京都千代田区丸の内1丁目1番2号", "139.767258,35.681604"),
    ("東京都千代田区丸の内2丁目1番1号", "139.763447,35.680022"),
    ("東京都港区赤坂1丁目1番1号", "139.740331,35.667012"),
]

# Places under all their names, separated by U+001F for `name-list`. Numbered
# most important first, so that the completions this script writes — the
# smallest document IDs of a prefix — are the ones worth showing. `東京` and
# `東北` share their first character, so its bytes are prefixes too, including
# the two that stop inside it.
NAME_DOCUMENTS = [
    ("Black Sea\x1fČerné moře", "Black Sea"),
    ("Black Forest\x1fSchwarzwald", "Black Forest"),
    ("Blackpool", "Blackpool"),
    ("Bled", "Bled"),
    ("Tokyo\x1f東京", "Tokyo"),
    ("Tohoku\x1f東北", "Tohoku"),
]

# A segment added to `names` later (MANIFEST.md): a new place, and a new one
# with a name in another script.
NAME_DELTA_DOCUMENTS = [
    ("Black Mountain", "Black Mountain"),
    ("Tokyo Tower\x1f東京タワー", "Tokyo Tower"),
]

# `names`, then `names-delta`, with Black Sea — the first document every `b`
# entry of `names` lists — deleted. The delta's places are all in Japan, and
# the manifest says so; `names` states no region, so it is asked whatever the
# place (MANIFEST.md 7).
COLLECTION = [("names", [0], None), ("names-delta", [], ["JP"])]

# prefix, limit, expected (segment, document, payload) triples, reads, and
# optionally the countries the query is about
COLLECTION_CASES = [
    # The `bl` entry lists Black Sea and Black Forest. With Black Sea deleted it
    # is short, and lists no more than that, so `names` reads its dictionary.
    ("bl", 2, [(0, 1, "Black Forest"), (0, 2, "Blackpool")], 5),
    # `names` has two live `black` places and the delta a third, after them.
    ("black", 10, [(0, 1, "Black Forest"), (0, 2, "Blackpool"), (1, 0, "Black Mountain")], 7),
    # The base fills the limit, so the delta is never asked.
    ("t", 2, [(0, 4, "Tokyo"), (0, 5, "Tohoku")], 1),
    # Nothing in `names`: one block proves it. In the delta, the prefix sorts
    # before the boundary key `東京タワー`, so the block before it is read on
    # faith (SPEC.md 4.1), then that one, then its run and the payload.
    ("東京タ", 10, [(1, 1, "Tokyo Tower")], 5),
    # About France, so the delta is not opened: its region is Japan. `names`
    # is, and proves the prefix absent with one block.
    ("東京タ", 10, [], 1, ["FR"]),
    # About Japan: both, and the same answer as with no place given.
    ("東京タ", 10, [(1, 1, "Tokyo Tower")], 5, ["JP", "KR"]),
]

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


# Unicode White_Space, spelled out. Python's str.isspace() is not it: it also
# counts U+001C to U+001F, which would make the name-list separator a space.
WHITE_SPACE = re.compile(
    "[\u0009-\u000d\u0020\u0085\u00a0\u1680\u2000-\u200a"
    "\u2028\u2029\u202f\u205f\u3000]+"
)


def name(text):
    """One name as TOKENIZERS.md's `name-list` normalizes it."""
    lowered = unicodedata.normalize("NFKC", text).lower()
    return " ".join(w for w in WHITE_SPACE.split(lowered) if w)


def tokenize(text, tokenizer_id):
    """The fixture's tokenizers. Recorded, not specified."""
    if tokenizer_id == "identity":
        return [text]
    if tokenizer_id == "name-list":
        return [n for n in map(name, text.split("\x1f")) if n]
    assert tokenizer_id == "space-split", tokenizer_id
    return text.split(" ")


def build_index(documents, tokenizer_id):
    """Term to document to the positions it occupies there."""
    postings = {}
    for doc_id, (text, _) in enumerate(documents):
        for at, token in enumerate(tokenize(text, tokenizer_id)):
            postings.setdefault(token, {}).setdefault(doc_id, []).append(at)
    # Byte order, which is what the dictionary is sorted by; for text outside
    # ASCII it is not the order Python sorts strings in.
    return {term: postings[term] for term in sorted(postings, key=str.encode)}


def zigzag(v):
    return (v << 1) ^ (v >> 63) if v < 0 else v << 1


def scale(text, decimals):
    """A decimal number as an integer of 10^-decimals units, rounded half up."""
    negative = text.startswith("-")
    whole, _, fraction = text.lstrip("+-").partition(".")
    value = int(whole or 0)
    for i in range(decimals):
        value = value * 10 + (int(fraction[i]) if i < len(fraction) else 0)
    if len(fraction) > decimals and fraction[decimals] >= "5":
        value += 1
    return -value if negative else value


def encode_positions(docs):
    """One run per term: each document's positions, gaps after the first."""
    out = bytearray()
    for doc_id in sorted(docs):
        places = docs[doc_id]
        values = [places[0]] + [places[i] - places[i - 1] - 1 for i in range(1, len(places))]
        out += pack_values(values)
    return bytes(out)


def encode_postings(docs, term_id):
    doc_ids = sorted(docs)
    # The dictionary can state this by itself: one document, whose ID is this
    # term's own, at frequency one (SPEC.md section 5.3.4). Writing it out would
    # record nothing and cost a range read to read back.
    if doc_ids == [term_id] and len(docs[term_id]) == 1:
        return b""

    values = [doc_ids[0]] + [
        doc_ids[i] - doc_ids[i - 1] - 1 for i in range(1, len(doc_ids))
    ]
    freqs = [len(docs[d]) - 1 for d in doc_ids]
    return bytes([0]) + varint(len(doc_ids)) + pack_values(values) + pack_values(freqs)


def inline_postings(index, terms, runs, bound):
    """Moves short postings into their entries (SPEC.md 5.3.4).

    Per block, since each entry's first document is predicted from the last
    document an earlier implied or inline entry in the same block named.
    Returns the inline bytes by term, and empties the runs of the terms moved.
    """
    inline = {}
    for start in range(0, len(terms), TERMS_PER_BLOCK):
        expected = start
        for term_id in range(start, min(start + TERMS_PER_BLOCK, len(terms))):
            term = terms[term_id]
            if not runs[term]:  # implied: the document is the term's own ID
                expected = term_id + 1
                continue
            docs = sorted(index[term])
            out = bytearray(varint(zigzag(docs[0] - expected)))
            for a, b in zip(docs, docs[1:]):
                out += varint(b - a - 1)
            for d in docs:
                out += varint(len(index[term][d]) - 1)
            if len(out) <= bound and len(out) <= len(runs[term]):
                inline[term] = bytes(out)
                runs[term] = b""
                expected = docs[-1] + 1
    return inline


def encode_term_block(entries, first_term_id, postings_offsets, positions_offsets=None):
    out = bytearray(varint(len(entries)) + varint(first_term_id))
    previous_key = b""
    previous_offset = None
    previous_positions = None
    for i, ((term, doc_freq, length, inline), offset) in enumerate(zip(entries, postings_offsets)):
        out += front_coded(term.encode(), previous_key)
        out += varint(doc_freq)
        out += varint(offset if previous_offset is None else offset - previous_offset)
        if inline is not None:
            out += varint(INLINE) + inline
        else:
            out += varint(length)
        if positions_offsets is not None:
            at, plen = positions_offsets[i]
            out += varint(at if previous_positions is None else at - previous_positions)
            out += varint(plen)
            previous_positions = at
        previous_key = term.encode()
        previous_offset = offset
    return bytes(out)


def encode_payload_block(values, decimals=None):
    """One payload block, opaque or as delta-coded integers (SPEC.md 5.5)."""
    out = bytearray(varint(len(values)))
    if decimals is None:
        for value in values:
            raw = value.encode()
            out += varint(len(raw)) + raw
        return bytes(out)

    previous = None
    for value in values:
        row = [scale(field, decimals) for field in value.split(",")]
        for i, v in enumerate(row):
            # Deltas never cross a block boundary: a block is addressed on its
            # own and must decode on its own.
            out += varint(zigzag(v if previous is None else v - previous[i]))
        previous = row
    return bytes(out)


def encode_completions(index, runs, min_terms, max_span, docs_per_entry):
    """The completions section (SPEC.md 5.3.5), by brute force.

    Every non-empty byte prefix of every term is tried against the definition,
    rather than walked the way a writer might to avoid trying them: the point
    of this script is to follow the prose, not to be quick.
    """
    keys = {term.encode(): term for term in index}
    prefixes = {key[:n] for key in keys for n in range(1, len(key) + 1)}
    entries = []
    for prefix in sorted(prefixes):
        matched = [term for key, term in keys.items() if key.startswith(prefix)]
        span = sum(len(runs[term]) for term in matched)
        if not ((min_terms and len(matched) >= min_terms) or (max_span and span >= max_span)):
            continue
        # This script's choice of documents: the smallest IDs, which are the
        # most important because the fixture was numbered that way.
        docs = sorted({doc for term in matched for doc in index[term]})[:docs_per_entry]
        entries.append((prefix, docs))

    out = bytearray(varint(min_terms) + varint(max_span) + varint(docs_per_entry))
    out += varint(len(entries))
    previous = b""
    for prefix, docs in entries:
        out += front_coded(prefix, previous)
        previous = prefix
        out += varint(len(docs))
        last = 0
        for doc in docs:
            out += varint(zigzag(doc - last))
            last = doc
    return bytes(out)


def chunk(items, size):
    return [items[i : i + size] for i in range(0, len(items), size)]


def build(documents=None, decimals=None, corpus_name="tiny.corpus.txt",
          tokenizer_id="space-split", doc_lengths=False, columns=None,
          positions=False, extra=None, completions=None, license=None,
          inline=INLINE_POSTINGS):
    documents = DOCUMENTS if documents is None else documents
    index = build_index(documents, tokenizer_id)
    terms = list(index)
    runs = {term: encode_postings(index[term], i) for i, term in enumerate(terms)}
    bound = inline
    inline = inline_postings(index, terms, runs, bound)
    places = {term: encode_positions(index[term]) for term in terms} if positions else None
    term_chunks = chunk(terms, TERMS_PER_BLOCK)

    payload_blocks = [
        encode_payload_block([p for _, p in group], decimals)
        for group in chunk(documents, DOCS_PER_PAYLOAD_BLOCK)
    ]
    values_per_doc = 0 if decimals is None else len(documents[0][1].split(","))

    # The length in tokens of each document, which is what a scorer needs and
    # the header cannot give (SPEC.md 5.7).
    lengths = None
    if doc_lengths:
        lengths = pack_values([len(tokenize(t, tokenizer_id)) for t, _ in documents])

    # Named columns of per-document numbers (SPEC.md 5.8): payload that can be
    # read in bulk and read early.
    attributes = None
    if columns:
        out = bytearray(varint(len(columns)))
        for name, exponent, _ in columns:
            raw = name.encode()
            out += varint(len(raw)) + raw + bytes([exponent & 0xFF])
        for _, _, values in columns:
            out += pack_values([zigzag(v) for v in values])
        attributes = bytes(out)

    term_index = bytearray(len(term_chunks).to_bytes(4, "little"))
    previous = b""
    for group in term_chunks:
        term_index += front_coded(group[0].encode(), previous)
        previous = group[0].encode()
    term_index = bytes(term_index)

    completed = encode_completions(index, runs, *completions) if completions else None

    corpus = (HERE / corpus_name).read_bytes()
    meta = cbor(
        {
            "generation": 1,
            "source": [
                {
                    "content_hash": "sha2-256:" + hashlib.sha256(corpus).hexdigest(),
                    "uri": f"conformance/fixtures/{corpus_name}",
                }
            ],
            "generator": {
                "name": "build_fixtures.py",
                "version": "0.1",
                "params": {
                    "terms_per_block": TERMS_PER_BLOCK,
                    "docs_per_payload_block": DOCS_PER_PAYLOAD_BLOCK,
                    "payload": "opaque" if decimals is None else f"ints:{decimals}",
                    "doc_lengths": 1 if doc_lengths else 0,
                    "positions": 1 if positions else 0,
                    "inline_postings": bound,
                    **(
                        {"completions": "{},{},{} by document order".format(*completions)}
                        if completions
                        else {}
                    ),
                    **(
                        {"attributes": [f"{n}:{-e}" for n, e, _ in columns]}
                        if columns
                        else {}
                    ),
                },
            },
            "tokenizer": {"id": tokenizer_id, "params": {}},
            # The index's license and the credit its results carry (SPEC.md 6).
            **({"license": license[0], "attribution": license[1]} if license else {}),
        }
    )

    # Postings are placed before the term blocks that point into them, so a
    # postings offset is known before any term block is built. With the
    # opposite order each depends on the size of the other, which has no fixed
    # point once blocks are compressed.
    order = [SEC_META, SEC_TERM_INDEX]
    # Resident, and depends on no offset, so it sits with the term index.
    if completed is not None:
        order.append(SEC_COMPLETIONS)
    order += [SEC_TERM_BLOCK_OFFSETS, SEC_PAYLOAD_OFFSETS]
    if lengths is not None:
        order.append(SEC_DOC_LENGTHS)
    if attributes is not None:
        order.append(SEC_DOC_ATTRIBUTES)
    order.append(SEC_POSTINGS)
    if places is not None:
        order.append(SEC_POSITIONS)
    order += [SEC_TERM_BLOCKS, SEC_PAYLOAD]
    # Sections this version does not define, placed last so that everything a
    # reader looks for is where it would be without them. A conformant reader
    # ignores both (SPEC.md 5.2); they are here so that "ignores" is tested
    # rather than assumed.
    extra = dict(extra or {})
    order += sorted(extra)
    head = HEADER_LEN + SECTION_ENTRY_LEN * len(order)
    postings_at = (
        head
        + len(meta)
        + len(term_index)
        + (len(completed) if completed is not None else 0)
        + 8 * (len(term_chunks) + 1)
        + 12
        + 8 * (len(payload_blocks) + 1)
        + (len(lengths) if lengths is not None else 0)
        + (len(attributes) if attributes is not None else 0)
    )

    offsets, cursor = {}, postings_at
    for term in terms:
        offsets[term] = cursor
        cursor += len(runs[term])

    positions_at = cursor
    place_offsets = {}
    if places is not None:
        cursor = positions_at
        for term in terms:
            place_offsets[term] = (cursor, len(places[term]))
            cursor += len(places[term])

    blocks, term_id = [], 0
    for group in term_chunks:
        entries = [(t, len(index[t]), len(runs[t]), inline.get(t)) for t in group]
        blocks.append(
            encode_term_block(
                entries,
                term_id,
                [offsets[t] for t in group],
                [place_offsets[t] for t in group] if places is not None else None,
            )
        )
        term_id += len(group)

    sizes = {
        SEC_META: len(meta),
        **({SEC_DOC_LENGTHS: len(lengths)} if lengths is not None else {}),
        **({SEC_DOC_ATTRIBUTES: len(attributes)} if attributes is not None else {}),
        SEC_TERM_INDEX: len(term_index),
        **({SEC_COMPLETIONS: len(completed)} if completed is not None else {}),
        SEC_TERM_BLOCK_OFFSETS: 8 * (len(term_chunks) + 1),
        SEC_PAYLOAD_OFFSETS: 12 + 8 * (len(payload_blocks) + 1),
        SEC_POSTINGS: sum(len(runs[t]) for t in terms),
        **({SEC_POSITIONS: sum(len(places[t]) for t in terms)} if places is not None else {}),
        SEC_TERM_BLOCKS: sum(len(b) for b in blocks),
        SEC_PAYLOAD: sum(len(b) for b in payload_blocks),
        **{sid: len(body) for sid, body in extra.items()},
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
        bytes(
            [
                PAYLOAD_OPAQUE if decimals is None else PAYLOAD_DELTA_INTS,
                values_per_doc,
                (-decimals) & 0xFF if decimals is not None else 0,
                0,  # reserved
            ]
        )
        + DOCS_PER_PAYLOAD_BLOCK.to_bytes(4, "little")
        + len(payload_blocks).to_bytes(4, "little")
        + b"".join(
            (placed[SEC_PAYLOAD] + o).to_bytes(8, "little")
            for o in running_offsets(payload_blocks)
        )
    )

    bodies = {
        SEC_META: meta,
        **({SEC_DOC_LENGTHS: lengths} if lengths is not None else {}),
        **({SEC_DOC_ATTRIBUTES: attributes} if attributes is not None else {}),
        SEC_TERM_INDEX: term_index,
        **({SEC_COMPLETIONS: completed} if completed is not None else {}),
        SEC_TERM_BLOCK_OFFSETS: term_block_offsets,
        SEC_PAYLOAD_OFFSETS: payload_offsets,
        SEC_TERM_BLOCKS: b"".join(blocks),
        SEC_POSTINGS: b"".join(runs[t] for t in terms),
        **({SEC_POSITIONS: b"".join(places[t] for t in terms)} if places is not None else {}),
        SEC_PAYLOAD: b"".join(payload_blocks),
        **extra,
    }
    for sid, body in bodies.items():
        assert len(body) == sizes[sid], sid

    header = bytearray(HEADER_LEN)
    header[0:8] = MAGIC
    header[8] = SPEC_MAJOR
    header[9] = SPEC_MINOR
    flags = FLAG_HAS_PAYLOAD | FLAG_HAS_FREQS
    if places is not None:
        flags |= FLAG_HAS_POSITIONS
    header[10:12] = flags.to_bytes(2, "little")
    header[12:16] = len(order).to_bytes(4, "little")
    header[16:24] = len(documents).to_bytes(8, "little")
    header[24:32] = sum(len(tokenize(t, tokenizer_id)) for t, _ in documents).to_bytes(
        8, "little"
    )
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
        SEC_COMPLETIONS: "completions",
        SEC_POSTINGS: "postings",
        SEC_POSITIONS: "positions",
        SEC_PAYLOAD: "payload",
        SEC_PAYLOAD_OFFSETS: "payload offsets",
        SEC_DOC_LENGTHS: "document lengths",
        SEC_DOC_ATTRIBUTES: "document attributes",
    }.get(sid, f"unrecognized section {sid:#06x}")


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
                (SEC_META, SEC_TERM_INDEX, SEC_COMPLETIONS, SEC_TERM_BLOCK_OFFSETS,
                 SEC_PAYLOAD_OFFSETS)}
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


def term_entries(block, has_positions, has_freqs, num_docs):
    """Every entry of a term block, in order.

    Yields (key, term_id, doc_freq, offset, length, places, stated): `length`
    is the bytes of the postings section the term occupies, and `stated` the
    postings the entry gives itself, implied or inline, or None for a run.
    """
    count, pos = read_varint(block, 0)
    first_term_id, pos = read_varint(block, pos)
    key, offset = b"", None
    positions_at = None
    expected = first_term_id
    for i in range(count):
        term_id = first_term_id + i
        key, pos = read_front_coded(block, pos, key)
        doc_freq, pos = read_varint(block, pos)
        delta, pos = read_varint(block, pos)
        offset = delta if offset is None else offset + delta
        length, pos = read_varint(block, pos)
        stated = None
        if length == 0:
            assert doc_freq == 1, "implied postings must have a doc_freq of 1"
            assert term_id < num_docs, "implied postings name a document outside the index"
            stated = {term_id: 1}
            expected = term_id + 1
        elif length == INLINE:
            # The first document predicted from the entry before, the rest as
            # gaps less one, then frequencies less one (SPEC.md 5.3.4).
            assert doc_freq > 0, "inline postings must name a document"
            raw, pos = read_varint(block, pos)
            docs = [expected + unzigzag(raw)]
            for _ in range(doc_freq - 1):
                gap, pos = read_varint(block, pos)
                docs.append(docs[-1] + gap + 1)
            assert 0 <= docs[0] and docs[-1] < num_docs, "inline postings outside the index"
            freqs = [1] * doc_freq
            if has_freqs:
                for j in range(doc_freq):
                    raw, pos = read_varint(block, pos)
                    freqs[j] = raw + 1
            stated = dict(zip(docs, freqs))
            expected = docs[-1] + 1
        places = None
        if has_positions:
            # Two more varints an entry carries only when the file has
            # positions, which is why the flag has to be read first.
            pdelta, pos = read_varint(block, pos)
            positions_at = pdelta if positions_at is None else positions_at + pdelta
            plen, pos = read_varint(block, pos)
            places = (positions_at, plen)
        stored = 0 if stated is not None else length
        yield key, term_id, doc_freq, offset, stored, places, stated
    assert pos == len(block), "trailing bytes in term block"


def lookup(f, resident, ordinal, term, has_positions, has_freqs, num_docs):
    offsets = resident[SEC_TERM_BLOCK_OFFSETS]
    start = int.from_bytes(offsets[ordinal * 8 : ordinal * 8 + 8], "little")
    end = int.from_bytes(offsets[ordinal * 8 + 8 : ordinal * 8 + 16], "little")
    block = f.read(start, end - start)
    for key, term_id, doc_freq, offset, length, places, stated in term_entries(
        block, has_positions, has_freqs, num_docs
    ):
        if key == term:
            return term_id, doc_freq, offset, length, places, stated
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


def unzigzag(v):
    return (v >> 1) ^ -(v & 1)


def render(values, exponent):
    """value * 10^exponent, keeping the zeros the file's scale claims."""
    out = []
    for v in values:
        if exponent == 0:
            out.append(str(v))
            continue
        unit = 10 ** -exponent
        sign = "-" if v < 0 else ""
        v = abs(v)
        out.append(f"{sign}{v // unit}.{v % unit:0{-exponent}d}")
    return ",".join(out)


def payload_of(f, resident, doc_id, cache):
    section = resident[SEC_PAYLOAD_OFFSETS]
    codec = section[0]
    values_per_doc = section[1]
    exponent = section[2] - 256 if section[2] > 127 else section[2]
    per_block = int.from_bytes(section[4:8], "little")
    ordinal, index = divmod(doc_id, per_block)
    # Results are visited in document ID order and payload is stored in that
    # order, so documents sharing a block share a read. This is the difference
    # between m and m' in SPEC section 4.
    if ordinal not in cache:
        base = 12 + ordinal * 8
        start = int.from_bytes(section[base : base + 8], "little")
        end = int.from_bytes(section[base + 8 : base + 16], "little")
        cache[ordinal] = f.read(start, end - start)
    block = cache[ordinal]

    count, pos = read_varint(block, 0)
    assert index < count
    if codec == 0:
        for i in range(count):
            length, pos = read_varint(block, pos)
            if i == index:
                return block[pos : pos + length].decode()
            pos += length
        raise AssertionError("unreachable")

    # Every entry up to the wanted one, because each is a difference from the
    # one before it.
    values = [0] * values_per_doc
    for entry in range(index + 1):
        for i in range(values_per_doc):
            raw, pos = read_varint(block, pos)
            delta = unzigzag(raw)
            values[i] = delta if entry == 0 else values[i] + delta
    return render(values, exponent)


def search(data, query):
    """An intersection query, reporting what it cost."""
    f = CountingFile(data)
    header, _, resident = load_resident(f)
    flags = int.from_bytes(header[10:12], "little")
    has_freqs = bool(flags & FLAG_HAS_FREQS)
    has_positions = bool(flags & FLAG_HAS_POSITIONS)
    f.reads = 0  # the resident set is accounted for separately

    results = None
    for term in query:
        ordinal = find_block(resident[SEC_TERM_INDEX], term.encode())
        if ordinal is None:
            return {}, f.reads
        num_docs = int.from_bytes(header[16:24], "little")
        entry = lookup(f, resident, ordinal, term.encode(), has_positions, has_freqs, num_docs)
        if entry is None:
            return {}, f.reads
        term_id, doc_freq, offset, length, _places, stated = entry
        docs = stated if stated is not None else decode_postings(f.read(offset, length), has_freqs)
        assert len(docs) == doc_freq, "doc_freq disagrees with the postings run"
        results = docs.keys() if results is None else results & docs.keys()

    cache = {}
    return {d: payload_of(f, resident, d, cache) for d in sorted(results)}, f.reads


def read_completions(section, num_docs):
    """Entries by prefix, checking what SPEC.md 5.3.5 requires of a reader."""
    min_terms, pos = read_varint(section, 0)
    max_span, pos = read_varint(section, pos)
    docs_per_entry, pos = read_varint(section, pos)
    count, pos = read_varint(section, pos)
    assert docs_per_entry > 0
    entries, prefix = {}, b""
    for _ in range(count):
        key, pos = read_front_coded(section, pos, prefix)
        assert key and key > prefix, "completion prefixes are not strictly increasing"
        prefix = key
        n, pos = read_varint(section, pos)
        assert 0 < n <= docs_per_entry
        docs, last = [], 0
        for _ in range(n):
            raw, pos = read_varint(section, pos)
            last += unzigzag(raw)
            assert 0 <= last < num_docs and last not in docs
            docs.append(last)
        entries[key] = docs
    assert pos == len(section), "trailing bytes in the completions section"
    return docs_per_entry, entries


def complete(data, prefix, limit, payload=True):
    """The first `limit` documents of a prefix, and whether an entry answered.

    From the completions section when the prefix has an entry that reaches the
    limit — or lists fewer than it may, which means it lists them all — at no
    read before the payload. Otherwise every term the prefix matches, walked
    as SPEC.md 4.1 describes: the first block on faith, later ones only while
    their boundary key still shares the prefix, and one read for the postings
    of the whole run of terms.
    """
    f = CountingFile(data)
    header, _, resident = load_resident(f)
    num_docs = int.from_bytes(header[16:24], "little")
    has_freqs = bool(int.from_bytes(header[10:12], "little") & FLAG_HAS_FREQS)
    f.reads = 0
    prefix = prefix.encode()

    docs = None
    if SEC_COMPLETIONS in resident:
        docs_per_entry, entries = read_completions(resident[SEC_COMPLETIONS], num_docs)
        listed = entries.get(prefix)
        if listed is not None and (limit <= len(listed) or len(listed) < docs_per_entry):
            docs = listed[:limit]
    from_table = docs is not None

    if docs is None:
        term_index = resident[SEC_TERM_INDEX]
        count = int.from_bytes(term_index[0:4], "little")
        boundaries, pos, key = [], 4, b""
        for _ in range(count):
            key, pos = read_front_coded(term_index, pos, key)
            boundaries.append(key)
        start = find_block(term_index, prefix) or 0
        offsets = resident[SEC_TERM_BLOCK_OFFSETS]
        matched, term_id = [], None
        for ordinal in range(start, count):
            if ordinal > start and not boundaries[ordinal].startswith(prefix):
                break
            lo = int.from_bytes(offsets[ordinal * 8 : ordinal * 8 + 8], "little")
            hi = int.from_bytes(offsets[ordinal * 8 + 8 : ordinal * 8 + 16], "little")
            block = f.read(lo, hi - lo)
            for key, term_id, doc_freq, offset, length, _, stated in term_entries(
                block, False, has_freqs, num_docs
            ):
                if key.startswith(prefix):
                    matched.append((term_id, doc_freq, offset, length, stated))
        stored = [m for m in matched if m[3]]
        span = b""
        if stored:
            lo = stored[0][2]
            span = f.read(lo, stored[-1][2] + stored[-1][3] - lo)
        found = set()
        for term_id, doc_freq, offset, length, stated in matched:
            if stated is None:
                run = span[offset - stored[0][2] : offset - stored[0][2] + length]
                stated = decode_postings(run, has_freqs)
            found |= stated.keys()
        docs = sorted(found)[:limit]

    if not payload:
        return docs, f.reads, from_table
    cache = {}
    return [(d, payload_of(f, resident, d, cache)) for d in docs], f.reads, from_table


def listed_per_entry(data):
    """How many documents a segment's completion entries list, or 0 for none."""
    f = CountingFile(data)
    header, _, resident = load_resident(f)
    if SEC_COMPLETIONS not in resident:
        return 0
    num_docs = int.from_bytes(header[16:24], "little")
    return read_completions(resident[SEC_COMPLETIONS], num_docs)[0]


def collection_complete(segments, prefix, limit):
    """The first `limit` live documents of a prefix across a collection.

    MANIFEST.md 3.2: segments in manifest order, stopping once there are
    enough. In each, the entry first; then, if deletes took some, all the entry
    lists; then as many again as the segment has deletes, which is enough
    however they fall. `segments` is (file bytes, deletes) in manifest order.
    Returns (segment, document, payload) triples in collection order, and the
    reads, payload included.
    """
    hits, reads = [], 0
    for at, (data, deletes) in enumerate(segments):
        need = limit - len(hits)
        if need == 0:
            break
        live = []
        for ask in (need, max(listed_per_entry(data), need), need + len(deletes)):
            docs, spent, _ = complete(data, prefix, ask, payload=False)
            reads += spent
            live = [d for d in docs if d not in deletes]
            if len(live) >= need or len(docs) < ask:
                break
        hits += [(at, d) for d in live[:need]]

    out = []
    for at, (data, _) in enumerate(segments):
        f = CountingFile(data)
        _, _, resident = load_resident(f)
        f.reads = 0
        cache = {}
        mine = sorted(d for s, d in hits if s == at)
        payloads = {d: payload_of(f, resident, d, cache) for d in mine}
        reads += f.reads
        out += [(s, d, payloads[d]) for s, d in hits if s == at]
    order = {(s, d): i for i, (s, d) in enumerate(hits)}
    out.sort(key=lambda t: order[(t[0], t[1])])
    return out, reads


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


FIXTURES = [
    {
        "name": "tiny",
        "documents": DOCUMENTS,
        "decimals": None,
        "tokenizer_id": "space-split",
        "doc_lengths": True,
        "positions": True,
        # query, expected result, expected reads
        "cases": [
            (["tokyo"], {0: "35.694,139.754", 1: "35.671,139.772", 2: "35.664,139.698"}, 4),
            # `chuo`'s two documents are inline in its entry (SPEC.md 5.3.4);
            # `tokyo`'s run is shorter than inline would be, so it stays a
            # run.
            (["tokyo", "chuo"], {1: "35.671,139.772"}, 4),
            # `chiyoda` names document 0 and is term 0, so its postings are
            # implied and cost no read: the term block and the payload, nothing
            # else.
            (["chiyoda"], {0: "35.694,139.754"}, 2),
            (["osaka", "tokyo"], {}, 3),
            # `osaka` is term 2 and names document 3, so it cannot be implied,
            # but its postings are inline: the term block and the payload.
            (["osaka"], {3: "34.681,135.513"}, 2),
            # An absent term still costs the block read that proves it absent.
            (["kyoto"], {}, 1),
        ],
    },
    {
        # `tiny`, plus two sections this version does not define: one in the
        # range reserved for the specification and one in the application range.
        # A reader has to answer identically and read exactly as often, which is
        # what forward compatibility means in practice.
        "name": "tiny-extra",
        "documents": DOCUMENTS,
        "decimals": None,
        "tokenizer_id": "space-split",
        "doc_lengths": True,
        "positions": True,
        "extra": {
            0x0099: b"a section a later version of this specification might define",
            0x8000: b"a section an application defined, which is nobody else's business",
        },
        "cases": [
            (["tokyo"], {0: "35.694,139.754", 1: "35.671,139.772", 2: "35.664,139.698"}, 4),
            (["chiyoda"], {0: "35.694,139.754"}, 2),
            (["kyoto"], {}, 1),
        ],
    },
    {
        "name": "dict",
        "documents": DICT_DOCUMENTS,
        "decimals": 6,
        "tokenizer_id": "identity",
        # A dictionary of places wants a ranking prior, and this is where one
        # goes: read for many records at once, before any payload is fetched.
        "columns": [("importance", 0, [40, 90, 85, 70, 60])],
        "cases": [
            (["東京都千代田区丸の内1丁目1番1号"], {1: "139.767201,35.681560"}, 2),
            (["大阪府大阪市中央区本町1丁目1番1号"], {0: "135.501000,34.682000"}, 2),
            (["東京都新宿区"], {}, 1),
        ],
    },
    {
        # Completion (SPEC.md 5.3.5): each name one term, and an entry for
        # every prefix that at least two terms begin with.
        "name": "names",
        "documents": NAME_DOCUMENTS,
        "decimals": None,
        "tokenizer_id": "name-list",
        "completions": (2, 0, 2),
        "license": ("CC0-1.0", "Mokuji conformance fixtures"),
        # Every postings run stored, so that a prefix whose terms have several
        # is read in one span (SPEC.md 4.1); `tiny` is where inline postings
        # are exercised.
        "inline_postings": 0,
        "cases": [
            (["tokyo"], {4: "Tokyo"}, 3),
            (["東京"], {4: "Tokyo"}, 3),
        ],
        # prefix, limit, expected (doc, payload) pairs, reads, from the table
        "completions_cases": [
            # Shared by four names, so an entry answers, and the two documents
            # it lists share a payload block: one read in all.
            ("bl", 2, [(0, "Black Sea"), (1, "Black Forest")], 1, True),
            # One more than the entry lists, so the dictionary: two term blocks,
            # one postings read for the two runs stored (`blackpool` and `bled`
            # name their own documents), and two payload blocks.
            ("bl", 3, [(0, "Black Sea"), (1, "Black Forest"), (2, "Blackpool")], 5, False),
            # A prefix of one name is cheap and has no entry.
            ("black s", 10, [(0, "Black Sea")], 3, False),
            # The first character of `東京` and `東北`.
            ("東", 2, [(4, "Tokyo"), (5, "Tohoku")], 1, True),
            # Proven absent by the one block it would be in.
            ("x", 10, [], 1, False),
        ],
    },
    {
        "name": "names-delta",
        "documents": NAME_DELTA_DOCUMENTS,
        "decimals": None,
        "tokenizer_id": "name-list",
        "completions": (2, 0, 2),
        "license": ("CC0-1.0", "Mokuji conformance fixtures"),
        # Written the way `names` is, since a segment is written the way its
        # collection's base is.
        "inline_postings": 0,
        "cases": [],
    },
]


def check(spec, data):
    for prefix, limit, expected, expected_reads, expected_table in spec.get("completions_cases", []):
        got, reads, from_table = complete(data, prefix, limit)
        assert got == expected, (spec["name"], prefix, got, expected)
        assert from_table == expected_table, (spec["name"], prefix, from_table)
        assert reads == expected_reads, (spec["name"], prefix, reads, expected_reads)
    for query, expected, expected_reads in spec["cases"]:
        got, reads = search(data, query)
        assert got == expected, (spec["name"], query, got, expected)
        bound = 2 * len(query) + len(expected)
        assert reads <= bound, (spec["name"], query, reads, bound)
        assert reads == expected_reads, (spec["name"], query, reads, expected_reads)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true", help="verify, do not write")
    args = parser.parse_args()

    stale = []
    for spec in FIXTURES:
        name = spec["name"]
        corpus_name = f"{name}.corpus.txt"
        (HERE / corpus_name).write_text(
            "".join(f"{text}\t{payload}\n" for text, payload in spec["documents"])
        )
        data, regions = build(
            documents=spec["documents"],
            decimals=spec["decimals"],
            corpus_name=corpus_name,
            tokenizer_id=spec["tokenizer_id"],
            doc_lengths=spec.get("doc_lengths", False),
            columns=spec.get("columns"),
            positions=spec.get("positions", False),
            extra=spec.get("extra"),
            completions=spec.get("completions"),
            license=spec.get("license"),
            inline=spec.get("inline_postings", INLINE_POSTINGS),
        )
        # Every case is checked here as well as in cases.toml, so that a change
        # to a fixture cannot quietly invalidate the expectations the other
        # implementations are held to.
        check(spec, data)

        dump = hexdump(data, regions)
        if args.check:
            if (HERE / f"{name}.mokuji").read_bytes() != data:
                stale.append(f"{name}.mokuji")
            elif (HERE / f"{name}.hex").read_text() != dump:
                stale.append(f"{name}.hex")
            else:
                print(f"{name} is current ({len(data)} bytes)")
            continue

        (HERE / f"{name}.mokuji").write_bytes(data)
        (HERE / f"{name}.hex").write_text(dump)
        print(f"wrote {name}.mokuji ({len(data)} bytes) and {name}.hex")

    # The collection, from the fixtures just built.
    built = {name: (HERE / f"{name}.mokuji").read_bytes() for name, _, _ in COLLECTION}
    for prefix, limit, expected, expected_reads, *about in COLLECTION_CASES:
        # Restricted to a place, the collection is its segments whose region
        # includes it and those with none, in the same order (MANIFEST.md 7).
        # Segment numbers in results are positions in the restricted list.
        segments = [
            (built[name], deletes)
            for name, deletes, region in COLLECTION
            if not about or region is None or set(region) & set(about[0])
        ]
        got, reads = collection_complete(segments, prefix, limit)
        assert got == expected, ("collection", prefix, got, expected)
        assert reads == expected_reads, ("collection", prefix, reads, expected_reads)
    manifest = json.dumps(
        {
            "mokuji_manifest": "0.1",
            "generation": 1,
            "segments": [
                {
                    "path": f"{name}.mokuji",
                    "content_hash": "sha2-256:" + hashlib.sha256(built[name]).hexdigest(),
                    "length": len(built[name]),
                    **({"deletes": deletes} if deletes else {}),
                    **({"region": {"countries": region}} if region else {}),
                }
                for name, deletes, region in COLLECTION
            ],
        },
        indent=2,
        ensure_ascii=False,
    ) + "\n"
    if args.check:
        if (HERE / "collection.json").read_text() != manifest:
            stale.append("collection.json")
        else:
            print("collection.json is current")
    else:
        (HERE / "collection.json").write_text(manifest)
        print("wrote collection.json")

    if stale:
        sys.exit(f"stale: {', '.join(stale)}; re-run this script")


if __name__ == "__main__":
    main()
