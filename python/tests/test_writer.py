# The writer is held to the fixtures, not the other way round.
#
# The raw fixtures were built by conformance/fixtures/build_fixtures.py, an
# independent implementation written from SPEC.md, and the Rust writer
# reproduces them byte for byte. If this writer does too, they agree on the
# layout down to the last varint.

import random
import unittest

from support import FIXTURES, BytesSource, cases, fixture_bytes

from mokuji import (
    Builder,
    BuildError,
    Column,
    CompletionOptions,
    Metadata,
    MokujiIndex,
    WriteOptions,
    encode_cbor,
    split,
)

SUITE = cases()


def meta_of(file):
    """A fixture's metadata section, verbatim, so that a comparison of
    layouts is not one of CBOR."""
    section = MokujiIndex.open(BytesSource(file)).sections[0x0001]
    return file[section.offset : section.offset + section.length]


def corpus(name):
    lines = (FIXTURES / f"{name}.corpus.txt").read_text().rstrip("\n").split("\n")
    return [tuple(line.split("\t", 1)) for line in lines]


def invert(texts, tokenizer):
    """Postings in the order a Builder takes them: terms by bytes, documents
    ascending, with where each term occurs."""
    places = {}
    for doc, text in enumerate(texts):
        for at, token in enumerate(split(tokenizer, text)):
            places.setdefault(token.encode(), {}).setdefault(doc, []).append(at)
    return sorted(places.items())


def build_tiny(meta, compress):
    builder = Builder(
        WriteOptions(
            terms_per_block=2, docs_per_payload_block=2, positions=True, doc_lengths=True, compress=compress
        )
    )
    rows = corpus("tiny")
    for _, payload in rows:
        builder.add_document(payload)
    for term, docs in invert([text for text, _ in rows], "space-split"):
        for doc, places in sorted(docs.items()):
            builder.add_posting_at(term, doc, places)
    return builder.finish(meta)


def parse_values(text, decimals):
    out = []
    for value in text.split(","):
        whole, _, fraction = value.strip().partition(".")
        out.append(int(whole + (fraction + "0" * decimals)[:decimals]))
    return out


def build_dict(meta, compress):
    builder = Builder(
        WriteOptions(terms_per_block=2, docs_per_payload_block=2, ints=(2, -6), compress=compress)
    )
    rows = corpus("dict")
    for _, payload in rows:
        builder.add_document_values(parse_values(payload, 6))
    # The corpus is already in term order, which is what lets every term's
    # postings be implied.
    for doc, (term, _) in enumerate(rows):
        builder.add_posting(term, doc, 1)
    builder.add_column(Column("importance", 0, [40, 90, 85, 70, 60]))
    return builder.finish(meta)


def build_names(name, meta, compress):
    builder = Builder(
        WriteOptions(
            terms_per_block=2,
            docs_per_payload_block=2,
            compress=compress,
            completions=CompletionOptions(min_terms=2, docs_per_entry=2),
            # As the fixture builder writes `names`: every run stored.
            inline_postings=0,
        )
    )
    rows = corpus(name)
    for _, payload in rows:
        builder.add_document(payload)
    for term, docs in invert([text for text, _ in rows], "name-list"):
        for doc, places in sorted(docs.items()):
            builder.add_posting(term, doc, len(places))
    return builder.finish(meta)


BUILDS = {
    "tiny": build_tiny,
    "dict": build_dict,
    "names": lambda meta, compress: build_names("names", meta, compress),
    "names-delta": lambda meta, compress: build_names("names-delta", meta, compress),
}


def units(file):
    """Every unit of a file but the dictionary blocks, decompressed, by
    section: what a file says, whatever compressor wrote it."""
    index = MokujiIndex.open(BytesSource(file))
    out = {}
    for section_id, section in index.sections.items():
        if section_id in (0x0011, 0x0012, 0x0031):
            # The offset tables say where units are, which compression moves;
            # the dictionary's offsets are absolute, and are compared below.
            continue
        if section_id == 0x0030:
            pieces = [file[o : o + n] for o, n in map(index._payload_block_location, range(index._payload_blocks))]
        else:
            pieces = [file[section.offset : section.offset + section.length]]
        out[section_id] = [index._unit(p, section.codec) for p in pieces]
    return out


