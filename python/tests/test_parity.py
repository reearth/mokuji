# What the four implementations share beyond the conformance cases: phrase
# search, ranking without payload, a collection's search, manifests, the
# metadata a reader acts on, and holding the dictionary. The same questions the
# Rust, TypeScript and Go tests ask, with the same answers.

import hashlib
import json
import unittest

from support import FIXTURES, BytesSource, fixture_bytes, fixture_source

from mokuji import (
    MokujiCollection,
    MokujiError,
    MokujiIndex,
    NoColumnError,
    decode_cbor,
    manifest_to_json,
    parse_manifest,
    within,
)

# `tiny`: 0 "tokyo chiyoda", 1 "tokyo chuo", 2 "tokyo shibuya", 3 "osaka chuo".


def tiny():
    return MokujiIndex.open(fixture_source("tiny"))


def docs(result):
    return [r.doc for r in result.results]


class PhraseTest(unittest.TestCase):
    def test_the_terms_in_the_order_typed_at_consecutive_positions(self):
        self.assertEqual(docs(tiny().search("tokyo chuo", phrase=True)), [1])
        self.assertEqual(docs(tiny().search("chuo tokyo", phrase=True)), [])
        self.assertEqual(docs(tiny().search("osaka chuo", phrase=True)), [3])

    def test_a_probe_tests_the_terms_it_read_at_their_places_in_the_query(self):
        # `chuo` is the rarer, so it alone is read, and a phrase of one term
        # holds wherever the term does.
        index = tiny()
        found = index.search("tokyo chuo", phrase=True, probe=1)
        self.assertEqual([p.term for p in found.probed], ["chuo"])
        self.assertEqual(
            [(r.doc, r.score) for r in found.results],
            [(r.doc, r.score) for r in tiny().search("tokyo chuo", probe=1).results],
        )
        self.assertEqual(docs(found), [1, 3])

    def test_with_any_a_document_lacking_a_term_does_not_hold_the_phrase(self):
        self.assertEqual(docs(tiny().search("tokyo chuo", phrase=True, any=True)), [1])

    def test_a_filter_that_leaves_nothing_reads_no_positions(self):
        plain = tiny()
        plain.search("tokyo chuo", filter=lambda doc: doc != 1)
        phrased = tiny()
        found = phrased.search("tokyo chuo", phrase=True, filter=lambda doc: doc != 1)
        self.assertEqual(found.matched, 0)
        self.assertEqual(phrased.stats, plain.stats)

    def test_the_positions_cost_one_read_per_term_and_only_for_a_phrase(self):
        plain = tiny()
        plain.search("tokyo chuo")
        phrased = tiny()
        phrased.search("tokyo chuo", phrase=True)
        self.assertEqual(phrased.stats.requests, plain.stats.requests + 2)

    def test_a_file_without_positions_cannot_test_a_phrase(self):
        dictionary = MokujiIndex.open(fixture_source("dict"))
        with self.assertRaisesRegex(MokujiError, "carries no positions, so it cannot test a phrase"):
            dictionary.search("東京都", phrase=True)


class RankTest(unittest.TestCase):
    def test_the_ranking_of_search_without_its_payload(self):
        index = tiny()
        index.reset_stats()
        ranking = index.rank(["tokyo", "chuo"])
        ranked_reads = index.stats.requests
        found = tiny().search("tokyo chuo")
        self.assertEqual([(r.doc, r.score) for r in ranking.ranked], [(r.doc, r.score) for r in found.results])
        self.assertEqual([(p.term, p.doc_freq) for p in ranking.probed], [("tokyo", 3), ("chuo", 2)])
        self.assertEqual(ranking.skipped, 0)
        # Everything search reads but the payload.
        searched = tiny()
        searched.reset_stats()
        searched.search("tokyo chuo")
        self.assertEqual(ranked_reads, searched.stats.requests - 1)

    def test_an_unknown_column_is_its_own_error_and_still_a_value_error(self):
        with self.assertRaises(NoColumnError) as caught:
            tiny().search("tokyo", boost="importance")
        self.assertEqual(caught.exception.column, "importance")
        self.assertIsInstance(caught.exception, ValueError)


