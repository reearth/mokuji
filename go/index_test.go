package mokuji

// The reader beyond the conformance cases: the same checks the other
// readers' tests make, and what is particular to reading from a path or a
// URL here.

import (
	"context"
	"errors"
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"testing"
)

// countingSource counts reads itself, so a count is not only the reader's
// word for it.
type countingSource struct {
	Source
	mu    sync.Mutex
	reads int
	bytes int64
}

func (s *countingSource) ReadRange(ctx context.Context, offset, length int64) ([]byte, error) {
	data, err := s.Source.ReadRange(ctx, offset, length)
	s.mu.Lock()
	s.reads++
	s.bytes += int64(len(data))
	s.mu.Unlock()
	return data, err
}

func (s *countingSource) Size() int64 { return s.Source.(Sizer).Size() }

// must is v, or a failed test. A panic fails the test it happens in, which is
// all a test here needs of an error it does not expect.
func must[T any](v T, err error) T {
	if err != nil {
		panic(err)
	}
	return v
}

func termsOf(entries []TermEntry) []string {
	out := []string{}
	for _, e := range entries {
		out = append(out, string(e.Term))
	}
	return out
}

func TestMetadata(t *testing.T) {
	names := openFixture(t, "names")
	if names.License != "CC0-1.0" || names.Attribution != "Mokuji conformance fixtures" {
		t.Fatalf("license %q, attribution %q", names.License, names.Attribution)
	}
	tiny := openFixture(t, "tiny")
	if tiny.License != "" || tiny.Attribution != "" {
		t.Fatal("tiny states no license or credit")
	}

	// What a file was derived from. Neither date is required, and a file
	// that states none says nothing.
	if len(tiny.Sources) != 1 {
		t.Fatalf("%d sources", len(tiny.Sources))
	}
	source := tiny.Sources[0]
	if !regexp.MustCompile(`^sha2-256:[0-9a-f]{64}$`).MatchString(source.ContentHash) ||
		source.URI != "conformance/fixtures/tiny.corpus.txt" || source.Retrieved != "" || source.Modified != "" {
		t.Fatalf("%+v", source)
	}

	if id := openFixture(t, "dict").TokenizerID; id != "identity" {
		t.Fatal(id)
	}
	if names.TokenizerID != "name-list" {
		t.Fatal(names.TokenizerID)
	}

	// The statistics scoring needs.
	if tiny.NumDocs != 4 || tiny.SumDocLen != 8 || !slices.Equal(tiny.DocLengths, []uint64{2, 2, 2, 2}) {
		t.Fatalf("%d %d %v", tiny.NumDocs, tiny.SumDocLen, tiny.DocLengths)
	}

	extra := openFixture(t, "tiny-extra")
	unknown := false
	for id := range extra.Sections {
		unknown = unknown || id >= 0x8000
	}
	if !unknown {
		t.Fatal("the application's section is not listed")
	}
}

func TestOpeningCostsOneRead(t *testing.T) {
	source := &countingSource{Source: BytesSource(fixtureBytes(t, "tiny"))}
	index := must(Open(context.Background(), source))
	if source.reads != 1 || index.Stats().Requests != 1 {
		t.Fatalf("%d reads, %d counted", source.reads, index.Stats().Requests)
	}
}

func TestReadsADeflatedFile(t *testing.T) {
	// The fixtures the other implementations reproduce byte for byte are
	// stored raw, so without this decompression is never exercised.
	index := openFixture(t, "dict-deflate")
	found := must(index.Lookup(context.Background(), "東京都千代田区丸の内1丁目1番1号", 10))
	if len(found.Results) != 1 || found.Results[0].Payload != "139.767201,35.681560" {
		t.Fatalf("%+v", found.Results)
	}
}

func TestAPrefixRangeCostsTheReadsTheSpecificationSays(t *testing.T) {
	ctx := context.Background()
	index := openUncoalesced(t, "dict")
	found := must(index.Lookup(ctx, "東京都千代田区丸の内", 100))
	// Two term blocks of two terms hold the three matches, and their
	// documents span two payload blocks. No postings: they are implied.
	if len(found.Results) != 3 || index.Stats().Requests != 4 {
		t.Fatalf("%d results in %d reads", len(found.Results), index.Stats().Requests)
	}

	// The two payload blocks are neighbours, so a reader that coalesces reads
	// them as one.
	index.Coalesce = true
	index.ResetStats()
	must(index.Lookup(ctx, "東京都千代田区丸の内", 100))
	if n := index.Stats().Requests; n != 3 {
		t.Fatalf("%d reads coalesced", n)
	}
}

