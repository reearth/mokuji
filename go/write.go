package mokuji

// The writer: the Rust crate's `mokuji::write`, in Go.
//
// SPEC.md §3.4 asks a writer for a byte-identical file from identical input
// and parameters. This one is held to more than that: stored raw, its output
// is the Rust writer's byte for byte, and the tests rebuild the conformance
// fixtures to show it. Compressed, it is not, and cannot be: no
// specification of deflate fixes which of the valid encodings a compressor
// emits, and Go's is not Rust's. That is what `generator.name` and
// `generator.version` in the metadata are for: a rebuild is checkable against
// the writer that made it.

import (
	"bytes"
	"compress/flate"
	"encoding/binary"
	"errors"
	"fmt"
	"math"
	"math/bits"
	"slices"
	"sort"
	"strconv"
)

// GeneratorName is what this writer calls itself in `generator.name`.
const GeneratorName = "mokuji-go"

// SectionApplicationMin is the first section identifier an application may
// use (SPEC.md §5.2). A reader that does not recognize one ignores it.
const SectionApplicationMin uint16 = 0x8000

const specMinor = 1

// PayloadEncoding is how payload is written down (SPEC.md §5.5): not what it
// means, which the format has no opinion about. The zero value is opaque
// bytes.
type PayloadEncoding struct {
	// Ints writes a fixed number of signed integers per document,
	// delta-coded: worth taking whenever documents that sort together hold
	// similar numbers, as addresses do.
	Ints bool
	// How many integers each document carries, with Ints.
	ValuesPerDoc uint8
	// A stored v stands for v * 10^ScaleExponent, with Ints. Recorded, not
	// interpreted.
	ScaleExponent int8
}

// CompletionOptions says which prefixes get an entry in the completions
// section (SPEC.md §5.3.5), and how many documents each lists.
//
// The documents listed are the ones with the smallest IDs among those holding
// a term that begins with the prefix: this writer ranks by nothing but
// document order, so a caller who wants the most important documents first
// numbers them by importance before adding them.
type CompletionOptions struct {
	// A prefix that at least this many terms begin with gets an entry. Zero
	// promises nothing on this count.
	MinTerms uint64
	// A prefix whose terms' postings runs total at least this many bytes gets
	// an entry. Zero promises nothing on this count.
	MaxSpan uint64
	// How many documents an entry lists at most.
	DocsPerEntry uint32
}

// WriteOptions are a build's parameters. They belong in the metadata's
// `generator.params`, since SPEC.md §3.4 requires that the parameters be
// sufficient to reproduce the file; Params says them.
type WriteOptions struct {
	// Terms per dictionary block: bytes per lookup against the size of the
	// dictionary and of the resident term index.
	TermsPerBlock int
	// Documents per payload block. Larger blocks let more results share a
	// read and make each read bigger.
	DocsPerPayloadBlock uint32
	// Whether to store term frequencies. An address dictionary, where a term
	// occurs at most once per document, omits them.
	HasFreqs bool
	Payload  PayloadEncoding
	// Whether to store where each term occurs in each document: what a
	// phrase query needs and nothing else does.
	Positions bool
	// Whether to store every document's length in tokens: what BM25 needs and
	// the header cannot give.
	DocLengths bool
	// Whether to compress the sections that can be. Each addressed unit is
	// compressed on its own, and a section whose units do not shrink is
	// stored raw.
	Compress bool
	// Whether to answer expensive prefixes ahead of time; nil for not.
	Completions *CompletionOptions
	// The most bytes a term's postings may take written inline in its
	// dictionary entry (SPEC.md §5.3.4); zero writes every term a run.
	InlinePostings int
}

// DefaultWriteOptions are the Rust writer's defaults.
func DefaultWriteOptions() WriteOptions {
	return WriteOptions{
		TermsPerBlock:       1024,
		DocsPerPayloadBlock: 64,
		HasFreqs:            true,
		Compress:            true,
		InlinePostings:      8,
	}
}

