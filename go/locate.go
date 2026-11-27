package mokuji

import (
	"context"
	"errors"
	"math"
	"regexp"
	"slices"
	"strconv"
	"strings"
)

// Questions that take more than one lookup, answered from the primitives of
// Index: a prefix that falls back to the longest term a query begins with,
// and the polygons holding a point, from a file of geohash coverings. The
// same as js/src/query.ts.

// PrefixMatch is how a hit of LookupPrefix relates to what was asked.
type PrefixMatch string

const (
	// The term asked for.
	MatchExact PrefixMatch = "exact"
	// A term that begins with what was asked.
	MatchPrefix PrefixMatch = "prefix"
	// The longest term a reading begins with: what was asked lies within it.
	MatchWithin PrefixMatch = "within"
)

// PrefixHit is one document LookupPrefix found.
type PrefixHit struct {
	Term    string
	Doc     int
	Payload string
	Match   PrefixMatch
}

// LookupPrefix is the terms that begin with normalized, as many as limit
// documents, or, when none does, the longest term that one of readings
// begins with.
//
// The fallback is for keys that a query runs past: an address typed with its
// block and lot after the town, when the file keys towns, finds the town.
// Every prefix of every reading, from two characters to one fewer than the
// reading, is looked up at once; prefixes of one string fall in one or two
// dictionary blocks, read together. normalized and readings are already
// normalized, as the file's tokenizer does (Index.Normalize).
func LookupPrefix(ctx context.Context, ix *Index, normalized string, limit int, readings []string) ([]PrefixHit, error) {
	if limit <= 0 {
		return []PrefixHit{}, nil
	}
	entries, err := ix.Prefix(ctx, normalized, limit)
	if err != nil {
		return nil, err
	}
	within := false
	if len(entries) == 0 && len(readings) > 0 {
		var prefixes []string
		seen := map[string]bool{}
		for _, reading := range readings {
			chars := []rune(reading)
			for n := 2; n < len(chars); n++ {
				if p := string(chars[:n]); !seen[p] {
					seen[p] = true
					prefixes = append(prefixes, p)
				}
			}
		}
		found, err := ix.ExactMany(ctx, prefixes)
		if err != nil {
			return nil, err
		}
		longest := -1
		for i, entry := range found {
			if entry != nil && (longest < 0 || len([]rune(prefixes[i])) > len([]rune(prefixes[longest]))) {
				longest = i
			}
		}
		if longest >= 0 {
			entries = []TermEntry{*found[longest]}
			within = true
		}
	}
	postings, err := ix.PostingsSpan(ctx, entries)
	if err != nil {
		return nil, err
	}
	hits := []PrefixHit{}
	for i, entry := range entries {
		term := lossy(entry.Term)
		match := MatchPrefix
		if within {
			match = MatchWithin
		} else if term == normalized {
			match = MatchExact
		}
		for _, doc := range postings[i].Docs {
			hits = append(hits, PrefixHit{Term: term, Doc: doc, Match: match})
		}
	}
	hits = hits[:min(max(limit, 0), len(hits))]
	docs := make([]int, len(hits))
	for i, hit := range hits {
		docs[i] = hit.Doc
	}
	payloads, err := ix.PayloadSpan(ctx, docs)
	if err != nil {
		return nil, err
	}
	for i := range hits {
		hits[i].Payload = payloads[i]
	}
	return hits, nil
}

// --- geohash coverings -------------------------------------------------------
//
// A file in the shape demo/reverse builds: each polygon is a document, keyed
// by the geohash cells that cover it, coarse in its interior and fine along
// its boundary, with its label and rings as payload (`label|rings`). The
// format has no opinion about geohashes; the file says how it was built in
// its application-defined `attrs` (SPEC.md §6), and this reads them.

const geohashBase32 = "0123456789bcdefghjkmnpqrstuvwxyz"

var (
	// ErrNotCoverings is Locate asked of a file that does not say its
	// geohash precisions and geometry scale, by a caller that did not say
	// them either.
	ErrNotCoverings = errors.New("mokuji: the file does not say its geohash precisions and geometry scale")
	// ErrRings is a payload whose rings are not the integers
	// `geometry=delta-int-1e…` writes.
	ErrRings = errors.New("mokuji: rings are not integers")
)