func TestReportsEveryRequest(t *testing.T) {
	index := openFixture(t, "dict")
	var seen []RequestInfo
	index.OnRequest = func(r RequestInfo) { seen = append(seen, r) }
	index.ResetStats()
	must(index.Lookup(context.Background(), "東京都千代田区丸の内", 100))
	var total int64
	for _, r := range seen {
		total += r.Length
	}
	if len(seen) != index.Stats().Requests || total != index.Stats().Bytes {
		t.Fatalf("%d requests of %d bytes reported, %+v counted", len(seen), total, index.Stats())
	}
}

func TestRangeReturnsTheTermsBetweenTwoKeysBothInclusive(t *testing.T) {
	ctx := context.Background()
	index := openFixture(t, "dict")
	terms := func(low, high string, limit int) []string {
		return termsOf(must(index.Range(ctx, low, high, limit)))
	}
	for _, c := range []struct {
		low, high string
		limit     int
		want      []string
	}{
		{"東京都千代田区丸の内1丁目1番2号", "東京都港区赤坂1丁目1番1号", 0,
			[]string{"東京都千代田区丸の内1丁目1番2号", "東京都千代田区丸の内2丁目1番1号", "東京都港区赤坂1丁目1番1号"}},
		// Neither end has to be a term.
		{"東京都千代田区丸の内1丁目1番1号a", "東京都千代田区丸の内3", 0,
			[]string{"東京都千代田区丸の内1丁目1番2号", "東京都千代田区丸の内2丁目1番1号"}},
		{"東京都", "東京都千代田区丸の内9", 1, []string{"東京都千代田区丸の内1丁目1番1号"}},
	} {
		if got := terms(c.low, c.high, c.limit); !slices.Equal(got, c.want) {
			t.Fatalf("%s to %s: %q", c.low, c.high, got)
		}
	}
	// Both ends sort before the first block, so the resident index says the
	// range is empty without reading anything.
	index.ResetStats()
	if got := terms("北海道", "千葉県", 0); len(got) != 0 || index.Stats().Requests != 0 {
		t.Fatalf("%q in %d reads", got, index.Stats().Requests)
	}
}

func TestExactFindsATermWithoutCompletingIt(t *testing.T) {
	ctx := context.Background()
	index := openFixture(t, "tiny")
	// `tiny` is a document collection, so a term is a word.
	if entry := must(index.Exact(ctx, "tokyo")); entry == nil || string(entry.Term) != "tokyo" {
		t.Fatalf("%+v", entry)
	}
	for _, absent := range []string{"toky", "tokyoo", ""} {
		if entry := must(index.Exact(ctx, absent)); entry != nil {
			t.Fatalf("%q found %q", absent, entry.Term)
		}
	}
}

func TestExactCostsOneDictionaryRead(t *testing.T) {
	ctx := context.Background()
	source := &countingSource{Source: BytesSource(fixtureBytes(t, "tiny"))}
	index := must(Open(ctx, source))
	// The block a term would be in is answerable from the resident index, so
	// a term that is absent costs exactly what a term that is present does.
	before := source.reads
	must(index.Exact(ctx, "tokyo"))
	must(index.Exact(ctx, "nowhere"))
	if n := source.reads - before; n != 2 {
		t.Fatalf("%d reads", n)
	}
}

func TestATermBeforeEveryBlockCostsNoReadAtAll(t *testing.T) {
	ctx := context.Background()
	// `aomori` sorts before `chiyoda`, the first boundary key, so the
	// resident index settles it without touching the file (SPEC.md §5.3.2).
	index := openFixture(t, "tiny")
	index.ResetStats()
	if must(index.Exact(ctx, "aomori")) != nil || len(must(index.Prefix(ctx, "a", 20))) != 0 {
		t.Fatal("found what sorts before every term")
	}
	if n := index.Stats().Requests; n != 0 {
		t.Fatalf("%d reads", n)
	}
	// A prefix of the first term is not before it in that sense.
	if got := termsOf(must(index.Prefix(ctx, "c", 20))); !slices.Equal(got, []string{"chiyoda", "chuo"}) {
		t.Fatal(got)
	}
}

func TestExactManyReadsASharedBlockOnce(t *testing.T) {
	index := openUncoalesced(t, "tiny")
	found := must(index.ExactMany(context.Background(), []string{"chiyoda", "chuo", "kyoto"}))
	if found[0] == nil || string(found[0].Term) != "chiyoda" || found[1] == nil || string(found[1].Term) != "chuo" || found[2] != nil {
		t.Fatalf("%+v", found)
	}
	// `chiyoda` and `chuo` share block 0; `kyoto` would be in block 0 too.
	if n := index.Stats().Requests; n != 1 {
		t.Fatalf("%d reads", n)
	}
}