// Params is the options as `mokuji build` records them in
// `generator.params`, and whether the build compressed, for a Metadata the
// caller composes. A caller with parameters of its own, such as how the
// documents were ordered, adds them.
func (o WriteOptions) Params() map[string]any {
	flag := func(b bool) uint64 {
		if b {
			return 1
		}
		return 0
	}
	payload := "opaque"
	if o.Payload.Ints {
		payload = "ints:" + strconv.Itoa(-int(o.Payload.ScaleExponent))
	}
	params := map[string]any{
		"terms_per_block":        uint64(o.TermsPerBlock),
		"docs_per_payload_block": uint64(o.DocsPerPayloadBlock),
		"has_freqs":              flag(o.HasFreqs),
		"payload":                payload,
		"doc_lengths":            flag(o.DocLengths),
		"positions":              flag(o.Positions),
		"inline_postings":        uint64(o.InlinePostings),
		"compress":               flag(o.Compress),
	}
	if c := o.Completions; c != nil {
		params["completions"] = fmt.Sprintf("%d,%d,%d by document order", c.MinTerms, c.MaxSpan, c.DocsPerEntry)
	}
	return params
}

func buildError(msg string) error { return errors.New("mokuji: " + msg) }

// Builder builds a Mokuji file from documents and a sorted stream of
// postings.
//
// Documents are added first, in document ID order; postings are then pushed
// in term order, and within a term in document order. Both orderings are
// checked rather than assumed, since a stream that is merely nearly sorted
// would produce a file that is subtly wrong rather than obviously broken.
// Sorting stays with the caller, so that a corpus larger than memory does not
// need the writer to own an external sort.
type Builder struct {
	options    WriteOptions
	payloads   [][]byte
	values     [][]int64
	columns    []Column
	sumDocLen  uint64
	docLengths []uint64
	terms      []builtTerm
	open       *openTerm
	sections   map[uint16][]byte
}

type builtTerm struct {
	term    []byte
	docFreq uint64
	// The postings run; empty when the postings are implied or inline.
	run []byte
	// The postings as written in the dictionary entry, when they are.
	inline []byte
	// The documents and frequencies, kept while the term is short enough that
	// it might yet be inlined.
	short     bool
	docs      []uint64
	freqs     []uint64
	positions []byte
	// The term's first documents, as many as a completion lists.
	head []uint64
}

type openTerm struct {
	term      []byte
	docs      []uint64
	freqs     []uint64
	positions [][]uint64
}

// NewBuilder starts a build.
func NewBuilder(options WriteOptions) (*Builder, error) {
	if options.TermsPerBlock <= 0 || options.DocsPerPayloadBlock == 0 {
		return nil, buildError("block sizes must be non-zero")
	}
	// A positions run says nothing about where one document's positions end
	// and the next begin; the frequency is that boundary.
	if options.Positions && !options.HasFreqs {
		return nil, buildError("positions need frequencies, which say how many belong to each document")
	}
	if options.InlinePostings < 0 {
		return nil, buildError("an inline postings bound cannot be negative")
	}
	if c := options.Completions; c != nil {
		if c.DocsPerEntry == 0 {
			return nil, buildError("a completion entry lists at least one document")
		}
		// Both zero would be a table with no rule for what goes in it.
		if c.MinTerms == 0 && c.MaxSpan == 0 {
			return nil, buildError("completions need a term count or a postings size to cover")
		}
	}
	return &Builder{options: options, sections: map[uint16][]byte{}}, nil
}

// AddDocument appends a document whose payload is opaque bytes, returning
// its ID.
func (b *Builder) AddDocument(payload []byte) (int, error) {
	if b.options.Payload.Ints {
		return 0, buildError("this index encodes payload as integers")
	}
	b.payloads = append(b.payloads, bytes.Clone(payload))
	return b.Documents() - 1, nil
}

// AddDocumentValues appends a document whose payload is a fixed row of
// integers, returning its ID.
func (b *Builder) AddDocumentValues(values []int64) (int, error) {
	if !b.options.Payload.Ints {
		return 0, buildError("this index encodes payload as opaque bytes")
	}
	if len(values) != int(b.options.Payload.ValuesPerDoc) {
		return 0, buildError("a document carries the wrong number of values")
	}
	b.values = append(b.values, slices.Clone(values))
	return b.Documents() - 1, nil
}