class MetadataTest(unittest.TestCase):
    def test_generation_attrs_and_tokenizer_params(self):
        index = tiny()
        self.assertEqual(index.generation, 1)
        self.assertIsNone(index.attrs)
        self.assertEqual(index.tokenizer_params, {})

    def test_decode_cbor_reads_what_the_metadata_section_holds(self):
        index = tiny()
        section = index.sections[0x0001]
        raw = fixture_bytes("tiny")[section.offset : section.offset + section.length]
        self.assertEqual(decode_cbor(raw), index.meta)
        self.assertEqual(decode_cbor(bytes([0x82, 0x01, 0x61, 0x61])), [1, "a"])
        with self.assertRaises(MokujiError):
            decode_cbor(bytes([0x01, 0x01]))

    def test_decode_cbor_refuses_what_runs_past_its_bytes(self):
        for hostile in (
            b"",
            b"\xf8",  # a simple value with its byte missing
            b"\x19\x01",  # an integer cut short
            b"\x62a",  # text longer than what is left
            b"\x9b\xff\xff\xff\xff\xff\xff\xff\xff",  # an array of 2^64 - 1 items
            b"\xba\xff\xff\xff\xff",  # a map of 2^32 - 1 entries
            b"\x82\x01",  # an array missing an item
        ):
            with self.assertRaises(MokujiError, msg=hostile):
                decode_cbor(hostile)


class PhraseRepeatTest(unittest.TestCase):
    def test_a_term_typed_twice_reads_its_positions_once(self):
        # `tokyo tokyo` reads `tokyo`'s postings twice, as any repeated term,
        # and its positions once; no document has it twice in a row.
        # Compared without payload, which only the plain search reads.
        plain = tiny()
        plain.reset_stats()
        plain.rank(["tokyo", "tokyo"])
        phrased = tiny()
        phrased.reset_stats()
        self.assertEqual(phrased.rank(["tokyo", "tokyo"], phrase=True).ranked, [])
        self.assertEqual(phrased.stats.requests, plain.stats.requests + 1)

    def test_positions_that_do_not_number_the_postings_are_refused(self):
        index = tiny()
        index.positions = lambda entry, postings: [[0]]
        with self.assertRaisesRegex(MokujiError, "positions disagree with the postings"):
            index.search("tokyo chuo", phrase=True)


class ResidentDictionaryTest(unittest.TestCase):
    def test_one_more_read_at_open_and_none_for_a_lookup(self):
        fetched = MokujiIndex.open(fixture_source("dict"))
        held = MokujiIndex.open(fixture_source("dict"), resident_dictionary=True)
        self.assertEqual(held.stats.requests, fetched.stats.requests + 1)
        held.reset_stats()
        term = "東京都千代田区丸の内1丁目1番1号"
        self.assertEqual(held.exact(term), fetched.exact(term))
        self.assertEqual(held.prefix("東京都", None), fetched.prefix("東京都", None))
        self.assertEqual(held.stats.requests, 0)


class BytesSourceTest(unittest.TestCase):
    def test_says_its_length(self):
        index = MokujiIndex.open(BytesSource(fixture_bytes("tiny")))
        self.assertEqual(index.file_length, len(fixture_bytes("tiny")))


def two_of_names():
    """`names` twice: the first with `Black Sea` deleted, the second whole."""
    names = fixture_bytes("names")
    digest = "sha2-256:" + hashlib.sha256(names).hexdigest()
    manifest = {
        "mokuji_manifest": "0.1",
        "generation": 1,
        "segments": [
            {"path": "a.mokuji", "content_hash": digest, "length": len(names), "deletes": [0]},
            {"path": "b.mokuji", "content_hash": digest, "length": len(names)},
        ],
    }
    return MokujiCollection.open(manifest, lambda path: BytesSource(names))


def collection():
    manifest = json.loads((FIXTURES / "collection.json").read_text())
    sources = {s["path"]: fixture_source(s["path"].removesuffix(".mokuji")) for s in manifest["segments"]}
    return MokujiCollection.open(manifest, sources.get)


class CollectionSearchTest(unittest.TestCase):
    def test_every_live_document_in_segment_order_then_document_order(self):
        found = two_of_names().search("Tokyo")
        self.assertEqual(found.terms, ["tokyo"])
        self.assertEqual([(h.segment, h.doc) for h in found.hits], [(0, 4), (1, 4)])
        self.assertEqual(found.payloads, ["Tokyo", "Tokyo"])
        self.assertEqual(found.matched, 2)

    def test_a_deleted_document_is_not_found(self):
        found = two_of_names().search("Black Sea")
        self.assertEqual([(h.segment, h.doc) for h in found.hits], [(1, 0)])

    def test_the_limit_counts_hits_and_matched_counts_them_all(self):
        found = two_of_names().search("Tokyo", limit=1)
        self.assertEqual([(h.segment, h.doc) for h in found.hits], [(0, 4)])
        self.assertEqual(found.payloads, ["Tokyo"])
        self.assertEqual(found.matched, 2)

    def test_no_segments_leaves_the_query_one_term(self):
        empty = MokujiCollection(sample(), [])
        self.assertEqual(empty.search("Tokyo Tower").terms, ["Tokyo Tower"])

    def test_a_segment_outside_the_collection_is_refused(self):
        from mokuji import Hit

        names = two_of_names()
        with self.assertRaisesRegex(MokujiError, "the collection has no segment 2"):
            names.live_in(2, "tokyo", 1)
        with self.assertRaisesRegex(MokujiError, "the collection has no segment 5"):
            names.payloads([Hit(5, 0)])

    def test_a_term_only_a_later_segment_has(self):
        found = collection().search("Tokyo Tower")
        self.assertEqual([(h.segment, h.doc) for h in found.hits], [(1, 1)])
        self.assertEqual(found.payloads, ["Tokyo Tower"])
        self.assertEqual(collection().search("Black Sea").hits, [])


