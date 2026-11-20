package mokuji

// The Go reader against the conformance suite.
//
// Every case is checked for its result and for the number of range reads it
// took, because the bound in SPEC.md §4 is the property the format exists to
// provide. The cases count block by block, so coalescing is turned off for
// them and checked separately at the end.

import (
	"context"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/BurntSushi/toml"
)

var fixtures = filepath.Join("..", "conformance", "fixtures")

type suite struct {
	Case []struct {
		Name      string
		Fixture   string
		Query     []string
		Docs      []int
		Payloads  []string
		Reads     int
		Positions [][]uint64
		Attribute string
		Values    []int64
	}
	Completion []struct {
		Name      string
		Fixture   string
		Prefix    string
		Limit     int
		Docs      []int
		Payloads  []string
		FromTable bool `toml:"from_table"`
		Reads     int
	}
	Collection []struct {
		Name      string
		Manifest  string
		Prefix    string
		Limit     int
		Countries []string
		Segments  []int
		Docs      []int
		Payloads  []string
		Reads     int
	}
}

// loadSuite reads the suite rather than restating it: a copy of the expectations
// would drift, and the whole point is that every implementation answers to
// the same ones.
func loadSuite(t *testing.T) suite {
	t.Helper()
	var s suite
	if _, err := toml.DecodeFile(filepath.Join("..", "conformance", "cases.toml"), &s); err != nil {
		t.Fatal(err)
	}
	if len(s.Case) == 0 || len(s.Completion) == 0 || len(s.Collection) == 0 {
		t.Fatal("the suite is empty")
	}
	return s
}

func fixtureBytes(t testing.TB, name string) []byte {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(fixtures, name+".mokuji"))
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func openFixture(t testing.TB, name string) *Index {
	t.Helper()
	index, err := Open(context.Background(), BytesSource(fixtureBytes(t, name)))
	if err != nil {
		t.Fatal(err)
	}
	return index
}

func openUncoalesced(t testing.TB, name string) *Index {
	t.Helper()
	index := openFixture(t, name)
	index.Coalesce = false
	// Opening is accounted for separately: SPEC.md §4 charges the resident
	// set to a cold start, not to a query.
	index.ResetStats()
	return index
}

// termQuery is the documents holding every term exactly, with their payload:
// how the Rust reader answers a case, built from the primitives a caller has
// here. One dictionary block per term, one postings read per term whose run
// is stored, and the payload blocks of what matched.
func termQuery(t *testing.T, index *Index, terms []string) ([]int, []string) {
	t.Helper()
	ctx := context.Background()
	var runs []Postings
	for _, term := range terms {
		entry, err := index.Exact(ctx, term)
		if err != nil {
			t.Fatal(err)
		}
		if entry == nil {
			return []int{}, []string{}
		}
		run, err := index.PostingsSpan(ctx, []TermEntry{*entry})
		if err != nil {
			t.Fatal(err)
		}
		runs = append(runs, run[0])
	}
	docs := []int{}
	for _, doc := range runs[0].Docs {
		if !slices.ContainsFunc(runs[1:], func(p Postings) bool { return !slices.Contains(p.Docs, doc) }) {
			docs = append(docs, doc)
		}
	}
	payloads, err := index.PayloadSpan(ctx, docs)
	if err != nil {
		t.Fatal(err)
	}
	return docs, payloads
}

func orEmpty[T any](s []T) []T {
	if s == nil {
		return []T{}
	}
	return s
}

func TestEveryCaseWithinItsReads(t *testing.T) {
	ctx := context.Background()
	for _, c := range loadSuite(t).Case {
		t.Run(c.Name, func(t *testing.T) {
			index := openUncoalesced(t, c.Fixture)
			docs, payloads := termQuery(t, index, c.Query)
			if !slices.Equal(docs, orEmpty(c.Docs)) || !slices.Equal(payloads, orEmpty(c.Payloads)) {
				t.Fatalf("got %v %q, want %v %q", docs, payloads, c.Docs, c.Payloads)
			}

			// Resident, so asking for it must not cost a read, which is
			// checked by reading it before the count is taken.
			if c.Attribute != "" {
				column := index.Column(c.Attribute)
				if column == nil {
					t.Fatalf("no column %q", c.Attribute)
				}
				var values []int64
				for _, d := range docs {
					values = append(values, column.Values[d])
				}
				if !slices.Equal(values, c.Values) {
					t.Fatalf("values %v, want %v", values, c.Values)
				}
			}

			reads := index.Stats().Requests
			if reads != c.Reads {
				t.Fatalf("%d reads, want %d", reads, c.Reads)
			}
			// The bound is what the specification promises, and the count
			// could otherwise be updated to match a regression.
			if bound := 2*len(c.Query) + len(c.Docs); reads > bound {
				t.Fatalf("%d reads exceed the bound %d", reads, bound)
			}

			// Positions are the one section a reader pays for only when it
			// asks, and asking costs exactly one more read.
			if c.Positions != nil {
				entry, err := index.Exact(ctx, c.Query[0])
				if err != nil || entry == nil {
					t.Fatalf("term is absent: %v", err)
				}
				postings, err := index.PostingsSpan(ctx, []TermEntry{*entry})
				if err != nil {
					t.Fatal(err)
				}
				before := index.Stats().Requests
				positions, err := index.Positions(ctx, entry, postings[0])
				if err != nil {
					t.Fatal(err)
				}
				if !slices.EqualFunc(positions, c.Positions, slices.Equal) {
					t.Fatalf("positions %v, want %v", positions, c.Positions)
				}
				if n := index.Stats().Requests - before; n != 1 {
					t.Fatalf("positions cost %d reads, not one", n)
				}
			}
		})
	}
}