// Documents is how many documents have been added.
func (b *Builder) Documents() int {
	if b.options.Payload.Ints {
		return len(b.values)
	}
	return len(b.payloads)
}

// AddColumn adds a column of per-document numbers (SPEC.md §5.8), checked
// at Finish against the document count, since the documents may not all
// have been added yet.
func (b *Builder) AddColumn(column Column) error {
	if column.ScaleExponent < math.MinInt8 || column.ScaleExponent > math.MaxInt8 {
		return buildError("a column's scale exponent does not fit a signed byte")
	}
	for _, c := range b.columns {
		if c.Name == column.Name {
			return buildError("two document attribute columns share a name")
		}
	}
	column.Values = slices.Clone(column.Values)
	b.columns = append(b.columns, column)
	return nil
}

// AddPosting records that term occurs freq times in doc.
//
// Terms must be pushed in ascending byte order and, within a term, documents
// must ascend too.
func (b *Builder) AddPosting(term []byte, doc int, freq uint64) error {
	if freq == 0 {
		return buildError("a posting with zero frequency is not a posting")
	}
	if doc < 0 || doc >= b.Documents() {
		return buildError("posting refers to a document that was never added")
	}
	d := uint64(doc)
	switch {
	case b.open != nil && bytes.Equal(b.open.term, term):
		if d <= b.open.docs[len(b.open.docs)-1] {
			return buildError("documents within a term are not ascending")
		}
		b.open.docs = append(b.open.docs, d)
		b.open.freqs = append(b.open.freqs, freq)
		b.open.positions = append(b.open.positions, nil)
	case b.open != nil && bytes.Compare(term, b.open.term) <= 0:
		return buildError("terms are not ascending")
	default:
		b.closeTerm()
		b.open = &openTerm{term: bytes.Clone(term), docs: []uint64{d}, freqs: []uint64{freq}, positions: [][]uint64{nil}}
	}
	b.sumDocLen += freq
	if b.options.DocLengths {
		if len(b.docLengths) <= doc {
			b.docLengths = append(b.docLengths, make([]uint64, doc+1-len(b.docLengths))...)
		}
		b.docLengths[doc] += freq
	}
	return nil
}

// AddPostingAt records that term occurs in doc at exactly these positions,
// which must ascend. The frequency is the number of positions.
func (b *Builder) AddPostingAt(term []byte, doc int, positions []uint64) error {
	if len(positions) == 0 {
		return buildError("a posting with no positions is not a posting")
	}
	for i := 1; i < len(positions); i++ {
		if positions[i] <= positions[i-1] {
			return buildError("positions within a document are not ascending")
		}
	}
	if err := b.AddPosting(term, doc, uint64(len(positions))); err != nil {
		return err
	}
	b.open.positions[len(b.open.positions)-1] = slices.Clone(positions)
	return nil
}

// AddSection attaches an application-defined section (SPEC.md §5.2), with an
// identifier at or above SectionApplicationMin. It is stored uncompressed and
// placed last, so that attaching one moves nothing a reader looks for.
func (b *Builder) AddSection(id uint16, data []byte) error {
	if id < SectionApplicationMin {
		return buildError("a section this specification defines cannot be supplied by an application")
	}
	b.sections[id] = bytes.Clone(data)
	return nil
}

// --- encoders --------------------------------------------------------------

func putVarint(out []byte, v uint64) []byte {
	for v >= 0x80 {
		out = append(out, byte(v)|0x80)
		v >>= 7
	}
	return append(out, byte(v))
}

func zigzag(v int64) uint64 { return uint64(v<<1) ^ uint64(v>>63) }

// putKey appends key, front-coded against previous (SPEC.md §5.3.1).
func putKey(out, key, previous []byte) []byte {
	shared := 0
	for shared < len(key) && shared < len(previous) && key[shared] == previous[shared] {
		shared++
	}
	out = putVarint(out, uint64(shared))
	out = putVarint(out, uint64(len(key)-shared))
	return append(out, key[shared:]...)
}

