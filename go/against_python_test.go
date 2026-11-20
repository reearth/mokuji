package mokuji

// The Go reader against the Python one, query by query.
//
// The conformance cases fix results and read counts, but not the floats a
// ranking produces or every string a tokenizer might be given. Here both
// readers answer the same questions over the same fixtures and must agree:
// order, payloads and cost exactly, scores to a part in 10^12. Skipped when
// there is no python3 to ask.

import (
	"bytes"
	"context"
	"encoding/json"
	"math"
	"math/rand"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"testing"
)

type question struct {
	Fixture string `json:"fixture"`
	Method  string `json:"method"`
	Text    string `json:"text"`
	Limit   int    `json:"limit"`
	// Search options; Probe < 0 is no probe.
	Any        bool    `json:"any"`
	Probe      int     `json:"probe"`
	LengthNorm float64 `json:"length_norm"`
	Boost      string  `json:"boost"`
}

// The questions the Python reader's tests ask the TypeScript reader.
var questions = []question{
	{Fixture: "tiny", Method: "search", Text: "tokyo chuo", Limit: 10, Probe: -1, LengthNorm: 0.75},
	{Fixture: "tiny", Method: "search", Text: "tokyo chuo", Limit: 10, Any: true, Probe: -1, LengthNorm: 0.75},
	{Fixture: "tiny", Method: "search", Text: "tokyo chuo shibuya osaka", Limit: 2, Any: true, Probe: -1, LengthNorm: 0.75},
	{Fixture: "tiny", Method: "search", Text: "tokyo chuo", Limit: 10, Any: true, Probe: 1, LengthNorm: 0.75},
	{Fixture: "tiny", Method: "search", Text: "chuo tokyo", Limit: 10, Any: true, Probe: -1, LengthNorm: 0},
	{Fixture: "tiny", Method: "search", Text: "tokyo nowhere", Limit: 10, Any: true, Probe: -1, LengthNorm: 0.75},
	{Fixture: "tiny", Method: "lookup", Text: "c", Limit: 20},
	{Fixture: "tiny", Method: "lookup", Text: "tokyo", Limit: 1},
	{Fixture: "tiny", Method: "complete", Text: "o", Limit: 10},
	{Fixture: "dict", Method: "lookup", Text: "東京都千代田区丸の内", Limit: 100},
	{Fixture: "dict", Method: "lookup", Text: "大阪", Limit: 2},
	{Fixture: "dict", Method: "search", Text: "東京都港区赤坂1丁目1番1号", Limit: 10, Probe: -1, LengthNorm: 0.75, Boost: "importance"},
	{Fixture: "dict", Method: "complete", Text: "東京都", Limit: 2},
	{Fixture: "dict-deflate", Method: "lookup", Text: "東京都", Limit: 100},
	{Fixture: "names", Method: "complete", Text: "bl", Limit: 2},
	{Fixture: "names", Method: "complete", Text: "bl", Limit: 3},
	{Fixture: "names", Method: "complete", Text: "Black S", Limit: 10},
	{Fixture: "names", Method: "complete", Text: "東", Limit: 2},
	{Fixture: "names", Method: "complete", Text: "ＴＯ", Limit: 10},
	{Fixture: "names", Method: "lookup", Text: "b", Limit: 100},
	{Fixture: "names", Method: "search", Text: "Tokyo\u001fBlack Sea", Limit: 10, Any: true, Probe: -1, LengthNorm: 0.75},
	{Fixture: "names-delta", Method: "complete", Text: "東京タ", Limit: 10},
}

// answer is a question's answer in a shape both readers can write. Scores
// are kept apart, since they compare to a tolerance and the rest exactly.
type answer struct {
	Normalized string     `json:"normalized"`
	FromTable  bool       `json:"from_table"`
	Terms      []string   `json:"terms"`
	Probed     []any      `json:"probed"`
	Skipped    int        `json:"skipped"`
	Matched    int        `json:"matched"`
	Results    []any      `json:"results"`
	Scores     []*float64 `json:"scores"`
	Stats      [2]int64   `json:"stats"`
}

