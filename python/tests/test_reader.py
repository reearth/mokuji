# The reader beyond the conformance cases: the same checks the TypeScript
# tests make, and what is particular to reading from a path or a URL here.

import http.server
import os
import tempfile
import threading
import unittest
from unittest import mock

from support import FIXTURES, BytesSource, fixture_bytes, fixture_source

from mokuji import MokujiCollection, MokujiError, MokujiIndex


class CountingSource(BytesSource):
    """Counts reads itself, so a count is not only the reader's word for it."""

    def __init__(self, data):
        super().__init__(data)
        self.reads = 0

    def read(self, offset, length):
        self.reads += 1
        return super().read(offset, length)


class Metadata(unittest.TestCase):
    def test_carries_the_license_and_the_credit_a_file_states(self):
        names = MokujiIndex.open(fixture_source("names"))
        self.assertEqual(names.license, "CC0-1.0")
        self.assertEqual(names.attribution, "Mokuji conformance fixtures")
        tiny = MokujiIndex.open(fixture_source("tiny"))
        self.assertIsNone(tiny.license)
        self.assertIsNone(tiny.attribution)

    def test_names_what_a_file_was_derived_from(self):
        tiny = MokujiIndex.open(fixture_source("tiny"))
        [source] = tiny.sources
        self.assertRegex(source.content_hash, r"^sha2-256:[0-9a-f]{64}$")
        self.assertEqual(source.uri, "conformance/fixtures/tiny.corpus.txt")
        # Neither date is required, and a file that states none says nothing.
        self.assertIsNone(source.retrieved)
        self.assertIsNone(source.modified)

    def test_names_what_wrote_a_file_and_how(self):
        generator = MokujiIndex.open(fixture_source("tiny")).generator
        self.assertEqual((generator.name, generator.version), ("build_fixtures.py", "0.1"))
        self.assertEqual(generator.params["terms_per_block"], 2)
        self.assertEqual(generator.params["payload"], "opaque")

    def test_holds_the_tokenizer_the_index_declares(self):
        self.assertEqual(MokujiIndex.open(fixture_source("dict")).tokenizer_id, "identity")
        self.assertEqual(MokujiIndex.open(fixture_source("names")).tokenizer_id, "name-list")

    def test_the_header_carries_the_statistics_scoring_needs(self):
        tiny = MokujiIndex.open(fixture_source("tiny"))
        self.assertEqual(tiny.num_docs, 4)
        self.assertEqual(tiny.sum_doclen, 8)
        self.assertEqual(tiny.doc_lengths, [2, 2, 2, 2])

    def test_unrecognized_sections_are_ignored(self):
        extra = MokujiIndex.open(fixture_source("tiny-extra"))
        self.assertTrue(any(i >= 0x8000 for i in extra.sections))


class Reads(unittest.TestCase):
    def test_opening_costs_one_read(self):
        source = CountingSource(fixture_bytes("tiny"))
        index = MokujiIndex.open(source)
        self.assertEqual(source.reads, 1)
        self.assertEqual(index.stats.requests, 1)

    def test_reads_a_deflated_file(self):
        # The fixtures the other implementations reproduce byte for byte are
        # stored raw, so without this decompression is never exercised.
        index = MokujiIndex.open(fixture_source("dict-deflate"))
        results = index.lookup("東京都千代田区丸の内1丁目1番1号", 10).results
        self.assertEqual(len(results), 1)
        self.assertEqual(results[0].payload, "139.767201,35.681560")

    def test_a_prefix_range_costs_the_reads_the_specification_says(self):
        index = MokujiIndex.open(fixture_source("dict"))
        index.coalesce = False
        index.reset_stats()
        results = index.lookup("東京都千代田区丸の内", 100).results
        self.assertEqual(len(results), 3)
        # Two term blocks of two terms hold the three matches, and their
        # documents span two payload blocks. No postings: they are implied.
        self.assertEqual(index.stats.requests, 4)

        # The two payload blocks are neighbours, so a reader that coalesces
        # reads them as one.
        index.coalesce = True
        index.reset_stats()
        index.lookup("東京都千代田区丸の内", 100)
        self.assertEqual(index.stats.requests, 3)

    def test_reports_every_request(self):
        index = MokujiIndex.open(fixture_source("dict"))
        seen = []
        index.on_request = seen.append
        index.reset_stats()
        index.lookup("東京都千代田区丸の内", 100)
        self.assertEqual(len(seen), index.stats.requests)
        self.assertEqual(sum(r.length for r in seen), index.stats.bytes)