func TestPrefixManyReadsEachBlockOnce(t *testing.T) {
	index := openUncoalesced(t, "dict")
	found := must(index.PrefixMany(context.Background(), []string{"東京都千代田区丸の内1", "東京都千代田区丸の内"}))
	if len(found[0]) != 2 || len(found[1]) != 3 || index.Stats().Requests != 2 {
		t.Fatalf("%d and %d in %d reads", len(found[0]), len(found[1]), index.Stats().Requests)
	}
}

func TestDocFreqAndThePostingsRunMustAgree(t *testing.T) {
	data := fixtureBytes(t, "tiny")
	index := must(Open(context.Background(), BytesSource(data)))
	// The first block is `entry_count`, `first_term_id`, then the first
	// entry: a front-coded key of `chiyoda` in nine bytes, then `doc_freq`.
	at := index.Sections[SectionTermBlocks].Offset + 2 + 2 + 7
	if data[at] != 1 {
		t.Fatalf("doc_freq is %d", data[at])
	}
	// Claiming two documents for a postings stated as one must be caught
	// rather than trusted.
	data[at] = 2
	broken := must(Open(context.Background(), BytesSource(data)))
	var format *FormatError
	if _, err := broken.Lookup(context.Background(), "chiyoda", 20); !errors.As(err, &format) {
		t.Fatalf("got %v", err)
	}
}

func TestRejectsWhatIsNotAMokujiFile(t *testing.T) {
	ctx := context.Background()
	var format *FormatError
	if _, err := Open(ctx, BytesSource(append([]byte("PK\x03\x04"), make([]byte, 100)...))); !errors.As(err, &format) {
		t.Fatalf("a zip: %v", err)
	}
	data := fixtureBytes(t, "tiny")
	data[8] = 1
	if _, err := Open(ctx, BytesSource(data)); !errors.As(err, &format) {
		t.Fatalf("version 1: %v", err)
	}
	if _, err := Open(ctx, BytesSource(fixtureBytes(t, "tiny")[:400])); !errors.As(err, &format) {
		t.Fatalf("truncated: %v", err)
	}
}

// Every prefix of every fixture, truncated or with a byte flipped, either
// opens and answers or fails with an error; none panics.
func TestMalformedFilesFailCleanly(t *testing.T) {
	ctx := context.Background()
	for _, name := range []string{"tiny", "dict", "dict-deflate", "names"} {
		data := fixtureBytes(t, name)
		try := func(b []byte) {
			index, err := Open(ctx, BytesSource(b))
			if err != nil {
				return
			}
			_, _ = index.Lookup(ctx, "", 100)
			_, _ = index.Complete(ctx, "b", 10)
			_, _ = index.Search(ctx, "tokyo chuo", Any())
		}
		for n := 0; n < len(data); n += 7 {
			try(data[:n])
		}
		for i := range data {
			flipped := slices.Clone(data)
			flipped[i] ^= 0xFF
			try(flipped)
		}
	}
}

func TestSearchRanksAsMokujiSearchDoes(t *testing.T) {
	ctx := context.Background()
	tiny := openFixture(t, "tiny")
	// Scores to three places, as `mokuji search` prints them for the same
	// file.
	rounded := func(r *SearchResult) []string {
		out := []string{}
		for _, x := range r.Results {
			out = append(out, fmt.Sprintf("%d %.3f %s", x.Doc, x.Score, x.Payload))
		}
		return out
	}
	if got := rounded(must(tiny.Search(ctx, "tokyo chuo"))); !slices.Equal(got, []string{"1 1.050 35.671,139.772"}) {
		t.Fatal(got)
	}
	want := []string{"1 1.050 35.671,139.772", "3 0.693 34.681,135.513", "0 0.357 35.694,139.754", "2 0.357 35.664,139.698"}
	if got := rounded(must(tiny.Search(ctx, "tokyo chuo", Any()))); !slices.Equal(got, want) {
		t.Fatal(got)
	}
	probed := must(tiny.Search(ctx, "tokyo chuo", Any(), Probe(1)))
	if len(probed.Probed) != 1 || probed.Probed[0].Term != "chuo" || probed.Skipped != 1 {
		t.Fatalf("%+v", probed)
	}
	if n := must(tiny.Search(ctx, "tokyo nowhere")).Matched; n != 0 {
		t.Fatal(n)
	}
	if _, err := tiny.Search(ctx, "tokyo", Boost("absent")); err == nil {
		t.Fatal("boosted by a column that is not there")
	}
}

