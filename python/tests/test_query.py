# lookup_prefix and locate, against the conformance fixtures and a file of
# geohash coverings built here: the checks js/test/query.test.js makes.

import unittest

from support import BytesSource, fixture_source

from mokuji import (
    Builder,
    Metadata,
    MokujiError,
    MokujiIndex,
    Provenance,
    WriteOptions,
    contains,
    decode_rings,
    geohash,
    locate,
    lookup_prefix,
)

MARUNOUCHI = "東京都千代田区丸の内1丁目1番1号"


def dictionary():
    return MokujiIndex.open(fixture_source("dict"))


def encode_rings(rings):
    """Rings as delta-int-1e5: the first vertex, then differences, unclosed."""
    parts = []
    for ring in rings:
        ints = [(round(x * 1e5), round(y * 1e5)) for x, y in ring]
        values = []
        for i, (x, y) in enumerate(ints):
            dx, dy = (x - ints[i - 1][0], y - ints[i - 1][1]) if i else (x, y)
            values += [str(dx), str(dy)]
        parts.append(",".join(values))
    return ";".join(parts)


def square(x0, y0, x1, y1):
    return [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]


def coverings():
    """Two blocks side by side, the first with a courtyard, each keyed by the
    cells its corners are in at precisions 5 and 6."""
    places = [
        ("A", "Marunouchi", [square(139.76, 35.68, 139.765, 35.685), square(139.762, 35.682, 139.763, 35.683)]),
        ("B", "Otemachi", [square(139.765, 35.68, 139.77, 35.685)]),
    ]
    builder = Builder(WriteOptions(terms_per_block=4, docs_per_payload_block=2))
    postings = {}
    for doc, (code, name, rings) in enumerate(places):
        builder.add_document(f"{code}\x1f{name}|{encode_rings(rings)}".encode())
        for lon, lat in rings[0]:
            for precision in (5, 6):
                postings.setdefault(geohash(lat, lon, precision), set()).add(doc)
    for cell in sorted(postings):
        for doc in sorted(postings[cell]):
            builder.add_posting(cell.encode(), doc)
    meta = Metadata(
        sources=[Provenance("sha2-256:" + "0" * 64, None, None, None)],
        attrs={"geohash-precisions": "5,6", "geometry": "delta-int-1e5", "payload-fields": "CODE,NAME"},
    ).encode()
    return MokujiIndex.open(BytesSource(builder.finish(meta)))


class LookupPrefixTest(unittest.TestCase):
    def test_the_terms_a_prefix_begins_and_whether_each_is_the_one_asked(self):
        hits = lookup_prefix(dictionary(), "東京都千代田区丸の内1丁目1番", 10)
        self.assertEqual(
            [(h.term, h.doc, h.match) for h in hits],
            [(MARUNOUCHI, 1, "prefix"), ("東京都千代田区丸の内1丁目1番2号", 2, "prefix")],
        )
        self.assertEqual(hits[0].payload, "139.767201,35.681560")
        self.assertEqual([h.match for h in lookup_prefix(dictionary(), MARUNOUCHI, 10)], ["exact"])

    def test_an_address_typed_past_the_key_finds_the_term_it_begins_with(self):
        typed = MARUNOUCHI + "5階"
        self.assertEqual(lookup_prefix(dictionary(), typed, 10), [])
        hits = lookup_prefix(dictionary(), typed, 10, [typed])
        self.assertEqual(
            [(h.term, h.doc, h.match, h.payload) for h in hits],
            [(MARUNOUCHI, 1, "within", "139.767201,35.681560")],
        )
        self.assertEqual(lookup_prefix(dictionary(), "京都府", 10, ["京都府京都市"]), [])

    def test_the_limit_counts_documents(self):
        self.assertEqual(len(lookup_prefix(dictionary(), "東京都", 2)), 2)

    def test_a_limit_of_zero_reads_nothing(self):
        index = dictionary()
        index.reset_stats()
        self.assertEqual(lookup_prefix(index, "東京都", 0, ["東京都千代田区"]), [])
        self.assertEqual(index.stats.requests, 0)


class CoveringsTest(unittest.TestCase):
    def test_geohash_agrees_with_the_published_example(self):
        self.assertEqual(geohash(57.64911, 10.40744, 11), "u4pruydqqvj")

    def test_decode_rings_closes_each_ring_and_contains_is_even_odd(self):
        rings = decode_rings("0,0,10,0,0,10,-10,0;3,3,4,0,0,4,-4,0", 1)
        self.assertEqual(rings[0], [(0, 0), (10, 0), (10, 10), (0, 10), (0, 0)])
        self.assertTrue(contains(rings, 1, 1))
        self.assertFalse(contains(rings, 5, 5))
        self.assertFalse(contains(rings, 11, 5))
        for hostile in ("0,x", "1_0,0", "١,0", "1.5,0", ",0"):
            with self.assertRaisesRegex(MokujiError, "rings are not integers", msg=hostile):
                decode_rings(hostile, 1)
        self.assertEqual(decode_rings(" +1 , -2 ", 1)[0][0], (1, -2))

    def test_locate_the_polygons_holding_a_point_named_by_the_files_fields(self):
        index = coverings()
        found = locate(index, 35.6801, 139.7601)
        self.assertEqual(found.cell, geohash(35.6801, 139.7601, 6))
        self.assertGreaterEqual(found.candidates, 1)
        self.assertEqual(
            [(p.doc, p.label, p.fields) for p in found.inside],
            [(0, "A\x1fMarunouchi", {"CODE": "A", "NAME": "Marunouchi"})],
        )
        self.assertEqual(len(found.inside[0].rings), 2)

        self.assertEqual([p.fields["CODE"] for p in locate(index, 35.6801, 139.7699).inside], ["B"])
        # In the courtyard: a candidate, but in neither.
        courtyard = locate(index, 35.6825, 139.7625)
        self.assertGreaterEqual(courtyard.candidates, 1)
        self.assertEqual(courtyard.inside, [])

    def test_locate_refuses_a_file_that_does_not_say_how_it_was_built(self):
        with self.assertRaisesRegex(MokujiError, "geohash precisions"):
            locate(dictionary(), 35.68, 139.76)
        # Unless the caller says.
        self.assertEqual(locate(dictionary(), 35.68, 139.76, precisions=[5], scale=1e5).inside, [])
        # Saying nothing is not saying.
        for scale in (0, -1, float("inf"), float("nan")):
            with self.assertRaisesRegex(MokujiError, "geohash precisions"):
                locate(dictionary(), 35.68, 139.76, precisions=[5], scale=scale)
        with self.assertRaisesRegex(MokujiError, "geohash precisions"):
            locate(dictionary(), 35.68, 139.76, precisions=[], scale=1e5)
        for precisions in ([0], [13], [5.0], [True]):
            with self.assertRaisesRegex(MokujiError, "geohash precisions"):
                locate(dictionary(), 35.68, 139.76, precisions=precisions, scale=1e5)

    def test_locate_falls_back_to_the_file_when_the_caller_says_nothing(self):
        index = coverings()
        self.assertEqual(
            locate(index, 35.6801, 139.7601, precisions=[], scale=float("nan")).inside,
            locate(index, 35.6801, 139.7601).inside,
        )


if __name__ == "__main__":
    unittest.main()