class Terms(unittest.TestCase):
    def test_range_returns_the_terms_between_two_keys_both_inclusive(self):
        index = MokujiIndex.open(fixture_source("dict"))

        def terms(low, high, limit=None):
            return [e.term.decode() for e in index.range(low, high, limit)]

        self.assertEqual(
            terms("東京都千代田区丸の内1丁目1番2号", "東京都港区赤坂1丁目1番1号"),
            ["東京都千代田区丸の内1丁目1番2号", "東京都千代田区丸の内2丁目1番1号", "東京都港区赤坂1丁目1番1号"],
        )
        # Neither end has to be a term.
        self.assertEqual(
            terms("東京都千代田区丸の内1丁目1番1号a", "東京都千代田区丸の内3"),
            ["東京都千代田区丸の内1丁目1番2号", "東京都千代田区丸の内2丁目1番1号"],
        )
        self.assertEqual(terms("東京都", "東京都千代田区丸の内9", 1), ["東京都千代田区丸の内1丁目1番1号"])
        index.reset_stats()
        self.assertEqual(terms("北海道", "千葉県"), [])
        # Both ends sort before the first block, so the resident index says
        # the range is empty without reading anything.
        self.assertEqual(index.stats.requests, 0)

    def test_exact_finds_a_term_without_completing_it(self):
        index = MokujiIndex.open(fixture_source("tiny"))
        # `tiny` is a document collection, so a term is a word.
        entry = index.exact("tokyo")
        self.assertIsNotNone(entry)
        self.assertEqual(entry.term, b"tokyo")
        self.assertIsNone(index.exact("toky"), "a proper prefix is not a term")
        self.assertIsNone(index.exact("tokyoo"), "an extension is not a term")
        self.assertIsNone(index.exact(""), "the empty string is not a term")

    def test_exact_costs_one_dictionary_read(self):
        source = CountingSource(fixture_bytes("tiny"))
        index = MokujiIndex.open(source)
        # The block a term would be in is answerable from the resident index,
        # so a term that is absent costs exactly what a term that is present
        # does.
        before = source.reads
        index.exact("tokyo")
        self.assertEqual(source.reads - before, 1)
        index.exact("nowhere")
        self.assertEqual(source.reads - before, 2)

    def test_a_term_before_every_block_costs_no_read_at_all(self):
        # `aomori` sorts before `chiyoda`, the first boundary key, so the
        # resident index settles it without touching the file (SPEC.md §5.3.2).
        index = MokujiIndex.open(fixture_source("tiny"))
        index.reset_stats()
        self.assertIsNone(index.exact("aomori"))
        self.assertEqual(index.prefix("a"), [])
        self.assertEqual(index.stats.requests, 0)
        # A prefix of the first term is not before it in that sense.
        self.assertEqual([e.term for e in index.prefix("c")], [b"chiyoda", b"chuo"])

    def test_exact_many_reads_a_shared_block_once(self):
        index = MokujiIndex.open(fixture_source("tiny"))
        index.coalesce = False
        index.reset_stats()
        found = index.exact_many(["chiyoda", "chuo", "kyoto"])
        self.assertEqual([e and e.term for e in found], [b"chiyoda", b"chuo", None])
        # `chiyoda` and `chuo` share block 0; `kyoto` would be in block 0 too.
        self.assertEqual(index.stats.requests, 1)

    def test_prefix_many_reads_each_block_once(self):
        index = MokujiIndex.open(fixture_source("dict"))
        index.coalesce = False
        index.reset_stats()
        a, b = index.prefix_many(["東京都千代田区丸の内1", "東京都千代田区丸の内"])
        self.assertEqual(len(a), 2)
        self.assertEqual(len(b), 3)
        self.assertEqual(index.stats.requests, 2)

    def test_doc_freq_and_the_postings_run_must_agree(self):
        data = bytearray(fixture_bytes("tiny"))
        index = MokujiIndex.open(BytesSource(bytes(data)))
        blocks = index.sections[0x0011]
        # The first block is `entry_count`, `first_term_id`, then the first
        # entry: a front-coded key of `chiyoda` in nine bytes, then `doc_freq`.
        at = blocks.offset + 2 + 2 + 7
        self.assertEqual(data[at], 1)
        # Claiming two documents for a postings stated as one must be caught
        # rather than trusted.
        data[at] = 2
        broken = MokujiIndex.open(BytesSource(bytes(data)))
        with self.assertRaises(MokujiError):
            broken.lookup("chiyoda")

    def test_rejects_what_is_not_a_mokuji_file(self):
        with self.assertRaises(MokujiError):
            MokujiIndex.open(BytesSource(b"PK\x03\x04" + bytes(100)))
        data = bytearray(fixture_bytes("tiny"))
        data[8] = 1
        with self.assertRaises(MokujiError):
            MokujiIndex.open(BytesSource(bytes(data)))

    def test_rejects_a_truncated_file(self):
        data = fixture_bytes("tiny")
        with self.assertRaises(MokujiError):
            MokujiIndex.open(BytesSource(data[:400]))