// putValues appends values in bit-packed groups of 128 (SPEC.md §5.4.1).
func putValues(out []byte, values []uint64) []byte {
	for start := 0; start < len(values); start += 128 {
		group := values[start:min(start+128, len(values))]
		width := 0
		for _, v := range group {
			width = max(width, bits.Len64(v))
		}
		out = append(out, byte(width))
		if width == 0 {
			continue
		}
		base := len(out)
		out = append(out, make([]byte, (len(group)*width+7)/8)...)
		for i, v := range group {
			at := i * width
			for j := 0; j < width; j++ {
				if v>>j&1 == 1 {
					out[base+(at+j)/8] |= 1 << ((at + j) % 8)
				}
			}
		}
	}
	return out
}

// compress is the deflate codec: the length, then raw deflate at the
// strongest level.
func compressUnit(unit []byte) ([]byte, error) {
	var out bytes.Buffer
	out.Write(putVarint(nil, uint64(len(unit))))
	w, err := flate.NewWriter(&out, flate.BestCompression)
	if err != nil {
		return nil, err
	}
	if _, err := w.Write(unit); err != nil {
		return nil, err
	}
	if err := w.Close(); err != nil {
		return nil, err
	}
	return out.Bytes(), nil
}

// --- finishing -------------------------------------------------------------

func (b *Builder) closeTerm() {
	open := b.open
	if open == nil {
		return
	}
	b.open = nil

	// Positions, in the same document order the postings run uses.
	var positions []byte
	if b.options.Positions {
		for i, row := range open.positions {
			values := make([]uint64, 0, open.freqs[i])
			for j, at := range row {
				if j == 0 {
					values = append(values, at)
				} else {
					values = append(values, at-row[j-1]-1)
				}
			}
			// A posting recorded without positions still needs its share of
			// the run, or the frequencies stop lining up.
			for uint64(len(values)) < open.freqs[i] {
				values = append(values, 0)
			}
			positions = putValues(positions, values)
		}
	}

	var head []uint64
	if c := b.options.Completions; c != nil {
		head = slices.Clone(open.docs[:min(len(open.docs), int(c.DocsPerEntry))])
	}

	// The dictionary can state this term's postings by itself: it names one
	// document, that document's ID is the ID this term is about to be given,
	// and it occurs there once (SPEC.md §5.3.4).
	termID := uint64(len(b.terms))
	if len(open.docs) == 1 && open.docs[0] == termID && open.freqs[0] == 1 {
		b.terms = append(b.terms, builtTerm{term: open.term, docFreq: 1, positions: positions, head: head})
		return
	}

	run := []byte{0} // codec 0
	run = putVarint(run, uint64(len(open.docs)))
	// The first value is a document ID and the rest are gaps less one, so
	// that consecutive documents become zeros and pack to nothing.
	values := make([]uint64, len(open.docs))
	for i, doc := range open.docs {
		if i == 0 {
			values[i] = doc
		} else {
			values[i] = doc - open.docs[i-1] - 1
		}
	}
	run = putValues(run, values)
	if b.options.HasFreqs {
		freqs := make([]uint64, len(open.freqs))
		for i, f := range open.freqs {
			freqs[i] = f - 1
		}
		run = putValues(run, freqs)
	}

	t := builtTerm{term: open.term, docFreq: uint64(len(open.docs)), run: run, positions: positions, head: head}
	// Every inline value costs at least a byte, so a term with more documents
	// than the bound allows bytes can never be inlined.
	if len(open.docs) <= b.options.InlinePostings {
		t.short, t.docs, t.freqs = true, open.docs, open.freqs
	}
	b.terms = append(b.terms, t)
}

