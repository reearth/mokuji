# The Python reader, against the conformance suite.
#
# Every case is checked for its result and for the number of range reads it
# took, because the bound in SPEC.md §4 is the property the format exists to
# provide. The cases count block by block, so coalescing is turned off for them
# and checked separately at the end. Run with
# `python3 -m unittest discover -s python/tests` from the repository root.

import json
import unittest

from support import FIXTURES, cases, fixture_source

from mokuji import MokujiCollection, MokujiIndex

SUITE = cases()


def open_uncoalesced(name):
    index = MokujiIndex.open(fixture_source(name))
    index.coalesce = False
    # Opening is accounted for separately: SPEC.md §4 charges the resident set
    # to a cold start, not to a query.
    index.reset_stats()
    return index


def term_query(index, terms):
    """Documents holding every term exactly, with their payload.

    How the Rust reader answers a case, built from the same primitives a
    caller has here: one dictionary block per term, one postings read per term
    whose run is stored, and the payload blocks of what matched. The
    TypeScript tests run only the single-term cases, as a prefix; this runs
    them all, so a multi-term case's count is checked too.
    """
    entries = [index.exact(term) for term in terms]
    if any(entry is None for entry in entries):
        return [], []
    runs = [index.postings_span([entry])[0] for entry in entries]
    rest = [set(run.docs) for run in runs[1:]]
    docs = [doc for doc in runs[0].docs if all(doc in s for s in rest)]
    return docs, index.payload_span(docs)


class Cases(unittest.TestCase):
    def test_the_suite_is_not_empty(self):
        self.assertTrue(SUITE["case"])
        self.assertTrue(SUITE["completion"])
        self.assertTrue(SUITE["collection"])

    def test_every_case_within_its_reads(self):
        for case in SUITE["case"]:
            with self.subTest(case["name"], fixture=case["fixture"]):
                index = open_uncoalesced(case["fixture"])
                docs, payloads = term_query(index, case["query"])
                self.assertEqual(docs, case["docs"])
                self.assertEqual(payloads, case["payloads"])

                # Resident, so asking for it must not cost a read — which is
                # checked by reading it before the count is taken.
                if "attribute" in case:
                    column = index.column(case["attribute"])
                    self.assertIsNotNone(column, f"no column '{case['attribute']}'")
                    self.assertEqual([column.values[d] for d in docs], case["values"])

                reads = index.stats.requests
                self.assertEqual(reads, case["reads"], "read count")
                # The bound is what the specification promises, and the count
                # could otherwise be updated to match a regression.
                self.assertLessEqual(reads, 2 * len(case["query"]) + len(case["docs"]))

                # Positions are the one section a reader pays for only when it
                # asks, and asking costs exactly one more read.
                if "positions" in case:
                    entry = index.exact(case["query"][0])
                    self.assertIsNotNone(entry, "term is absent")
                    [postings] = index.postings_span([entry])
                    before = index.stats.requests
                    self.assertEqual(index.positions(entry, postings), case["positions"])
                    self.assertEqual(index.stats.requests - before, 1, "positions must cost one read")

    def test_every_single_term_case_as_a_prefix(self):
        # What the TypeScript tests run: the term as a prefix, filtered to the
        # term itself, through the index's tokenizer.
        for case in SUITE["case"]:
            if len(case["query"]) != 1:
                continue
            with self.subTest(case["name"], fixture=case["fixture"]):
                index = open_uncoalesced(case["fixture"])
                [term] = case["query"]
                found = index.lookup(term, 100)
                exact = [r for r in found.results if r.term == index.normalize(term)]
                self.assertEqual([r.doc for r in exact], case["docs"])
                self.assertEqual([r.payload for r in exact], case["payloads"])

    def test_every_completion_from_where_the_case_says(self):
        for case in SUITE["completion"]:
            with self.subTest(case["name"], fixture=case["fixture"]):
                index = open_uncoalesced(case["fixture"])
                done = index.complete(case["prefix"], case["limit"])
                self.assertEqual(done.docs, case["docs"])
                self.assertEqual(done.payloads, case["payloads"])
                self.assertEqual(done.from_table, case["from_table"])
                if done.from_table:
                    self.assertEqual(done.requests_before_payload, 0, "an entry costs no read")
                self.assertEqual(index.stats.requests, case["reads"], "read count")

    def test_every_collection_case(self):
        for case in SUITE["collection"]:
            with self.subTest(case["name"]):
                manifest = json.loads((FIXTURES / f"{case['manifest']}.json").read_text())
                sources = {s["path"]: fixture_source(s["path"].removesuffix(".mokuji")) for s in manifest["segments"]}
                collection = MokujiCollection.open(manifest, sources.get, countries=case.get("countries"))
                for segment in collection.segments:
                    segment.coalesce = False
                collection.reset_stats()
                done = collection.complete(case["prefix"], case["limit"])
                self.assertEqual([h.segment for h in done.hits], case["segments"])
                self.assertEqual([h.doc for h in done.hits], case["docs"])
                self.assertEqual(done.payloads, case["payloads"])
                self.assertEqual(collection.stats.requests, case["reads"], "read count")

    def test_coalescing_never_costs_more_than_the_cases_say(self):
        fewer = 0
        for case in SUITE["case"]:
            with self.subTest(case["name"]):
                index = MokujiIndex.open(fixture_source(case["fixture"]))
                index.reset_stats()
                docs, payloads = term_query(index, case["query"])
                self.assertEqual(docs, case["docs"])
                self.assertEqual(payloads, case["payloads"])
                self.assertLessEqual(index.stats.requests, case["reads"])
                fewer += index.stats.requests < case["reads"]
        for case in SUITE["completion"]:
            with self.subTest(case["name"]):
                index = MokujiIndex.open(fixture_source(case["fixture"]))
                index.reset_stats()
                done = index.complete(case["prefix"], case["limit"])
                self.assertEqual(done.docs, case["docs"])
                self.assertEqual(done.payloads, case["payloads"])
                self.assertLessEqual(index.stats.requests, case["reads"])
                fewer += index.stats.requests < case["reads"]
        # Payload blocks next to one another are common in the fixtures, so
        # some case has to come out cheaper, or coalescing is not happening.
        self.assertGreater(fewer, 0)


if __name__ == "__main__":
    unittest.main()
