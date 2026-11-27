package mokuji

// What the four libraries have in common beyond the conformance cases: a
// phrase, a ranking without payload, a collection's search, manifests,
// metadata, errors and a held dictionary. The same vectors as the other
// languages' tests.

import (
	"context"
	"errors"
	"math"
	"os"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
)

func docsOf(r *SearchResult) []int {
	out := []int{}
	for _, x := range r.Results {
		out = append(out, x.Doc)
	}
	return out
}

func TestAPhraseHoldsTheTermsToTheirPlaces(t *testing.T) {
	ctx := context.Background()
	tiny := openFixture(t, "tiny")
	for _, c := range []struct {
		query string
		opts  []SearchOption
		want  []int
	}{
		{"tokyo chuo", nil, []int{1}},
		{"chuo tokyo", nil, []int{}},
		{"osaka chuo", nil, []int{3}},
		{"tokyo chuo", []SearchOption{Any()}, []int{1}},
	} {
		got := docsOf(must(tiny.Search(ctx, c.query, append(c.opts, Phrase())...)))
		if !slices.Equal(got, c.want) {
			t.Errorf("%q: got %v, want %v", c.query, got, c.want)
		}
	}
}

func TestAPhraseOfTheTermsAProbeReadRequiresOnlyThose(t *testing.T) {
	ctx := context.Background()
	tiny := openFixture(t, "tiny")
	phrase := must(tiny.Search(ctx, "tokyo chuo", Phrase(), Probe(1)))
	if len(phrase.Probed) != 1 || phrase.Probed[0].Term != "chuo" {
		t.Fatalf("probed %v", phrase.Probed)
	}
	plain := must(tiny.Search(ctx, "tokyo chuo", Probe(1)))
	if got, want := docsOf(phrase), docsOf(plain); !slices.Equal(got, want) || !slices.Equal(slices.Sorted(slices.Values(got)), []int{1, 3}) {
		t.Fatalf("got %v, want %v", got, want)
	}
}

func TestAPhraseReadsNoPositionsForWhatTheFilterLeftOut(t *testing.T) {
	ctx := context.Background()
	tiny := openFixture(t, "tiny")
	notOne := Filter(func(doc int) bool { return doc != 1 })
	tiny.ResetStats()
	must(tiny.Search(ctx, "tokyo chuo", notOne))
	without := tiny.Stats().Requests
	tiny.ResetStats()
	found := must(tiny.Search(ctx, "tokyo chuo", notOne, Phrase()))
	if len(found.Results) != 0 || found.Matched != 0 {
		t.Fatalf("got %+v", found)
	}
	if got := tiny.Stats().Requests; got > without {
		t.Fatalf("%d requests with the phrase, %d without", got, without)
	}
}

func TestAPhraseNeedsPositions(t *testing.T) {
	_, err := openFixture(t, "dict").Search(context.Background(), "東京都", Phrase())
	var format *FormatError
	if !errors.As(err, &format) || !strings.Contains(err.Error(), "this index carries no positions, so it cannot test a phrase") {
		t.Fatalf("got %v", err)
	}
}

func TestRankIsSearchWithoutThePayload(t *testing.T) {
	ctx := context.Background()
	tiny := openFixture(t, "tiny")
	found := must(tiny.Search(ctx, "tokyo chuo"))
	tiny.ResetStats()
	ranking := must(tiny.Rank(ctx, []string{"tokyo", "chuo"}))
	afterRank := tiny.Stats().Requests
	if len(ranking.Ranked) != len(found.Results) || ranking.Scored != found.Scored {
		t.Fatalf("got %+v, want %+v", ranking, found)
	}
	for i, r := range ranking.Ranked {
		if r.Doc != found.Results[i].Doc || r.Score != found.Results[i].Score {
			t.Fatalf("got %+v, want %+v", ranking.Ranked, found.Results)
		}
	}
	tiny.ResetStats()
	must(tiny.Search(ctx, "tokyo chuo"))
	if afterRank >= tiny.Stats().Requests {
		t.Fatalf("rank read %d, search %d: rank read the payload", afterRank, tiny.Stats().Requests)
	}
}

