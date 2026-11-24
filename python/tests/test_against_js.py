# The Python reader against the TypeScript one, query by query.
#
# The conformance cases fix results and read counts, but not the floats a
# ranking produces or every string a tokenizer might be given. Here both
# readers answer the same questions over the same fixtures and must agree
# exactly, scores included. Skipped when node or the built js/dist is absent.

import json
import random
import shutil
import subprocess
import unittest

from support import FIXTURES, REPOSITORY, fixture_source

from mokuji import SPLITTERS, MokujiIndex, bigram, ja_address, name_list

DIST = REPOSITORY / "js" / "dist" / "index.js"
NODE = shutil.which("node")

# Each question is [fixture, method, args]; `search` takes its options as the
# TypeScript reader does, in camelCase.
QUESTIONS = [
    ["tiny", "search", ["tokyo chuo", {}]],
    ["tiny", "search", ["tokyo chuo", {"any": True}]],
    ["tiny", "search", ["tokyo chuo shibuya osaka", {"any": True, "limit": 2}]],
    ["tiny", "search", ["tokyo chuo", {"any": True, "probe": 1}]],
    ["tiny", "search", ["chuo tokyo", {"any": True, "lengthNorm": 0}]],
    ["tiny", "search", ["tokyo nowhere", {"any": True}]],
    ["tiny", "lookup", ["c", 20]],
    ["tiny", "lookup", ["tokyo", 1]],
    ["tiny", "complete", ["o", 10]],
    ["dict", "lookup", ["東京都千代田区丸の内", 100]],
    ["dict", "lookup", ["大阪", 2]],
    ["dict", "search", ["東京都港区赤坂1丁目1番1号", {"boost": "importance"}]],
    ["dict", "complete", ["東京都", 2]],
    ["dict-deflate", "lookup", ["東京都", 100]],
    ["names", "complete", ["bl", 2]],
    ["names", "complete", ["bl", 3]],
    ["names", "complete", ["Black S", 10]],
    ["names", "complete", ["東", 2]],
    ["names", "complete", ["ＴＯ", 10]],
    ["names", "lookup", ["b", 100]],
    ["names", "search", ["Tokyo\u001fBlack Sea", {"any": True}]],
    ["names-delta", "complete", ["東京タ", 10]],
]

SCRIPT = r"""
import { readFileSync } from "node:fs";
const [dist, fixtures] = process.argv.slice(1);
const { MokujiIndex, SPLITTERS, bigram, jaAddress, nameList } = await import(dist);
const { questions, strings } = JSON.parse(readFileSync(0, "utf8"));
const answers = [];
for (const [fixture, method, args] of questions) {
  const bytes = new Uint8Array(readFileSync(`${fixtures}/${fixture}.mokuji`));
  const index = await MokujiIndex.open({ read: (o, n) => bytes.subarray(o, o + n) });
  index.resetStats();
  const r = await index[method](...args);
  const shape =
    method === "search"
      ? { terms: r.terms, probed: r.probed.map((p) => [p.term, p.docFreq]), skipped: r.skipped,
          matched: r.matched, results: r.results.map((x) => [x.doc, x.score, x.payload]) }
      : method === "lookup"
        ? { normalized: r.normalized, results: r.results.map((x) => [x.term, x.doc, x.payload]) }
        : { normalized: r.normalized, fromTable: r.fromTable, docs: r.docs, payloads: r.payloads };
  answers.push({ ...shape, stats: [index.stats.requests, index.stats.bytes] });
}
const tokens = strings.map((s) => [jaAddress(s), bigram(s), nameList(s), SPLITTERS["ja-address-bigram"](s)]);
process.stdout.write(JSON.stringify({ answers, tokens }));
"""


