package mokuji

// The writer is held to the fixtures, not the other way round.
//
// The raw fixtures were built by conformance/fixtures/build_fixtures.py, an
// independent implementation written from SPEC.md, and the Rust writer
// reproduces them byte for byte. If this writer does too, the three agree on
// the layout down to the last varint.

import (
	"bytes"
	"context"
	"fmt"
	"math/rand"
	"os"
	"path/filepath"
	"slices"
	"sort"
	"strconv"
	"strings"
	"testing"
)

// metaOf is a fixture's metadata section, taken verbatim, so that a
// comparison of layouts is not one of CBOR.
func metaOf(t *testing.T, file []byte) []byte {
	t.Helper()
	index := must(Open(context.Background(), BytesSource(file)))
	s := index.Sections[SectionMeta]
	return file[s.Offset : s.Offset+s.Length]
}

// corpusLines is a fixture's corpus, as text and payload per line.
func corpusLines(t *testing.T, name string) [][2]string {
	t.Helper()
	data, err := os.ReadFile(filepath.Join(fixtures, name+".corpus.txt"))
	if err != nil {
		t.Fatal(err)
	}
	var out [][2]string
	for _, line := range strings.Split(strings.TrimRight(string(data), "\n"), "\n") {
		text, payload, ok := strings.Cut(line, "\t")
		if !ok {
			t.Fatalf("no tab in %q", line)
		}
		out = append(out, [2]string{text, payload})
	}
	return out
}

// invert is a corpus's postings in the order a Builder takes them: terms in
// byte order, and each term's documents ascending with where it occurs.
func invert(documents []string, tokenizer string) ([]string, map[string][]int, map[string]map[int][]uint64) {
	places := map[string]map[int][]uint64{}
	for doc, text := range documents {
		tokens, _ := Split(tokenizer, text)
		for at, token := range tokens {
			if places[token] == nil {
				places[token] = map[int][]uint64{}
			}
			places[token][doc] = append(places[token][doc], uint64(at))
		}
	}
	terms := make([]string, 0, len(places))
	docs := map[string][]int{}
	for term, byDoc := range places {
		terms = append(terms, term)
		for doc := range byDoc {
			docs[term] = append(docs[term], doc)
		}
		slices.Sort(docs[term])
	}
	slices.Sort(terms) // Go compares strings bytewise, as the format does.
	return terms, docs, places
}

func sameBytes(t *testing.T, name string, built, expected []byte) {
	t.Helper()
	if len(built) != len(expected) {
		t.Fatalf("%s: built %d bytes, the fixture is %d", name, len(built), len(expected))
	}
	for i := range built {
		if built[i] != expected[i] {
			t.Fatalf("%s: first difference at offset %d: built %#04x, fixture %#04x", name, i, built[i], expected[i])
		}
	}
}

func buildTiny(t *testing.T, meta []byte, compress bool) []byte {
	options := WriteOptions{
		TermsPerBlock: 2, DocsPerPayloadBlock: 2, HasFreqs: true,
		Positions: true, DocLengths: true, Compress: compress, InlinePostings: 8,
	}
	b := must(NewBuilder(options))
	var texts []string
	for _, line := range corpusLines(t, "tiny") {
		must(b.AddDocument([]byte(line[1])))
		texts = append(texts, line[0])
	}
	terms, docs, places := invert(texts, "space-split")
	for _, term := range terms {
		for _, doc := range docs[term] {
			if err := b.AddPostingAt([]byte(term), doc, places[term][doc]); err != nil {
				t.Fatal(err)
			}
		}
	}
	return must(b.Finish(meta))
}

// parseValues reads `139.767201,35.681560` as integers scaled by 10^decimals.
func parseValues(t *testing.T, text string, decimals int) []int64 {
	var out []int64
	for _, field := range strings.Split(text, ",") {
		whole, fraction, _ := strings.Cut(strings.TrimSpace(field), ".")
		fraction = (fraction + strings.Repeat("0", decimals))[:decimals]
		v, err := strconv.ParseInt(whole+fraction, 10, 64)
		if err != nil {
			t.Fatal(err)
		}
		out = append(out, v)
	}
	return out
}

