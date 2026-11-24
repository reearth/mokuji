package mokuji

import (
	"context"
	"fmt"
	"math"
	"slices"
	"sort"
)

// LookupResult is one document a looked-up term names.
type LookupResult struct {
	Term    string
	Doc     int
	Payload string
}

// Lookup is what Index.Lookup found: the query as normalized, and every
// match.
type Lookup struct {
	Normalized string
	Results    []LookupResult
}

// Completion is what Index.Complete found, and how.
type Completion struct {
	Normalized string
	// Whether the completions section answered, at no read before the
	// payload.
	FromTable bool
	Docs      []int
	Payloads  []string
	// The requests this completion made before it read the payload.
	RequestsBeforePayload int
}

// Probed is a term whose postings Search read, with how many documents it
// names.
type Probed struct {
	Term    string
	DocFreq uint64
}

// Ranked is one document Search returned.
type Ranked struct {
	Doc int
	// The BM25 score, when SearchResult.Scored.
	Score   float64
	Payload string
}

// SearchResult is what Search found, and what deciding it took.
type SearchResult struct {
	// The terms the query became.
	Terms []string
	// The terms whose postings were read.
	Probed []Probed
	// Terms present but not read, because the probe left them out.
	Skipped int
	// How many documents matched.
	Matched int
	// Whether the results carry scores. An index without document lengths
	// cannot be ranked, and its results come in document order.
	Scored bool
	// The first `limit` matches, best first.
	Results []Ranked
}

type searchOptions struct {
	limit      int
	any        bool
	probe      int
	lengthNorm float64
	boost      string
	filter     func(doc int) bool
}

// SearchOption changes how Search matches and ranks.
type SearchOption func(*searchOptions)

// Limit returns at most n results; ten without it.
func Limit(n int) SearchOption { return func(o *searchOptions) { o.limit = n } }

// Any takes the union of the terms rather than their intersection: what an
// n-gram query wants, since a misspelling shares every gram but two.
func Any() SearchOption { return func(o *searchOptions) { o.any = true } }

// Probe fetches postings for only n of the terms, n zero or more: the
// rarest, which the dictionary says without reading any postings.
func Probe(n int) SearchOption { return func(o *searchOptions) { o.probe = n } }

// LengthNorm sets BM25's b, how much a long document is discounted; 0.75
// without it.
func LengthNorm(b float64) SearchOption { return func(o *searchOptions) { o.lengthNorm = b } }

// Boost names an attribute column added to each score as ln(1 + value), a
// static prior.
func Boost(column string) SearchOption { return func(o *searchOptions) { o.boost = column } }

// Filter keeps only the documents keep accepts, before they are ranked and
// before any payload is read. Matched then counts what it kept.
//
// What a caller would otherwise do by asking for more results and discarding
// some, which costs the payload of every one discarded. A filter costs no
// request: it sees document IDs, so it can ask anything the caller holds
// resident about a document — an ID range, when the corpus was numbered by
// kind, or an attribute column.
func Filter(keep func(doc int) bool) SearchOption {
	return func(o *searchOptions) { o.filter = keep }
}

