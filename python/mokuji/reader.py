# A Mokuji reader for Python, with no dependencies beyond the standard library.
#
# This is the fourth implementation of SPEC.md, after the Rust crates, the
# Python fixture builder and the TypeScript reader, and it follows the last of
# those closely: the same API, the same ranking, the same read counts. Where the
# two differ it is because the specification asks for something the TypeScript
# reader does not do; README.md in this directory lists those places.
#
# It reads a file the way the format was designed to be read, a range at a
# time. A path is read with positional reads and a URL with Range requests, so
# a file of gigabytes costs what a query touches.

import bisect
import json
import math
import os
import struct
import threading
import urllib.parse
import urllib.request
import warnings
import zlib
from dataclasses import dataclass, field

from .tokenizers import SPLITTERS, TOKENIZERS

MAGIC = b"MOKUJI\x00\x00"
# The magic of files written before it lost its version, which a 0.x reader
# still reads (SPEC.md Appendix A). Nothing writes it.
MAGIC_V0 = b"MOKUJIv0"
HEADER_LEN = 64
SECTION_ENTRY_LEN = 24

FLAG_HAS_PAYLOAD = 1 << 1
FLAG_HAS_FREQS = 1 << 2
FLAG_HAS_POSITIONS = 1 << 3

# A dictionary entry's `postings_len` when it states its postings itself
# (SPEC.md §5.3.4). No run is this short.
IMPLIED = 0
INLINE = 1

META = 0x0001
TERM_INDEX = 0x0010
TERM_BLOCKS = 0x0011
TERM_BLOCK_OFFSETS = 0x0012
COMPLETIONS = 0x0013
POSTINGS = 0x0020
PAYLOAD = 0x0030
PAYLOAD_OFFSETS = 0x0031
DOC_LENGTHS = 0x0032
DOC_ATTRIBUTES = 0x0033
POSITIONS = 0x0060

CODEC_RAW = 0
CODEC_DEFLATE = 1

PAYLOAD_OPAQUE = 0
PAYLOAD_DELTA_INTS = 1

# How much to ask for when opening. The resident set is sized to be fetched in
# one request; this is a reader's guess at how large that is, not a property of
# the format.
OPEN_PREFIX = 256 * 1024

# The largest unit this reader will decompress. SPEC.md §5.6 lets a reader
# refuse a declared length it is unwilling to allocate, and §8 asks it to: the
# length is read from the file before any of the unit has been checked.
MAX_UNIT = 1 << 30


class MokujiError(ValueError):
    """A file that is not one this reader can read, or that is malformed; or a
    query the file cannot answer."""


class NoColumnError(MokujiError):
    """A query named an attribute column the file does not carry."""

    def __init__(self, column):
        super().__init__(f"this index carries no attribute column called '{column}'")
        self.column = column


# --- results ----------------------------------------------------------------


@dataclass
class Stats:
    """What a query cost, so a caller can show it rather than claim it."""

    requests: int = 0
    bytes: int = 0


@dataclass(frozen=True)
class RequestInfo:
    """One range request, as reported to `on_request`."""

    offset: int
    length: int


@dataclass
class Postings:
    """One term's postings: ascending document IDs and their frequencies."""

    docs: list
    freqs: list


@dataclass
class TermEntry:
    """One dictionary entry (SPEC.md §5.3.4)."""

    term: bytes
    term_id: int
    doc_freq: int
    postings_offset: int
    # Bytes of the postings section the term's run occupies: zero when the
    # entry states its postings itself, implied or inline.
    postings_length: int
    # The postings, when the entry carries them inline.
    inline: Postings | None = None
    positions_offset: int | None = None
    positions_length: int | None = None


@dataclass(frozen=True)
class Generator:
    """The writer a file records, and the parameters it was built with (SPEC.md §6)."""

    name: str | None
    version: str | None
    # Every build parameter, by name. A pipeline may record its own here,
    # beside the writer's, and check them when it opens the file.
    params: dict


@dataclass(frozen=True)
class Provenance:
    """One artifact an index was derived from, as its metadata records it (SPEC.md §6)."""

    # `<algorithm>:<lowercase hex>`.
    content_hash: str
    # Where it can be obtained.
    uri: str | None
    # When the builder obtained it, RFC 3339.
    retrieved: str | None
    # When its publisher last changed it, RFC 3339 — how current the data is.
    modified: str | None


@dataclass
class Column:
    """One named column of per-document numbers (SPEC.md §5.8)."""

    name: str
    scale_exponent: int
    values: list


@dataclass
class Completions:
    """The completions section (SPEC.md §5.3.5), keyed by prefix bytes."""

    min_terms: int
    max_span: int
    docs_per_entry: int
    entries: dict


@dataclass(frozen=True)
class LookupResult:
    term: str
    doc: int
    payload: str


@dataclass
class Lookup:
    """What `lookup` found: the query as normalized, and every match."""

    normalized: str
    results: list


@dataclass
class Completion:
    """What `complete` found, and how."""

    normalized: str
    # Whether the completions section answered, at no read before the payload.
    from_table: bool
    docs: list
    payloads: list
    requests_before_payload: int


@dataclass(frozen=True)
class Probed:
    term: str
    doc_freq: int


@dataclass(frozen=True)
class Ranked:
    doc: int
    # None when the index carries no document lengths, so cannot be ranked.
    score: float | None
    payload: str


@dataclass(frozen=True)
class Scored:
    doc: int
    # None when the index carries no document lengths, so cannot be ranked.
    score: float | None


@dataclass
class Ranking:
    """What `rank` found: every document matched, best first, and what
    finding them read."""

    ranked: list
    # The terms whose postings were read, with how many documents each names.
    probed: list
    # Terms present but not read, because `probe` left them out.
    skipped: int


@dataclass
class SearchResult:
    """What `search` found, and what deciding it took."""

    # The terms the query became.
    terms: list
    # The terms whose postings were read, with how many documents each names.
    probed: list
    # Terms present but not read, because `probe` left them out.
    skipped: int
    # Every document matched.
    matched: int
    # The first `limit` of them, best first.
    results: list


@dataclass(frozen=True)
class _Section:
    codec: int
    offset: int
    length: int


# --- sources ----------------------------------------------------------------


class FileSource:
    """A file on disk, read a range at a time.

    Positional reads, so a range costs that range and no more, and two threads
    reading the same file do not move a shared position under each other.
    """

    def __init__(self, path):
        self._file = open(path, "rb")
        self.size = os.fstat(self._file.fileno()).st_size
        self._lock = threading.Lock()

    def read(self, offset, length):
        if hasattr(os, "pread"):
            chunks = []
            while length > 0:
                chunk = os.pread(self._file.fileno(), length, offset)
                if not chunk:
                    break
                chunks.append(chunk)
                offset += len(chunk)
                length -= len(chunk)
            return b"".join(chunks)
        # Windows has no pread; a seek and a read under a lock are the same.
        with self._lock:
            self._file.seek(offset)
            return self._file.read(length)

    def close(self):
        self._file.close()


class HttpSource:
    """A file served over HTTP, read with Range requests.

    A server that ignores the Range header answers with the whole file, which
    is the one thing this format exists to avoid, so that is an error rather
    than a slow success.
    """

    def __init__(self, url, timeout=30):
        self.url = url
        self.timeout = timeout
        self.size = None

    def read(self, offset, length):
        request = urllib.request.Request(
            self.url,
            headers={
                "Range": f"bytes={offset}-{offset + length - 1}",
                "User-Agent": "mokuji-python",
            },
        )
        with urllib.request.urlopen(request, timeout=self.timeout) as response:
            if response.status != 206:
                raise MokujiError(
                    f"the server ignored the Range header (status {response.status}); "
                    "Mokuji needs range requests to be honoured"
                )
            content_range = response.headers.get("Content-Range")
            if content_range and self.size is None:
                total = content_range.rpartition("/")[2]
                if total.isdigit():
                    self.size = int(total)
            return response.read()