def entries(file):
    """Every dictionary entry, its offsets relative to the sections they point
    into."""
    index = MokujiIndex.open(BytesSource(file))
    out = []
    for ordinal in range(index.block_count):
        for entry in index.term_block(ordinal):
            entry.postings_offset -= index.sections[0x0020].offset
            if entry.positions_offset is not None:
                entry.positions_offset -= index.sections[0x0060].offset
            out.append(entry)
    return out


class Fixtures(unittest.TestCase):
    def test_reproduces_the_raw_fixtures_byte_for_byte(self):
        for name, build in BUILDS.items():
            with self.subTest(name):
                expected = fixture_bytes(name)
                meta = meta_of(expected)
                self.assertEqual(build(meta, False), expected)
                # And again: one input, one file.
                self.assertEqual(build(meta, False), expected)

    def test_a_compressed_file_says_what_the_rust_writers_does(self):
        # dict-deflate was written by the Rust writer, whose deflate is not
        # zlib's, so the bytes differ. What they decompress to may not.
        expected = fixture_bytes("dict-deflate")
        built = build_dict(meta_of(expected), True)
        self.assertEqual(units(built), units(expected))
        self.assertEqual(entries(built), entries(expected))
        # Which sections shrink is each compressor's own answer, so the codecs
        # may differ; the term index shrinks under any, or nothing above was
        # about deflate.
        self.assertEqual(MokujiIndex.open(BytesSource(built)).sections[0x0010].codec, 1)

    def test_compressed_fixtures_answer_every_case(self):
        compressed = {name: BUILDS[name](meta_of(fixture_bytes(name)), True) for name in ("tiny", "dict", "names")}

        def opened(name):
            index = MokujiIndex.open(BytesSource(compressed[name]))
            index.coalesce = False
            index.reset_stats()
            return index

        for case in SUITE["case"]:
            if case["fixture"] not in compressed:
                continue
            with self.subTest(case["name"]):
                index = opened(case["fixture"])
                entries_ = [index.exact(t) for t in case["query"]]
                docs = []
                if all(entries_):
                    runs = [index.postings_span([e])[0] for e in entries_]
                    docs = [d for d in runs[0].docs if all(d in r.docs for r in runs[1:])]
                self.assertEqual(docs, case["docs"])
                self.assertEqual(index.payload_span(docs), case["payloads"])
                self.assertEqual(index.stats.requests, case["reads"])
        for case in SUITE["completion"]:
            with self.subTest(case["name"]):
                index = opened(case["fixture"])
                done = index.complete(case["prefix"], case["limit"])
                self.assertEqual(done.payloads, case["payloads"])
                self.assertEqual(done.from_table, case["from_table"])
                self.assertEqual(index.stats.requests, case["reads"])


class Meta(unittest.TestCase):
    def test_encodes_as_the_fixtures_do(self):
        # Canonical CBOR has one encoding for one value.
        for name in ("tiny", "dict", "names", "names-delta"):
            with self.subTest(name):
                file = fixture_bytes(name)
                index = MokujiIndex.open(BytesSource(file))
                meta = Metadata(
                    generation=index.meta["generation"],
                    sources=index.sources,
                    generator_name=index.meta["generator"]["name"],
                    generator_version=index.meta["generator"]["version"],
                    generator_params=index.meta["generator"]["params"],
                    tokenizer_id=index.tokenizer_id,
                    tokenizer_params=index.meta["tokenizer"]["params"],
                    attrs=index.meta.get("attrs"),
                    license=index.license,
                    attribution=index.attribution,
                )
                self.assertEqual(meta.encode(), meta_of(file))

    def test_canonical_cbor(self):
        for value, hex_ in [
            (0, "00"),
            (24, "1818"),
            (256, "190100"),
            (1 << 32, "1b0000000100000000"),
            ("a", "6161"),
            ([1, "b"], "82016162"),
            # Keys sorted by their encoding: shorter first, then bytewise.
            ({"bb": 1, "c": 2, "a": 3}, "a3616103616302626262" "01"),
        ]:
            self.assertEqual(encode_cbor(value).hex(), hex_, value)
        for bad in (-1, 1.5, {1: 2}, None):
            with self.assertRaises(BuildError):
                encode_cbor(bad)