func buildDict(t *testing.T, meta []byte, compress bool) []byte {
	b := must(NewBuilder(WriteOptions{
		TermsPerBlock: 2, DocsPerPayloadBlock: 2, HasFreqs: true,
		Payload:  PayloadEncoding{Ints: true, ValuesPerDoc: 2, ScaleExponent: -6},
		Compress: compress, InlinePostings: 8,
	}))
	lines := corpusLines(t, "dict")
	for _, line := range lines {
		must(b.AddDocumentValues(parseValues(t, line[1], 6)))
	}
	// The corpus is already in term order, which is what lets every term's
	// postings be implied.
	for doc, line := range lines {
		if err := b.AddPosting([]byte(line[0]), doc, 1); err != nil {
			t.Fatal(err)
		}
	}
	if err := b.AddColumn(Column{Name: "importance", Values: []int64{40, 90, 85, 70, 60}}); err != nil {
		t.Fatal(err)
	}
	return must(b.Finish(meta))
}

func buildNames(t *testing.T, name string, meta []byte, compress bool) []byte {
	b := must(NewBuilder(WriteOptions{
		TermsPerBlock: 2, DocsPerPayloadBlock: 2, HasFreqs: true, Compress: compress,
		Completions: &CompletionOptions{MinTerms: 2, DocsPerEntry: 2},
		// As the fixture builder writes `names`: every run stored.
		InlinePostings: 0,
	}))
	var texts []string
	for _, line := range corpusLines(t, name) {
		must(b.AddDocument([]byte(line[1])))
		texts = append(texts, line[0])
	}
	terms, docs, places := invert(texts, "name-list")
	for _, term := range terms {
		for _, doc := range docs[term] {
			if err := b.AddPosting([]byte(term), doc, uint64(len(places[term][doc]))); err != nil {
				t.Fatal(err)
			}
		}
	}
	return must(b.Finish(meta))
}

func TestTheWriterReproducesTheFixturesByteForByte(t *testing.T) {
	for name, build := range map[string]func([]byte) []byte{
		"tiny":        func(meta []byte) []byte { return buildTiny(t, meta, false) },
		"dict":        func(meta []byte) []byte { return buildDict(t, meta, false) },
		"names":       func(meta []byte) []byte { return buildNames(t, "names", meta, false) },
		"names-delta": func(meta []byte) []byte { return buildNames(t, "names-delta", meta, false) },
	} {
		t.Run(name, func(t *testing.T) {
			expected := fixtureBytes(t, name)
			meta := metaOf(t, expected)
			sameBytes(t, name, build(meta), expected)
			// And again: one input, one file.
			sameBytes(t, name, build(meta), expected)
		})
	}
}

// units is every unit of a file, decompressed, by section: what a file says,
// whatever compressor wrote it.
func units(t *testing.T, file []byte) map[uint16][][]byte {
	t.Helper()
	index := must(Open(context.Background(), BytesSource(file)))
	out := map[uint16][][]byte{}
	for id, s := range index.Sections {
		data := file[s.Offset : s.Offset+s.Length]
		var pieces [][]byte
		switch id {
		case SectionTermBlocks:
			// Compared entry by entry, below: an entry's offsets are absolute,
			// and move with the size of every compressed section before them.
			continue
		case SectionPayload:
			for i := 0; i < index.payloadBlocks; i++ {
				offset, length := must2(index.payloadBlockLocation(i))
				pieces = append(pieces, file[offset:offset+length])
			}
		case SectionTermBlockOffsets, SectionPayloadOffsets:
			// Where the units are, which compression moves.
			continue
		default:
			pieces = [][]byte{data}
		}
		for i, p := range pieces {
			pieces[i] = must(unit(p, s.Codec))
		}
		out[id] = pieces
	}
	return out
}

