package mokuji

import (
	"context"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"reflect"
	"slices"
)

// Manifest is a collection's manifest (MANIFEST.md §2). Keys it does not
// name are ignored, as a reader must.
type Manifest struct {
	MokujiManifest string            `json:"mokuji_manifest"`
	Generation     int64             `json:"generation"`
	Segments       []ManifestSegment `json:"segments"`
}

// ManifestSegment is one file of a collection.
type ManifestSegment struct {
	// A URI reference, resolved against the manifest's own location.
	Path        string  `json:"path"`
	ContentHash string  `json:"content_hash"`
	Length      int64   `json:"length"`
	Deletes     []int   `json:"deletes,omitempty"`
	Region      *Region `json:"region,omitempty"`
}

// Region is the countries every document of a segment lies within
// (MANIFEST.md §7).
type Region struct {
	Countries []string `json:"countries"`
}

// Hit is one document of a collection: its segment, and its ID there.
type Hit struct {
	Segment int
	Doc     int
}

// Consulted is a segment a completion asked, and whether its completions
// section answered.
type Consulted struct {
	Segment   int
	FromTable bool
}

// CollectionCompletion is what a collection's completion found, and how.
type CollectionCompletion struct {
	Normalized string
	Hits       []Hit
	Consulted  []Consulted
	Payloads   []string
	// The requests this completion made, across every segment, before it
	// read the payload.
	RequestsBeforePayload int
}

// CollectionStats is what queries have cost across a collection's segments.
type CollectionStats struct {
	Requests int
	Bytes    int64
	// The most requests any one segment made: the round trips, when the
	// segments are asked at once.
	RoundTrips int
}

// CollectionOptions changes how OpenCollection opens a collection.
type CollectionOptions struct {
	// Countries, when not nil, opens only the segments whose region includes
	// one of them, and those that state no region (MANIFEST.md §7). Segment
	// numbers are then positions among those.
	Countries []string
	// SourceFor supplies a segment's source, given its location resolved
	// against the manifest's, in place of the reader opening a path or a URL
	// itself. The collection does not close what it returns.
	SourceFor func(ctx context.Context, location string) (Source, error)
}

// Collection is segments in a manifest's order, less their deletes
// (MANIFEST.md). Every segment is an ordinary Mokuji file, opened on its own.
type Collection struct {
	Manifest *Manifest
	Segments []*Index
	// How many segments the manifest lists, before a region left some out.
	ManifestSegments int

	deletes []map[int]bool
}

// OpenCollection opens the manifest at location, a path or an http(s) URL,
// and every segment it names.
func OpenCollection(ctx context.Context, location string, options *CollectionOptions) (*Collection, error) {
	var data []byte
	var err error
	if isURL(location) {
		data, err = fetch(ctx, location)
	} else {
		data, err = os.ReadFile(location)
	}
	if err != nil {
		return nil, err
	}
	manifest, err := ParseManifest(data)
	if err != nil {
		return nil, fmt.Errorf("mokuji: %s is not a manifest: %w", location, err)
	}
	return NewCollection(ctx, manifest, location, options)
}

func fetch(ctx context.Context, location string) ([]byte, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, location, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", "mokuji-go")
	// The manifest is the one object that changes; a stale copy names files
	// that may be gone.
	req.Header.Set("Cache-Control", "no-cache")
	resp, err := http.DefaultClient.Do(req)
	if err != nil {
		return nil, err
	}
	defer func() { _ = resp.Body.Close() }()
	if resp.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("mokuji: %s answered %s", location, resp.Status)
	}
	return io.ReadAll(resp.Body)
}

// NewCollection opens every segment a manifest already parsed names. Their
// paths are resolved against base, the manifest's own location, a path or a
// URL; an empty base leaves them as they are.
func NewCollection(ctx context.Context, manifest *Manifest, base string, options *CollectionOptions) (*Collection, error) {
	if options == nil {
		options = &CollectionOptions{}
	}
	// The checks ParseManifest makes, for a manifest that did not come
	// through it.
	if err := manifest.check(); err != nil {
		return nil, err
	}
	listed := len(manifest.Segments)
	if options.Countries != nil {
		manifest = manifest.Within(options.Countries)
	}

	c := &Collection{Manifest: manifest, ManifestSegments: listed}
	ok := false
	defer func() {
		if !ok {
			_ = c.Close()
		}
	}()
	for _, s := range manifest.Segments {
		location, err := resolve(base, s.Path)
		if err != nil {
			return nil, err
		}
		var index *Index
		if options.SourceFor != nil {
			src, err := options.SourceFor(ctx, location)
			if err != nil {
				return nil, err
			}
			index, err = Open(ctx, src)
			if err != nil {
				return nil, err
			}
		} else if index, err = openLocation(ctx, location); err != nil {
			return nil, err
		}
		c.Segments = append(c.Segments, index)
		// A reader MUST reject a segment whose length differs from the
		// manifest's; one that cannot say its length is taken on trust.
		if n, known := index.FileLength(); known && n != s.Length {
			return nil, malformed("%s is %d bytes, not the %d the manifest says", s.Path, n, s.Length)
		}
		deleted := make(map[int]bool, len(s.Deletes))
		for i, d := range s.Deletes {
			if d < 0 || uint64(d) >= index.NumDocs || (i > 0 && d <= s.Deletes[i-1]) {
				return nil, malformed("%s has deletes that are out of order or out of range", s.Path)
			}
			deleted[d] = true
		}
		c.deletes = append(c.deletes, deleted)
	}
	// A query has to mean the same thing in every segment.
	if len(c.Segments) > 0 {
		tokenizer := field(c.Segments[0].Meta, "tokenizer")
		for i, index := range c.Segments {
			if !reflect.DeepEqual(field(index.Meta, "tokenizer"), tokenizer) {
				return nil, malformed("%s was tokenized differently from the first segment", manifest.Segments[i].Path)
			}
		}
	}
	ok = true
	return c, nil
}