class Search(unittest.TestCase):
    def test_ranks_as_mokuji_search_does(self):
        tiny = MokujiIndex.open(fixture_source("tiny"))

        # Scores to three places, as `mokuji search` prints them for the same
        # file.
        def rounded(r):
            return [(x.doc, f"{x.score:.3f}", x.payload) for x in r.results]

        self.assertEqual(rounded(tiny.search("tokyo chuo")), [(1, "1.050", "35.671,139.772")])
        self.assertEqual(
            rounded(tiny.search("tokyo chuo", any=True)),
            [
                (1, "1.050", "35.671,139.772"),
                (3, "0.693", "34.681,135.513"),
                (0, "0.357", "35.694,139.754"),
                (2, "0.357", "35.664,139.698"),
            ],
        )
        probed = tiny.search("tokyo chuo", any=True, probe=1)
        self.assertEqual([p.term for p in probed.probed], ["chuo"])
        self.assertEqual(probed.skipped, 1)

        self.assertEqual(tiny.search("tokyo nowhere").matched, 0)
        with self.assertRaises(ValueError):
            tiny.search("tokyo", boost="absent")

    def test_a_filter_leaves_out_documents_before_ranking(self):
        tiny = MokujiIndex.open(fixture_source("tiny"))
        even = lambda doc: doc % 2 == 0
        found = tiny.search("tokyo chuo", any=True, filter=even, limit=1)
        # Ranked as without the filter, the odd ones gone.
        self.assertEqual((found.matched, [(r.doc, f"{r.score:.3f}") for r in found.results]), (2, [(0, "0.357")]))
        self.assertEqual(tiny.search("tokyo chuo", filter=even).matched, 0)

    def test_a_probe_of_nothing_matches_nothing(self):
        # Every term left out leaves nothing to intersect, rather than an
        # error.
        tiny = MokujiIndex.open(fixture_source("tiny"))
        found = tiny.search("tokyo chuo", probe=0)
        self.assertEqual((found.matched, found.skipped, found.results), (0, 2, []))

    def test_boost_without_document_lengths_scores_nothing(self):
        index = MokujiIndex.open(fixture_source("dict"))
        plain = index.search("東京都千代田区丸の内1丁目1番1号")
        boosted = index.search("東京都千代田区丸の内1丁目1番1号", boost="importance")
        self.assertEqual([r.doc for r in boosted.results], [1])
        # Without document lengths nothing is scored, prior or not.
        self.assertEqual(plain.results[0].score, boosted.results[0].score)