// entries is every dictionary entry of a file, its offsets made relative to
// the sections they point into.
func entries(t *testing.T, file []byte) []TermEntry {
	t.Helper()
	index := must(Open(context.Background(), BytesSource(file)))
	var out []TermEntry
	for i := range index.Boundaries {
		for _, e := range must(index.TermBlock(context.Background(), i)) {
			e.PostingsOffset -= index.Sections[SectionPostings].Offset
			if s, ok := index.Sections[SectionPositions]; ok {
				e.PositionsOffset -= s.Offset
			}
			out = append(out, e)
		}
	}
	return out
}

func must2[A, B any](a A, b B, err error) (A, B) {
	if err != nil {
		panic(err)
	}
	return a, b
}

// The compressed fixture was written by the Rust writer, whose deflate is not
// Go's, so the bytes differ. What they decompress to may not.
func TestACompressedFileSaysWhatTheRustWritersDoes(t *testing.T) {
	expected := fixtureBytes(t, "dict-deflate")
	built := buildDict(t, metaOf(t, expected), true)
	theirs, ours := units(t, expected), units(t, built)
	if len(theirs) != len(ours) {
		t.Fatalf("%d sections, the fixture has %d", len(ours), len(theirs))
	}
	for id, pieces := range theirs {
		if !slices.EqualFunc(ours[id], pieces, bytes.Equal) {
			t.Errorf("section %#06x differs once decompressed", id)
		}
	}
	if fmt.Sprint(entries(t, built)) != fmt.Sprint(entries(t, expected)) {
		t.Error("the dictionary entries differ")
	}
	// The codecs are each writer's own choice, made by what its compressor
	// saves; in this file both compress the term index and nothing else.
	ourIndex := must(Open(context.Background(), BytesSource(built)))
	theirIndex := must(Open(context.Background(), BytesSource(expected)))
	for id, s := range theirIndex.Sections {
		if ourIndex.Sections[id].Codec != s.Codec {
			t.Errorf("section %#06x: codec %d, the Rust writer's %d", id, ourIndex.Sections[id].Codec, s.Codec)
		}
	}
}

// Every conformance case, against the fixtures as this writer compresses
// them: the same answers, and the same reads.
func TestCompressedFixturesAnswerEveryCase(t *testing.T) {
	compressed := map[string][]byte{}
	for _, name := range []string{"tiny", "dict", "names"} {
		meta := metaOf(t, fixtureBytes(t, name))
		switch name {
		case "tiny":
			compressed[name] = buildTiny(t, meta, true)
		case "dict":
			compressed[name] = buildDict(t, meta, true)
		default:
			compressed[name] = buildNames(t, name, meta, true)
		}
	}
	open := func(name string) *Index {
		index := must(Open(context.Background(), BytesSource(compressed[name])))
		index.Coalesce = false
		index.ResetStats()
		return index
	}
	s := loadSuite(t)
	for _, c := range s.Case {
		if compressed[c.Fixture] == nil {
			continue
		}
		index := open(c.Fixture)
		docs, payloads := termQuery(t, index, c.Query)
		if !slices.Equal(docs, orEmpty(c.Docs)) || !slices.Equal(payloads, orEmpty(c.Payloads)) || index.Stats().Requests != c.Reads {
			t.Errorf("%s: %v %q in %d reads", c.Name, docs, payloads, index.Stats().Requests)
		}
	}
	for _, c := range s.Completion {
		index := open(c.Fixture)
		done := must(index.Complete(context.Background(), c.Prefix, c.Limit))
		if !slices.Equal(done.Payloads, orEmpty(c.Payloads)) || done.FromTable != c.FromTable || index.Stats().Requests != c.Reads {
			t.Errorf("%s: %q in %d reads", c.Name, done.Payloads, index.Stats().Requests)
		}
	}
}