// inlinePostings moves short postings into their dictionary entries
// (SPEC.md §5.3.4), once every term is known: a term's first document is
// written against the entries before it in its block, and the decision
// changes how long the postings section is.
func (b *Builder) inlinePostings() {
	bound := b.options.InlinePostings
	per := b.options.TermsPerBlock
	for start := 0; start < len(b.terms); start += per {
		firstTermID := uint64(start)
		expected := firstTermID
		for i := start; i < min(start+per, len(b.terms)); i++ {
			t := &b.terms[i]
			if !t.short {
				if len(t.run) == 0 {
					// Implied: the document is the term's own ID.
					expected = uint64(i) + 1
				}
				continue
			}
			docs, freqs := t.docs, t.freqs
			t.short, t.docs, t.freqs = false, nil, nil
			if bound == 0 {
				continue
			}
			inline := putVarint(nil, zigzag(int64(docs[0])-int64(expected)))
			for j := 1; j < len(docs); j++ {
				inline = putVarint(inline, docs[j]-docs[j-1]-1)
			}
			if b.options.HasFreqs {
				for _, f := range freqs {
					inline = putVarint(inline, f-1)
				}
			}
			if len(inline) <= bound && len(inline) <= len(t.run) {
				expected = docs[len(docs)-1] + 1
				t.inline = inline
				t.run = nil
			}
		}
	}
}

// pack compresses each unit, or leaves them all raw if that is no smaller.
// Decided per section, since the codec is recorded per section.
func (b *Builder) pack(units [][]byte) ([][]byte, uint8, error) {
	if !b.options.Compress {
		return units, codecRaw, nil
	}
	raw, packedLen := 0, 0
	packed := make([][]byte, len(units))
	for i, unit := range units {
		p, err := compressUnit(unit)
		if err != nil {
			return nil, 0, err
		}
		packed[i] = p
		raw += len(unit)
		packedLen += len(p)
	}
	if packedLen < raw {
		return packed, codecDeflate, nil
	}
	return units, codecRaw, nil
}

func (b *Builder) packOne(unit []byte) ([]byte, uint8, error) {
	units, codec, err := b.pack([][]byte{unit})
	if err != nil {
		return nil, 0, err
	}
	return units[0], codec, nil
}

func sumLen(units [][]byte) uint64 {
	var n uint64
	for _, u := range units {
		n += uint64(len(u))
	}
	return n
}

