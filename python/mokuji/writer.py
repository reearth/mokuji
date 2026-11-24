# A Mokuji writer for Python: the Rust crate's `mokuji::write` and
# `mokuji::meta`, with nothing but the standard library.
#
# SPEC.md §3.4 asks a writer for a byte-identical file from identical input
# and parameters. This one is held to more than that: stored raw, its output
# is the Rust writer's byte for byte, and the tests rebuild the conformance
# fixtures to show it. Compressed, it is not, and cannot be: no specification
# of deflate fixes which of the valid encodings a compressor emits, and
# zlib's is not the Rust crate's. That is what `generator.name` and
# `generator.version` in the metadata are for: a rebuild is checkable against
# the writer that made it.

import struct
import zlib
from dataclasses import dataclass, field

from .reader import (
    CODEC_DEFLATE,
    CODEC_RAW,
    COMPLETIONS,
    DOC_ATTRIBUTES,
    DOC_LENGTHS,
    HEADER_LEN,
    MAGIC,
    META,
    PAYLOAD,
    PAYLOAD_DELTA_INTS,
    PAYLOAD_OFFSETS,
    PAYLOAD_OPAQUE,
    POSITIONS,
    POSTINGS,
    SECTION_ENTRY_LEN,
    TERM_BLOCK_OFFSETS,
    TERM_BLOCKS,
    TERM_INDEX,
    Column,
    Provenance,
)
from .tokenizers import SPLITTERS

GENERATOR_NAME = "mokuji-python"

# What this package records as `generator.version`. A release sets it with
# the other packages' versions (RELEASING.md), and publishing checks that
# they agree.
VERSION = "0.1.1"

# The first section identifier an application may use (SPEC.md §5.2).
SECTION_APPLICATION_MIN = 0x8000

_SPEC_MINOR = 1
_FLAG_HAS_PAYLOAD = 1 << 1
_FLAG_HAS_FREQS = 1 << 2
_FLAG_HAS_POSITIONS = 1 << 3
_INLINE = 1
_MASK64 = (1 << 64) - 1


class BuildError(ValueError):
    """Input a Builder cannot make a file of: out of order, or inconsistent."""


@dataclass(frozen=True)
class CompletionOptions:
    """Which prefixes get an entry in the completions section (SPEC.md
    §5.3.5), and how many documents each lists.

    The documents listed are the ones with the smallest IDs among those
    holding a term that begins with the prefix: this writer ranks by nothing
    but document order, so a caller who wants the most important documents
    first numbers them by importance before adding them.
    """

    # A prefix that at least this many terms begin with gets an entry; zero
    # promises nothing on this count.
    min_terms: int = 0
    # A prefix whose terms' postings runs total at least this many bytes gets
    # an entry; zero promises nothing on this count.
    max_span: int = 0
    # How many documents an entry lists at most.
    docs_per_entry: int = 1


@dataclass(frozen=True)
class WriteOptions:
    """A build's parameters, defaulting to the Rust writer's. They belong in
    the metadata's `generator.params` (SPEC.md §3.4); `params()` says them."""

    # Terms per dictionary block.
    terms_per_block: int = 1024
    # Documents per payload block.
    docs_per_payload_block: int = 64
    # Whether to store term frequencies.
    has_freqs: bool = True
    # None for opaque payload; or (values_per_doc, scale_exponent) for a
    # fixed row of integers per document, delta-coded (SPEC.md §5.5.3).
    ints: tuple | None = None
    # Whether to store where each term occurs in each document.
    positions: bool = False
    # Whether to store every document's length in tokens, for BM25.
    doc_lengths: bool = False
    # Whether to compress the sections that can be: each unit on its own,
    # and a section whose units do not shrink stored raw.
    compress: bool = True
    # Whether to answer expensive prefixes ahead of time.
    completions: CompletionOptions | None = None
    # The most bytes a term's postings may take written inline in its
    # dictionary entry (SPEC.md §5.3.4); zero writes every term a run.
    inline_postings: int = 8

    def params(self):
        """The options as `mokuji build` records them in `generator.params`,
        and whether the build compressed. A caller with parameters of its
        own, such as how the documents were ordered, adds them."""
        params = {
            "terms_per_block": self.terms_per_block,
            "docs_per_payload_block": self.docs_per_payload_block,
            "has_freqs": int(self.has_freqs),
            "payload": "opaque" if self.ints is None else f"ints:{-self.ints[1]}",
            "doc_lengths": int(self.doc_lengths),
            "positions": int(self.positions),
            "inline_postings": self.inline_postings,
            "compress": int(self.compress),
        }
        if self.completions is not None:
            c = self.completions
            params["completions"] = f"{c.min_terms},{c.max_span},{c.docs_per_entry} by document order"
        return params