def sample():
    return {
        "mokuji_manifest": "0.1",
        "generation": 3,
        "segments": [
            {"path": "base.mokuji", "content_hash": "sha2-256:" + "ab" * 32, "length": 1000, "deletes": [2, 7]},
            {
                "path": "delta.mokuji",
                "content_hash": "sha2-256:" + "cd" * 32,
                "length": 100,
                "region": {"countries": ["JP", "KR"]},
            },
        ],
    }


class ManifestTest(unittest.TestCase):
    def test_writes_the_bytes_the_rust_crate_writes(self):
        text = (FIXTURES / "collection.json").read_text()
        self.assertEqual(manifest_to_json(parse_manifest(text)), text)
        self.assertEqual(
            manifest_to_json(sample()),
            "{\n"
            '  "mokuji_manifest": "0.1",\n'
            '  "generation": 3,\n'
            '  "segments": [\n'
            "    {\n"
            '      "path": "base.mokuji",\n'
            f'      "content_hash": "sha2-256:{"ab" * 32}",\n'
            '      "length": 1000,\n'
            '      "deletes": [\n'
            "        2,\n"
            "        7\n"
            "      ]\n"
            "    },\n"
            "    {\n"
            '      "path": "delta.mokuji",\n'
            f'      "content_hash": "sha2-256:{"cd" * 32}",\n'
            '      "length": 100,\n'
            '      "region": {\n'
            '        "countries": [\n'
            '          "JP",\n'
            '          "KR"\n'
            "        ]\n"
            "      }\n"
            "    }\n"
            "  ]\n"
            "}\n",
        )

    def test_round_trips_and_ignores_keys_it_does_not_know(self):
        text = manifest_to_json(sample())
        self.assertEqual(manifest_to_json(parse_manifest(text)), text)
        extra = text.replace("{", '{\n  "published": "2026-09-30",', 1)
        self.assertEqual(parse_manifest(extra), parse_manifest(text))

    def test_restricting_keeps_what_has_no_region_and_what_includes_the_place(self):
        manifest = parse_manifest(manifest_to_json(sample()))
        self.assertEqual(len(within(manifest, ["JP"])["segments"]), 2)
        elsewhere = within(manifest, ["FR"])
        self.assertEqual([s["path"] for s in elsewhere["segments"]], ["base.mokuji"])
        self.assertEqual(elsewhere["segments"][0]["deletes"], [2, 7])

    def test_rejects_what_it_cannot_trust(self):
        def bad(change):
            manifest = sample()
            change(manifest)
            with self.assertRaises(MokujiError):
                parse_manifest(json.dumps(manifest))

        bad(lambda m: m["segments"][0].update(deletes=[7, 2]))
        bad(lambda m: m["segments"][0].update(deletes=[2, 2]))
        bad(lambda m: m["segments"][1].update(content_hash="abcd"))
        bad(lambda m: m.update(mokuji_manifest="1.0"))
        bad(lambda m: m.update(segments=[]))
        bad(lambda m: m["segments"][0].update(path=""))
        bad(lambda m: m["segments"][0].update(length=-1))
        for countries in ([], ["jp"], ["JPN"], ["JP", "JP"]):
            bad(lambda m, c=countries: m["segments"][1].update(region={"countries": c}))
        with self.assertRaises(MokujiError):
            parse_manifest("not json")
        bad(lambda m: m["segments"][0].update(length=2**64))
        twice = manifest_to_json(sample()).replace('"generation": 3,', '"generation": 3,\n  "generation": 4,')
        with self.assertRaisesRegex(MokujiError, "twice"):
            parse_manifest(twice)
        self.assertEqual(parse_manifest(json.dumps({**sample(), "generation": 2**64 - 1}))["generation"], 2**64 - 1)

    def test_opening_a_collection_checks_the_manifest(self):
        manifest = sample()
        manifest["mokuji_manifest"] = "1.0"
        with self.assertRaisesRegex(MokujiError, "version 1.0"):
            MokujiCollection.open(manifest, lambda path: None)


if __name__ == "__main__":
    unittest.main()