const askPython = `
import json, sys
sys.path.insert(0, sys.argv[1])
from mokuji import MokujiIndex, bigram, ja_address, name_list
from mokuji.tokenizers import _OTHER_ALPHABETIC

class Bytes:
    def __init__(self, data):
        self.data = data
    def read(self, offset, length):
        return self.data[offset : offset + length]

asked = json.load(sys.stdin)
answers = []
for q in asked["questions"]:
    with open(f"{sys.argv[2]}/{q['fixture']}.mokuji", "rb") as f:
        index = MokujiIndex.open(Bytes(f.read()))
    index.reset_stats()
    a = {"normalized": "", "from_table": False, "terms": [], "probed": [], "skipped": 0,
         "matched": 0, "results": [], "scores": []}
    if q["method"] == "search":
        r = index.search(q["text"], limit=q["limit"], any=q["any"],
                         probe=None if q["probe"] < 0 else q["probe"],
                         length_norm=q["length_norm"], boost=q["boost"] or None)
        a.update(terms=r.terms, probed=[[p.term, p.doc_freq] for p in r.probed],
                 skipped=r.skipped, matched=r.matched,
                 results=[[x.doc, x.payload] for x in r.results],
                 scores=[x.score for x in r.results])
    elif q["method"] == "lookup":
        r = index.lookup(q["text"], q["limit"])
        a.update(normalized=r.normalized, results=[[x.term, x.doc, x.payload] for x in r.results])
    else:
        r = index.complete(q["text"], q["limit"])
        a.update(normalized=r.normalized, from_table=r.from_table,
                 results=[[d, p] for d, p in zip(r.docs, r.payloads)])
    a["stats"] = [index.stats.requests, index.stats.bytes]
    answers.append(a)
tokens = [[ja_address(s), bigram(s), name_list(s)] for s in asked["strings"]]
json.dump({"answers": answers, "tokens": tokens, "other_alphabetic": _OTHER_ALPHABETIC}, sys.stdout)
`

func goAnswer(t *testing.T, q question) answer {
	ctx := context.Background()
	index := openFixture(t, q.Fixture)
	index.ResetStats()
	a := answer{Terms: []string{}, Probed: []any{}, Results: []any{}, Scores: []*float64{}}
	switch q.Method {
	case "search":
		options := []SearchOption{Limit(q.Limit), LengthNorm(q.LengthNorm)}
		if q.Any {
			options = append(options, Any())
		}
		if q.Probe >= 0 {
			options = append(options, Probe(q.Probe))
		}
		if q.Boost != "" {
			options = append(options, Boost(q.Boost))
		}
		r := must(index.Search(ctx, q.Text, options...))
		a.Terms, a.Skipped, a.Matched = r.Terms, r.Skipped, r.Matched
		for _, p := range r.Probed {
			a.Probed = append(a.Probed, []any{p.Term, p.DocFreq})
		}
		for _, x := range r.Results {
			a.Results = append(a.Results, []any{x.Doc, x.Payload})
			var score *float64
			if r.Scored {
				score = &x.Score
			}
			a.Scores = append(a.Scores, score)
		}
	case "lookup":
		r := must(index.Lookup(ctx, q.Text, q.Limit))
		a.Normalized = r.Normalized
		for _, x := range r.Results {
			a.Results = append(a.Results, []any{x.Term, x.Doc, x.Payload})
		}
	default:
		r := must(index.Complete(ctx, q.Text, q.Limit))
		a.Normalized, a.FromTable = r.Normalized, r.FromTable
		for i, d := range r.Docs {
			a.Results = append(a.Results, []any{d, r.Payloads[i]})
		}
	}
	s := index.Stats()
	a.Stats = [2]int64{int64(s.Requests), s.Bytes}
	// Through JSON, as the Python answer comes, so that the two compare as
	// the same types.
	var out answer
	if err := json.Unmarshal(must(json.Marshal(a)), &out); err != nil {
		t.Fatal(err)
	}
	return out
}