def python_answer(fixture, method, args):
    index = MokujiIndex.open(fixture_source(fixture))
    index.reset_stats()
    if method == "search":
        query, options = args
        r = index.search(
            query,
            limit=options.get("limit", 10),
            any=options.get("any", False),
            probe=options.get("probe"),
            length_norm=options.get("lengthNorm", 0.75),
            boost=options.get("boost"),
        )
        shape = {
            "terms": r.terms,
            "probed": [[p.term, p.doc_freq] for p in r.probed],
            "skipped": r.skipped,
            "matched": r.matched,
            "results": [[x.doc, x.score, x.payload] for x in r.results],
        }
    elif method == "lookup":
        r = index.lookup(*args)
        shape = {"normalized": r.normalized, "results": [[x.term, x.doc, x.payload] for x in r.results]}
    else:
        r = index.complete(*args)
        shape = {"normalized": r.normalized, "fromTable": r.from_table, "docs": r.docs, "payloads": r.payloads}
    shape["stats"] = [index.stats.requests, index.stats.bytes]
    return shape


def scores(answer):
    """Takes the scores out of a search answer, leaving the rest to compare."""
    if "probed" not in answer:
        return []
    taken = [result[1] for result in answer["results"]]
    answer["results"] = [[doc, payload] for doc, _, payload in answer["results"]]
    return taken


def strings():
    """Text in the scripts and forms the tokenizers treat differently."""
    pieces = [
        "東京都", "千代田区", "丸ノ内", "丸の内", "一丁目", "二十", "丁目", "１２", "ﾏﾙﾉｳﾁ", "ケーキ",
        "茅ケ崎", "二之瀬", "ヵ", "Black", "Sea", "İstanbul", "ΟΔΟΣ", "Straße", "ǅ", "हिंदी",
        "தமிழ்", "ไทย", "Ⓐⓑ", "½", "x²", "ＣｉｔｙＧＭＬ", "3D", "é", " ", " ", "　",
        "\u001f", "\t", "-", "・", "、", "😀", "ﬁ", "Ⅻ", "٣", " ",
    ]
    rng = random.Random(7)
    out = list(pieces)
    for _ in range(400):
        out.append("".join(rng.choice(pieces) for _ in range(rng.randint(1, 6))))
    return out


@unittest.skipUnless(NODE and DIST.exists(), "node and js/dist are needed to compare against")
class AgainstTypeScript(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.strings = strings()
        done = subprocess.run(
            [NODE, "--input-type=module", "-e", SCRIPT, DIST.as_uri(), str(FIXTURES)],
            input=json.dumps({"questions": QUESTIONS, "strings": cls.strings}),
            capture_output=True,
            text=True,
            check=True,
        )
        cls.js = json.loads(done.stdout)

    def test_answers_and_costs_agree(self):
        for question, theirs in zip(QUESTIONS, self.js["answers"]):
            with self.subTest(question):
                ours = python_answer(*question)
                # The order, the payloads and the cost compare exactly. Scores
                # compare to a part in 10^12: V8's `Math.log` is fdlibm's, which
                # is a unit in the last place off the correctly rounded value
                # for about one argument in seventy, and `math.log` (like
                # Rust's `ln`) is the platform's, which is not.
                ours_scores, theirs_scores = scores(ours), scores(theirs)
                self.assertEqual(ours, theirs)
                self.assertEqual(len(ours_scores), len(theirs_scores))
                for a, b in zip(ours_scores, theirs_scores):
                    if a is None or b is None:
                        self.assertEqual(a, b)
                    else:
                        self.assertAlmostEqual(a, b, delta=abs(b) * 1e-12)

    def test_tokenizers_agree(self):
        for text, (address, grams, name, address_grams) in zip(self.strings, self.js["tokens"]):
            with self.subTest(text=text):
                self.assertEqual(bigram(text), grams)
                self.assertEqual(SPLITTERS["ja-address-bigram"](text), address_grams)
                self.assertEqual(name_list(text), name)
                # JavaScript's `trim` and `White_Space` differ on U+0085 and
                # U+FEFF, which the strings above do not put at either end.
                self.assertEqual(ja_address(text), address)


if __name__ == "__main__":
    unittest.main()