// Metadata.Encode writes the fixtures' metadata exactly as the fixture
// builder did: canonical CBOR has one encoding for one value.
func TestMetadataEncodesAsTheFixturesDo(t *testing.T) {
	for _, name := range []string{"tiny", "dict", "names", "names-delta"} {
		file := fixtureBytes(t, name)
		index := must(Open(context.Background(), BytesSource(file)))
		generator := field(index.Meta, "generator")
		m := Metadata{
			Generation:       field(index.Meta, "generation").(uint64),
			Sources:          index.Sources,
			GeneratorName:    text(field(generator, "name")),
			GeneratorVersion: text(field(generator, "version")),
			GeneratorParams:  stringKeys(field(generator, "params")),
			TokenizerID:      index.TokenizerID,
			TokenizerParams:  stringKeys(field(field(index.Meta, "tokenizer"), "params")),
			License:          index.License,
			Attribution:      index.Attribution,
		}
		if attrs := field(index.Meta, "attrs"); attrs != nil {
			m.Attrs = stringKeys(attrs)
		}
		sameBytes(t, name, must(m.Encode()), metaOf(t, file))
	}
}

func stringKeys(v any) map[string]any {
	out := map[string]any{}
	m, _ := v.(map[any]any)
	for key, value := range m {
		if inner, ok := value.(map[any]any); ok {
			value = stringKeys(inner)
		}
		out[key.(string)] = value
	}
	return out
}

func TestCanonicalCBOR(t *testing.T) {
	for _, c := range []struct {
		value any
		hex   string
	}{
		{uint64(0), "00"},
		{23, "17"},
		{24, "1818"},
		{uint64(256), "190100"},
		{uint64(1 << 32), "1b0000000100000000"},
		{"a", "6161"},
		{[]any{uint64(1), "b"}, "82016162"},
		// Keys sorted by their encoding: shorter first, then bytewise.
		{map[string]any{"bb": uint64(1), "c": uint64(2), "a": uint64(3)}, "a3616103616302626262" + "01"},
	} {
		got := fmt.Sprintf("%x", must(EncodeCBOR(c.value)))
		if got != c.hex {
			t.Errorf("%v: %s, want %s", c.value, got, c.hex)
		}
	}
	for _, bad := range []any{-1, 1.5, true, map[int]any{}} {
		if _, err := EncodeCBOR(bad); err == nil {
			t.Errorf("encoded %v", bad)
		}
	}
}

func TestTheBuilderRejectsWhatIsOutOfOrder(t *testing.T) {
	options := DefaultWriteOptions()
	b := must(NewBuilder(options))
	must(b.AddDocument([]byte("a")))
	must(b.AddDocument([]byte("b")))
	if err := b.AddPosting([]byte("tokyo"), 1, 1); err != nil {
		t.Fatal(err)
	}
	for _, bad := range []func() error{
		func() error { return b.AddPosting([]byte("tokyo"), 0, 1) },
		func() error { return b.AddPosting([]byte("osaka"), 0, 1) },
		func() error { return b.AddPosting([]byte("zz"), 7, 1) },
		func() error { return b.AddPosting([]byte("zz"), 0, 0) },
		func() error { return b.AddPostingAt([]byte("zz"), 0, []uint64{2, 1}) },
		func() error { return b.AddSection(0x0040, nil) },
		func() error { _, err := b.AddDocumentValues([]int64{1}); return err },
	} {
		if bad() == nil {
			t.Error("accepted")
		}
	}
	if _, err := must(NewBuilder(options)).Finish(nil); err == nil {
		t.Error("built an index with no terms")
	}
	for _, o := range []WriteOptions{
		{TermsPerBlock: 0, DocsPerPayloadBlock: 1},
		{TermsPerBlock: 1, DocsPerPayloadBlock: 1, Positions: true},
		{TermsPerBlock: 1, DocsPerPayloadBlock: 1, Completions: &CompletionOptions{DocsPerEntry: 1}},
	} {
		if _, err := NewBuilder(o); err == nil {
			t.Errorf("accepted %+v", o)
		}
	}
}