func openCollectionFixture(t *testing.T) *Collection {
	t.Helper()
	c := must(OpenCollection(context.Background(), filepath.Join(fixtures, "collection.json"), nil))
	t.Cleanup(func() { _ = c.Close() })
	return c
}

func TestACollectionSearchesEverySegmentLessItsDeletes(t *testing.T) {
	ctx := context.Background()
	c := openCollectionFixture(t)
	for _, tc := range []struct {
		query    string
		hits     []Hit
		payloads []string
	}{
		{"Tokyo", []Hit{{0, 4}}, []string{"Tokyo"}},
		{"Black Mountain", []Hit{{1, 0}}, []string{"Black Mountain"}},
		{"東京タワー", []Hit{{1, 1}}, []string{"Tokyo Tower"}},
		// `names` has it, as document 0, which the manifest deletes.
		{"Black Sea", []Hit{}, []string{}},
		{"Atlantis", []Hit{}, []string{}},
	} {
		found := must(c.Search(ctx, tc.query, 10))
		if !reflect.DeepEqual(found.Hits, tc.hits) || !slices.Equal(found.Payloads, tc.payloads) || found.Matched != len(tc.hits) {
			t.Errorf("%q: got %+v", tc.query, found)
		}
	}
}

func TestACollectionSearchIsInCollectionOrder(t *testing.T) {
	ctx := context.Background()
	// Two segments both holding `x`, the first deleting its document 1.
	segment := func(payloads ...string) []byte {
		b := must(NewBuilder(DefaultWriteOptions()))
		for _, p := range payloads {
			must(b.AddDocument([]byte(p)))
		}
		for doc := range payloads {
			if err := b.AddPosting([]byte("x"), doc, 1); err != nil {
				t.Fatal(err)
			}
		}
		return must(b.Finish(must((&Metadata{
			Sources:     []Provenance{{ContentHash: "sha2-256:" + strings.Repeat("0", 64)}},
			TokenizerID: "identity",
		}).Encode())))
	}
	files := map[string][]byte{"a": segment("a0", "a1", "a2"), "b": segment("b0", "b1")}
	hash := "sha2-256:" + strings.Repeat("0", 64)
	manifest := &Manifest{MokujiManifest: "0.1", Generation: 1, Segments: []ManifestSegment{
		{Path: "a", ContentHash: hash, Length: int64(len(files["a"])), Deletes: []int{1}},
		{Path: "b", ContentHash: hash, Length: int64(len(files["b"]))},
	}}
	c := must(NewCollection(ctx, manifest, "", &CollectionOptions{
		SourceFor: func(_ context.Context, location string) (Source, error) { return BytesSource(files[location]), nil },
	}))
	found := must(c.Search(ctx, "x", 3))
	if want := []Hit{{0, 0}, {0, 2}, {1, 0}}; !reflect.DeepEqual(found.Hits, want) || found.Matched != 4 {
		t.Fatalf("got %+v", found)
	}
	if want := []string{"a0", "a2", "b0"}; !slices.Equal(found.Payloads, want) {
		t.Fatalf("got %v", found.Payloads)
	}
}

func TestLiveInIsOneSegmentsFirstLiveDocuments(t *testing.T) {
	c := openCollectionFixture(t)
	// The case "a delete leaves an entry short": `names` lists `bl` with
	// its deleted document 0.
	live, fromTable := must2(c.LiveIn(context.Background(), 0, "bl", 2))
	if !slices.Equal(live, []int{1, 2}) || fromTable {
		t.Fatalf("got %v %v", live, fromTable)
	}
	if _, _, err := c.LiveIn(context.Background(), 2, "bl", 2); err == nil {
		t.Fatal("a segment past the last answered")
	}
}