// The term as a prefix, filtered to the term itself, through the index's
// tokenizer: what the TypeScript tests run.
func TestEverySingleTermCaseAsAPrefix(t *testing.T) {
	for _, c := range loadSuite(t).Case {
		if len(c.Query) != 1 {
			continue
		}
		t.Run(c.Name, func(t *testing.T) {
			index := openUncoalesced(t, c.Fixture)
			found, err := index.Lookup(context.Background(), c.Query[0], 100)
			if err != nil {
				t.Fatal(err)
			}
			docs, payloads := []int{}, []string{}
			for _, r := range found.Results {
				if r.Term == index.Normalize(c.Query[0]) {
					docs = append(docs, r.Doc)
					payloads = append(payloads, r.Payload)
				}
			}
			if !slices.Equal(docs, orEmpty(c.Docs)) || !slices.Equal(payloads, orEmpty(c.Payloads)) {
				t.Fatalf("got %v %q, want %v %q", docs, payloads, c.Docs, c.Payloads)
			}
		})
	}
}

func TestEveryCompletionFromWhereTheCaseSays(t *testing.T) {
	for _, c := range loadSuite(t).Completion {
		t.Run(c.Name, func(t *testing.T) {
			index := openUncoalesced(t, c.Fixture)
			done, err := index.Complete(context.Background(), c.Prefix, c.Limit)
			if err != nil {
				t.Fatal(err)
			}
			if !slices.Equal(done.Docs, orEmpty(c.Docs)) || !slices.Equal(done.Payloads, orEmpty(c.Payloads)) {
				t.Fatalf("got %v %q, want %v %q", done.Docs, done.Payloads, c.Docs, c.Payloads)
			}
			if done.FromTable != c.FromTable {
				t.Fatalf("from the table: %v, want %v", done.FromTable, c.FromTable)
			}
			if done.FromTable && done.RequestsBeforePayload != 0 {
				t.Fatalf("an entry cost %d reads", done.RequestsBeforePayload)
			}
			if reads := index.Stats().Requests; reads != c.Reads {
				t.Fatalf("%d reads, want %d", reads, c.Reads)
			}
		})
	}
}

// fixtureSources opens a manifest's segments from the fixtures in memory.
func fixtureSources(t *testing.T) func(context.Context, string) (Source, error) {
	return func(_ context.Context, location string) (Source, error) {
		return BytesSource(fixtureBytes(t, strings.TrimSuffix(location, ".mokuji"))), nil
	}
}

func TestEveryCollectionCase(t *testing.T) {
	for _, c := range loadSuite(t).Collection {
		t.Run(c.Name, func(t *testing.T) {
			data, err := os.ReadFile(filepath.Join(fixtures, c.Manifest+".json"))
			if err != nil {
				t.Fatal(err)
			}
			var manifest Manifest
			if err := json.Unmarshal(data, &manifest); err != nil {
				t.Fatal(err)
			}
			collection, err := NewCollection(context.Background(), &manifest, "", &CollectionOptions{
				Countries: c.Countries,
				SourceFor: fixtureSources(t),
			})
			if err != nil {
				t.Fatal(err)
			}
			for _, segment := range collection.Segments {
				segment.Coalesce = false
			}
			collection.ResetStats()
			done, err := collection.Complete(context.Background(), c.Prefix, c.Limit)
			if err != nil {
				t.Fatal(err)
			}
			segments, docs := []int{}, []int{}
			for _, h := range done.Hits {
				segments = append(segments, h.Segment)
				docs = append(docs, h.Doc)
			}
			if !slices.Equal(segments, orEmpty(c.Segments)) || !slices.Equal(docs, orEmpty(c.Docs)) ||
				!slices.Equal(done.Payloads, orEmpty(c.Payloads)) {
				t.Fatalf("got %v %v %q, want %v %v %q", segments, docs, done.Payloads, c.Segments, c.Docs, c.Payloads)
			}
			if reads := collection.Stats().Requests; reads != c.Reads {
				t.Fatalf("%d reads, want %d", reads, c.Reads)
			}
		})
	}
}

func TestCoalescingNeverCostsMoreThanTheCasesSay(t *testing.T) {
	s := loadSuite(t)
	fewer := 0
	for _, c := range s.Case {
		index := openFixture(t, c.Fixture)
		index.ResetStats()
		docs, _ := termQuery(t, index, c.Query)
		if !slices.Equal(docs, orEmpty(c.Docs)) {
			t.Fatalf("%s: got %v, want %v", c.Name, docs, c.Docs)
		}
		reads := index.Stats().Requests
		if reads > c.Reads {
			t.Fatalf("%s: %d reads, more than %d", c.Name, reads, c.Reads)
		}
		if reads < c.Reads {
			fewer++
		}
	}
	for _, c := range s.Completion {
		index := openFixture(t, c.Fixture)
		index.ResetStats()
		done, err := index.Complete(context.Background(), c.Prefix, c.Limit)
		if err != nil {
			t.Fatal(err)
		}
		if !slices.Equal(done.Payloads, orEmpty(c.Payloads)) {
			t.Fatalf("%s: got %q, want %q", c.Name, done.Payloads, c.Payloads)
		}
		reads := index.Stats().Requests
		if reads > c.Reads {
			t.Fatalf("%s: %d reads, more than %d", c.Name, reads, c.Reads)
		}
		if reads < c.Reads {
			fewer++
		}
	}
	// Payload blocks next to one another are common in the fixtures, so some
	// case has to come out cheaper, or coalescing is not happening.
	if fewer == 0 {
		t.Fatal("no case came out cheaper with coalescing")
	}
}