// Random corpora, built with every combination of options that changes the
// layout, read back: every term finds exactly the documents and payloads it
// was given, positions included, with or without compression.
func TestRandomCorporaReadBackAsTheyWereWritten(t *testing.T) {
	ctx := context.Background()
	rng := rand.New(rand.NewSource(1))
	words := []string{"a", "ab", "abc", "b", "東京", "東京都", "tokyo", "z", "zz", "丸の内"}
	for round := range 60 {
		var texts []string
		for range 1 + rng.Intn(40) {
			n := 1 + rng.Intn(5)
			var parts []string
			for range n {
				parts = append(parts, words[rng.Intn(len(words))])
			}
			texts = append(texts, strings.Join(parts, " "))
		}
		options := WriteOptions{
			TermsPerBlock: 1 + rng.Intn(4), DocsPerPayloadBlock: uint32(1 + rng.Intn(5)),
			HasFreqs: true, Positions: rng.Intn(2) == 0, DocLengths: rng.Intn(2) == 0,
			Compress: rng.Intn(2) == 0, InlinePostings: rng.Intn(10),
		}
		if rng.Intn(2) == 0 {
			options.Completions = &CompletionOptions{MinTerms: uint64(1 + rng.Intn(3)), DocsPerEntry: uint32(1 + rng.Intn(3))}
		}
		b := must(NewBuilder(options))
		for i := range texts {
			must(b.AddDocument(fmt.Appendf(nil, "doc %d", i)))
		}
		terms, docs, places := invert(texts, "space-split")
		for _, term := range terms {
			for _, doc := range docs[term] {
				if err := b.AddPostingAt([]byte(term), doc, places[term][doc]); err != nil {
					t.Fatal(err)
				}
			}
		}
		meta := must((&Metadata{GeneratorName: GeneratorName, GeneratorVersion: Version,
			GeneratorParams: options.Params(), TokenizerID: "space-split"}).Encode())
		file := must(b.Finish(meta))
		index, err := Open(ctx, BytesSource(file))
		if err != nil {
			t.Fatalf("round %d: %v", round, err)
		}
		for _, term := range terms {
			entry := must(index.Exact(ctx, term))
			if entry == nil {
				t.Fatalf("round %d: %q is absent", round, term)
			}
			postings := must(index.PostingsSpan(ctx, []TermEntry{*entry}))[0]
			if !slices.Equal(postings.Docs, docs[term]) {
				t.Fatalf("round %d: %q names %v, not %v", round, term, postings.Docs, docs[term])
			}
			payloads := must(index.PayloadSpan(ctx, postings.Docs))
			for i, doc := range postings.Docs {
				if payloads[i] != fmt.Sprintf("doc %d", doc) || postings.Freqs[i] != uint64(len(places[term][doc])) {
					t.Fatalf("round %d: %q in %d: %q, freq %d", round, term, doc, payloads[i], postings.Freqs[i])
				}
			}
			if options.Positions {
				positions := must(index.Positions(ctx, entry, postings))
				for i, doc := range postings.Docs {
					if !slices.Equal(positions[i], places[term][doc]) {
						t.Fatalf("round %d: %q in %d at %v, not %v", round, term, doc, positions[i], places[term][doc])
					}
				}
			}
		}
		// A completion answers as walking the dictionary would.
		for _, prefix := range []string{"a", "東", "z", "q"} {
			done := must(index.Complete(ctx, prefix, 2))
			var want []int
			for _, term := range terms {
				if strings.HasPrefix(term, prefix) {
					want = append(want, docs[term]...)
				}
			}
			sort.Ints(want)
			want = slices.Compact(want)
			want = want[:min(2, len(want))]
			if !slices.Equal(done.Docs, orEmpty(want)) {
				t.Fatalf("round %d: %q completes to %v, not %v", round, prefix, done.Docs, want)
			}
		}
	}
}