class Order(unittest.TestCase):
    def test_rejects_what_is_out_of_order(self):
        builder = Builder()
        builder.add_document(b"a")
        builder.add_document(b"b")
        builder.add_posting("tokyo", 1)
        for bad in (
            lambda: builder.add_posting("tokyo", 0),
            lambda: builder.add_posting("osaka", 0),
            lambda: builder.add_posting("zz", 7),
            lambda: builder.add_posting("zz", 0, 0),
            lambda: builder.add_posting_at("zz", 0, [2, 1]),
            lambda: builder.add_section(0x0040, b""),
            lambda: builder.add_document_values([1]),
        ):
            with self.assertRaises(BuildError):
                bad()
        with self.assertRaises(BuildError):
            Builder().finish(b"")
        for options in (
            WriteOptions(terms_per_block=0),
            WriteOptions(positions=True, has_freqs=False),
            WriteOptions(completions=CompletionOptions(docs_per_entry=1)),
        ):
            with self.assertRaises(BuildError):
                Builder(options)


class RoundTrip(unittest.TestCase):
    def test_random_corpora_read_back_as_they_were_written(self):
        # Every combination of options that changes the layout, read back:
        # every term finds exactly the documents, payloads and positions it
        # was given, and a completion answers as walking the dictionary would.
        rng = random.Random(1)
        words = ["a", "ab", "abc", "b", "東京", "東京都", "tokyo", "z", "zz", "丸の内"]
        for round_ in range(60):
            texts = [" ".join(rng.choice(words) for _ in range(rng.randint(1, 5))) for _ in range(rng.randint(1, 40))]
            options = WriteOptions(
                terms_per_block=rng.randint(1, 4),
                docs_per_payload_block=rng.randint(1, 5),
                positions=rng.random() < 0.5,
                doc_lengths=rng.random() < 0.5,
                compress=rng.random() < 0.5,
                inline_postings=rng.randint(0, 9),
                completions=(
                    CompletionOptions(min_terms=rng.randint(1, 3), docs_per_entry=rng.randint(1, 3))
                    if rng.random() < 0.5
                    else None
                ),
            )
            builder = Builder(options)
            for i in range(len(texts)):
                builder.add_document(f"doc {i}")
            inverted = invert(texts, "space-split")
            for term, docs in inverted:
                for doc, places in sorted(docs.items()):
                    builder.add_posting_at(term, doc, places)
            meta = Metadata(generator_params=options.params(), tokenizer_id="space-split").encode()
            index = MokujiIndex.open(BytesSource(builder.finish(meta)))
            with self.subTest(round=round_, options=options):
                for term, docs in inverted:
                    entry = index.exact(term.decode())
                    [postings] = index.postings_span([entry])
                    self.assertEqual(postings.docs, sorted(docs))
                    self.assertEqual(postings.freqs, [len(docs[d]) for d in sorted(docs)])
                    self.assertEqual(index.payload_span(postings.docs), [f"doc {d}" for d in sorted(docs)])
                    if options.positions:
                        self.assertEqual(index.positions(entry, postings), [docs[d] for d in sorted(docs)])
                for prefix in ("a", "東", "z", "q"):
                    want = sorted({d for term, docs in inverted if term.startswith(prefix.encode()) for d in docs})
                    self.assertEqual(index.complete(prefix, 2).docs, want[:2])


if __name__ == "__main__":
    unittest.main()