// Finish finishes the file, with meta as the metadata section: canonical
// CBOR, such as Metadata.Encode makes. It carries the provenance of a source
// the writer never sees, so composing it is the caller's job.
func (b *Builder) Finish(meta []byte) ([]byte, error) {
	b.closeTerm()
	if len(b.terms) == 0 {
		return nil, buildError("an index with no terms cannot be built")
	}
	b.inlinePostings()
	per := b.options.TermsPerBlock
	var chunks [][]builtTerm
	for start := 0; start < len(b.terms); start += per {
		chunks = append(chunks, b.terms[start:min(start+per, len(b.terms))])
	}

	// Payload blocks. Deltas never cross a block boundary: a block is
	// addressed on its own and must decode on its own.
	perBlock := int(b.options.DocsPerPayloadBlock)
	var rawPayload [][]byte
	for start := 0; start < b.Documents(); start += perBlock {
		end := min(start+perBlock, b.Documents())
		block := putVarint(nil, uint64(end-start))
		for doc := start; doc < end; doc++ {
			if !b.options.Payload.Ints {
				block = putVarint(block, uint64(len(b.payloads[doc])))
				block = append(block, b.payloads[doc]...)
				continue
			}
			for i, v := range b.values[doc] {
				if doc > start {
					v -= b.values[doc-1][i]
				}
				block = putVarint(block, zigzag(v))
			}
		}
		rawPayload = append(rawPayload, block)
	}
	payloadBlocks, payloadCodec, err := b.pack(rawPayload)
	if err != nil {
		return nil, err
	}

	termIndex := binary.LittleEndian.AppendUint32(nil, uint32(len(chunks)))
	var previous []byte
	for _, chunk := range chunks {
		termIndex = putKey(termIndex, chunk[0].term, previous)
		previous = chunk[0].term
	}
	termIndex, termIndexCodec, err := b.packOne(termIndex)
	if err != nil {
		return nil, err
	}

	// Like the term index, completions depend on the terms and their runs and
	// on no offset, so they can be sized before anything is placed.
	var completions []byte
	var completionsCodec uint8
	if c := b.options.Completions; c != nil {
		if completions, completionsCodec, err = b.packOne(completionsSection(b.terms, *c)); err != nil {
			return nil, err
		}
	}

	var docLengths []byte
	if b.options.DocLengths {
		lengths := make([]uint64, b.Documents())
		copy(lengths, b.docLengths)
		docLengths = putValues([]byte{}, lengths)
	}

	var attributes []byte
	if len(b.columns) > 0 {
		attributes = putVarint(nil, uint64(len(b.columns)))
		for _, column := range b.columns {
			if len(column.Values) != b.Documents() {
				return nil, buildError("a document attribute column does not have one value per document")
			}
			attributes = putVarint(attributes, uint64(len(column.Name)))
			attributes = append(attributes, column.Name...)
			attributes = append(attributes, byte(int8(column.ScaleExponent)))
		}
		for _, column := range b.columns {
			zigzagged := make([]uint64, len(column.Values))
			for i, v := range column.Values {
				zigzagged[i] = zigzag(v)
			}
			attributes = putValues(attributes, zigzagged)
		}
	}

	// The order the sections are written in. The resident ones go first, so
	// that a cold start fetches them in one read; postings and positions go
	// before the term blocks that point into them, so that an offset is known
	// before any block is built and the file is written in one pass.
	order := []uint16{SectionMeta, SectionTermIndex}
	if completions != nil {
		order = append(order, SectionCompletions)
	}
	order = append(order, SectionTermBlockOffsets, SectionPayloadOffsets)
	if docLengths != nil {
		order = append(order, SectionDocLengths)
	}
	if attributes != nil {
		order = append(order, SectionDocAttributes)
	}
	order = append(order, SectionPostings)
	if b.options.Positions {
		order = append(order, SectionPositions)
	}
	order = append(order, SectionTermBlocks, SectionPayload)
	// Application sections go last, behind everything the format defines.
	application := make([]uint16, 0, len(b.sections))
	for id := range b.sections {
		application = append(application, id)
	}
	slices.Sort(application)
	order = append(order, application...)

	tableLen := uint64(sectionEntryLen * len(order))
	var postingsLen, positionsLen uint64
	for _, t := range b.terms {
		postingsLen += uint64(len(t.run))
		positionsLen += uint64(len(t.positions))
	}
	termBlockOffsetsLen := 8 * uint64(len(chunks)+1)
	payloadOffsetsLen := 12 + 8*uint64(len(payloadBlocks)+1)
	postingsAt := headerLen + tableLen + uint64(len(meta)) + uint64(len(termIndex)) +
		uint64(len(completions)) + termBlockOffsetsLen + payloadOffsetsLen +
		uint64(len(docLengths)) + uint64(len(attributes))
	positionsAt := postingsAt + postingsLen
	blocks, termBlocksCodec, err := b.pack(b.termBlocks(chunks, postingsAt, positionsAt))
	if err != nil {
		return nil, err
	}

	lengthOf := map[uint16]uint64{
		SectionMeta:             uint64(len(meta)),
		SectionTermIndex:        uint64(len(termIndex)),
		SectionCompletions:      uint64(len(completions)),
		SectionTermBlockOffsets: termBlockOffsetsLen,
		SectionPayloadOffsets:   payloadOffsetsLen,
		SectionDocLengths:       uint64(len(docLengths)),
		SectionDocAttributes:    uint64(len(attributes)),
		SectionPostings:         postingsLen,
		SectionPositions:        positionsLen,
		SectionTermBlocks:       sumLen(blocks),
		SectionPayload:          sumLen(payloadBlocks),
	}
	for id, data := range b.sections {
		lengthOf[id] = uint64(len(data))
	}
	offsetOf := map[uint16]uint64{}
	cursor := headerLen + tableLen
	for _, id := range order {
		offsetOf[id] = cursor
		cursor += lengthOf[id]
	}

	codecOf := map[uint16]uint8{
		SectionTermIndex:   termIndexCodec,
		SectionCompletions: completionsCodec,
		SectionTermBlocks:  termBlocksCodec,
		SectionPayload:     payloadCodec,
		// Metadata stays readable without a decompressor, and the offset
		// tables stay raw because their size feeds back into the placement
		// they describe.
	}

	payloadOffsets := []byte{payloadOpaque, 0, 0, 0}
	if b.options.Payload.Ints {
		payloadOffsets = []byte{payloadDeltaInts, b.options.Payload.ValuesPerDoc, byte(b.options.Payload.ScaleExponent), 0}
	}
	payloadOffsets = binary.LittleEndian.AppendUint32(payloadOffsets, b.options.DocsPerPayloadBlock)
	payloadOffsets = binary.LittleEndian.AppendUint32(payloadOffsets, uint32(len(payloadBlocks)))
	payloadOffsets = offsetTable(payloadOffsets, offsetOf[SectionPayload], payloadBlocks)

	flags := uint16(flagHasPayload)
	if b.options.HasFreqs {
		flags |= flagHasFreqs
	}
	if b.options.Positions {
		flags |= flagHasPositions
	}
	out := make([]byte, headerLen, cursor)
	copy(out, magic)
	out[9] = specMinor
	binary.LittleEndian.PutUint16(out[10:], flags)
	binary.LittleEndian.PutUint32(out[12:], uint32(len(order)))
	binary.LittleEndian.PutUint64(out[16:], uint64(b.Documents()))
	binary.LittleEndian.PutUint64(out[24:], b.sumDocLen)
	binary.LittleEndian.PutUint64(out[32:], headerLen)
	binary.LittleEndian.PutUint32(out[40:], uint32(tableLen))

	// The table is sorted by identifier, which is not the order the sections
	// are written in.
	sorted := slices.Clone(order)
	slices.Sort(sorted)
	for _, id := range sorted {
		entry := make([]byte, sectionEntryLen)
		binary.LittleEndian.PutUint16(entry, id)
		entry[2] = codecOf[id]
		binary.LittleEndian.PutUint64(entry[8:], offsetOf[id])
		binary.LittleEndian.PutUint64(entry[16:], lengthOf[id])
		out = append(out, entry...)
	}

	for _, id := range order {
		switch id {
		case SectionMeta:
			out = append(out, meta...)
		case SectionTermIndex:
			out = append(out, termIndex...)
		case SectionCompletions:
			out = append(out, completions...)
		case SectionTermBlockOffsets:
			out = offsetTable(out, offsetOf[SectionTermBlocks], blocks)
		case SectionPayloadOffsets:
			out = append(out, payloadOffsets...)
		case SectionDocLengths:
			out = append(out, docLengths...)
		case SectionDocAttributes:
			out = append(out, attributes...)
		case SectionPostings:
			for _, t := range b.terms {
				out = append(out, t.run...)
			}
		case SectionPositions:
			for _, t := range b.terms {
				out = append(out, t.positions...)
			}
		case SectionTermBlocks:
			for _, block := range blocks {
				out = append(out, block...)
			}
		case SectionPayload:
			for _, block := range payloadBlocks {
				out = append(out, block...)
			}
		default:
			out = append(out, b.sections[id]...)
		}
	}
	return out, nil
}