const sampleManifest = `{
  "mokuji_manifest": "0.1",
  "generation": 3,
  "segments": [
    {
      "path": "base.mokuji",
      "content_hash": "sha2-256:abababababababababababababababababababababababababababababababab",
      "length": 1000,
      "deletes": [
        2,
        7
      ]
    },
    {
      "path": "delta.mokuji",
      "content_hash": "sha2-256:cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd",
      "length": 100,
      "region": {
        "countries": [
          "JP",
          "KR"
        ]
      }
    }
  ]
}
`

func TestAManifestIsWrittenAsTheRustCrateWritesIt(t *testing.T) {
	m := must(ParseManifest([]byte(sampleManifest)))
	if got := string(m.JSON()); got != sampleManifest {
		t.Fatalf("got\n%s", got)
	}
	data := must(os.ReadFile(filepath.Join(fixtures, "collection.json")))
	if got := must(ParseManifest(data)).JSON(); string(got) != string(data) {
		t.Fatalf("got\n%s", got)
	}
	// Unknown keys are ignored.
	extra := strings.Replace(sampleManifest, "{", "{\n  \"published\": \"2026-09-30\",", 1)
	if !reflect.DeepEqual(must(ParseManifest([]byte(extra))), m) {
		t.Fatal("an unknown key changed the manifest")
	}
}

func TestAManifestIsRefusedForWhatCannotBeTrusted(t *testing.T) {
	for name, bad := range map[string]string{
		"unsorted deletes":      strings.Replace(sampleManifest, "2,\n        7", "7,\n        2", 1),
		"a delete twice":        strings.Replace(sampleManifest, "2,\n        7", "2,\n        2", 1),
		"a bare hash":           strings.Replace(sampleManifest, `"sha2-256:cdcd`, `"cdcd`, 1),
		"an uppercase hash":     strings.Replace(sampleManifest, "sha2-256:cdcd", "sha2-256:CDcd", 1),
		"a major version ahead": strings.Replace(sampleManifest, `"0.1"`, `"1.0"`, 1),
		"no segments":           `{"mokuji_manifest": "0.1", "generation": 1, "segments": []}`,
		"no path":               strings.Replace(sampleManifest, `"base.mokuji"`, `""`, 1),
		"no generation":         strings.Replace(sampleManifest, `"generation": 3,`, ``, 1),
		"a negative length":     strings.Replace(sampleManifest, `1000`, `-1`, 1),
		"no countries":          strings.Replace(sampleManifest, "\"JP\",\n          \"KR\"", "", 1),
		"a lowercase country":   strings.Replace(sampleManifest, `"JP"`, `"jp"`, 1),
		"a three-letter code":   strings.Replace(sampleManifest, `"JP"`, `"JPN"`, 1),
		"a country twice":       strings.Replace(sampleManifest, `"KR"`, `"JP"`, 1),
		"not JSON":              `{`,
	} {
		_, err := ParseManifest([]byte(bad))
		var format *FormatError
		if !errors.As(err, &format) {
			t.Errorf("%s: got %v", name, err)
		}
	}
}

func TestWithinKeepsWhatHasNoRegionAndWhatIncludesThePlace(t *testing.T) {
	m := must(ParseManifest([]byte(sampleManifest)))
	if n := len(m.Within([]string{"JP"}).Segments); n != 2 {
		t.Fatal(n)
	}
	elsewhere := m.Within([]string{"FR"})
	if len(elsewhere.Segments) != 1 || elsewhere.Segments[0].Path != "base.mokuji" || !slices.Equal(elsewhere.Segments[0].Deletes, []int{2, 7}) {
		t.Fatalf("got %+v", elsewhere)
	}
	if len(m.Segments) != 2 {
		t.Fatal("Within changed the manifest it was asked of")
	}
}

