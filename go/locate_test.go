package mokuji

// LookupPrefix and Locate, against the conformance fixtures and a file of
// geohash coverings built here: the checks js/test/query.test.js makes.

import (
	"context"
	"fmt"
	"maps"
	"math"
	"reflect"
	"slices"
	"strings"
	"testing"
)

const marunouchi = "東京都千代田区丸の内1丁目1番1号"

type prefixRow struct {
	Term    string
	Doc     int
	Match   PrefixMatch
	Payload string
}

func rowsOf(hits []PrefixHit) []prefixRow {
	rows := []prefixRow{}
	for _, h := range hits {
		rows = append(rows, prefixRow{h.Term, h.Doc, h.Match, h.Payload})
	}
	return rows
}

func TestLookupPrefixSaysWhetherEachTermIsTheOneAsked(t *testing.T) {
	ctx := context.Background()
	dict := openFixture(t, "dict")
	hits := must(LookupPrefix(ctx, dict, "東京都千代田区丸の内1丁目1番", 10, nil))
	want := []prefixRow{
		{marunouchi, 1, MatchPrefix, "139.767201,35.681560"},
		{"東京都千代田区丸の内1丁目1番2号", 2, MatchPrefix, hits[1].Payload},
	}
	if got := rowsOf(hits); !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
	exact := must(LookupPrefix(ctx, dict, marunouchi, 10, nil))
	if len(exact) != 1 || exact[0].Match != MatchExact {
		t.Fatalf("got %v", exact)
	}
}

func TestLookupPrefixFindsTheTermAnAddressTypedPastItBeginsWith(t *testing.T) {
	ctx := context.Background()
	dict := openFixture(t, "dict")
	typed := marunouchi + "5階"
	if hits := must(LookupPrefix(ctx, dict, typed, 10, nil)); len(hits) != 0 {
		t.Fatalf("without readings: %v", hits)
	}
	hits := must(LookupPrefix(ctx, dict, typed, 10, []string{typed}))
	want := []prefixRow{{marunouchi, 1, MatchWithin, "139.767201,35.681560"}}
	if got := rowsOf(hits); !reflect.DeepEqual(got, want) {
		t.Fatalf("got %v, want %v", got, want)
	}
	if hits := must(LookupPrefix(ctx, dict, "京都府", 10, []string{"京都府京都市"})); len(hits) != 0 {
		t.Fatalf("got %v", hits)
	}
}

func TestLookupPrefixLimitCountsDocuments(t *testing.T) {
	if hits := must(LookupPrefix(context.Background(), openFixture(t, "dict"), "東京都", 2, nil)); len(hits) != 2 {
		t.Fatalf("got %d hits", len(hits))
	}
}

func TestGeohashAgreesWithThePublishedExample(t *testing.T) {
	if got := Geohash(57.64911, 10.40744, 11); got != "u4pruydqqvj" {
		t.Fatal(got)
	}
}

func TestDecodeRingsClosesEachRingAndContainsIsEvenOdd(t *testing.T) {
	rings := must(DecodeRings("0,0,10,0,0,10,-10,0;3,3,4,0,0,4,-4,0", 1))
	if want := [][2]float64{{0, 0}, {10, 0}, {10, 10}, {0, 10}, {0, 0}}; !reflect.DeepEqual(rings[0], want) {
		t.Fatalf("got %v", rings[0])
	}
	if !Contains(rings, 1, 1) || Contains(rings, 5, 5) || Contains(rings, 11, 5) {
		t.Fatal("contains is not even-odd")
	}
	if _, err := DecodeRings("0,x", 1); err == nil {
		t.Fatal("rings that are not integers decoded")
	}
}

// encodeRings writes rings as delta-int-1e5: the first vertex, then
// differences, unclosed.
func encodeRings(rings [][][2]float64) string {
	parts := make([]string, len(rings))
	for r, ring := range rings {
		var values []string
		var px, py int64
		for i, v := range ring {
			x, y := int64(math.Round(v[0]*1e5)), int64(math.Round(v[1]*1e5))
			if i == 0 {
				values = append(values, fmt.Sprint(x), fmt.Sprint(y))
			} else {
				values = append(values, fmt.Sprint(x-px), fmt.Sprint(y-py))
			}
			px, py = x, y
		}
		parts[r] = strings.Join(values, ",")
	}
	return strings.Join(parts, ";")
}