# --- encoders ---------------------------------------------------------------


def _varint(value, out):
    while value >= 0x80:
        out.append((value & 0x7F) | 0x80)
        value >>= 7
    out.append(value)


def _zigzag(v):
    """An i64 as the unsigned integer it zigzags to."""
    return ((v << 1) ^ (v >> 63)) & _MASK64


def _wrap64(v):
    """Wraps an integer to i64, as Rust's wrapping arithmetic does."""
    return ((v + (1 << 63)) & _MASK64) - (1 << 63)


def _key(key, previous, out):
    """Appends `key`, front-coded against `previous` (SPEC.md §5.3.1)."""
    shared = 0
    while shared < len(key) and shared < len(previous) and key[shared] == previous[shared]:
        shared += 1
    _varint(shared, out)
    _varint(len(key) - shared, out)
    out += key[shared:]


def _values(values, out):
    """Appends values in bit-packed groups of 128 (SPEC.md §5.4.1)."""
    for start in range(0, len(values), 128):
        group = values[start : start + 128]
        width = max(v.bit_length() for v in group)
        out.append(width)
        if width:
            packed = 0
            for i, v in enumerate(group):
                packed |= v << (i * width)
            out += packed.to_bytes((len(group) * width + 7) // 8, "little")


def _compress(unit):
    """The deflate codec: the length, then raw deflate at the strongest level."""
    out = bytearray()
    _varint(len(unit), out)
    compressor = zlib.compressobj(9, zlib.DEFLATED, -15)
    out += compressor.compress(bytes(unit)) + compressor.flush()
    return bytes(out)


# --- metadata ---------------------------------------------------------------


def _cbor_head(major, value, out):
    major <<= 5
    if value < 24:
        out.append(major | value)
    elif value <= 0xFF:
        out += bytes([major | 24, value])
    elif value <= 0xFFFF:
        out += bytes([major | 25]) + value.to_bytes(2, "big")
    elif value <= 0xFFFFFFFF:
        out += bytes([major | 26]) + value.to_bytes(4, "big")
    else:
        out += bytes([major | 27]) + value.to_bytes(8, "big")


def encode_cbor(value):
    """One value as canonical CBOR (RFC 8949 §4.2), from the subset metadata
    uses: non-negative integers, text, lists, and dicts with text keys.
    Booleans are written as 0 and 1, as `mokuji build` records them."""
    out = bytearray()
    _cbor(value, out)
    return bytes(out)


def _cbor(value, out):
    if isinstance(value, bool):
        value = int(value)
    if isinstance(value, int):
        if not 0 <= value <= _MASK64:
            raise BuildError("metadata holds only non-negative integers of 64 bits")
        _cbor_head(0, value, out)
    elif isinstance(value, str):
        raw = value.encode()
        _cbor_head(3, len(raw), out)
        out += raw
    elif isinstance(value, (list, tuple)):
        _cbor_head(4, len(value), out)
        for item in value:
            _cbor(item, out)
    elif isinstance(value, dict):
        pairs = []
        for key, item in value.items():
            if not isinstance(key, str):
                raise BuildError("metadata maps have text keys")
            pairs.append((encode_cbor(key), encode_cbor(item)))
        # Keys sorted by their encoded bytes: shorter first, then bytewise.
        pairs.sort()
        _cbor_head(5, len(pairs), out)
        for key, item in pairs:
            out += key + item
    else:
        raise BuildError(f"metadata cannot hold a {type(value).__name__}")


@dataclass
class Metadata:
    """The metadata section (SPEC.md §6), for a writer: what an index was
    derived from and how, which together are required to be sufficient to
    rebuild it byte for byte (§3.4)."""

    # A file is immutable; a new index is a new generation, not an edit.
    generation: int = 1
    # The sources it was derived from. `content_hash` is required, as
    # `<algorithm>:<lowercase hex>`; the rest are recorded when present.
    sources: list = field(default_factory=list)
    generator_name: str = GENERATOR_NAME
    generator_version: str = VERSION
    # The full build parameters, such as `WriteOptions.params()` gives.
    generator_params: dict = field(default_factory=dict)
    # An identifier from TOKENIZERS.md, and its parameters.
    tokenizer_id: str = "identity"
    tokenizer_params: dict = field(default_factory=dict)
    # Application-defined additions, recorded when not None.
    attrs: dict | None = None
    # The index's own license, as an SPDX expression, and the credit its
    # results carry; recorded when not None.
    license: str | None = None
    attribution: str | None = None

    def encode(self):
        """The section, as canonical CBOR."""
        sources = []
        for source in self.sources:
            if not source.content_hash:
                raise BuildError("a source has no content hash")
            entry = {"content_hash": source.content_hash}
            for key in ("uri", "retrieved", "modified"):
                if getattr(source, key):
                    entry[key] = getattr(source, key)
            sources.append(entry)
        root = {
            "generation": self.generation,
            "source": sources,
            "generator": {
                "name": self.generator_name,
                "version": self.generator_version,
                "params": self.generator_params,
            },
            "tokenizer": {"id": self.tokenizer_id, "params": self.tokenizer_params},
        }
        if self.attrs is not None:
            root["attrs"] = self.attrs
        if self.license is not None:
            root["license"] = self.license
        if self.attribution is not None:
            root["attribution"] = self.attribution
        return encode_cbor(root)


def split(tokenizer_id, text):
    """The terms a document's text becomes under a tokenizer from
    TOKENIZERS.md, as a writer builds them. A key tokenizer, `identity` or
    `ja-address`, makes the whole text one term."""
    splitter = SPLITTERS.get(tokenizer_id)
    if splitter is None:
        raise BuildError(f"unknown tokenizer '{tokenizer_id}'")
    return splitter(text)


# --- the builder ------------------------------------------------------------


class _Term:
    __slots__ = ("term", "doc_freq", "run", "inline", "short", "positions", "head")

    def __init__(self, term, doc_freq, run, positions, head, short=None):
        self.term = term
        self.doc_freq = doc_freq
        # The postings run; empty when the postings are implied or inline.
        self.run = run
        # The postings as written in the dictionary entry, when they are.
        self.inline = None
        # (docs, freqs), kept while the term is short enough that it might yet
        # be inlined.
        self.short = short
        self.positions = positions
        # The term's first documents, as many as a completion lists.
        self.head = head


class Builder:
    """Builds a Mokuji file from documents and a sorted stream of postings.

    Documents are added first, in document ID order; postings are then pushed
    in term order (by bytes) and within a term in document order. Both
    orderings are checked rather than assumed. Sorting stays with the caller,
    so that a corpus larger than memory does not need the writer to own an
    external sort.
    """

    def __init__(self, options=None):
        options = options or WriteOptions()
        if options.terms_per_block <= 0 or options.docs_per_payload_block <= 0:
            raise BuildError("block sizes must be non-zero")
        # A positions run says nothing about where one document's positions
        # end and the next begin; the frequency is that boundary.
        if options.positions and not options.has_freqs:
            raise BuildError("positions need frequencies, which say how many belong to each document")
        if options.inline_postings < 0:
            raise BuildError("an inline postings bound cannot be negative")
        if options.ints is not None:
            values_per_doc, scale_exponent = options.ints
            if not 0 <= values_per_doc <= 255 or not -128 <= scale_exponent <= 127:
                raise BuildError("ints are (values_per_doc, scale_exponent), a byte and a signed byte")
        c = options.completions
        if c is not None:
            if c.docs_per_entry <= 0:
                raise BuildError("a completion entry lists at least one document")
            # Both zero would be a table with no rule for what goes in it.
            if c.min_terms == 0 and c.max_span == 0:
                raise BuildError("completions need a term count or a postings size to cover")
        self.options = options
        self._payloads = []
        self._values = []
        self._columns = []
        self._sum_doclen = 0
        self._doc_lengths = []
        self._terms = []
        self._open = None
        self._sections = {}

    @property
    def documents(self):
        """How many documents have been added."""
        return len(self._payloads) if self.options.ints is None else len(self._values)

    def add_document(self, payload):
        """Appends a document whose payload is opaque bytes (or text, written
        as UTF-8), returning its ID."""
        if self.options.ints is not None:
            raise BuildError("this index encodes payload as integers")
        self._payloads.append(payload.encode() if isinstance(payload, str) else bytes(payload))
        return self.documents - 1

    def add_document_values(self, values):
        """Appends a document whose payload is a fixed row of integers,
        returning its ID."""
        if self.options.ints is None:
            raise BuildError("this index encodes payload as opaque bytes")
        if len(values) != self.options.ints[0]:
            raise BuildError("a document carries the wrong number of values")
        if any(not -(1 << 63) <= v < 1 << 63 for v in values):
            raise BuildError("a value does not fit 64 bits")
        self._values.append(list(values))
        return self.documents - 1

    def add_column(self, column):
        """Adds a Column of per-document numbers (SPEC.md §5.8), checked at
        `finish` against the document count."""
        if not -128 <= column.scale_exponent <= 127:
            raise BuildError("a column's scale exponent does not fit a signed byte")
        if any(c.name == column.name for c in self._columns):
            raise BuildError("two document attribute columns share a name")
        self._columns.append(Column(column.name, column.scale_exponent, list(column.values)))

    def add_posting(self, term, doc, freq=1):
        """Records that `term` (bytes, or text written as UTF-8) occurs `freq`
        times in `doc`. Terms ascend by bytes; within a term, documents
        ascend."""
        term = term.encode() if isinstance(term, str) else bytes(term)
        if freq <= 0:
            raise BuildError("a posting with zero frequency is not a posting")
        if not 0 <= doc < self.documents:
            raise BuildError("posting refers to a document that was never added")
        open_term = self._open
        if open_term is not None and open_term["term"] == term:
            if doc <= open_term["docs"][-1]:
                raise BuildError("documents within a term are not ascending")
            open_term["docs"].append(doc)
            open_term["freqs"].append(freq)
            open_term["positions"].append(None)
        elif open_term is not None and term <= open_term["term"]:
            raise BuildError("terms are not ascending")
        else:
            self._close_term()
            self._open = {"term": term, "docs": [doc], "freqs": [freq], "positions": [None]}
        self._sum_doclen += freq
        if self.options.doc_lengths:
            if len(self._doc_lengths) <= doc:
                self._doc_lengths.extend([0] * (doc + 1 - len(self._doc_lengths)))
            self._doc_lengths[doc] += freq

    def add_posting_at(self, term, doc, positions):
        """Records that `term` occurs in `doc` at exactly these positions,
        which ascend. The frequency is the number of positions."""
        positions = list(positions)
        if not positions:
            raise BuildError("a posting with no positions is not a posting")
        if any(b <= a for a, b in zip(positions, positions[1:])):
            raise BuildError("positions within a document are not ascending")
        self.add_posting(term, doc, len(positions))
        self._open["positions"][-1] = positions

    def add_section(self, section_id, data):
        """Attaches an application-defined section (SPEC.md §5.2), stored
        uncompressed and placed last, so that it moves nothing a reader looks
        for."""
        if not SECTION_APPLICATION_MIN <= section_id <= 0xFFFF:
            raise BuildError("a section this specification defines cannot be supplied by an application")
        self._sections[section_id] = bytes(data)

    # --- finishing ----------------------------------------------------------

    def _close_term(self):
        open_term, self._open = self._open, None
        if open_term is None:
            return
        docs, freqs = open_term["docs"], open_term["freqs"]

        # Positions, in the same document order the postings run uses.
        positions = bytearray()
        if self.options.positions:
            for row, freq in zip(open_term["positions"], freqs):
                row = row or []
                values = [at if i == 0 else at - row[i - 1] - 1 for i, at in enumerate(row)]
                # A posting recorded without positions still needs its share
                # of the run, or the frequencies stop lining up.
                values += [0] * (freq - len(values))
                _values(values, positions)

        c = self.options.completions
        head = docs[: c.docs_per_entry] if c is not None else []

        # The dictionary can state this term's postings by itself: it names
        # one document, whose ID is the ID this term is about to be given,
        # once (SPEC.md §5.3.4).
        term_id = len(self._terms)
        if docs == [term_id] and freqs == [1]:
            self._terms.append(_Term(open_term["term"], 1, b"", bytes(positions), head))
            return

        run = bytearray([0])  # codec 0
        _varint(len(docs), run)
        # The first value is a document ID and the rest are gaps less one, so
        # that consecutive documents become zeros and pack to nothing.
        _values([d if i == 0 else d - docs[i - 1] - 1 for i, d in enumerate(docs)], run)
        if self.options.has_freqs:
            _values([f - 1 for f in freqs], run)
        # Every inline value costs at least a byte, so a term with more
        # documents than the bound allows bytes can never be inlined.
        short = (docs, freqs) if len(docs) <= self.options.inline_postings else None
        self._terms.append(_Term(open_term["term"], len(docs), bytes(run), bytes(positions), head, short))

    def _inline_postings(self):
        """Moves short postings into their dictionary entries (SPEC.md
        §5.3.4), once every term is known: a term's first document is written
        against the entries before it in its block."""
        bound = self.options.inline_postings
        per = self.options.terms_per_block
        for start in range(0, len(self._terms), per):
            expected = start
            for i in range(start, min(start + per, len(self._terms))):
                term = self._terms[i]
                if term.short is None:
                    if not term.run:
                        # Implied: the document is the term's own ID.
                        expected = i + 1
                    continue
                docs, freqs = term.short
                term.short = None
                if bound == 0:
                    continue
                inline = bytearray()
                _varint(_zigzag(docs[0] - expected), inline)
                for a, b in zip(docs, docs[1:]):
                    _varint(b - a - 1, inline)
                if self.options.has_freqs:
                    for f in freqs:
                        _varint(f - 1, inline)
                if len(inline) <= bound and len(inline) <= len(term.run):
                    expected = docs[-1] + 1
                    term.inline = bytes(inline)
                    term.run = b""

    def _pack(self, units):
        """Compresses each unit, or leaves them all raw if that is no smaller.
        Decided per section, since the codec is recorded per section."""
        units = [bytes(u) for u in units]
        if not self.options.compress:
            return units, CODEC_RAW
        packed = [_compress(u) for u in units]
        if sum(map(len, packed)) < sum(map(len, units)):
            return packed, CODEC_DEFLATE
        return units, CODEC_RAW

    def finish(self, meta):
        """The file, as bytes, with `meta` as the metadata section: canonical
        CBOR, such as `Metadata.encode()` makes. It carries the provenance of
        a source the writer never sees, so composing it is the caller's job."""
        self._close_term()
        if not self._terms:
            raise BuildError("an index with no terms cannot be built")
        self._inline_postings()
        options = self.options
        per = options.terms_per_block
        chunks = [self._terms[i : i + per] for i in range(0, len(self._terms), per)]

        # Payload blocks. Deltas never cross a block boundary: a block is
        # addressed on its own and must decode on its own.
        raw_payload = []
        per_block = options.docs_per_payload_block
        for start in range(0, self.documents, per_block):
            end = min(start + per_block, self.documents)
            block = bytearray()
            _varint(end - start, block)
            for doc in range(start, end):
                if options.ints is None:
                    _varint(len(self._payloads[doc]), block)
                    block += self._payloads[doc]
                    continue
                for i, v in enumerate(self._values[doc]):
                    if doc > start:
                        v = _wrap64(v - self._values[doc - 1][i])
                    _varint(_zigzag(v), block)
            raw_payload.append(block)
        payload_blocks, payload_codec = self._pack(raw_payload)

        term_index = bytearray(struct.pack("<I", len(chunks)))
        previous = b""
        for chunk in chunks:
            _key(chunk[0].term, previous, term_index)
            previous = chunk[0].term
        [term_index], term_index_codec = self._pack([term_index])

        # Like the term index, completions depend on the terms and their runs
        # and on no offset, so they can be sized before anything is placed.
        completions = None
        completions_codec = CODEC_RAW
        if options.completions is not None:
            [completions], completions_codec = self._pack([_completions(self._terms, options.completions)])

        doc_lengths = None
        if options.doc_lengths:
            lengths = self._doc_lengths + [0] * (self.documents - len(self._doc_lengths))
            doc_lengths = bytearray()
            _values(lengths, doc_lengths)

        attributes = None
        if self._columns:
            attributes = bytearray()
            _varint(len(self._columns), attributes)
            for column in self._columns:
                if len(column.values) != self.documents:
                    raise BuildError("a document attribute column does not have one value per document")
                name = column.name.encode()
                _varint(len(name), attributes)
                attributes += name
                attributes.append(column.scale_exponent & 0xFF)
            for column in self._columns:
                _values([_zigzag(v) for v in column.values], attributes)

        # The order the sections are written in. The resident ones go first,
        # so that a cold start fetches them in one read; postings and
        # positions go before the term blocks that point into them, so that an
        # offset is known before any block is built.
        order = [META, TERM_INDEX]
        if completions is not None:
            order.append(COMPLETIONS)
        order += [TERM_BLOCK_OFFSETS, PAYLOAD_OFFSETS]
        if doc_lengths is not None:
            order.append(DOC_LENGTHS)
        if attributes is not None:
            order.append(DOC_ATTRIBUTES)
        order.append(POSTINGS)
        if options.positions:
            order.append(POSITIONS)
        order += [TERM_BLOCKS, PAYLOAD]
        # Application sections go last, behind everything the format defines.
        order += sorted(self._sections)

        table_len = SECTION_ENTRY_LEN * len(order)
        postings_len = sum(len(t.run) for t in self._terms)
        positions_len = sum(len(t.positions) for t in self._terms)
        term_block_offsets_len = 8 * (len(chunks) + 1)
        payload_offsets_len = 12 + 8 * (len(payload_blocks) + 1)
        postings_at = (
            HEADER_LEN
            + table_len
            + len(meta)
            + len(term_index)
            + len(completions or b"")
            + term_block_offsets_len
            + payload_offsets_len
            + len(doc_lengths or b"")
            + len(attributes or b"")
        )
        blocks, term_blocks_codec = self._pack(self._term_blocks(chunks, postings_at, postings_at + postings_len))

        length_of = {
            META: len(meta),
            TERM_INDEX: len(term_index),
            COMPLETIONS: len(completions or b""),
            TERM_BLOCK_OFFSETS: term_block_offsets_len,
            PAYLOAD_OFFSETS: payload_offsets_len,
            DOC_LENGTHS: len(doc_lengths or b""),
            DOC_ATTRIBUTES: len(attributes or b""),
            POSTINGS: postings_len,
            POSITIONS: positions_len,
            TERM_BLOCKS: sum(map(len, blocks)),
            PAYLOAD: sum(map(len, payload_blocks)),
        }
        for section_id, data in self._sections.items():
            length_of[section_id] = len(data)
        offset_of = {}
        cursor = HEADER_LEN + table_len
        for section_id in order:
            offset_of[section_id] = cursor
            cursor += length_of[section_id]

        # Metadata stays readable without a decompressor, and the offset
        # tables stay raw because their size feeds back into the placement
        # they describe.
        codec_of = {
            TERM_INDEX: term_index_codec,
            COMPLETIONS: completions_codec,
            TERM_BLOCKS: term_blocks_codec,
            PAYLOAD: payload_codec,
        }

        if options.ints is None:
            payload_offsets = bytearray([PAYLOAD_OPAQUE, 0, 0, 0])
        else:
            values_per_doc, scale_exponent = options.ints
            payload_offsets = bytearray([PAYLOAD_DELTA_INTS, values_per_doc, scale_exponent & 0xFF, 0])
        payload_offsets += struct.pack("<II", options.docs_per_payload_block, len(payload_blocks))
        payload_offsets += _offset_table(offset_of[PAYLOAD], payload_blocks)

        flags = _FLAG_HAS_PAYLOAD
        if options.has_freqs:
            flags |= _FLAG_HAS_FREQS
        if options.positions:
            flags |= _FLAG_HAS_POSITIONS
        out = bytearray(MAGIC)
        out += struct.pack(
            "<BBHIQQQI", 0, _SPEC_MINOR, flags, len(order), self.documents, self._sum_doclen, HEADER_LEN, table_len
        )
        out += bytes(HEADER_LEN - len(out))

        # The table is sorted by identifier, which is not the order the
        # sections are written in.
        for section_id in sorted(order):
            out += struct.pack(
                "<HB5xQQ", section_id, codec_of.get(section_id, CODEC_RAW), offset_of[section_id], length_of[section_id]
            )

        for section_id in order:
            if section_id == META:
                out += meta
            elif section_id == TERM_INDEX:
                out += term_index
            elif section_id == COMPLETIONS:
                out += completions
            elif section_id == TERM_BLOCK_OFFSETS:
                out += _offset_table(offset_of[TERM_BLOCKS], blocks)
            elif section_id == PAYLOAD_OFFSETS:
                out += payload_offsets
            elif section_id == DOC_LENGTHS:
                out += doc_lengths
            elif section_id == DOC_ATTRIBUTES:
                out += attributes
            elif section_id == POSTINGS:
                out += b"".join(t.run for t in self._terms)
            elif section_id == POSITIONS:
                out += b"".join(t.positions for t in self._terms)
            elif section_id == TERM_BLOCKS:
                out += b"".join(blocks)
            elif section_id == PAYLOAD:
                out += b"".join(payload_blocks)
            else:
                out += self._sections[section_id]
        return bytes(out)

    def _term_blocks(self, chunks, postings_at, positions_at):
        blocks = []
        term_id = 0
        for chunk in chunks:
            block = bytearray()
            _varint(len(chunk), block)
            _varint(term_id, block)
            previous_key = b""
            for i, term in enumerate(chunk):
                _key(term.term, previous_key, block)
                _varint(term.doc_freq, block)
                # The first entry's offsets are absolute; the rest are the
                # distance from the entry before, which is that entry's length.
                _varint(postings_at if i == 0 else len(chunk[i - 1].run), block)
                if term.inline is not None:
                    _varint(_INLINE, block)
                    block += term.inline
                else:
                    # Zero for implied postings, which have no run.
                    _varint(len(term.run), block)
                if self.options.positions:
                    _varint(positions_at if i == 0 else len(chunk[i - 1].positions), block)
                    _varint(len(term.positions), block)
                    positions_at += len(term.positions)
                previous_key = term.term
                postings_at += len(term.run)
            term_id += len(chunk)
            blocks.append(block)
        return blocks


def _offset_table(base, blocks):
    out = bytearray()
    cursor = base
    for block in blocks:
        out += struct.pack("<Q", cursor)
        cursor += len(block)
    out += struct.pack("<Q", cursor)
    return out


def _completions(terms, c):
    """The completions section, uncompressed (SPEC.md §5.3.5).

    Every prefix of every term is a candidate, and most are cheap, so this
    does not enumerate them. It walks down from one byte: the terms sharing a
    prefix are a contiguous run of the sorted dictionary, a prefix that is not
    expensive has no expensive extension, and so only the runs that were
    expensive at one length are split at the next.
    """
    entries = []
    pending = [(0, len(terms), 1)]
    while pending:
        start, end, length = pending.pop()
        # Split the run by the byte at length - 1. A term too short to have
        # one is the shared prefix itself, and sorts first.
        groups = []
        at = start
        while at < end and len(terms[at].term) < length:
            at += 1
        while at < end:
            key = terms[at].term[:length]
            following = at + 1
            while following < end and terms[following].term.startswith(key):
                following += 1
            groups.append((at, following))
            at = following
        expensive = [
            (a, b)
            for a, b in groups
            if (c.min_terms and b - a >= c.min_terms) or (c.max_span and sum(len(t.run) for t in terms[a:b]) >= c.max_span)
        ]
        for a, b in expensive:
            pending.append((a, b, length + 1))
            best = sorted({doc for t in terms[a:b] for doc in t.head})[: c.docs_per_entry]
            entries.append((terms[a].term[:length], best))
    # The walk visits prefixes depth-first from a stack; the section wants
    # them in byte order.
    entries.sort(key=lambda e: e[0])

    out = bytearray()
    for value in (c.min_terms, c.max_span, c.docs_per_entry, len(entries)):
        _varint(value, out)
    previous = b""
    for prefix, docs in entries:
        _key(prefix, previous, out)
        previous = prefix
        _varint(len(docs), out)
        last = 0
        for doc in docs:
            _varint(_zigzag(_wrap64(doc - last)), out)
            last = doc
    return out