func TestTheMetadataReadsAsItWasWritten(t *testing.T) {
	meta := must((&Metadata{
		Generation:      7,
		Sources:         []Provenance{{ContentHash: "sha2-256:" + strings.Repeat("0", 64)}},
		TokenizerID:     "identity",
		TokenizerParams: map[string]any{"form": "nfkc"},
		Attrs:           map[string]any{"geometry": "delta-int-1e5"},
	}).Encode())
	b := must(NewBuilder(DefaultWriteOptions()))
	must(b.AddDocument([]byte("x")))
	if err := b.AddPosting([]byte("x"), 0, 1); err != nil {
		t.Fatal(err)
	}
	index := must(Open(context.Background(), BytesSource(must(b.Finish(meta)))))
	if index.Generation != 7 || index.TokenizerParams["form"] != "nfkc" || index.Attrs["geometry"] != "delta-int-1e5" {
		t.Fatalf("got %d %v %v", index.Generation, index.TokenizerParams, index.Attrs)
	}
	tiny := openFixture(t, "tiny")
	if tiny.Attrs != nil || tiny.TokenizerParams == nil {
		t.Fatalf("got %v %v", tiny.Attrs, tiny.TokenizerParams)
	}
	decoded := must(DecodeCBOR(meta))
	if !reflect.DeepEqual(decoded, index.Meta) {
		t.Fatalf("got %v, want %v", decoded, index.Meta)
	}
	if _, err := DecodeCBOR(append(slices.Clone(meta), 0)); err == nil {
		t.Fatal("trailing bytes decoded")
	}
}

func TestErrorsSayWhatWasRefused(t *testing.T) {
	ctx := context.Background()
	_, err := openFixture(t, "tiny").Search(ctx, "tokyo", Boost("population"))
	var column *NoColumnError
	if !errors.As(err, &column) || column.Name != "population" {
		t.Fatalf("got %v", err)
	}
	b := must(NewBuilder(DefaultWriteOptions()))
	must(b.AddDocument([]byte("x")))
	must(b.AddDocument([]byte("y")))
	if err := b.AddPosting([]byte("x"), 1, 1); err != nil {
		t.Fatal(err)
	}
	var build *BuildError
	if err := b.AddPosting([]byte("x"), 0, 1); !errors.As(err, &build) {
		t.Fatalf("got %v", err)
	}
	if _, err := Locate(ctx, openFixture(t, "dict"), 35.68, 139.76, nil); !errors.Is(err, ErrNotCoverings) {
		t.Fatalf("got %v", err)
	}
	if _, err := DecodeRings("0,x", 1); !errors.Is(err, ErrRings) {
		t.Fatalf("got %v", err)
	}
}

func TestNormalizeByTokenizerIsWhatTheIndexDoes(t *testing.T) {
	dict := openFixture(t, "dict")
	typed := "東京都千代田区丸の内一丁目"
	got, ok := Normalize(dict.TokenizerID, typed)
	if !ok || got != dict.Normalize(typed) {
		t.Fatalf("got %q %v", got, ok)
	}
	if got, ok := Normalize("no-such-tokenizer", typed); ok || got != typed {
		t.Fatalf("got %q %v", got, ok)
	}
}

func TestAHeldDictionaryAnswersWithoutReadingIt(t *testing.T) {
	ctx := context.Background()
	data := fixtureBytes(t, "dict")
	plain := must(Open(ctx, BytesSource(data)))
	held := must(OpenWith(ctx, BytesSource(data), OpenOptions{ResidentDictionary: true}))
	if held.Stats().Requests != plain.Stats().Requests+1 {
		t.Fatalf("opening cost %d, against %d", held.Stats().Requests, plain.Stats().Requests)
	}
	held.ResetStats()
	plain.ResetStats()
	a := must(held.Lookup(ctx, "東京都千代田区丸の内", 10))
	b := must(plain.Lookup(ctx, "東京都千代田区丸の内", 10))
	if !reflect.DeepEqual(a, b) {
		t.Fatalf("got %+v, want %+v", a, b)
	}
	entry := must(held.Exact(ctx, "東京都千代田区丸の内1丁目1番1号"))
	if entry == nil {
		t.Fatal("not found")
	}
	if held.Stats().Requests >= plain.Stats().Requests {
		t.Fatalf("held %d requests, fetched %d", held.Stats().Requests, plain.Stats().Requests)
	}
	held.ResetStats()
	must(held.Exact(ctx, "東京都千代田区丸の内1丁目1番1号"))
	if n := held.Stats().Requests; n != 0 {
		t.Fatalf("a held lookup cost %d requests", n)
	}
}