func square(x0, y0, x1, y1 float64) [][2]float64 {
	return [][2]float64{{x0, y0}, {x1, y0}, {x1, y1}, {x0, y1}}
}

// coverings is two blocks side by side, the first with a courtyard, each
// keyed by the cells its corners are in at precisions 5 and 6.
func coverings(t *testing.T) *Index {
	t.Helper()
	places := []struct {
		code, name string
		rings      [][][2]float64
	}{
		{"A", "Marunouchi", [][][2]float64{square(139.76, 35.68, 139.765, 35.685), square(139.762, 35.682, 139.763, 35.683)}},
		{"B", "Otemachi", [][][2]float64{square(139.765, 35.68, 139.77, 35.685)}},
	}
	options := DefaultWriteOptions()
	options.TermsPerBlock = 4
	options.DocsPerPayloadBlock = 2
	builder := must(NewBuilder(options))
	postings := map[string][]int{}
	for doc, place := range places {
		must(builder.AddDocument([]byte(place.code + "\x1f" + place.name + "|" + encodeRings(place.rings))))
		for _, v := range place.rings[0] {
			for _, precision := range []int{5, 6} {
				cell := Geohash(v[1], v[0], precision)
				if !slices.Contains(postings[cell], doc) {
					postings[cell] = append(postings[cell], doc)
				}
			}
		}
	}
	for _, cell := range slices.Sorted(maps.Keys(postings)) {
		for _, doc := range postings[cell] {
			if err := builder.AddPosting([]byte(cell), doc, 1); err != nil {
				t.Fatal(err)
			}
		}
	}
	meta := must((&Metadata{
		Sources:     []Provenance{{ContentHash: "sha2-256:" + strings.Repeat("0", 64)}},
		TokenizerID: "identity",
		Attrs:       map[string]any{"geohash-precisions": "5,6", "geometry": "delta-int-1e5", "payload-fields": "CODE,NAME"},
	}).Encode())
	return must(Open(context.Background(), BytesSource(must(builder.Finish(meta)))))
}

func TestLocateFindsThePolygonsHoldingAPoint(t *testing.T) {
	ctx := context.Background()
	index := coverings(t)
	found := must(Locate(ctx, index, 35.6801, 139.7601, nil))
	if found.Cell != Geohash(35.6801, 139.7601, 6) || found.Candidates < 1 {
		t.Fatalf("got %+v", found)
	}
	if len(found.Inside) != 1 {
		t.Fatalf("got %+v", found.Inside)
	}
	p := found.Inside[0]
	if p.Doc != 0 || p.Label != "A\x1fMarunouchi" || !reflect.DeepEqual(p.Fields, map[string]string{"CODE": "A", "NAME": "Marunouchi"}) || len(p.Rings) != 2 {
		t.Fatalf("got %+v", p)
	}

	east := must(Locate(ctx, index, 35.6801, 139.7699, nil))
	if len(east.Inside) != 1 || east.Inside[0].Fields["CODE"] != "B" {
		t.Fatalf("got %+v", east.Inside)
	}
	// In the courtyard: a candidate, but in neither.
	courtyard := must(Locate(ctx, index, 35.6825, 139.7625, nil))
	if courtyard.Candidates < 1 || len(courtyard.Inside) != 0 {
		t.Fatalf("got %+v", courtyard)
	}
}

func TestLocateRefusesAFileThatDoesNotSayHowItWasBuilt(t *testing.T) {
	ctx := context.Background()
	dict := openFixture(t, "dict")
	if _, err := Locate(ctx, dict, 35.68, 139.76, nil); err == nil || !strings.Contains(err.Error(), "geohash precisions") {
		t.Fatalf("got %v", err)
	}
	// Unless the caller says.
	found := must(Locate(ctx, dict, 35.68, 139.76, &LocateOptions{Precisions: []int{5}, Scale: 1e5}))
	if len(found.Inside) != 0 {
		t.Fatalf("got %+v", found.Inside)
	}
}