// Search ranks documents by BM25 over the terms the query becomes.
//
// The same ranking as `mokuji search` and the other readers:
// Robertson–Spärck Jones idf, k1 of 1.2, b from LengthNorm, a union scored
// over the terms a document actually has, and ties broken by document ID.
// The scores agree with theirs to about 1e-12, not to the last bit, since
// logarithms differ in the last place between platforms.
//
// One request per run of dictionary blocks the terms fall in, then one per
// term whose postings are stored as a run, then the payload of the first
// `limit`.
func (ix *Index) Search(ctx context.Context, query string, options ...SearchOption) (*SearchResult, error) {
	o := searchOptions{limit: 10, probe: -1, lengthNorm: 0.75}
	for _, option := range options {
		option(&o)
	}
	terms := ix.Split(query)
	var column *Column
	if o.boost != "" {
		if column = ix.Column(o.boost); column == nil {
			return nil, fmt.Errorf("mokuji: this index carries no attribute column called %q", o.boost)
		}
	}
	result := &SearchResult{Terms: terms, Probed: []Probed{}, Results: []Ranked{}}
	if len(terms) == 0 {
		return result, nil
	}

	entries, err := ix.ExactMany(ctx, terms)
	if err != nil {
		return nil, err
	}
	type found struct {
		term  string
		entry *TermEntry
	}
	var present []found
	for i, entry := range entries {
		if entry != nil {
			present = append(present, found{terms[i], entry})
		} else if !o.any {
			// A term nobody wrote down cannot be in an intersection.
			return result, nil
		}
	}
	if len(present) == 0 {
		return result, nil
	}

	// Rarest first: a term matching few documents says the most about which
	// ones, and costs the least to say it. Stable, as the CLI's sort is.
	if o.probe >= 0 {
		sort.SliceStable(present, func(i, j int) bool { return present[i].entry.DocFreq < present[j].entry.DocFreq })
		result.Skipped = max(0, len(present)-o.probe)
		present = present[:min(o.probe, len(present))]
	}
	for _, p := range present {
		result.Probed = append(result.Probed, Probed{p.term, p.entry.DocFreq})
	}
	if len(present) == 0 {
		// Every term was left out, so there is nothing to match against.
		return result, nil
	}
	runs := make([]Postings, len(present))
	for i, p := range present {
		run, err := ix.PostingsSpan(ctx, []TermEntry{*p.entry})
		if err != nil {
			return nil, err
		}
		runs[i] = run[0]
	}

	var matched []int
	if o.any {
		for _, run := range runs {
			matched = append(matched, run.Docs...)
		}
		slices.Sort(matched)
		matched = slices.Compact(matched)
	} else {
		rest := make([]map[int]bool, len(runs)-1)
		for i, run := range runs[1:] {
			rest[i] = make(map[int]bool, len(run.Docs))
			for _, doc := range run.Docs {
				rest[i][doc] = true
			}
		}
	next:
		for _, doc := range runs[0].Docs {
			for _, s := range rest {
				if !s[doc] {
					continue next
				}
			}
			matched = append(matched, doc)
		}
	}

	if o.filter != nil {
		matched = slices.DeleteFunc(matched, func(doc int) bool { return !o.filter(doc) })
	}
	result.Matched = len(matched)
	ranked := make([]Ranked, len(matched))
	for i, doc := range matched {
		ranked[i].Doc = doc
	}
	if ix.DocLengths != nil {
		result.Scored = true
		n := float64(ix.NumDocs)
		avgdl := 1.0
		if ix.NumDocs > 0 {
			avgdl = max(float64(ix.SumDocLen)/n, 1)
		}
		const k1 = 1.2
		b := o.lengthNorm
		at := make([]map[int]uint64, len(runs))
		for i, run := range runs {
			at[i] = make(map[int]uint64, len(run.Docs))
			for j, doc := range run.Docs {
				at[i][doc] = run.Freqs[j]
			}
		}
		for i := range ranked {
			doc := ranked[i].Doc
			var dl float64
			if doc < len(ix.DocLengths) {
				dl = float64(ix.DocLengths[doc])
			}
			score := 0.0
			// The same operations in the same order as the other readers and
			// the CLI, so that the scores agree as closely as logarithms let
			// them.
			for j, p := range present {
				freq, ok := at[j][doc]
				if !ok {
					continue
				}
				tf := float64(freq)
				df := float64(p.entry.DocFreq)
				idf := math.Log((n-df+0.5)/(df+0.5) + 1)
				score += (idf * (tf * (k1 + 1))) / (tf + k1*(1-b+(b*dl)/avgdl))
			}
			if column != nil {
				var value int64
				if doc < len(column.Values) {
					value = column.Values[doc]
				}
				score += math.Log(1 + float64(max(value, 0)))
			}
			ranked[i].Score = score
		}
		sort.SliceStable(ranked, func(i, j int) bool {
			if ranked[i].Score != ranked[j].Score {
				return ranked[i].Score > ranked[j].Score
			}
			return ranked[i].Doc < ranked[j].Doc
		})
	}

	shown := ranked[:min(max(o.limit, 0), len(ranked))]
	wanted := make([]int, len(shown))
	for i, r := range shown {
		wanted[i] = r.Doc
	}
	slices.Sort(wanted)
	payloads, err := ix.PayloadSpan(ctx, wanted)
	if err != nil {
		return nil, err
	}
	byDoc := make(map[int]string, len(wanted))
	for i, doc := range wanted {
		byDoc[doc] = payloads[i]
	}
	for i := range shown {
		shown[i].Payload = byDoc[shown[i].Doc]
	}
	result.Results = shown
	return result, nil
}