// resolve is where a segment is, given the manifest's location
// (MANIFEST.md §2).
func resolve(base, path string) (string, error) {
	if base == "" {
		return path, nil
	}
	if isURL(base) {
		b, err := url.Parse(base)
		if err != nil {
			return "", err
		}
		p, err := url.Parse(path)
		if err != nil {
			return "", err
		}
		return b.ResolveReference(p).String(), nil
	}
	if isURL(path) || filepath.IsAbs(path) {
		return path, nil
	}
	return filepath.Join(filepath.Dir(base), filepath.FromSlash(path)), nil
}

// Close closes the segments the collection opened itself.
func (c *Collection) Close() error {
	var first error
	for _, index := range c.Segments {
		if err := index.Close(); err != nil && first == nil {
			first = err
		}
	}
	return first
}

// ResetStats starts every segment's count again.
func (c *Collection) ResetStats() {
	for _, index := range c.Segments {
		index.ResetStats()
	}
}

// Stats is requests and bytes, summed; and the most requests any one segment
// made.
func (c *Collection) Stats() CollectionStats {
	var out CollectionStats
	for _, index := range c.Segments {
		s := index.Stats()
		out.Requests += s.Requests
		out.Bytes += s.Bytes
		out.RoundTrips = max(out.RoundTrips, s.Requests)
	}
	return out
}

// Normalize is what a query goes through, the same in every segment.
func (c *Collection) Normalize(typed string) string {
	if len(c.Segments) == 0 {
		return typed
	}
	return c.Segments[0].Normalize(typed)
}

// Attributions is each segment's credit, once each, for a caller that shows
// results.
func (c *Collection) Attributions() []string {
	out := []string{}
	for _, index := range c.Segments {
		if index.Attribution != "" && !slices.Contains(out, index.Attribution) {
			out = append(out, index.Attribution)
		}
	}
	return out
}

// LiveIn is one segment's first `need` live documents for a prefix already
// normalized, and whether its completions section answered: the entry, then
// all it lists, then as many again as the segment has deletes (MANIFEST.md
// §3.2). `at` is the segment's position in the collection.
func (c *Collection) LiveIn(ctx context.Context, at int, normalized string, need int) ([]int, bool, error) {
	if at < 0 || at >= len(c.Segments) {
		return nil, false, noSegment(at)
	}
	return c.liveIn(ctx, at, normalized, need)
}

func (c *Collection) liveIn(ctx context.Context, at int, normalized string, need int) ([]int, bool, error) {
	index := c.Segments[at]
	deleted := c.deletes[at]
	listed := 0
	if index.Completions != nil {
		listed = int(min(index.Completions.DocsPerEntry, uint64(maxDocs)))
	}
	fromTable := true
	live := []int{}
	for _, ask := range []int{need, max(listed, need), need + len(deleted)} {
		docs, table, err := index.CompleteDocs(ctx, normalized, ask)
		if err != nil {
			return nil, false, err
		}
		fromTable = fromTable && table
		live = live[:0]
		for _, doc := range docs {
			if !deleted[doc] {
				live = append(live, doc)
			}
		}
		if len(live) >= need || len(docs) < ask {
			break
		}
	}
	return live[:min(need, len(live))], fromTable, nil
}

// Complete is the first `limit` live documents, in collection order.
func (c *Collection) Complete(ctx context.Context, typed string, limit int) (*CollectionCompletion, error) {
	ctx, tally := withTally(ctx)
	out := &CollectionCompletion{Normalized: c.Normalize(typed), Hits: []Hit{}, Consulted: []Consulted{}}
	for at := range c.Segments {
		if len(out.Hits) >= limit {
			break
		}
		live, fromTable, err := c.liveIn(ctx, at, out.Normalized, limit-len(out.Hits))
		if err != nil {
			return nil, err
		}
		out.Consulted = append(out.Consulted, Consulted{at, fromTable})
		for _, doc := range live {
			out.Hits = append(out.Hits, Hit{at, doc})
		}
	}
	out.RequestsBeforePayload = int(tally.Load())
	var err error
	if out.Payloads, err = c.Payloads(ctx, out.Hits); err != nil {
		return nil, err
	}
	return out, nil
}