// Geohash is the geohash cell of precision characters holding (lat, lon).
func Geohash(lat, lon float64, precision int) string {
	lonLo, lonHi, latLo, latHi := -180.0, 180.0, -90.0, 90.0
	out := make([]byte, 0, max(precision, 0))
	code := 0
	for i := 0; i < precision*5; i++ {
		if i%2 == 0 {
			mid := (lonLo + lonHi) / 2
			code *= 2
			if lon >= mid {
				code++
				lonLo = mid
			} else {
				lonHi = mid
			}
		} else {
			mid := (latLo + latHi) / 2
			code *= 2
			if lat >= mid {
				code++
				latLo = mid
			} else {
				latHi = mid
			}
		}
		if i%5 == 4 {
			out = append(out, geohashBase32[code])
			code = 0
		}
	}
	return string(out)
}

func attr(ix *Index, name string) (string, bool) {
	text, ok := field(field(ix.Meta, "attrs"), name).(string)
	return text, ok
}

// PrecisionsOf is the geohash precisions the file is keyed at
// (`geohash-precisions`, such as `5,6,7`), or nil when it does not say.
func PrecisionsOf(ix *Index) []int {
	declared, ok := attr(ix, "geohash-precisions")
	if !ok {
		return nil
	}
	var out []int
	for _, part := range strings.Split(declared, ",") {
		p, err := strconv.Atoi(strings.TrimSpace(part))
		if err != nil || p <= 0 || p > 12 {
			return nil
		}
		out = append(out, p)
	}
	return out
}

var geometryScale = regexp.MustCompile(`^delta-int-1e(\d{1,2})$`)

// ScaleOf is the integers per degree the rings are stored in (`geometry`,
// such as `delta-int-1e5`), or zero when the file does not say.
func ScaleOf(ix *Index) float64 {
	declared, _ := attr(ix, "geometry")
	m := geometryScale.FindStringSubmatch(declared)
	if m == nil {
		return 0
	}
	exponent, _ := strconv.Atoi(m[1])
	return math.Pow10(exponent)
}

// FieldsOf is the names of the fields a label is cut into at U+001F
// (`payload-fields`), or nil when a label is one name.
func FieldsOf(ix *Index) []string {
	declared, _ := attr(ix, "payload-fields")
	if declared == "" {
		return nil
	}
	names := strings.Split(declared, ",")
	for i, name := range names {
		names[i] = strings.TrimSpace(name)
	}
	return names
}

// Rings is a polygon as [lon, lat] rings, each closed.
type Rings [][][2]float64

// DecodeRings reads rings as `geometry=delta-int-1e…` writes them: `;`
// between rings, each the first vertex then the difference from the vertex
// before, in integers of 1/scale degree, without the closing vertex, which is
// put back.
func DecodeRings(text string, scale float64) (Rings, error) {
	parts := strings.Split(text, ";")
	rings := make(Rings, len(parts))
	for r, part := range parts {
		values := strings.Split(part, ",")
		ring := [][2]float64{}
		var x, y int64
		for i := 0; i+1 < len(values); i += 2 {
			dx, err := strconv.ParseInt(strings.TrimSpace(values[i]), 10, 64)
			if err != nil {
				return nil, ErrRings
			}
			dy, err := strconv.ParseInt(strings.TrimSpace(values[i+1]), 10, 64)
			if err != nil {
				return nil, ErrRings
			}
			if dx > 0 && x > math.MaxInt64-dx || dx < 0 && x < math.MinInt64-dx ||
				dy > 0 && y > math.MaxInt64-dy || dy < 0 && y < math.MinInt64-dy {
				return nil, ErrRings
			}
			x += dx
			y += dy
			ring = append(ring, [2]float64{float64(x) / scale, float64(y) / scale})
		}
		if len(ring) > 0 {
			ring = append(ring, ring[0])
		}
		rings[r] = ring
	}
	return rings, nil
}