// Complete is the first `limit` documents for what has been typed.
//
// From the completions section when the prefix has an entry that reaches the
// limit, or lists fewer than it may and so lists them all, which costs no
// request before the payload. Otherwise every term the prefix matches, whose
// runs are united and returned in document order.
func (ix *Index) Complete(ctx context.Context, typed string, limit int) (*Completion, error) {
	ctx, tally := withTally(ctx)
	normalized := ix.Normalize(typed)
	docs, fromTable, err := ix.CompleteDocs(ctx, normalized, limit)
	if err != nil {
		return nil, err
	}
	before := int(tally.Load())
	payloads, err := ix.PayloadSpan(ctx, docs)
	if err != nil {
		return nil, err
	}
	return &Completion{normalized, fromTable, docs, payloads, before}, nil
}

// CompleteDocs is Complete for a prefix already normalized, without the
// payload: the documents, and whether the completions section gave them.
func (ix *Index) CompleteDocs(ctx context.Context, normalized string, limit int) ([]int, bool, error) {
	limit = max(limit, 0)
	if ix.Completions != nil {
		listed, ok := ix.Completions.Entries[normalized]
		if ok && (limit <= len(listed) || uint64(len(listed)) < ix.Completions.DocsPerEntry) {
			return slices.Clone(listed[:min(limit, len(listed))]), true, nil
		}
	}
	entries, err := ix.Prefix(ctx, normalized, 0)
	if err != nil {
		return nil, false, err
	}
	postings, err := ix.PostingsSpan(ctx, entries)
	if err != nil {
		return nil, false, err
	}
	docs := unionOf(postings)
	return docs[:min(limit, len(docs))], false, nil
}

// unionOf is every document the postings name, once each, ascending.
func unionOf(postings []Postings) []int {
	docs := []int{}
	for _, p := range postings {
		docs = append(docs, p.Docs...)
	}
	slices.Sort(docs)
	return slices.Compact(docs)
}

// Lookup completes a prefix, returning every matching document with its
// payload. `limit` is how many terms; a term may name more than one
// document.
//
// The query goes through the index's own tokenizer first. Without that, an
// index built from `一丁目` is unreachable by anyone typing `一丁目`.
func (ix *Index) Lookup(ctx context.Context, prefix string, limit int) (*Lookup, error) {
	normalized := ix.Normalize(prefix)
	entries, err := ix.Prefix(ctx, normalized, limit)
	if err != nil {
		return nil, err
	}
	postings, err := ix.PostingsSpan(ctx, entries)
	if err != nil {
		return nil, err
	}
	// A term may name more than one document. In a dictionary it never does,
	// but returning only the first would drop results silently elsewhere.
	// Documents are fetched in ascending order so that the payload reads
	// share blocks.
	wanted := unionOf(postings)
	payloads, err := ix.PayloadSpan(ctx, wanted)
	if err != nil {
		return nil, err
	}
	byDoc := make(map[int]string, len(wanted))
	for i, doc := range wanted {
		byDoc[doc] = payloads[i]
	}
	results := []LookupResult{}
	for i, entry := range entries {
		term := lossy(entry.Term)
		for _, doc := range postings[i].Docs {
			results = append(results, LookupResult{term, doc, byDoc[doc]})
		}
	}
	return &Lookup{normalized, results}, nil
}