func (b *Builder) termBlocks(chunks [][]builtTerm, postingsBase, positionsBase uint64) [][]byte {
	blocks := make([][]byte, 0, len(chunks))
	var termID uint64
	postingsAt, positionsAt := postingsBase, positionsBase
	for _, chunk := range chunks {
		block := putVarint(nil, uint64(len(chunk)))
		block = putVarint(block, termID)
		var previousKey []byte
		for i, t := range chunk {
			block = putKey(block, t.term, previousKey)
			block = putVarint(block, t.docFreq)
			// The first entry's offsets are absolute; the rest are the
			// distance from the entry before, which is that entry's length.
			if i == 0 {
				block = putVarint(block, postingsAt)
			} else {
				block = putVarint(block, uint64(len(chunk[i-1].run)))
			}
			if t.inline != nil {
				block = putVarint(block, postingsInline)
				block = append(block, t.inline...)
			} else {
				// Zero for implied postings, which have no run.
				block = putVarint(block, uint64(len(t.run)))
			}
			if b.options.Positions {
				if i == 0 {
					block = putVarint(block, positionsAt)
				} else {
					block = putVarint(block, uint64(len(chunk[i-1].positions)))
				}
				block = putVarint(block, uint64(len(t.positions)))
				positionsAt += uint64(len(t.positions))
			}
			previousKey = t.term
			postingsAt += uint64(len(t.run))
		}
		termID += uint64(len(chunk))
		blocks = append(blocks, block)
	}
	return blocks
}