// Contains is whether (x, y) is inside, even-odd over every ring, so a hole
// is outside.
func Contains(rings Rings, x, y float64) bool {
	inside := false
	for _, ring := range rings {
		for i := 0; i+1 < len(ring); i++ {
			x1, y1 := ring[i][0], ring[i][1]
			x2, y2 := ring[i+1][0], ring[i+1][1]
			if (y1 > y) != (y2 > y) && x < x1+(y-y1)*(x2-x1)/(y2-y1) {
				inside = !inside
			}
		}
	}
	return inside
}

// Polygon is one document of a file of geohash coverings.
type Polygon struct {
	Doc int
	// The payload before its rings.
	Label string
	// The label cut at U+001F and named by `payload-fields`, or nil when the
	// file names no fields.
	Fields map[string]string
	Rings  Rings
}

// Located is what Locate found.
type Located struct {
	// The point's cell, at the finest precision the file is keyed at.
	Cell string
	// How many polygons the cells named, before the point was tested.
	Candidates int
	// The ones holding the point.
	Inside []Polygon
}

// LocateOptions say how a file was built when the file itself does not.
// Empty precisions, and a scale that is not a positive finite number, defer
// to the file; a precision outside 1 to 12 is ErrNotCoverings.
type LocateOptions struct {
	Precisions []int
	Scale      float64
}

// Locate is the polygons holding (lat, lon), from a file of geohash
// coverings.
//
// Every precision is looked up, not only the finest that matches, since a
// point can be in one polygon's coarse interior cell and another's fine
// boundary cell at once. The cells are prefixes of one string, so they fall
// in neighbouring dictionary blocks, read together; the candidates' rings
// come with their payload, and an even-odd test settles them. A file that
// does not say its precisions and geometry is ErrNotCoverings, unless
// options say them.
func Locate(ctx context.Context, ix *Index, lat, lon float64, options *LocateOptions) (*Located, error) {
	var precisions []int
	var scale float64
	if options != nil {
		precisions, scale = options.Precisions, options.Scale
	}
	// What the caller leaves empty, or gives as no scale at all, the file
	// says.
	if len(precisions) == 0 {
		precisions = PrecisionsOf(ix)
	}
	if !(scale > 0) || math.IsInf(scale, 0) {
		scale = ScaleOf(ix)
	}
	if len(precisions) == 0 || scale == 0 {
		return nil, ErrNotCoverings
	}
	for _, p := range precisions {
		if p < 1 || p > 12 {
			return nil, ErrNotCoverings
		}
	}
	names := FieldsOf(ix)
	cell := Geohash(lat, lon, slices.Max(precisions))
	cells := make([]string, len(precisions))
	for i, p := range precisions {
		cells[i] = cell[:p]
	}
	found, err := ix.ExactMany(ctx, cells)
	if err != nil {
		return nil, err
	}
	var docs []int
	for _, entry := range found {
		if entry == nil {
			continue
		}
		// One entry at a time: the cells' runs need not be near each other,
		// and one span from the first to the last would read what lies
		// between.
		postings, err := ix.PostingsSpan(ctx, []TermEntry{*entry})
		if err != nil {
			return nil, err
		}
		docs = append(docs, postings[0].Docs...)
	}
	slices.Sort(docs)
	docs = slices.Compact(docs)
	payloads, err := ix.PayloadSpan(ctx, docs)
	if err != nil {
		return nil, err
	}
	located := &Located{Cell: cell, Candidates: len(docs), Inside: []Polygon{}}
	for i, doc := range docs {
		label, geometry, _ := strings.Cut(payloads[i], "|")
		rings, err := DecodeRings(geometry, scale)
		if err != nil {
			return nil, err
		}
		var fields map[string]string
		if names != nil {
			values := strings.Split(label, "\x1f")
			fields = make(map[string]string, len(names))
			for j, name := range names {
				if j < len(values) {
					fields[name] = values[j]
				} else {
					fields[name] = ""
				}
			}
		}
		if Contains(rings, lon, lat) {
			located.Inside = append(located.Inside, Polygon{doc, label, fields, rings})
		}
	}
	return located, nil
}