// tokenizerStrings is text in the scripts and forms the tokenizers treat
// differently, alone and run together.
func tokenizerStrings() []string {
	pieces := []string{
		"東京都", "千代田区", "丸ノ内", "丸の内", "一丁目", "二十", "丁目", "１２", "ﾏﾙﾉｳﾁ", "ケーキ",
		"茅ケ崎", "二之瀬", "ヵ", "Black", "Sea", "İstanbul", "ΟΔΟΣ", "Straße", "ǅ", "हिंदी",
		"தமிழ்", "ไทย", "Ⓐⓑ", "½", "x²", "ＣｉｔｙＧＭＬ", "3D", "é", " ", "\u00a0", "　",
		"\u001f", "\t", "-", "・", "、", "😀", "ﬁ", "Ⅻ", "٣", "\u2028",
	}
	rng := rand.New(rand.NewSource(7))
	out := append([]string{}, pieces...)
	for range 400 {
		var b strings.Builder
		for range 1 + rng.Intn(6) {
			b.WriteString(pieces[rng.Intn(len(pieces))])
		}
		out = append(out, b.String())
	}
	return out
}

func TestAgainstPython(t *testing.T) {
	python, err := exec.LookPath("python3")
	if err != nil {
		t.Skip("no python3 to compare against")
	}
	texts := tokenizerStrings()
	input := must(json.Marshal(map[string]any{"questions": questions, "strings": texts}))
	cmd := exec.Command(python, "-I", "-c", askPython, filepath.Join("..", "python"), fixtures)
	cmd.Stdin = bytes.NewReader(input)
	var stderr bytes.Buffer
	cmd.Stderr = &stderr
	output, err := cmd.Output()
	if err != nil {
		t.Fatalf("python: %v\n%s", err, stderr.String())
	}
	var theirs struct {
		Answers         []answer
		Tokens          [][3]any
		OtherAlphabetic [][2]rune `json:"other_alphabetic"`
	}
	if err := json.Unmarshal(output, &theirs); err != nil {
		t.Fatal(err)
	}

	if len(theirs.Answers) != len(questions) || len(theirs.Tokens) != len(texts) {
		t.Fatalf("python answered %d of %d questions", len(theirs.Answers), len(questions))
	}

	t.Run("answers and costs", func(t *testing.T) {
		for i, q := range questions {
			ours, python := goAnswer(t, q), theirs.Answers[i]
			ourScores, theirScores := ours.Scores, python.Scores
			ours.Scores, python.Scores = nil, nil
			if !reflect.DeepEqual(ours, python) {
				t.Errorf("%+v:\n go     %+v\n python %+v", q, ours, python)
				continue
			}
			if len(ourScores) != len(theirScores) {
				t.Errorf("%+v: %d scores, python %d", q, len(ourScores), len(theirScores))
				continue
			}
			for j, a := range ourScores {
				b := theirScores[j]
				if (a == nil) != (b == nil) || (a != nil && math.Abs(*a-*b) > math.Abs(*b)*1e-12) {
					t.Errorf("%+v: score %d is %s, python %s", q, j, scoreText(a), scoreText(b))
				}
			}
		}
	})

	t.Run("the Other_Alphabetic table is the Python reader's", func(t *testing.T) {
		if !reflect.DeepEqual(otherAlphabetic, theirs.OtherAlphabetic) {
			t.Error("alphabetic.go differs from _OTHER_ALPHABETIC in python/mokuji/tokenizers.py")
		}
	})

	t.Run("tokenizers", func(t *testing.T) {
		for i, text := range texts {
			want := theirs.Tokens[i]
			grams := []any{}
			for _, g := range Bigram(text) {
				grams = append(grams, g)
			}
			if JaAddress(text) != want[0] || !reflect.DeepEqual(grams, want[1]) || NameList(text) != want[2] {
				t.Errorf("%+q: go %+q %+q %+q, python %+q", text, JaAddress(text), Bigram(text), NameList(text), want)
			}
		}
	})
}

func scoreText(score *float64) string {
	if score == nil {
		return "none"
	}
	return strconv.FormatFloat(*score, 'g', -1, 64)
}