func offsetTable(out []byte, base uint64, blocks [][]byte) []byte {
	cursor := base
	for _, block := range blocks {
		out = binary.LittleEndian.AppendUint64(out, cursor)
		cursor += uint64(len(block))
	}
	return binary.LittleEndian.AppendUint64(out, cursor)
}

// completionsSection is the completions section, uncompressed (SPEC.md
// §5.3.5).
//
// Every prefix of every term is a candidate, and most are cheap, so this
// does not enumerate them. It walks down from one byte: the terms sharing a
// prefix are a contiguous run of the sorted dictionary, a prefix that is not
// expensive has no expensive extension, and so only the runs that were
// expensive at one length are split at the next.
func completionsSection(terms []builtTerm, c CompletionOptions) []byte {
	type entry struct {
		prefix []byte
		docs   []uint64
	}
	type run struct{ start, end, length int }
	var entries []entry
	pending := []run{{0, len(terms), 1}}
	for len(pending) > 0 {
		r := pending[len(pending)-1]
		pending = pending[:len(pending)-1]
		// Split the run by the byte at length-1. A term too short to have
		// one is the shared prefix itself, and sorts first.
		var groups [][2]int
		at := r.start
		for at < r.end && len(terms[at].term) < r.length {
			at++
		}
		for at < r.end {
			key := terms[at].term[:r.length]
			next := at + 1
			for next < r.end && bytes.HasPrefix(terms[next].term, key) {
				next++
			}
			groups = append(groups, [2]int{at, next})
			at = next
		}
		var expensive [][2]int
		for _, g := range groups {
			count := uint64(g[1] - g[0])
			var span uint64
			for _, t := range terms[g[0]:g[1]] {
				span += uint64(len(t.run))
			}
			if (c.MinTerms != 0 && count >= c.MinTerms) || (c.MaxSpan != 0 && span >= c.MaxSpan) {
				expensive = append(expensive, g)
			}
		}
		for _, g := range expensive {
			pending = append(pending, run{g[0], g[1], r.length + 1})
			var best []uint64
			for _, t := range terms[g[0]:g[1]] {
				best = append(best, t.head...)
			}
			slices.Sort(best)
			best = slices.Compact(best)
			best = best[:min(len(best), int(c.DocsPerEntry))]
			entries = append(entries, entry{terms[g[0]].term[:r.length], best})
		}
	}
	// The walk visits prefixes depth-first from a stack; the section wants
	// them in byte order.
	sort.SliceStable(entries, func(i, j int) bool { return bytes.Compare(entries[i].prefix, entries[j].prefix) < 0 })

	out := putVarint(nil, c.MinTerms)
	out = putVarint(out, c.MaxSpan)
	out = putVarint(out, uint64(c.DocsPerEntry))
	out = putVarint(out, uint64(len(entries)))
	var previous []byte
	for _, e := range entries {
		out = putKey(out, e.prefix, previous)
		previous = e.prefix
		out = putVarint(out, uint64(len(e.docs)))
		var last uint64
		for _, doc := range e.docs {
			out = putVarint(out, zigzag(int64(doc-last)))
			last = doc
		}
	}
	return out
}