func TestLocateTakesFromTheFileWhatTheCallerLeftOut(t *testing.T) {
	ctx := context.Background()
	index := coverings(t)
	for _, options := range []*LocateOptions{{Precisions: []int{}}, {Scale: -1}, {Scale: math.Inf(1)}, {Scale: math.NaN()}} {
		found, err := Locate(ctx, index, 35.6801, 139.7601, options)
		if err != nil || len(found.Inside) != 1 {
			t.Fatalf("%+v: got %+v, %v", options, found, err)
		}
	}
	for _, p := range [][]int{{0}, {13}, {5, -1}} {
		if _, err := Locate(ctx, index, 35.6801, 139.7601, &LocateOptions{Precisions: p}); !errors.Is(err, ErrNotCoverings) {
			t.Fatalf("%v: got %v", p, err)
		}
	}
	if _, err := DecodeRings("9223372036854775807,0,1,0", 1); !errors.Is(err, ErrRings) {
		t.Fatalf("got %v", err)
	}
}

func TestLookupPrefixOfNoDocumentsReadsNothing(t *testing.T) {
	dict := openFixture(t, "dict")
	dict.ResetStats()
	hits := must(LookupPrefix(context.Background(), dict, "東京都", 0, []string{"東京都"}))
	if len(hits) != 0 || dict.Stats().Requests != 0 {
		t.Fatalf("got %v in %d requests", hits, dict.Stats().Requests)
	}
}

func TestACollectionRefusesASegmentItDoesNotHave(t *testing.T) {
	c := openCollectionFixture(t)
	if _, err := c.Payloads(context.Background(), []Hit{{Segment: 2, Doc: 0}}); err == nil || !strings.Contains(err.Error(), "the collection has no segment 2") {
		t.Fatalf("got %v", err)
	}
}

func TestWithinCopiesWhatItKeeps(t *testing.T) {
	m := must(ParseManifest([]byte(sampleManifest)))
	kept := m.Within([]string{"JP"})
	kept.Segments[0].Deletes[0] = 99
	kept.Segments[1].Region.Countries[0] = "FR"
	if m.Segments[0].Deletes[0] != 2 || m.Segments[1].Region.Countries[0] != "JP" {
		t.Fatal("changing the restriction changed the manifest")
	}
}

func TestASearchThatMatchesNothingEarlyIsNotScored(t *testing.T) {
	ctx := context.Background()
	// A file with document lengths, so that Scored says what it always has.
	b := must(NewBuilder(WriteOptions{TermsPerBlock: 4, DocsPerPayloadBlock: 2, HasFreqs: true, DocLengths: true}))
	must(b.AddDocument([]byte("x")))
	if err := b.AddPosting([]byte("x"), 0, 1); err != nil {
		t.Fatal(err)
	}
	index := must(Open(ctx, BytesSource(must(b.Finish(must((&Metadata{
		Sources:     []Provenance{{ContentHash: "sha2-256:" + strings.Repeat("0", 64)}},
		TokenizerID: "space-split",
	}).Encode()))))))
	if index.DocLengths == nil {
		t.Skip("the builder wrote no document lengths")
	}
	for _, q := range []string{"", "nothing"} {
		if found := must(index.Search(ctx, q)); found.Scored {
			t.Fatalf("%q: scored", q)
		}
	}
	if found := must(index.Search(ctx, "x")); !found.Scored {
		t.Fatal("a match is not scored")
	}
}