func TestAFilterLeavesOutDocumentsBeforeTheirPayload(t *testing.T) {
	ctx := context.Background()
	tiny := openFixture(t, "tiny")
	even := func(doc int) bool { return doc%2 == 0 }
	found := must(tiny.Search(ctx, "tokyo chuo", Any(), Filter(even), Limit(1)))
	// Ranked as without the filter, the odd ones gone: 1 and 3 outscore 0 and
	// 2, which tie and fall to document order.
	if found.Matched != 2 || len(found.Results) != 1 || found.Results[0].Doc != 0 {
		t.Fatalf("%+v", found)
	}
	unfiltered := must(tiny.Search(ctx, "tokyo chuo", Any(), Limit(4)))
	if found.Results[0].Score != unfiltered.Results[2].Score {
		t.Fatal("a filter changed a score")
	}
	none := must(tiny.Search(ctx, "tokyo chuo", Filter(even)))
	if none.Matched != 0 || len(none.Results) != 0 {
		t.Fatalf("%+v", none)
	}
}

func TestAProbeOfNothingMatchesNothing(t *testing.T) {
	// Every term left out leaves nothing to intersect, rather than an error.
	found := must(openFixture(t, "tiny").Search(context.Background(), "tokyo chuo", Probe(0)))
	if found.Matched != 0 || found.Skipped != 2 || len(found.Results) != 0 {
		t.Fatalf("%+v", found)
	}
}

func TestBoostWithoutDocumentLengthsScoresNothing(t *testing.T) {
	ctx := context.Background()
	index := openFixture(t, "dict")
	plain := must(index.Search(ctx, "東京都千代田区丸の内1丁目1番1号"))
	boosted := must(index.Search(ctx, "東京都千代田区丸の内1丁目1番1号", Boost("importance")))
	if len(boosted.Results) != 1 || boosted.Results[0].Doc != 1 {
		t.Fatalf("%+v", boosted.Results)
	}
	// Without document lengths nothing is scored, prior or not.
	if plain.Scored || boosted.Scored || plain.Results[0].Score != boosted.Results[0].Score {
		t.Fatal("scored without document lengths")
	}
}

func TestQueriesRunAtOnce(t *testing.T) {
	ctx := context.Background()
	index := openFixture(t, "names")
	index.ResetStats()
	var wg sync.WaitGroup
	for range 16 {
		wg.Add(1)
		go func() {
			defer wg.Done()
			done, err := index.Complete(ctx, "bl", 3)
			if err != nil || !slices.Equal(done.Payloads, []string{"Black Sea", "Black Forest", "Blackpool"}) {
				t.Errorf("%v %+v", err, done)
			}
			// What one completion cost, whatever the others did meanwhile.
			if done.RequestsBeforePayload != 3 {
				t.Errorf("%d requests before the payload", done.RequestsBeforePayload)
			}
		}()
	}
	wg.Wait()
}

func TestAPathReadsOnlyWhatAQueryTouches(t *testing.T) {
	// A file of four gigabytes, sparse so that the test costs nothing: the
	// dictionary fixture, then zeros.
	ctx := context.Background()
	path := filepath.Join(t.TempDir(), "large.mokuji")
	if err := os.WriteFile(path, fixtureBytes(t, "dict"), 0o644); err != nil {
		t.Fatal(err)
	}
	if err := os.Truncate(path, 4<<30); err != nil {
		t.Skip("no sparse files here:", err)
	}
	index := must(OpenFile(ctx, path))
	defer func() { _ = index.Close() }()
	if n, ok := index.FileLength(); !ok || n != 4<<30 {
		t.Fatal(n, ok)
	}
	opened := index.Stats().Bytes
	found := must(index.Lookup(ctx, "東京都千代田区丸の内1丁目1番1号", 10))
	if found.Results[0].Payload != "139.767201,35.681560" {
		t.Fatal(found.Results)
	}
	// The opening guess, and a few hundred bytes for the query.
	if opened > openPrefix || index.Stats().Bytes-opened >= 1024 {
		t.Fatalf("%d bytes to open, %d to query", opened, index.Stats().Bytes-opened)
	}
}