class BytesSource:
    """A file held in memory, read a range at a time as any other source is.

    The bytes are copied when the source is made, so a caller may reuse its
    buffer. Each range is a slice of what is already held, so a query costs no
    I/O; the requests and bytes it would have cost over a network are still
    counted.
    """

    def __init__(self, data):
        self.data = bytes(data)
        self.size = len(self.data)

    def read(self, offset, length):
        return self.data[offset : offset + length]


def _is_url(source):
    return isinstance(source, str) and source.split(":", 1)[0].lower() in ("http", "https")


# --- primitives -------------------------------------------------------------


def _unzigzag(v):
    return v >> 1 if v % 2 == 0 else -((v + 1) >> 1)


def _scaled(value, exponent):
    """Writes `value * 10^exponent` in full, keeping the zeros the file claims."""
    if exponent >= 0:
        return str(value) + "0" * exponent
    digits = -exponent
    unit = 10**digits
    sign = "-" if value < 0 else ""
    magnitude = abs(value)
    return f"{sign}{magnitude // unit}.{str(magnitude % unit).zfill(digits)}"


class _Cursor:
    def __init__(self, data, pos=0):
        self.data = data
        self.pos = pos

    def byte(self):
        if self.pos >= len(self.data):
            raise MokujiError("truncated")
        value = self.data[self.pos]
        self.pos += 1
        return value

    def take(self, length):
        if self.pos + length > len(self.data):
            raise MokujiError("truncated")
        out = bytes(self.data[self.pos : self.pos + length])
        self.pos += length
        return out

    def varint(self):
        # SPEC.md §5.3.1: at most ten bytes, and nothing past 64 bits.
        value = 0
        for i in range(10):
            byte = self.byte()
            value |= (byte & 0x7F) << (7 * i)
            if byte & 0x80 == 0:
                if value >> 64:
                    raise MokujiError("varint overflows 64 bits")
                return value
        raise MokujiError("varint longer than ten bytes")

    def key(self, previous):
        """A front-coded key, decoded against the previous one (SPEC.md §5.3.1)."""
        shared = self.varint()
        length = self.varint()
        if shared > len(previous):
            raise MokujiError("shared prefix too long")
        return previous[:shared] + self.take(length)

    def group(self, count):
        """One bit-packed group: minimal width, least significant bit first."""
        width = self.byte()
        if width > 64:
            raise MokujiError(f"bit width {width} exceeds 64")
        packed = int.from_bytes(self.take((count * width + 7) // 8), "little")
        mask = (1 << width) - 1
        out = []
        for _ in range(count):
            out.append(packed & mask)
            packed >>= width
        return out

    def values(self, count):
        out = []
        while len(out) < count:
            out.extend(self.group(min(128, count - len(out))))
        return out


def _inflate(unit):
    cursor = _Cursor(unit)
    length = cursor.varint()
    if length > MAX_UNIT:
        raise MokujiError(f"a unit declares {length} bytes, more than this reader allocates")
    inflater = zlib.decompressobj(-15)
    try:
        # One byte more than declared, so that a unit that would decompress to
        # more is caught without decompressing all of it.
        out = inflater.decompress(bytes(unit[cursor.pos :]), length + 1)
    except zlib.error as e:
        raise MokujiError(f"a unit does not decompress: {e}") from None
    if len(out) != length or not inflater.eof:
        raise MokujiError("unit does not match its declared length")
    return out


def _cbor(data, pos=0, depth=0):
    """One CBOR item (RFC 8949), and the position after it.

    The metadata section is canonical CBOR (SPEC.md §6), and a reader must
    tolerate keys it does not know — so every type is decoded, not only the
    ones the keys this reader uses have. Like the TypeScript reader this does
    not check that the encoding is canonical; that is `mokuji verify`'s job.
    """
    if depth > 64:
        raise MokujiError("metadata is nested too deeply")
    if pos >= len(data):
        raise MokujiError("truncated metadata")
    initial = data[pos]
    major = initial >> 5
    small = initial & 0x1F
    pos += 1

    if major == 7:
        # Simple values and floats carry their value in the argument bytes.
        if small == 20:
            return False, pos
        if small == 21:
            return True, pos
        if small in (22, 23):
            return None, pos
        if small < 24:
            return small, pos
        if small == 24:
            if pos >= len(data):
                raise MokujiError("truncated metadata")
            return data[pos], pos + 1
        formats = {25: (">e", 2), 26: (">f", 4), 27: (">d", 8)}
        if small not in formats:
            raise MokujiError("metadata uses an indefinite or reserved length")
        fmt, size = formats[small]
        if pos + size > len(data):
            raise MokujiError("truncated metadata")
        return struct.unpack_from(fmt, data, pos)[0], pos + size

    if small < 24:
        argument = small
    elif small <= 27:
        size = 1 << (small - 24)
        if pos + size > len(data):
            raise MokujiError("truncated metadata")
        argument = int.from_bytes(data[pos : pos + size], "big")
        pos += size
    else:
        raise MokujiError("metadata uses an indefinite or reserved length")

    if major == 0:
        return argument, pos
    if major == 1:
        return -1 - argument, pos
    if major in (2, 3):
        if pos + argument > len(data):
            raise MokujiError("truncated metadata")
        raw = bytes(data[pos : pos + argument])
        return (raw if major == 2 else raw.decode("utf-8", "replace")), pos + argument
    # Every item takes a byte at least, and a map entry two, so a count the
    # bytes left cannot hold is refused before anything is decoded for it.
    if major in (4, 5) and argument * (major - 3) > len(data) - pos:
        raise MokujiError("truncated metadata")
    if major == 4:
        items = []
        for _ in range(argument):
            item, pos = _cbor(data, pos, depth + 1)
            items.append(item)
        return items, pos
    if major == 5:
        mapping = {}
        for _ in range(argument):
            key, pos = _cbor(data, pos, depth + 1)
            value, pos = _cbor(data, pos, depth + 1)
            mapping[key if isinstance(key, (str, int, bytes)) else repr(key)] = value
        return mapping, pos
    # A tag: what it says about the value is not something this reader needs.
    return _cbor(data, pos, depth + 1)


def _decode_completions(data, num_docs):
    """The completions section (SPEC.md §5.3.5), checked as a reader must."""
    cursor = _Cursor(data)
    min_terms = cursor.varint()
    max_span = cursor.varint()
    docs_per_entry = cursor.varint()
    count = cursor.varint()
    if docs_per_entry == 0:
        raise MokujiError("a completion entry lists no documents")
    entries = {}
    previous = b""
    for i in range(count):
        key = cursor.key(previous)
        if not key or (i > 0 and key <= previous):
            raise MokujiError("completion prefixes are empty or out of order")
        previous = key
        n = cursor.varint()
        if n == 0 or n > docs_per_entry:
            raise MokujiError("a completion lists too few or too many")
        docs = []
        last = 0
        for _ in range(n):
            last += _unzigzag(cursor.varint())
            if last < 0 or last >= num_docs or last in docs:
                raise MokujiError("a completion lists a document outside the index, or twice")
            docs.append(last)
        entries[key] = docs
    if cursor.pos != len(data):
        raise MokujiError("trailing bytes in the completions section")
    return Completions(min_terms, max_span, docs_per_entry, entries)


def _decode_columns(data, num_docs):
    """Named columns of per-document numbers (SPEC.md §5.8)."""
    cursor = _Cursor(data)
    count = cursor.varint()
    headers = []
    for _ in range(count):
        name = cursor.take(cursor.varint()).decode("utf-8", "replace")
        raw = cursor.byte()
        if any(taken == name for taken, _ in headers):
            raise MokujiError("two attribute columns share a name")
        headers.append((name, raw - 256 if raw > 127 else raw))
    columns = {}
    for name, scale_exponent in headers:
        values = [_unzigzag(v) for v in cursor.values(num_docs)]
        columns[name] = Column(name, scale_exponent, values)
    if cursor.pos != len(data):
        raise MokujiError("trailing bytes in the document attributes section")
    return columns


def decode_cbor(data):
    """One CBOR item (RFC 8949), decoded as the metadata section is: maps as
    dicts, arrays as lists, byte strings as bytes, text as str."""
    value, end = _cbor(bytes(data))
    if end != len(data):
        raise MokujiError("trailing bytes after the metadata value")
    return value


def _text(value):
    return value if isinstance(value, str) else None


def _field(value, key):
    return value.get(key) if isinstance(value, dict) else None


def _starts_with(key, prefix):
    return key[: len(prefix)] == prefix


# --- a file -----------------------------------------------------------------


class MokujiIndex:
    """One open Mokuji file: its resident set held, everything else read on demand."""

    def __init__(self, source):
        """`source` is a path, an http(s) URL, or anything with
        `read(offset, length)` returning bytes — which is what lets the same
        reader be tested against a file in memory.

        Use `MokujiIndex.open`, which also reads the resident set.
        """
        self._owned = None
        # The whole term blocks section as stored, with its offset, when the
        # dictionary is held rather than fetched.
        self._term_blocks = None
        if _is_url(source):
            self.source = HttpSource(source)
        elif isinstance(source, (str, os.PathLike)):
            self.source = self._owned = FileSource(source)
        else:
            self.source = source
        # Everything a query costs, so a caller can show it rather than claim it.
        self.stats = Stats()
        # Whether neighbouring blocks are read in one request (SPEC.md §4
        # allows it: the count can only fall). On by default; the conformance
        # suite turns it off to check the count the cases state, block by block.
        self.coalesce = True
        # Called with a RequestInfo for every range request, so a caller can
        # show the requests themselves rather than a count of them.
        self.on_request = None

    # The file's length, once the source has said: a path or a URL always
    # does, and any other source does if it has a `size`.
    @property
    def file_length(self):
        return getattr(self.source, "size", None)

    def close(self):
        """Closes the file, when the index opened it from a path."""
        if self._owned is not None:
            self._owned.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def reset_stats(self):
        self.stats = Stats()

    def read(self, offset, length):
        """`length` bytes at `offset`, counted. Fewer only at the end of the file."""
        if length == 0:
            return b""
        data = self.source.read(offset, length)
        self.stats.requests += 1
        self.stats.bytes += len(data)
        if self.on_request is not None:
            self.on_request(RequestInfo(offset, len(data)))
        return data

    def _read_exact(self, offset, length):
        data = self.read(offset, length)
        if len(data) != length:
            raise MokujiError(f"{length} bytes at {offset} lie past the end of the file")
        return data

    def _unit(self, data, codec):
        if codec == CODEC_RAW:
            return data
        if codec == CODEC_DEFLATE:
            return _inflate(data)
        raise MokujiError(f"codec {codec} is registered but not implemented here")

    def _section(self, section_id):
        section = self.sections.get(section_id)
        if section is None:
            raise MokujiError(f"section {section_id:#06x} is missing")
        return section

    @classmethod
    def open(cls, source, *, resident_dictionary=False):
        """Opens `source` and reads its resident set, in one request when the
        file is laid out as SPEC.md §5 recommends.

        `resident_dictionary` holds the whole term dictionary as stored, at one
        more read now and its stored size in memory, and makes a term lookup
        cost no read at all: each block is decompressed from what is held
        (SPEC.md §4.2). Which to choose is a deployment question, not one the
        format answers.
        """
        index = cls(source)
        try:
            index._open()
            if resident_dictionary:
                section = index._section(TERM_BLOCKS)
                if section.length > 0xFFFFFFFF:
                    raise MokujiError("the term dictionary is too large to hold")
                index._term_blocks = (section.offset, index._read_exact(section.offset, section.length))
        except BaseException:
            index.close()
            raise
        return index

    def _open(self):
        # One request for the resident set, optimistically. SPEC.md §5
        # recommends a layout that puts all of it at the head of the file for
        # exactly this.
        prefix = self.read(0, OPEN_PREFIX)
        if len(prefix) < HEADER_LEN or prefix[:8] not in (MAGIC, MAGIC_V0):
            raise MokujiError("not a Mokuji file")
        if prefix[8] != 0:
            raise MokujiError(f"unsupported major version {prefix[8]}")

        (self.flags, count, self.num_docs, self.sum_doclen, table_offset, table_length) = (
            struct.unpack_from("<HIQQQI", prefix, 10)
        )
        # Without frequencies nothing says where one document's positions end
        # and the next begin (SPEC.md §5.6a).
        if self.flags & FLAG_HAS_POSITIONS and not self.flags & FLAG_HAS_FREQS:
            raise MokujiError("positions are flagged without frequencies")
        if table_length < count * SECTION_ENTRY_LEN:
            raise MokujiError("the section table is shorter than its entries")
        if table_offset + table_length <= len(prefix):
            table = prefix[table_offset : table_offset + table_length]
        else:
            table = self._read_exact(table_offset, table_length)

        self.sections = {}
        previous_id = -1
        for i in range(count):
            section_id, codec, offset, length = struct.unpack_from("<HB5xQQ", table, i * SECTION_ENTRY_LEN)
            if section_id <= previous_id:
                raise MokujiError("the section table is out of order")
            previous_id = section_id
            size = self.file_length
            if size is not None and offset + length > size:
                raise MokujiError(f"section {section_id:#06x} lies past the end of the file")
            self.sections[section_id] = _Section(codec, offset, length)

        for section_id in (TERM_BLOCKS, POSTINGS):
            self._section(section_id)
        wanted = [META, TERM_INDEX, TERM_BLOCK_OFFSETS]
        if self.flags & FLAG_HAS_PAYLOAD:
            self._section(PAYLOAD)
            wanted.append(PAYLOAD_OFFSETS)
        if self.flags & FLAG_HAS_POSITIONS:
            self._section(POSITIONS)
        # Both are read in bulk before any payload is, which is what makes them
        # worth having as sections of their own (SPEC.md §5.7, §5.8). A reader
        # that fetched them per query would pay for the separation twice.
        # Completions too: whether a prefix has an entry decides what it costs,
        # and has to be known before anything is read for it (SPEC.md §4.1.1).
        for section_id in (COMPLETIONS, DOC_LENGTHS, DOC_ATTRIBUTES):
            if section_id in self.sections:
                wanted.append(section_id)

        # The recommended layout puts everything resident at the head of the
        # file, so when the opening prefix does not reach far enough the remedy
        # is one wider read rather than one read per section.
        span_end = max(self._section(i).offset + self._section(i).length for i in wanted)
        span = prefix
        if span_end > len(prefix):
            # Only the part not already in hand. Re-reading the prefix would
            # double the bytes for no reason.
            span = prefix + self._read_exact(len(prefix), span_end - len(prefix))

        def resident(section_id):
            section = self._section(section_id)
            return self._unit(span[section.offset : section.offset + section.length], section.codec)

        self.meta = decode_cbor(resident(META))
        self.tokenizer_id = _text(_field(_field(self.meta, "tokenizer"), "id")) or "identity"
        tokenizer_params = _field(_field(self.meta, "tokenizer"), "params")
        # The tokenizer's parameters, and the application's own additions
        # (SPEC.md §6): a file of geohash coverings says in `attrs` how it was
        # built, and a reader acts on that rather than on an agreement.
        self.tokenizer_params = tokenizer_params if isinstance(tokenizer_params, dict) else {}
        attrs = _field(self.meta, "attrs")
        self.attrs = attrs if isinstance(attrs, dict) else None
        generation = _field(self.meta, "generation")
        self.generation = (
            generation if isinstance(generation, int) and not isinstance(generation, bool) and generation >= 0 else 0
        )
        # The index's license and the credit its results carry (SPEC.md §6). A
        # caller that shows results owes the credit, so it is kept at hand.
        self.license = _text(_field(self.meta, "license"))
        self.attribution = _text(_field(self.meta, "attribution"))
        generator = _field(self.meta, "generator")
        params = _field(generator, "params")
        self.generator = Generator(
            name=_text(_field(generator, "name")),
            version=_text(_field(generator, "version")),
            params={k: v for k, v in params.items() if isinstance(k, str)} if isinstance(params, dict) else {},
        )
        sources = _field(self.meta, "source")
        self.sources = [
            Provenance(
                content_hash=_text(_field(entry, "content_hash")) or "",
                uri=_text(_field(entry, "uri")),
                retrieved=_text(_field(entry, "retrieved")),
                modified=_text(_field(entry, "modified")),
            )
            for entry in (sources if isinstance(sources, list) else [])
        ]
        self._normalize = TOKENIZERS.get(self.tokenizer_id)
        if self._normalize is None:
            # Readable, just not reachable by an unnormalized query. Saying so
            # beats returning nothing and looking broken.
            warnings.warn(f"unknown tokenizer '{self.tokenizer_id}'; queries used as typed")
            self._normalize = lambda q: q

        self.doc_lengths = (
            _Cursor(resident(DOC_LENGTHS)).values(self.num_docs) if DOC_LENGTHS in self.sections else None
        )
        self.columns = (
            _decode_columns(resident(DOC_ATTRIBUTES), self.num_docs) if DOC_ATTRIBUTES in self.sections else {}
        )
        self.completions = (
            _decode_completions(resident(COMPLETIONS), self.num_docs) if COMPLETIONS in self.sections else None
        )

        self.term_index = resident(TERM_INDEX)
        self.term_block_offsets = resident(TERM_BLOCK_OFFSETS)
        self.payload_offsets = resident(PAYLOAD_OFFSETS) if self.flags & FLAG_HAS_PAYLOAD else None

        # Boundary keys, decoded once. They are what lets a prefix scan know
        # whether the next block can extend the range without fetching it.
        cursor = _Cursor(self.term_index)
        self.block_count = struct.unpack("<I", cursor.take(4))[0]
        self.boundaries = []
        previous = b""
        for i in range(self.block_count):
            key = cursor.key(previous)
            # SPEC.md §5.3.2: the search below would otherwise silently return
            # the wrong block.
            if i > 0 and key <= previous:
                raise MokujiError("the term index is not strictly increasing")
            self.boundaries.append(key)
            previous = key
        if len(self.term_block_offsets) < (self.block_count + 1) * 8:
            raise MokujiError("the term block offsets are fewer than the blocks")

        if self.payload_offsets is not None:
            header = self.payload_offsets
            if len(header) < 12:
                raise MokujiError("the payload offsets section is truncated")
            self._payload_codec = header[0]
            self._values_per_doc = header[1]
            # A signed byte: the stored integers stand for value * 10^exponent.
            self._scale_exponent = header[2] - 256 if header[2] > 127 else header[2]
            self._docs_per_block, self._payload_blocks = struct.unpack_from("<II", header, 4)
            if self._docs_per_block == 0:
                raise MokujiError("payload blocks hold no documents")
            if len(header) < 12 + (self._payload_blocks + 1) * 8:
                raise MokujiError("the payload offsets are fewer than the blocks")

    def normalize(self, text):
        """What a query goes through before it is looked up as a prefix."""
        return self._normalize(text)

    # --- blocks -------------------------------------------------------------

    def _offset_pair(self, table, base, i, section_id):
        """Where block `i` of a block-addressed section is: (offset, length)."""
        start, end = struct.unpack_from("<QQ", table, base + i * 8)
        section = self._section(section_id)
        if not section.offset <= start < end <= section.offset + section.length:
            raise MokujiError(f"block {i} lies outside its section")
        return start, end - start

    def _term_block_location(self, ordinal):
        return self._offset_pair(self.term_block_offsets, 0, ordinal, TERM_BLOCKS)

    def _payload_block_location(self, ordinal):
        if ordinal >= self._payload_blocks:
            raise MokujiError("a document lies past the last payload block")
        return self._offset_pair(self.payload_offsets, 12, ordinal, PAYLOAD)

    def _block_runs(self, ordinals, locate):
        """Blocks of one section by ordinal, a request for each run of neighbours.

        Blocks are laid end to end, so ordinals that follow one another are one
        contiguous range of the file, and reading them as one costs a single
        request where reading them apart costs one each. Returns each block as
        stored, before its codec is undone.
        """
        runs = []
        for ordinal in sorted(set(ordinals)):
            if runs and self.coalesce and runs[-1][-1] == ordinal - 1:
                runs[-1].append(ordinal)
            else:
                runs.append([ordinal])
        out = {}
        for run in runs:
            start, _ = locate(run[0])
            last_offset, last_length = locate(run[-1])
            data = self._read_exact(start, last_offset + last_length - start)
            for ordinal in run:
                offset, length = locate(ordinal)
                out[ordinal] = data[offset - start : offset - start + length]
        return out

    def _held_term_block(self, ordinal):
        start, held = self._term_blocks
        offset, length = self._term_block_location(ordinal)
        return held[offset - start : offset - start + length]

    def term_blocks(self, ordinals):
        """Several dictionary blocks, decoded, a request for each run of neighbours."""
        if self._term_blocks is not None:
            raw = {ordinal: self._held_term_block(ordinal) for ordinal in set(ordinals)}
        else:
            raw = self._block_runs(ordinals, self._term_block_location)
        return {ordinal: self._decode_term_block(data) for ordinal, data in raw.items()}

    def term_block(self, ordinal):
        if self._term_blocks is not None:
            return self._decode_term_block(self._held_term_block(ordinal))
        offset, length = self._term_block_location(ordinal)
        return self._decode_term_block(self._read_exact(offset, length))

    def _decode_term_block(self, raw):
        block = self._unit(raw, self._section(TERM_BLOCKS).codec)
        cursor = _Cursor(block)
        entry_count = cursor.varint()
        first_term_id = cursor.varint()
        has_positions = bool(self.flags & FLAG_HAS_POSITIONS)
        has_freqs = bool(self.flags & FLAG_HAS_FREQS)
        entries = []
        key = b""
        postings_offset = None
        positions_offset = None
        # One more than the last document an implied or inline entry named:
        # what an inline entry's first document is written against (SPEC.md
        # §5.3.4).
        expected = first_term_id
        for i in range(entry_count):
            key = cursor.key(key)
            term_id = first_term_id + i
            doc_freq = cursor.varint()
            delta = cursor.varint()
            postings_offset = delta if postings_offset is None else postings_offset + delta
            stored = cursor.varint()
            entry = TermEntry(
                term=key,
                term_id=term_id,
                doc_freq=doc_freq,
                postings_offset=postings_offset,
                postings_length=0 if stored == INLINE else stored,
            )
            if stored == IMPLIED:
                expected = term_id + 1
            elif stored == INLINE:
                if doc_freq == 0:
                    raise MokujiError("inline postings name no document")
                docs = [expected + _unzigzag(cursor.varint())]
                for _ in range(1, doc_freq):
                    docs.append(docs[-1] + cursor.varint() + 1)
                if docs[0] < 0 or docs[-1] >= self.num_docs:
                    raise MokujiError("inline postings name a missing document")
                freqs = [cursor.varint() + 1 for _ in docs] if has_freqs else [1] * len(docs)
                entry.inline = Postings(docs, freqs)
                expected = docs[-1] + 1
            if has_positions:
                # Two further varints an entry carries only when the file has
                # positions, which is why the flag is read before any entry is.
                gap = cursor.varint()
                positions_offset = gap if positions_offset is None else positions_offset + gap
                entry.positions_offset = positions_offset
                entry.positions_length = cursor.varint()
            entries.append(entry)
        return entries

    # --- the resident set ---------------------------------------------------

    def column(self, name):
        """One named column of per-document numbers, or None.

        Resident, so this costs no read: the point of the section is that a
        scorer can weigh many documents before deciding which payloads are
        worth fetching.
        """
        return self.columns.get(name)

    def block_for(self, term):
        """The dictionary block a term would be in, from the resident index alone.

        Block 0 for a term before every block, as the TypeScript reader
        answers; `exact` knows such a term is absent without reading it.
        """
        return max(bisect.bisect_right(self.boundaries, term) - 1, 0)

    def _before_every_block(self, term):
        # SPEC.md §5.3.2: if a term precedes the first boundary key, it is
        # absent, and the resident index has said so already.
        return self.block_count == 0 or term < self.boundaries[0]

    # --- terms --------------------------------------------------------------

    def positions(self, entry, postings):
        """Where a term occurs in each document its postings name.

        One read, and only for a caller that asks. The run is decoded from the
        front and the frequencies say how many values belong to each document,
        so the postings have to be in hand first.
        """
        if not self.flags & FLAG_HAS_POSITIONS or not entry.positions_length:
            return []
        section = self._section(POSITIONS)
        offset = entry.positions_offset or 0
        if not section.offset <= offset <= offset + entry.positions_length <= section.offset + section.length:
            raise MokujiError("a positions run lies outside its section")
        cursor = _Cursor(self._read_exact(offset, entry.positions_length))
        out = []
        for freq in postings.freqs:
            places = []
            for value in cursor.values(freq):
                # The first is a position; the rest are gaps less one, so that
                # adjacent occurrences pack to nothing.
                places.append(value if not places else places[-1] + value + 1)
            out.append(places)
        if cursor.pos != len(cursor.data):
            raise MokujiError("trailing bytes in positions run")
        return out

    def exact(self, term_text):
        """One term, exactly, or None.

        A prefix scan would answer this, and would cost more: `xn76ur` is a
        prefix of every cell inside it, so completing it fetches postings for
        all of them when the caller wanted one. At most one block is read.
        """
        term = term_text.encode()
        if self._before_every_block(term):
            return None
        for entry in self.term_block(self.block_for(term)):
            if entry.term == term:
                return entry
            if entry.term > term:
                break
        return None

    def exact_many(self, term_texts):
        """Several terms at once, each exactly, in one read per dictionary block.

        Cells one geohash precision apart are neighbours in the dictionary, so
        the lookups a point needs usually land in the same block. Asking for
        them together is the difference between reading it once and reading it
        twice.
        """
        wanted = []
        for text in term_texts:
            term = text.encode()
            block = None if self._before_every_block(term) else self.block_for(term)
            wanted.append((term, block))
        blocks = self.term_blocks(block for _, block in wanted if block is not None)
        out = []
        for term, block in wanted:
            found = None
            for entry in blocks.get(block, []):
                if entry.term == term:
                    found = entry
                    break
                if entry.term > term:
                    break
            out.append(found)
        return out

    def _nothing_begins_with(self, prefix):
        # The first boundary key is the smallest term. When it sorts after the
        # prefix without beginning with it, every term does, and none can begin
        # with it either.
        if self.block_count == 0:
            return True
        first = self.boundaries[0]
        return prefix < first and not _starts_with(first, prefix)

    def prefix(self, prefix_text, limit=20):
        """Terms beginning with `prefix_text`, at most `limit` of them (None: all)."""
        prefix = prefix_text.encode()
        if self._nothing_begins_with(prefix):
            return []
        start = self.block_for(prefix)
        found = []
        for ordinal in range(start, self.block_count):
            # Whether the range continues is answerable from the resident index
            # alone, so only the first block is fetched on faith.
            if ordinal > start and not _starts_with(self.boundaries[ordinal], prefix):
                break
            done = False
            for entry in self.term_block(ordinal):
                if _starts_with(entry.term, prefix):
                    found.append(entry)
                    if len(found) == limit:
                        return found
                elif entry.term > prefix and found:
                    done = True
                    break
            if done:
                break
        return found

    def prefix_many(self, prefix_texts):
        """Terms beginning with each of several prefixes, every one of them.

        Which blocks each prefix spans is answerable from the resident index,
        so all of them are known before any is read, and they are fetched
        together: a request for each run of neighbouring blocks, however many
        prefixes fall in it. This is what a query over an area needs — the
        cells covering it are many short prefixes, mostly in blocks next to one
        another — and what `prefix`, which walks one prefix a block at a time,
        would make a request per block per prefix.
        """
        prefixes = [text.encode() for text in prefix_texts]
        spans = []
        for prefix in prefixes:
            if self._nothing_begins_with(prefix):
                spans.append(None)
                continue
            start = end = self.block_for(prefix)
            while end + 1 < self.block_count and _starts_with(self.boundaries[end + 1], prefix):
                end += 1
            spans.append((start, end))
        ordinals = [i for span in spans if span for i in range(span[0], span[1] + 1)]
        blocks = self.term_blocks(ordinals)
        out = []
        for prefix, span in zip(prefixes, spans):
            found = []
            if span:
                for i in range(span[0], span[1] + 1):
                    found.extend(e for e in blocks[i] if _starts_with(e.term, prefix))
            out.append(found)
        return out

    def range(self, low_text, high_text, limit=None):
        """Terms from `low_text` to `high_text`, both inclusive, at most `limit`
        of them (None: all).

        The same walk as `prefix`, with a different test for where it stops:
        the dictionary is sorted, so a range of keys is a run of neighbouring
        entries, and whether the run continues past a block is answerable from
        the resident index alone.
        """
        low = low_text.encode()
        high = high_text.encode()
        if self.block_count == 0 or high < self.boundaries[0]:
            return []
        start = self.block_for(low)
        found = []
        for ordinal in range(start, self.block_count):
            if ordinal > start and self.boundaries[ordinal] > high:
                break
            for entry in self.term_block(ordinal):
                if entry.term > high:
                    return found
                if entry.term >= low:
                    found.append(entry)
                    if len(found) == limit:
                        return found
        return found

    # --- postings and payload -----------------------------------------------

    def implied(self, entry):
        """The postings a dictionary entry states itself (SPEC.md §5.3.4):
        inline, or implied — the term names one document, whose ID is the
        term's own, once."""
        if entry.inline is not None:
            return entry.inline
        if entry.doc_freq != 1:
            raise MokujiError("implied postings need a doc_freq of 1")
        if entry.term_id >= self.num_docs:
            raise MokujiError("implied postings name a missing document")
        return Postings([entry.term_id], [1])

    def postings_span(self, entries):
        """Postings for a contiguous range of entries, in one request — or none."""
        entries = list(entries)
        if not entries:
            return []
        # Entries whose postings are implied or inline contribute no bytes, so
        # an index of a dictionary spends no request here at all.
        stored = [e for e in entries if e.postings_length > 0]
        start = 0
        span = b""
        if stored:
            start = stored[0].postings_offset
            end = stored[-1].postings_offset + stored[-1].postings_length
            section = self._section(POSTINGS)
            if not section.offset <= start <= end <= section.offset + section.length:
                raise MokujiError("a postings run lies outside its section")
            span = self._read_exact(start, end - start)

        has_freqs = bool(self.flags & FLAG_HAS_FREQS)
        out = []
        for entry in entries:
            if entry.postings_length == 0:
                out.append(self.implied(entry))
                continue
            at = entry.postings_offset - start
            if at < 0 or at + entry.postings_length > len(span):
                raise MokujiError("postings runs are not in term order")
            cursor = _Cursor(span[at : at + entry.postings_length])
            if cursor.byte() != 0:
                raise MokujiError("unsupported postings codec")
            count = cursor.varint()
            # The dictionary and the postings are separate sections, and
            # nothing but this ties them together.
            if count != entry.doc_freq:
                raise MokujiError("doc_freq and the postings run disagree")
            docs = []
            # The first value is a document ID; the rest are gaps less one.
            for value in cursor.values(count):
                docs.append(value if not docs else docs[-1] + value + 1)
            if docs and docs[-1] >= self.num_docs:
                raise MokujiError("a postings run names a missing document")
            freqs = [f + 1 for f in cursor.values(count)] if has_freqs else [1] * count
            if cursor.pos != len(cursor.data):
                raise MokujiError("trailing bytes in a postings run")
            out.append(Postings(docs, freqs))
        return out

    def payload_span(self, docs):
        """Payload for documents, as text: opaque bytes decoded, or integers
        written out against the scale the file declares."""
        docs = list(docs)
        if self.payload_offsets is None:
            return ["" for _ in docs]
        per_block = self._docs_per_block
        # Every block the documents fall in, read once each. Neighbouring
        # blocks are one range of the file and read as one.
        raw = self._block_runs((doc // per_block for doc in docs), self._payload_block_location)
        codec = self._section(PAYLOAD).codec
        blocks = {ordinal: self._unit(data, codec) for ordinal, data in raw.items()}

        out = []
        for doc in docs:
            block = blocks[doc // per_block]
            cursor = _Cursor(block)
            count = cursor.varint()
            index = doc % per_block
            if index >= count:
                raise MokujiError("payload block is shorter than expected")

            if self._payload_codec == PAYLOAD_OPAQUE:
                for i in range(index + 1):
                    length = cursor.varint()
                    if i == index:
                        out.append(cursor.take(length).decode("utf-8", "replace"))
                    else:
                        cursor.take(length)
            elif self._payload_codec == PAYLOAD_DELTA_INTS:
                # Every entry up to the wanted one has to be decoded, because
                # each is a difference from the one before it. A block is what
                # gets fetched anyway, and it is sized so that this is cheap.
                values = [0] * self._values_per_doc
                for _ in range(index + 1):
                    for i in range(self._values_per_doc):
                        values[i] += _unzigzag(cursor.varint())
                out.append(",".join(_scaled(v, self._scale_exponent) for v in values))
            else:
                raise MokujiError(f"payload codec {self._payload_codec} is not implemented here")
        return out

    # --- queries ------------------------------------------------------------

    def search(
        self, query, *, limit=10, any=False, probe=None, length_norm=0.75, boost=None, filter=None, phrase=False
    ):
        """Documents ranked by BM25 over the terms the query becomes.

        The same ranking as `mokuji search` and the TypeScript reader:
        Robertson–Spärck Jones idf, `k1` of 1.2, `b` from `length_norm`, a
        union scored over the terms a document actually has, and ties broken by
        document ID. Without document lengths the order is by document ID and
        the scores are None.

        `any` takes the union of the terms rather than their intersection: what
        an n-gram query wants, since a misspelling shares every gram but two.
        `probe` fetches postings for only that many of the terms, the rarest,
        which the dictionary says without reading any postings. `boost` names
        an attribute column added to each score as `ln(1 + value)`, a static
        prior. `filter` keeps only the documents it returns true for, by
        document ID, before they are ranked and before any payload is read, so
        that discarding a document costs no payload; `matched` counts what it
        kept. `phrase` keeps only the documents where the terms read occur at
        consecutive positions, in the order they were typed; the file must
        carry positions.

        One request per run of dictionary blocks the terms fall in, then one
        per term whose postings are stored as a run, then, for a phrase, one
        per term for its positions, then the payload of the first `limit`.
        """
        split = SPLITTERS.get(self.tokenizer_id, lambda q: [q])
        terms = split(query)
        ranking = self.rank(
            terms, any=any, probe=probe, length_norm=length_norm, boost=boost, filter=filter, phrase=phrase
        )
        shown = ranking.ranked[:limit]
        wanted = sorted(r.doc for r in shown)
        by_doc = dict(zip(wanted, self.payload_span(wanted)))
        return SearchResult(
            terms=terms,
            probed=ranking.probed,
            skipped=ranking.skipped,
            matched=len(ranking.ranked),
            results=[Ranked(r.doc, r.score, by_doc[r.doc]) for r in shown],
        )

    def rank(
        self, terms, *, any=False, probe=None, length_norm=0.75, boost=None, filter=None, phrase=False, limit=None
    ):
        """Every document `terms` match, best first, and what finding them read:
        the half of `search` before any payload. `terms` are already split by
        the file's tokenizer; the options are `search`'s, and `limit`, which
        only decides how many payloads `search` reads, is ignored.
        """
        column = None if boost is None else self.column(boost)
        if boost is not None and column is None:
            raise NoColumnError(boost)
        if phrase and not self.flags & FLAG_HAS_POSITIONS:
            raise MokujiError("this index carries no positions, so it cannot test a phrase")
        if not terms:
            return Ranking([], [], 0)

        # Each with its place in the query, which a phrase is tested against:
        # probing reorders the terms, and the order typed is what a phrase is.
        present = []
        for at, (term, entry) in enumerate(zip(terms, self.exact_many(terms))):
            if entry is not None:
                present.append((at, term, entry))
            elif not any:
                # A term nobody wrote down cannot be in an intersection.
                return Ranking([], [], 0)
        if not present:
            return Ranking([], [], 0)

        # Rarest first: a term matching few documents says the most about which
        # ones, and costs the least to say it. Stable, as the CLI's sort is.
        skipped = 0
        if probe is not None:
            present.sort(key=lambda p: p[2].doc_freq)
            skipped = max(0, len(present) - probe)
            present = present[:probe]
        probed = [Probed(term, entry.doc_freq) for _, term, entry in present]
        if not present:
            # Every term was left out, so there is nothing to match against.
            return Ranking([], probed, skipped)
        runs = [self.postings_span([entry])[0] for _, _, entry in present]

        if any:
            matched = sorted({doc for run in runs for doc in run.docs})
        else:
            rest = [set(run.docs) for run in runs[1:]]
            matched = [doc for doc in runs[0].docs if all(doc in s for s in rest)]
        if filter is not None:
            matched = [doc for doc in matched if filter(doc)]
        if phrase and matched:
            matched = self._phrase(present, runs, matched)

        if self.doc_lengths is None:
            return Ranking([Scored(doc, None) for doc in matched], probed, skipped)
        n = self.num_docs
        avgdl = max(1 if n == 0 else self.sum_doclen / n, 1)
        k1 = 1.2
        b = length_norm
        at = [dict(zip(run.docs, run.freqs)) for run in runs]
        ranked = []
        for doc in matched:
            dl = self.doc_lengths[doc] if doc < len(self.doc_lengths) else 0
            score = 0
            # The same operations in the same order as the TypeScript reader
            # and the CLI. `math.log` is the platform's, as Rust's `ln` is;
            # V8's differs from it in the last place for about one argument in
            # seventy, which moves an order only between scores that close.
            for (_, _, entry), freqs in zip(present, at):
                tf = freqs.get(doc)
                if tf is None:
                    continue
                df = entry.doc_freq
                idf = math.log((n - df + 0.5) / (df + 0.5) + 1)
                score += (idf * (tf * (k1 + 1))) / (tf + k1 * (1 - b + (b * dl) / avgdl))
            if column is not None:
                value = column.values[doc] if doc < len(column.values) else 0
                score += math.log(1 + max(value, 0))
            ranked.append(Scored(doc, score))
        ranked.sort(key=lambda r: (-r.score, r.doc))
        return Ranking(ranked, probed, skipped)

    def _phrase(self, present, runs, matched):
        """The documents of `matched` where every term read occurs at its place
        in the query from one starting position: one read per term, for its
        positions, and only here."""
        places = []
        # A term typed twice is one term in the file, so its positions are
        # read once and tested at each of its places.
        read = {}
        for (at, _, entry), run in zip(present, runs):
            if entry.term not in read:
                per_doc = self.positions(entry, run)
                if len(per_doc) != len(run.docs):
                    raise MokujiError("positions disagree with the postings")
                read[entry.term] = {doc: set(p) for doc, p in zip(run.docs, per_doc)}
            places.append((at, read[entry.term]))
        kept = []
        for doc in matched:
            # A document lacking a term cannot hold the phrase, which is what
            # `any` leaves possible.
            if not all(doc in by_doc for _, by_doc in places):
                continue
            first, by_first = places[0]
            if any(all(start - first + at in by_doc[doc] for at, by_doc in places) for start in by_first[doc]):
                kept.append(doc)
        return kept

    def complete(self, typed, limit=10):
        """The first `limit` documents for what has been typed.

        From the completions section when the prefix has an entry that reaches
        the limit — or lists fewer than it may, so lists them all — which costs
        no request before the payload. Otherwise every term the prefix matches,
        whose runs are united and returned in document order.
        """
        normalized = self.normalize(typed)
        docs, from_table = self.complete_docs(normalized, limit)
        requests_before_payload = self.stats.requests
        payloads = self.payload_span(docs)
        return Completion(normalized, from_table, docs, payloads, requests_before_payload)

    def complete_docs(self, normalized, limit):
        """`complete` for a prefix already normalized, without the payload:
        (docs, from_table)."""
        listed = self.completions.entries.get(normalized.encode()) if self.completions else None
        if listed is not None and (limit <= len(listed) or len(listed) < self.completions.docs_per_entry):
            return listed[:limit], True
        entries = self.prefix(normalized, None)
        postings = self.postings_span(entries)
        docs = sorted({doc for p in postings for doc in p.docs})[:limit]
        return docs, False

    def lookup(self, prefix_text, limit=20):
        """Complete a prefix, returning every matching document with its payload.

        The query goes through the index's own tokenizer first. Without that,
        an index built from `一丁目` is unreachable by anyone typing `一丁目`.
        `limit` is how many terms; a term may name more than one document.
        """
        normalized = self.normalize(prefix_text)
        entries = self.prefix(normalized, limit)
        postings = self.postings_span(entries)

        # A term may name more than one document — in a dictionary it never
        # does, but `tiny` is not a dictionary, and returning only the first
        # would drop results silently. Documents are collected in ascending
        # order so that the payload reads share blocks.
        wanted = sorted({doc for p in postings for doc in p.docs})
        payloads = dict(zip(wanted, self.payload_span(wanted)))

        results = []
        for entry, p in zip(entries, postings):
            term = entry.term.decode("utf-8", "replace")
            results.extend(LookupResult(term, doc, payloads[doc]) for doc in p.docs)
        return Lookup(normalized, results)


# --- a collection -----------------------------------------------------------


@dataclass(frozen=True)
class Hit:
    """One document of a collection: its segment, and its ID there."""

    segment: int
    doc: int


@dataclass(frozen=True)
class Consulted:
    segment: int
    from_table: bool


@dataclass
class CollectionCompletion:
    """What a collection's completion found, and how."""

    normalized: str
    hits: list
    consulted: list
    payloads: list
    requests_before_payload: int


@dataclass
class CollectionStats:
    requests: int = 0
    bytes: int = 0
    # The most requests any one segment made.
    round_trips: int = 0


@dataclass
class CollectionSearch:
    """What a collection's search found: every live document holding every
    term, in collection order, and the first `limit` with their payload."""

    terms: list
    matched: int
    hits: list
    payloads: list


# --- manifests ----------------------------------------------------------------

# The version of MANIFEST.md this reader implements.
MANIFEST_VERSION = "0.1"


def _is_uint(value):
    # A u64, as the Rust crate reads it: JSON has no bound, so one is set here.
    return isinstance(value, int) and not isinstance(value, bool) and 0 <= value <= 0xFFFFFFFFFFFFFFFF


def _content_hash_ok(text):
    algorithm, colon, digest = text.partition(":")
    return bool(colon and algorithm and digest) and all(c in "0123456789abcdef" for c in digest)


def check_manifest(value):
    """A manifest already decoded from JSON, checked as `parse_manifest` checks
    it, with only the keys MANIFEST.md defines: `deletes` always a list, and
    `region` None when the segment states none.

    Unknown keys are dropped, as they are ignored in a file's metadata: a later
    version may add one, and a reader of this one still reads the rest.
    """

    def manifest_error(detail):
        return MokujiError(f"manifest: {detail}")

    if not isinstance(value, dict):
        raise manifest_error("not a JSON object")
    version = value.get("mokuji_manifest")
    generation = value.get("generation")
    segments = value.get("segments")
    if not isinstance(version, str):
        raise manifest_error("mokuji_manifest is not text")
    if not _is_uint(generation):
        raise manifest_error("generation is not an unsigned integer")
    if not isinstance(segments, list):
        raise manifest_error("segments is not a list")
    if version.split(".")[0] != MANIFEST_VERSION.split(".")[0]:
        raise manifest_error(f"version {version} is not one this reader implements")
    if not segments:
        raise manifest_error("a collection has no segments")
    out = []
    for segment in segments:
        if not isinstance(segment, dict):
            raise manifest_error("a segment is not a JSON object")
        path = segment.get("path")
        content_hash = segment.get("content_hash")
        length = segment.get("length")
        deletes = segment.get("deletes", [])
        region = segment.get("region")
        if not isinstance(path, str) or not isinstance(content_hash, str) or not _is_uint(length):
            raise manifest_error("a segment needs a path, a content_hash and a length")
        if not isinstance(deletes, list) or not all(_is_uint(d) for d in deletes):
            raise manifest_error("a segment's deletes are not unsigned integers")
        if region is not None and not (
            isinstance(region, dict)
            and isinstance(region.get("countries"), list)
            and all(isinstance(c, str) for c in region["countries"])
        ):
            raise manifest_error("a segment's region is not a list of countries")

        def segment_error(detail):
            return MokujiError(f"segment {path}: {detail}")

        if not path:
            raise manifest_error("a segment has no path")
        if not _content_hash_ok(content_hash):
            raise segment_error("content_hash is not <algorithm>:<hex>")
        if any(b <= a for a, b in zip(deletes, deletes[1:])):
            raise segment_error("deletes are not strictly increasing")
        if region is not None:
            countries = region["countries"]
            if not countries:
                raise segment_error("a region names no country")
            for code in countries:
                if len(code) != 2 or not all("A" <= c <= "Z" for c in code):
                    raise segment_error("a region's countries are ISO 3166-1 alpha-2 codes")
            if len(set(countries)) != len(countries):
                raise segment_error("a region names a country twice")
            region = {"countries": list(countries)}
        out.append(
            {"path": path, "content_hash": content_hash, "length": length, "deletes": list(deletes), "region": region}
        )
    return {"mokuji_manifest": version, "generation": generation, "segments": out}


def parse_manifest(text):
    """Parses and checks a manifest (MANIFEST.md §2), as the Rust crate's
    `Manifest::parse` does, refusing what it refuses."""
    def unique(pairs):
        keys = [key for key, _ in pairs]
        if len(set(keys)) != len(keys):
            raise MokujiError("manifest: a key appears twice in one object")
        return dict(pairs)

    try:
        value = json.loads(text, object_pairs_hook=unique)
    except MokujiError:
        raise
    except ValueError as e:
        raise MokujiError(f"manifest: {e}") from None
    return check_manifest(value)


def within(manifest, countries):
    """The collection within these countries: every segment whose region
    includes one of them, and every segment with no region, in the same order
    and with the same deletes (MANIFEST.md §7).

    Restricting keeps collection order, so a restricted collection answers as
    the whole one would with the other regions' documents deleted.
    """
    return {
        **manifest,
        "segments": [
            s
            for s in manifest["segments"]
            if not s.get("region") or any(c in countries for c in s["region"]["countries"])
        ],
    }


def manifest_to_json(manifest):
    """The manifest as JSON, one key order and one layout, so that the same
    collection is always written down the same way: the bytes the Rust crate's
    `Manifest::to_json` writes, an empty `deletes` and an absent `region` left
    out, and a newline at the end."""
    segments = []
    for s in manifest["segments"]:
        segment = {"path": s["path"], "content_hash": s["content_hash"], "length": s["length"]}
        if s.get("deletes"):
            segment["deletes"] = list(s["deletes"])
        if s.get("region") is not None:
            segment["region"] = {"countries": list(s["region"]["countries"])}
        segments.append(segment)
    document = {
        "mokuji_manifest": manifest["mokuji_manifest"],
        "generation": manifest["generation"],
        "segments": segments,
    }
    return json.dumps(document, indent=2, ensure_ascii=False) + "\n"


def _fetch_json(url):
    request = urllib.request.Request(url, headers={"User-Agent": "mokuji-python", "Cache-Control": "no-cache"})
    with urllib.request.urlopen(request, timeout=30) as response:
        return json.load(response)


class MokujiCollection:
    """A collection: segments in a manifest's order, less their deletes
    (MANIFEST.md). Every segment is an ordinary Mokuji file, opened on its own."""

    def __init__(self, manifest, segments):
        self.manifest = manifest
        self.segments = segments
        self.deletes = [set(s.get("deletes") or []) for s in manifest["segments"]]
        # How many segments the manifest lists, before a region left some out.
        self.manifest_segments = len(manifest["segments"])

    @classmethod
    def open(cls, manifest, source_for=None, *, countries=None):
        """Opens a manifest and every segment it names.

        `manifest` is a path or a URL to the JSON, or the manifest already
        parsed. A segment's path is resolved against the manifest's own
        location (MANIFEST.md §2), and `source_for(location)` may supply a
        source for it instead of the reader opening it — a dict of files in
        memory, say. With `countries`, only the segments whose region includes
        one of them, and those with none, are opened (MANIFEST.md §7).
        """
        base = None
        if isinstance(manifest, (str, os.PathLike)):
            base = os.fspath(manifest)
            if _is_url(base):
                manifest = _fetch_json(base)
            else:
                with open(base, encoding="utf-8") as f:
                    manifest = json.load(f)
        manifest = check_manifest(manifest)
        listed = len(manifest["segments"])
        if countries is not None:
            manifest = within(manifest, countries)

        def locate(path):
            if base is None:
                return path
            if _is_url(base):
                return urllib.parse.urljoin(base, path)
            return os.path.join(os.path.dirname(base), path)

        segments = []
        try:
            for s in manifest["segments"]:
                location = locate(s["path"])
                index = MokujiIndex.open(source_for(location) if source_for else location)
                segments.append(index)
                # A reader MUST reject a segment whose length differs from the
                # manifest's; one that cannot say its length is taken on trust.
                if index.file_length is not None and index.file_length != s["length"]:
                    raise MokujiError(f"{s['path']} is {index.file_length} bytes, not the {s['length']} the manifest says")
                deletes = s.get("deletes") or []
                if any(d >= index.num_docs or (i > 0 and d <= deletes[i - 1]) for i, d in enumerate(deletes)):
                    raise MokujiError(f"{s['path']} has deletes that are out of order or out of range")
            # A query has to mean the same thing in every segment.
            tokenizer = _field(segments[0].meta, "tokenizer") if segments else None
            for s, index in zip(manifest["segments"], segments):
                if _field(index.meta, "tokenizer") != tokenizer:
                    raise MokujiError(f"{s['path']} was tokenized differently from the first segment")
        except BaseException:
            for index in segments:
                index.close()
            raise
        collection = cls(manifest, segments)
        collection.manifest_segments = listed
        return collection

    def close(self):
        for index in self.segments:
            index.close()

    def __enter__(self):
        return self

    def __exit__(self, *exc):
        self.close()

    def reset_stats(self):
        for index in self.segments:
            index.reset_stats()

    @property
    def stats(self):
        """Requests and bytes, summed; and the most requests any one segment made."""
        per = [s.stats for s in self.segments]
        return CollectionStats(
            requests=sum(s.requests for s in per),
            bytes=sum(s.bytes for s in per),
            round_trips=max([0] + [s.requests for s in per]),
        )

    def normalize(self, typed):
        return self.segments[0].normalize(typed) if self.segments else typed

    def live_in(self, at, normalized, need):
        """One segment's first `need` live documents: the entry, then all it
        lists, then as many again as the segment has deletes (MANIFEST.md
        §3.2). Returns (live, from_table)."""
        if not 0 <= at < len(self.segments):
            raise MokujiError(f"the collection has no segment {at}")
        index = self.segments[at]
        deleted = self.deletes[at]
        listed = index.completions.docs_per_entry if index.completions else 0
        from_table = True
        live = []
        for ask in (need, max(listed, need), need + len(deleted)):
            docs, table = index.complete_docs(normalized, ask)
            from_table = from_table and table
            live = [d for d in docs if d not in deleted]
            if len(live) >= need or len(docs) < ask:
                break
        return live[:need], from_table

    def complete(self, typed, limit=10):
        """The first `limit` live documents, in collection order."""
        normalized = self.normalize(typed)
        hits = []
        consulted = []
        for at in range(len(self.segments)):
            if len(hits) >= limit:
                break
            live, from_table = self.live_in(at, normalized, limit - len(hits))
            consulted.append(Consulted(at, from_table))
            hits.extend(Hit(at, doc) for doc in live)
        requests_before_payload = self.stats.requests
        return CollectionCompletion(normalized, hits, consulted, self.payloads(hits), requests_before_payload)

    def candidates(self, typed, limit=10):
        """Every segment's own first `limit`, for a caller that ranks across
        segments by a score of its own (MANIFEST.md §3.3)."""
        normalized = self.normalize(typed)
        lists = [self.live_in(at, normalized, limit) for at in range(len(self.segments))]
        hits = [Hit(segment, doc) for segment, (live, _) in enumerate(lists) for doc in live]
        requests_before_payload = self.stats.requests
        return CollectionCompletion(
            normalized,
            hits,
            [Consulted(segment, from_table) for segment, (_, from_table) in enumerate(lists)],
            self.payloads(hits),
            requests_before_payload,
        )

    def payloads(self, hits):
        """Payload for hits, one span per segment, in the hits' order."""
        for hit in hits:
            if not 0 <= hit.segment < len(self.segments):
                raise MokujiError(f"the collection has no segment {hit.segment}")
        out = [None] * len(hits)
        for at, index in enumerate(self.segments):
            mine = sorted(((h.doc, i) for i, h in enumerate(hits) if h.segment == at))
            if not mine:
                continue
            for (_, i), payload in zip(mine, index.payload_span(doc for doc, _ in mine)):
                out[i] = payload
        return out

    def search(self, query, limit=10):
        """The live documents holding every term the query becomes, in
        collection order: segments in manifest order, documents in each
        segment's. No ranking, which would need a score the segments share.

        Each segment is asked on its own, a term at a time, and a segment that
        lacks a term or has nothing left of the intersection is asked no
        further, as the Rust crate's `Collection::search` does. `matched` counts
        every live document; the first `limit` come with their payload.
        """
        if not self.segments:
            return CollectionSearch([query], 0, [], [])
        split = SPLITTERS.get(self.segments[0].tokenizer_id, lambda q: [q])
        terms = split(query)
        hits = []
        for at, index in enumerate(self.segments):
            matched = None
            for term in terms:
                entry = index.exact(term)
                if entry is None:
                    matched = []
                    break
                docs = index.postings_span([entry])[0].docs
                if matched is None:
                    matched = docs
                else:
                    holding = set(docs)
                    matched = [d for d in matched if d in holding]
                if not matched:
                    break
            deleted = self.deletes[at]
            hits.extend(Hit(at, doc) for doc in (matched or []) if doc not in deleted)
        shown = hits[:limit]
        return CollectionSearch(terms, len(hits), shown, self.payloads(shown))

    @property
    def attributions(self):
        """Each segment's credit, once each, for a caller that shows results."""
        out = []
        for index in self.segments:
            if index.attribution and index.attribution not in out:
                out.append(index.attribution)
        return out