// Candidates is every segment's own first `limit`, for a caller that ranks
// across segments by a score of its own (MANIFEST.md §3.3).
func (c *Collection) Candidates(ctx context.Context, typed string, limit int) (*CollectionCompletion, error) {
	ctx, tally := withTally(ctx)
	out := &CollectionCompletion{Normalized: c.Normalize(typed), Hits: []Hit{}, Consulted: []Consulted{}}
	for at := range c.Segments {
		live, fromTable, err := c.liveIn(ctx, at, out.Normalized, limit)
		if err != nil {
			return nil, err
		}
		out.Consulted = append(out.Consulted, Consulted{at, fromTable})
		for _, doc := range live {
			out.Hits = append(out.Hits, Hit{at, doc})
		}
	}
	out.RequestsBeforePayload = int(tally.Load())
	var err error
	if out.Payloads, err = c.Payloads(ctx, out.Hits); err != nil {
		return nil, err
	}
	return out, nil
}

// Payloads is the payload of each hit, one span per segment, in the hits'
// order.
func (c *Collection) Payloads(ctx context.Context, hits []Hit) ([]string, error) {
	for _, h := range hits {
		if h.Segment < 0 || h.Segment >= len(c.Segments) {
			return nil, noSegment(h.Segment)
		}
	}
	out := make([]string, len(hits))
	for at, index := range c.Segments {
		var mine []int
		for i, h := range hits {
			if h.Segment == at {
				mine = append(mine, i)
			}
		}
		if len(mine) == 0 {
			continue
		}
		slices.SortStableFunc(mine, func(a, b int) int { return hits[a].Doc - hits[b].Doc })
		docs := make([]int, len(mine))
		for j, i := range mine {
			docs[j] = hits[i].Doc
		}
		payloads, err := index.PayloadSpan(ctx, docs)
		if err != nil {
			return nil, err
		}
		for j, i := range mine {
			out[i] = payloads[j]
		}
	}
	return out, nil
}

func noSegment(at int) error {
	return fmt.Errorf("mokuji: the collection has no segment %d", at)
}

// CollectionSearch is what a collection's search found.
type CollectionSearch struct {
	// The terms the query became.
	Terms []string
	// How many live documents hold every term, across the collection.
	Matched int
	// The first `limit` of them, in collection order, and their payloads.
	Hits     []Hit
	Payloads []string
}

// Search is the live documents holding every term the query becomes, in
// collection order: segments in the manifest's order, documents in each by
// ID. The query is split by the collection's tokenizer, as Index.Search
// splits it.
//
// Unranked. Scores from different segments are not comparable, since each
// segment's statistics are its own (MANIFEST.md §3.3), so a caller that ranks
// across them does so by a score of its own. Each segment is asked on its
// own, so the cost is the sum of what each costs.
func (c *Collection) Search(ctx context.Context, query string, limit int) (*CollectionSearch, error) {
	terms := []string{query}
	if len(c.Segments) > 0 {
		terms = c.Segments[0].Split(query)
	}
	out := &CollectionSearch{Terms: terms, Hits: []Hit{}}
	var all []Hit
	for at, index := range c.Segments {
		docs, err := intersection(ctx, index, terms)
		if err != nil {
			return nil, err
		}
		for _, doc := range docs {
			if !c.deletes[at][doc] {
				all = append(all, Hit{at, doc})
			}
		}
	}
	out.Matched = len(all)
	out.Hits = append(out.Hits, all[:min(max(limit, 0), len(all))]...)
	var err error
	if out.Payloads, err = c.Payloads(ctx, out.Hits); err != nil {
		return nil, err
	}
	return out, nil
}

// intersection is the documents of one file holding every term, ascending.
// The terms are looked up one at a time, in the order typed, as the Rust
// reader does, so that a term the file lacks, or an intersection that has
// become empty, ends it before the rest cost anything.
func intersection(ctx context.Context, index *Index, terms []string) ([]int, error) {
	var matched []int
	for i, term := range terms {
		entry, err := index.Exact(ctx, term)
		if err != nil {
			return nil, err
		}
		if entry == nil {
			return nil, nil
		}
		run, err := index.PostingsSpan(ctx, []TermEntry{*entry})
		if err != nil {
			return nil, err
		}
		if i == 0 {
			matched = run[0].Docs
		} else {
			matched = slices.DeleteFunc(slices.Clone(matched), func(doc int) bool {
				_, ok := slices.BinarySearch(run[0].Docs, doc)
				return !ok
			})
		}
		if len(matched) == 0 {
			return nil, nil
		}
	}
	return matched, nil
}