func TestAPathAnswersAsAnyOtherSourceDoes(t *testing.T) {
	ctx := context.Background()
	expected := openFixture(t, "dict")
	expected.ResetStats()
	want := must(expected.Lookup(ctx, "東京都千代田区丸の内", 100))
	index := must(OpenFile(ctx, filepath.Join(fixtures, "dict.mokuji")))
	defer func() { _ = index.Close() }()
	if n, _ := index.FileLength(); n != int64(len(fixtureBytes(t, "dict"))) {
		t.Fatal(n)
	}
	index.ResetStats()
	got := must(index.Lookup(ctx, "東京都千代田区丸の内", 100))
	if fmt.Sprint(got) != fmt.Sprint(want) || index.Stats() != expected.Stats() {
		t.Fatalf("%+v in %+v, want %+v in %+v", got, index.Stats(), want, expected.Stats())
	}
}

// serve serves one file over HTTP, honouring Range or not.
func serve(t *testing.T, data []byte, honourRange bool) string {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		spec, ok := strings.CutPrefix(r.Header.Get("Range"), "bytes=")
		if !honourRange || !ok {
			_, _ = w.Write(data)
			return
		}
		first, last, _ := strings.Cut(spec, "-")
		start, _ := strconv.Atoi(first)
		end, _ := strconv.Atoi(last)
		end = min(end, len(data)-1)
		w.Header().Set("Content-Range", fmt.Sprintf("bytes %d-%d/%d", start, end, len(data)))
		w.WriteHeader(http.StatusPartialContent)
		_, _ = w.Write(data[start : end+1])
	}))
	t.Cleanup(server.Close)
	return server.URL + "/names.mokuji"
}

func TestAURLIsReadWithRangeRequests(t *testing.T) {
	ctx := context.Background()
	data := fixtureBytes(t, "names")
	index := must(OpenURL(ctx, serve(t, data, true)))
	if n, ok := index.FileLength(); !ok || n != int64(len(data)) {
		t.Fatal(n, ok)
	}
	index.ResetStats()
	done := must(index.Complete(ctx, "bl", 2))
	if !slices.Equal(done.Payloads, []string{"Black Sea", "Black Forest"}) || index.Stats().Requests != 1 {
		t.Fatalf("%q in %d reads", done.Payloads, index.Stats().Requests)
	}
}

func TestAServerThatIgnoresRangeIsAnError(t *testing.T) {
	_, err := OpenURL(context.Background(), serve(t, fixtureBytes(t, "names"), false))
	var rangeErr *RangeError
	if !errors.As(err, &rangeErr) || rangeErr.Status != http.StatusOK {
		t.Fatalf("got %v", err)
	}
}

func TestACollectionOpenedByPathChecksLengths(t *testing.T) {
	ctx := context.Background()
	collection := must(OpenCollection(ctx, filepath.Join(fixtures, "collection.json"), nil))
	defer func() { _ = collection.Close() }()
	if len(collection.Segments) != 2 || !slices.Equal(collection.Attributions(), []string{"Mokuji conformance fixtures"}) {
		t.Fatalf("%d segments, %q", len(collection.Segments), collection.Attributions())
	}
	done := must(collection.Complete(ctx, "black", 10))
	if !slices.Equal(done.Payloads, []string{"Black Forest", "Blackpool", "Black Mountain"}) {
		t.Fatal(done.Payloads)
	}
	// Ranking across segments asks every one of them.
	both := must(collection.Candidates(ctx, "black", 10))
	segments := map[int]bool{}
	for _, h := range both.Hits {
		segments[h.Segment] = true
	}
	if len(segments) != 2 {
		t.Fatalf("%+v", both.Hits)
	}
}

func TestACollectionRejectsASegmentOfAnotherLength(t *testing.T) {
	manifest := Manifest{MokujiManifest: "0.1", Segments: []ManifestSegment{{Path: "names.mokuji", Length: 1}}}
	_, err := NewCollection(context.Background(), &manifest, filepath.Join(fixtures, "collection.json"), nil)
	var format *FormatError
	if !errors.As(err, &format) {
		t.Fatalf("got %v", err)
	}
}

func TestACollectionOverHTTPResolvesSegmentsAgainstTheManifest(t *testing.T) {
	ctx := context.Background()
	mux := http.NewServeMux()
	mux.Handle("/data/", http.StripPrefix("/data/", http.FileServer(http.Dir(fixtures))))
	server := httptest.NewServer(mux)
	defer server.Close()
	collection := must(OpenCollection(ctx, server.URL+"/data/collection.json", &CollectionOptions{Countries: []string{"JP"}}))
	defer func() { _ = collection.Close() }()
	done := must(collection.Complete(ctx, "東京タ", 10))
	if !slices.Equal(done.Payloads, []string{"Tokyo Tower"}) {
		t.Fatal(done.Payloads)
	}
}