class Sources(unittest.TestCase):
    def test_a_path_reads_only_what_a_query_touches(self):
        # A file of four gigabytes, sparse so that the test costs nothing: the
        # dictionary fixture, then zeros. Every byte the reader takes from it
        # goes through pread, which is counted here independently of the
        # reader's own count.
        if not hasattr(os, "pread"):
            self.skipTest("no os.pread on this platform")
        real_pread = os.pread
        taken = []

        def counting_pread(fd, length, offset):
            data = real_pread(fd, length, offset)
            taken.append(len(data))
            return data

        with tempfile.TemporaryDirectory() as directory:
            path = os.path.join(directory, "large.mokuji")
            with open(path, "wb") as f:
                f.write(fixture_bytes("dict"))
                f.truncate(4 << 30)
            with mock.patch("os.pread", counting_pread):
                with MokujiIndex.open(path) as index:
                    self.assertEqual(index.file_length, 4 << 30)
                    opened = sum(taken)
                    found = index.lookup("東京都千代田区丸の内1丁目1番1号", 10)
                    self.assertEqual(found.results[0].payload, "139.767201,35.681560")
            self.assertEqual(sum(taken), index.stats.bytes)
            # The opening guess, and a few hundred bytes for the query.
            self.assertLessEqual(opened, 256 * 1024)
            self.assertLess(sum(taken) - opened, 1024)

    def test_a_path_answers_as_any_other_source_does(self):
        expected = MokujiIndex.open(fixture_source("dict"))
        expected.reset_stats()
        want = expected.lookup("東京都千代田区丸の内", 100)
        with MokujiIndex.open(FIXTURES / "dict.mokuji") as index:
            self.assertEqual(index.file_length, len(fixture_bytes("dict")))
            index.reset_stats()
            self.assertEqual(index.lookup("東京都千代田区丸の内", 100), want)
            self.assertEqual(index.stats, expected.stats)

    def test_a_url_is_read_with_range_requests(self):
        data = fixture_bytes("names")
        with _serve(data, honour_range=True) as url:
            index = MokujiIndex.open(url)
            self.assertEqual(index.file_length, len(data))
            index.reset_stats()
            done = index.complete("bl", 2)
            self.assertEqual(done.payloads, ["Black Sea", "Black Forest"])
            self.assertEqual(index.stats.requests, 1)

    def test_a_server_that_ignores_range_is_an_error(self):
        with _serve(fixture_bytes("names"), honour_range=False) as url:
            with self.assertRaisesRegex(MokujiError, "Range"):
                MokujiIndex.open(url)

    def test_a_collection_opened_by_path_checks_lengths(self):
        with MokujiCollection.open(FIXTURES / "collection.json") as collection:
            self.assertEqual(len(collection.segments), 2)
            self.assertEqual(collection.attributions, ["Mokuji conformance fixtures"])
            done = collection.complete("black", 10)
            self.assertEqual(done.payloads, ["Black Forest", "Blackpool", "Black Mountain"])
            # Ranking across segments asks every one of them.
            both = collection.candidates("black", 10)
            self.assertEqual({h.segment for h in both.hits}, {0, 1})


class _serve:
    """A local HTTP server for one file, honouring Range or not."""

    def __init__(self, data, honour_range):
        self.data = data
        self.honour_range = honour_range

    def __enter__(self):
        data, honour = self.data, self.honour_range

        class Handler(http.server.BaseHTTPRequestHandler):
            def do_GET(self):
                spec = self.headers.get("Range", "")
                if honour and spec.startswith("bytes="):
                    first, _, last = spec[len("bytes=") :].partition("-")
                    start, end = int(first), min(int(last), len(data) - 1)
                    body = data[start : end + 1]
                    self.send_response(206)
                    self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
                else:
                    body = data
                    self.send_response(200)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *args):
                pass

        self.server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        return f"http://127.0.0.1:{self.server.server_address[1]}/names.mokuji"

    def __exit__(self, *exc):
        self.server.shutdown()
        self.server.server_close()


if __name__ == "__main__":
    unittest.main()
