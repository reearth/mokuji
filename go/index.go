package mokuji

import (
	"bytes"
	"context"
	"encoding/binary"
	"io"
	"math"
	"slices"
	"sort"
	"strings"
	"sync"
	"sync/atomic"
)

const (
	magic            = "MOKUJIv0"
	headerLen        = 64
	sectionEntryLen  = 24
	flagHasPayload   = 1 << 1
	flagHasFreqs     = 1 << 2
	flagHasPositions = 1 << 3

	// A dictionary entry's postings_len when it states its postings itself
	// (SPEC.md §5.3.4). No run is this short.
	postingsImplied = 0
	postingsInline  = 1

	codecRaw     = 0
	codecDeflate = 1

	payloadOpaque    = 0
	payloadDeltaInts = 1

	// How much to ask for when opening. The resident set is sized to be
	// fetched in one request; this is a reader's guess at how large that is,
	// not a property of the format.
	openPrefix = 256 * 1024
)

// Section identifiers (SPEC.md §5).
const (
	SectionMeta             uint16 = 0x0001
	SectionTermIndex        uint16 = 0x0010
	SectionTermBlocks       uint16 = 0x0011
	SectionTermBlockOffsets uint16 = 0x0012
	SectionCompletions      uint16 = 0x0013
	SectionPostings         uint16 = 0x0020
	SectionPayload          uint16 = 0x0030
	SectionPayloadOffsets   uint16 = 0x0031
	SectionDocLengths       uint16 = 0x0032
	SectionDocAttributes    uint16 = 0x0033
	SectionPositions        uint16 = 0x0060
)

// Section is one entry of the section table.
type Section struct {
	Codec  uint8
	Offset uint64
	Length uint64
}

// Stats is what queries cost, so a caller can show it rather than claim it.
type Stats struct {
	Requests int
	Bytes    int64
}

// RequestInfo is one range request, as reported to Index.OnRequest.
type RequestInfo struct {
	Offset int64
	Length int64
}

// Postings is one term's postings: ascending document IDs and their
// frequencies.
type Postings struct {
	Docs  []int
	Freqs []uint64
}

// TermEntry is one dictionary entry (SPEC.md §5.3.4).
type TermEntry struct {
	Term           []byte
	TermID         uint64
	DocFreq        uint64
	PostingsOffset uint64
	// Bytes of the postings section the term's run occupies: zero when the
	// entry states its postings itself, implied or inline.
	PostingsLength uint64
	// The postings, when the entry carries them inline.
	Inline *Postings
	// Where the term's positions are, when the file has positions.
	PositionsOffset uint64
	PositionsLength uint64
}

// Provenance is one artifact an index was derived from, as its metadata
// records it (SPEC.md §6). Absent strings are empty.
type Provenance struct {
	// `<algorithm>:<lowercase hex>`.
	ContentHash string
	// Where it can be obtained.
	URI string
	// When the builder obtained it, RFC 3339.
	Retrieved string
	// When its publisher last changed it, RFC 3339: how current the data is.
	Modified string
}

// Column is one named column of per-document numbers (SPEC.md §5.8). Value
// i stands for Values[i] * 10^ScaleExponent.
type Column struct {
	Name          string
	ScaleExponent int
	Values        []int64
}

// Completions is the completions section (SPEC.md §5.3.5), keyed by prefix
// bytes.
type Completions struct {
	MinTerms     uint64
	MaxSpan      uint64
	DocsPerEntry uint64
	Entries      map[string][]int
}

// Index is one open Mokuji file: its resident set held, everything else read
// on demand.
//
// The exported fields describe the file and are set by Open; treat them as
// read-only. Queries may run from several goroutines at once.
type Index struct {
	// Header fields (SPEC.md §5.1).
	Flags     uint16
	NumDocs   uint64
	SumDocLen uint64
	Sections  map[uint16]Section

	// The metadata section, decoded (see decodeCBOR for the Go types).
	Meta any
	// The tokenizer the index records, "identity" when it records none.
	TokenizerID string
	// The index's license and the credit its results carry (SPEC.md §6),
	// empty when the file states none. A caller that shows results owes the
	// credit.
	License     string
	Attribution string
	Sources     []Provenance

	// Resident sections, nil when the file has none.
	DocLengths  []uint64
	Columns     map[string]*Column
	Completions *Completions

	// The term index's boundary keys: the first term of each dictionary
	// block.
	Boundaries [][]byte

	// Whether neighbouring blocks are read in one request (SPEC.md §4 allows
	// it: the count can only fall). True after Open; the conformance tests
	// turn it off to check the count the cases state, block by block. Set it
	// before querying.
	Coalesce bool
	// Called for every range request, so a caller can show the requests
	// themselves rather than a count of them. Set it before querying; it may
	// be called from several goroutines at once.
	OnRequest func(RequestInfo)

	source   Source
	owned    io.Closer
	numDocs  int
	tokenize tokenizer

	termBlockOffsets []byte
	payloadOffsets   []byte
	payloadCodec     byte
	valuesPerDoc     int
	scaleExponent    int
	docsPerBlock     int
	payloadBlocks    int

	mu    sync.Mutex
	stats Stats
}

// Open reads the resident set of the file src holds, in one request when the
// file is laid out as SPEC.md §5 recommends. The caller keeps ownership of
// src.
func Open(ctx context.Context, src Source) (*Index, error) {
	ix := &Index{source: src, Coalesce: true}
	if err := ix.open(ctx); err != nil {
		return nil, err
	}
	return ix, nil
}

// OpenFile opens a file on disk. A file of gigabytes is searched without
// being loaded, and costs what a query touches. Close closes it.
func OpenFile(ctx context.Context, path string) (*Index, error) {
	src, err := OpenFileSource(path)
	if err != nil {
		return nil, err
	}
	ix, err := Open(ctx, src)
	if err != nil {
		_ = src.Close()
		return nil, err
	}
	ix.owned = src
	return ix, nil
}

// OpenURL opens a file served over HTTP, read with Range requests made by
// http.DefaultClient. For another client, Open an HTTPSource.
func OpenURL(ctx context.Context, url string) (*Index, error) {
	return Open(ctx, NewHTTPSource(url))
}

// openLocation opens a path or an http(s) URL.
func openLocation(ctx context.Context, location string) (*Index, error) {
	if isURL(location) {
		return OpenURL(ctx, location)
	}
	return OpenFile(ctx, location)
}

// Close closes the file, when the index opened it with OpenFile.
func (ix *Index) Close() error {
	if ix.owned != nil {
		return ix.owned.Close()
	}
	return nil
}

// Stats is what every query has cost since opening or ResetStats.
func (ix *Index) Stats() Stats {
	ix.mu.Lock()
	defer ix.mu.Unlock()
	return ix.stats
}

// ResetStats starts the count again.
func (ix *Index) ResetStats() {
	ix.mu.Lock()
	defer ix.mu.Unlock()
	ix.stats = Stats{}
}

// FileLength is the file's length, once the source has said it: a path
// always does, a URL after the first response, and any other source if it
// implements Sizer.
func (ix *Index) FileLength() (int64, bool) {
	if sized, ok := ix.source.(Sizer); ok {
		if n := sized.Size(); n >= 0 {
			return n, true
		}
	}
	return 0, false
}

// A per-call count of requests, carried in the context, so that a query can
// say what it cost itself while others run beside it.
type tallyKey struct{}

func withTally(ctx context.Context) (context.Context, *atomic.Int64) {
	t := new(atomic.Int64)
	return context.WithValue(ctx, tallyKey{}, t), t
}

// read returns length bytes at offset, counted; fewer only at the end of the
// file.
func (ix *Index) read(ctx context.Context, offset, length uint64) ([]byte, error) {
	if length == 0 {
		return nil, nil
	}
	if offset > math.MaxInt64 || length > math.MaxInt64-offset {
		return nil, malformed("%d bytes at %d lie past any file", length, offset)
	}
	data, err := ix.source.ReadRange(ctx, int64(offset), int64(length))
	if err != nil {
		return nil, err
	}
	ix.mu.Lock()
	ix.stats.Requests++
	ix.stats.Bytes += int64(len(data))
	ix.mu.Unlock()
	if t, ok := ctx.Value(tallyKey{}).(*atomic.Int64); ok {
		t.Add(1)
	}
	if ix.OnRequest != nil {
		ix.OnRequest(RequestInfo{Offset: int64(offset), Length: int64(len(data))})
	}
	return data, nil
}

func (ix *Index) readExact(ctx context.Context, offset, length uint64) ([]byte, error) {
	data, err := ix.read(ctx, offset, length)
	if err != nil {
		return nil, err
	}
	if uint64(len(data)) != length {
		return nil, malformed("%d bytes at %d lie past the end of the file", length, offset)
	}
	return data, nil
}

func unit(data []byte, codec uint8) ([]byte, error) {
	switch codec {
	case codecRaw:
		return data, nil
	case codecDeflate:
		return inflate(data)
	}
	return nil, malformed("codec %d is registered but not implemented here", codec)
}

func (ix *Index) section(id uint16) (Section, error) {
	s, ok := ix.Sections[id]
	if !ok {
		return Section{}, malformed("section %#06x is missing", id)
	}
	return s, nil
}

func (ix *Index) open(ctx context.Context) error {
	// One request for the resident set, optimistically. SPEC.md §5 recommends
	// a layout that puts all of it at the head of the file for exactly this.
	prefix, err := ix.read(ctx, 0, openPrefix)
	if err != nil {
		return err
	}
	if len(prefix) < headerLen || string(prefix[:8]) != magic {
		return malformed("not a Mokuji file")
	}
	if prefix[8] != 0 {
		return malformed("unsupported major version %d", prefix[8])
	}
	le := binary.LittleEndian
	ix.Flags = le.Uint16(prefix[10:])
	count := uint64(le.Uint32(prefix[12:]))
	ix.NumDocs = le.Uint64(prefix[16:])
	ix.SumDocLen = le.Uint64(prefix[24:])
	tableOffset := le.Uint64(prefix[32:])
	tableLength := uint64(le.Uint32(prefix[40:]))
	if ix.NumDocs > maxDocs {
		return malformed("%d documents are more than this reader supports", ix.NumDocs)
	}
	ix.numDocs = int(ix.NumDocs)

	// Without frequencies nothing says where one document's positions end
	// and the next begin (SPEC.md §5.6a).
	if ix.Flags&flagHasPositions != 0 && ix.Flags&flagHasFreqs == 0 {
		return malformed("positions are flagged without frequencies")
	}
	if tableLength < count*sectionEntryLen {
		return malformed("the section table is shorter than its entries")
	}
	var table []byte
	if tableOffset <= uint64(len(prefix)) && tableLength <= uint64(len(prefix))-tableOffset {
		table = prefix[tableOffset : tableOffset+tableLength]
	} else if table, err = ix.readExact(ctx, tableOffset, tableLength); err != nil {
		return err
	}

	ix.Sections = make(map[uint16]Section, count)
	size, sized := ix.FileLength()
	previous := -1
	for i := uint64(0); i < count; i++ {
		entry := table[i*sectionEntryLen:]
		id := le.Uint16(entry)
		s := Section{Codec: entry[2], Offset: le.Uint64(entry[8:]), Length: le.Uint64(entry[16:])}
		if int(id) <= previous {
			return malformed("the section table is out of order")
		}
		previous = int(id)
		if s.Offset > math.MaxInt64 || s.Length > math.MaxInt64-s.Offset ||
			(sized && s.Offset+s.Length > uint64(size)) {
			return malformed("section %#06x lies past the end of the file", id)
		}
		ix.Sections[id] = s
	}

	for _, id := range []uint16{SectionTermBlocks, SectionPostings} {
		if _, err := ix.section(id); err != nil {
			return err
		}
	}
	wanted := []uint16{SectionMeta, SectionTermIndex, SectionTermBlockOffsets}
	if ix.Flags&flagHasPayload != 0 {
		if _, err := ix.section(SectionPayload); err != nil {
			return err
		}
		wanted = append(wanted, SectionPayloadOffsets)
	}
	if ix.Flags&flagHasPositions != 0 {
		if _, err := ix.section(SectionPositions); err != nil {
			return err
		}
	}
	// Both are read in bulk before any payload is, which is what makes them
	// worth having as sections of their own (SPEC.md §5.7, §5.8). A reader
	// that fetched them per query would pay for the separation twice.
	// Completions too: whether a prefix has an entry decides what it costs,
	// and has to be known before anything is read for it (SPEC.md §4.1.1).
	for _, id := range []uint16{SectionCompletions, SectionDocLengths, SectionDocAttributes} {
		if _, ok := ix.Sections[id]; ok {
			wanted = append(wanted, id)
		}
	}

	// The recommended layout puts everything resident at the head of the
	// file, so when the opening prefix does not reach far enough the remedy
	// is one wider read rather than one read per section.
	var spanEnd uint64
	for _, id := range wanted {
		s, err := ix.section(id)
		if err != nil {
			return err
		}
		spanEnd = max(spanEnd, s.Offset+s.Length)
	}
	span := prefix
	if spanEnd > uint64(len(prefix)) {
		// Only the part not already in hand. Re-reading the prefix would
		// double the bytes for no reason.
		rest, err := ix.readExact(ctx, uint64(len(prefix)), spanEnd-uint64(len(prefix)))
		if err != nil {
			return err
		}
		span = append(slices.Clip(prefix), rest...)
	}
	resident := func(id uint16) ([]byte, error) {
		s := ix.Sections[id]
		return unit(span[s.Offset:s.Offset+s.Length], s.Codec)
	}

	meta, err := resident(SectionMeta)
	if err != nil {
		return err
	}
	if ix.Meta, _, err = decodeCBOR(meta, 0, 0); err != nil {
		return err
	}
	ix.readMeta()

	if _, ok := ix.Sections[SectionDocLengths]; ok {
		data, err := resident(SectionDocLengths)
		if err != nil {
			return err
		}
		c := cursor{data: data}
		if ix.DocLengths, err = c.values(ix.NumDocs); err != nil {
			return err
		}
	}
	ix.Columns = map[string]*Column{}
	if _, ok := ix.Sections[SectionDocAttributes]; ok {
		data, err := resident(SectionDocAttributes)
		if err != nil {
			return err
		}
		if ix.Columns, err = decodeColumns(data, ix.NumDocs); err != nil {
			return err
		}
	}
	if _, ok := ix.Sections[SectionCompletions]; ok {
		data, err := resident(SectionCompletions)
		if err != nil {
			return err
		}
		if ix.Completions, err = decodeCompletions(data, ix.numDocs); err != nil {
			return err
		}
	}

	termIndex, err := resident(SectionTermIndex)
	if err != nil {
		return err
	}
	if ix.termBlockOffsets, err = resident(SectionTermBlockOffsets); err != nil {
		return err
	}

	// Boundary keys, decoded once. They are what lets a prefix scan know
	// whether the next block can extend the range without fetching it.
	c := cursor{data: termIndex}
	head, err := c.take(4)
	if err != nil {
		return err
	}
	blockCount := uint64(le.Uint32(head))
	if uint64(len(ix.termBlockOffsets)) < (blockCount+1)*8 {
		return malformed("the term block offsets are fewer than the blocks")
	}
	ix.Boundaries = make([][]byte, 0, blockCount)
	var key []byte
	for i := uint64(0); i < blockCount; i++ {
		next, err := c.key(key)
		if err != nil {
			return err
		}
		// SPEC.md §5.3.2: the search below would otherwise silently return
		// the wrong block.
		if i > 0 && bytes.Compare(next, key) <= 0 {
			return malformed("the term index is not strictly increasing")
		}
		ix.Boundaries = append(ix.Boundaries, next)
		key = next
	}

	if ix.Flags&flagHasPayload != 0 {
		if ix.payloadOffsets, err = resident(SectionPayloadOffsets); err != nil {
			return err
		}
		header := ix.payloadOffsets
		if len(header) < 12 {
			return malformed("the payload offsets section is truncated")
		}
		ix.payloadCodec = header[0]
		ix.valuesPerDoc = int(header[1])
		// A signed byte: the stored integers stand for value * 10^exponent.
		ix.scaleExponent = int(int8(header[2]))
		ix.docsPerBlock = int(le.Uint32(header[4:]))
		ix.payloadBlocks = int(le.Uint32(header[8:]))
		if ix.docsPerBlock == 0 {
			return malformed("payload blocks hold no documents")
		}
		if len(header) < 12+(ix.payloadBlocks+1)*8 {
			return malformed("the payload offsets are fewer than the blocks")
		}
	}
	return nil
}

func (ix *Index) readMeta() {
	tokenizer := field(field(ix.Meta, "tokenizer"), "id")
	ix.TokenizerID = text(tokenizer)
	if ix.TokenizerID == "" {
		ix.TokenizerID = "identity"
	}
	ix.License = text(field(ix.Meta, "license"))
	ix.Attribution = text(field(ix.Meta, "attribution"))
	if sources, ok := field(ix.Meta, "source").([]any); ok {
		for _, s := range sources {
			ix.Sources = append(ix.Sources, Provenance{
				ContentHash: text(field(s, "content_hash")),
				URI:         text(field(s, "uri")),
				Retrieved:   text(field(s, "retrieved")),
				Modified:    text(field(s, "modified")),
			})
		}
	}
	// A tokenizer this reader does not know leaves the file readable, just
	// not reachable by an unnormalized query: it is used as typed.
	ix.tokenize = tokenizers[ix.TokenizerID]
}

func text(v any) string {
	s, _ := v.(string)
	return s
}

func field(v any, key string) any {
	if m, ok := v.(map[any]any); ok {
		return m[key]
	}
	return nil
}

// decodeCompletions reads the completions section (SPEC.md §5.3.5), checked
// as a reader must.
func decodeCompletions(data []byte, numDocs int) (*Completions, error) {
	c := cursor{data: data}
	var head [4]uint64
	for i := range head {
		v, err := c.varint()
		if err != nil {
			return nil, err
		}
		head[i] = v
	}
	out := &Completions{MinTerms: head[0], MaxSpan: head[1], DocsPerEntry: head[2], Entries: map[string][]int{}}
	if out.DocsPerEntry == 0 {
		return nil, malformed("a completion entry lists no documents")
	}
	var previous []byte
	for i := uint64(0); i < head[3]; i++ {
		key, err := c.key(previous)
		if err != nil {
			return nil, err
		}
		if len(key) == 0 || (i > 0 && bytes.Compare(key, previous) <= 0) {
			return nil, malformed("completion prefixes are empty or out of order")
		}
		previous = key
		n, err := c.varint()
		if err != nil {
			return nil, err
		}
		if n == 0 || n > out.DocsPerEntry {
			return nil, malformed("a completion lists too few or too many")
		}
		docs := make([]int, 0, min(n, uint64(c.remaining())))
		var last int64
		for j := uint64(0); j < n; j++ {
			v, err := c.varint()
			if err != nil {
				return nil, err
			}
			step := unzigzag(v)
			if step > 0 && last > math.MaxInt64-step {
				return nil, malformed("a completion lists a document outside the index, or twice")
			}
			last += step
			if last < 0 || last >= int64(numDocs) || slices.Contains(docs, int(last)) {
				return nil, malformed("a completion lists a document outside the index, or twice")
			}
			docs = append(docs, int(last))
		}
		out.Entries[string(key)] = docs
	}
	if c.remaining() != 0 {
		return nil, malformed("trailing bytes in the completions section")
	}
	return out, nil
}

// decodeColumns reads named columns of per-document numbers (SPEC.md §5.8).
func decodeColumns(data []byte, numDocs uint64) (map[string]*Column, error) {
	c := cursor{data: data}
	count, err := c.varint()
	if err != nil {
		return nil, err
	}
	var headers []*Column
	for i := uint64(0); i < count; i++ {
		length, err := c.varint()
		if err != nil {
			return nil, err
		}
		name, err := c.take(length)
		if err != nil {
			return nil, err
		}
		raw, err := c.byte()
		if err != nil {
			return nil, err
		}
		column := &Column{Name: lossy(name), ScaleExponent: int(int8(raw))}
		for _, taken := range headers {
			if taken.Name == column.Name {
				return nil, malformed("two attribute columns share a name")
			}
		}
		headers = append(headers, column)
	}
	columns := make(map[string]*Column, len(headers))
	for _, column := range headers {
		values, err := c.values(numDocs)
		if err != nil {
			return nil, err
		}
		column.Values = make([]int64, len(values))
		for i, v := range values {
			column.Values[i] = unzigzag(v)
		}
		columns[column.Name] = column
	}
	if c.remaining() != 0 {
		return nil, malformed("trailing bytes in the document attributes section")
	}
	return columns, nil
}

// Normalize is what a query goes through before it is looked up as a prefix:
// the normalization of the tokenizer the index records (TOKENIZERS.md).
func (ix *Index) Normalize(text string) string {
	if ix.tokenize.normalize == nil {
		return text
	}
	return ix.tokenize.normalize(text)
}

// Split is the terms a query is looked up as by Search: the query through
// the index's tokenizer whole, so a tokenizer that splits turns what was
// typed into several terms.
func (ix *Index) Split(query string) []string {
	if ix.tokenize.split == nil {
		return []string{query}
	}
	return ix.tokenize.split(query)
}

// --- blocks ----------------------------------------------------------------

// BlockCount is how many dictionary blocks the file has.
func (ix *Index) BlockCount() int { return len(ix.Boundaries) }

// offsetPair is where block i of a block-addressed section is.
func (ix *Index) offsetPair(table []byte, base, i int, id uint16) (offset, length uint64, err error) {
	start := binary.LittleEndian.Uint64(table[base+i*8:])
	end := binary.LittleEndian.Uint64(table[base+i*8+8:])
	s := ix.Sections[id]
	if start < s.Offset || start >= end || end > s.Offset+s.Length {
		return 0, 0, malformed("block %d lies outside its section", i)
	}
	return start, end - start, nil
}

func (ix *Index) termBlockLocation(ordinal int) (uint64, uint64, error) {
	if ordinal < 0 || ordinal >= len(ix.Boundaries) {
		return 0, 0, malformed("there is no dictionary block %d", ordinal)
	}
	return ix.offsetPair(ix.termBlockOffsets, 0, ordinal, SectionTermBlocks)
}

func (ix *Index) payloadBlockLocation(ordinal int) (uint64, uint64, error) {
	if ordinal >= ix.payloadBlocks {
		return 0, 0, malformed("a document lies past the last payload block")
	}
	return ix.offsetPair(ix.payloadOffsets, 12, ordinal, SectionPayload)
}

// blockRuns reads blocks of one section by ordinal, a request for each run
// of neighbours.
//
// Blocks are laid end to end, so ordinals that follow one another are one
// contiguous range of the file, and reading them as one costs a single
// request where reading them apart costs one each. Returns each block as
// stored, before its codec is undone.
func (ix *Index) blockRuns(ctx context.Context, ordinals []int, locate func(int) (uint64, uint64, error)) (map[int][]byte, error) {
	sorted := slices.Clone(ordinals)
	slices.Sort(sorted)
	sorted = slices.Compact(sorted)
	var runs [][]int
	for _, ordinal := range sorted {
		if n := len(runs); n > 0 && ix.Coalesce && runs[n-1][len(runs[n-1])-1] == ordinal-1 {
			runs[n-1] = append(runs[n-1], ordinal)
		} else {
			runs = append(runs, []int{ordinal})
		}
	}
	out := make(map[int][]byte, len(sorted))
	for _, run := range runs {
		start, _, err := locate(run[0])
		if err != nil {
			return nil, err
		}
		lastOffset, lastLength, err := locate(run[len(run)-1])
		if err != nil {
			return nil, err
		}
		if lastOffset < start {
			return nil, malformed("blocks are not laid out in order")
		}
		data, err := ix.readExact(ctx, start, lastOffset+lastLength-start)
		if err != nil {
			return nil, err
		}
		for _, ordinal := range run {
			offset, length, err := locate(ordinal)
			if err != nil {
				return nil, err
			}
			if offset < start || offset-start+length > uint64(len(data)) {
				return nil, malformed("blocks are not laid out in order")
			}
			out[ordinal] = data[offset-start : offset-start+length]
		}
	}
	return out, nil
}

// TermBlocks reads several dictionary blocks, decoded, a request for each run
// of neighbours.
func (ix *Index) TermBlocks(ctx context.Context, ordinals []int) (map[int][]TermEntry, error) {
	raw, err := ix.blockRuns(ctx, ordinals, ix.termBlockLocation)
	if err != nil {
		return nil, err
	}
	out := make(map[int][]TermEntry, len(raw))
	for ordinal, data := range raw {
		if out[ordinal], err = ix.decodeTermBlock(data); err != nil {
			return nil, err
		}
	}
	return out, nil
}

// TermBlock reads one dictionary block, decoded.
func (ix *Index) TermBlock(ctx context.Context, ordinal int) ([]TermEntry, error) {
	offset, length, err := ix.termBlockLocation(ordinal)
	if err != nil {
		return nil, err
	}
	data, err := ix.readExact(ctx, offset, length)
	if err != nil {
		return nil, err
	}
	return ix.decodeTermBlock(data)
}

func (ix *Index) decodeTermBlock(raw []byte) ([]TermEntry, error) {
	block, err := unit(raw, ix.Sections[SectionTermBlocks].Codec)
	if err != nil {
		return nil, err
	}
	c := cursor{data: block}
	entryCount, err := c.varint()
	if err != nil {
		return nil, err
	}
	firstTermID, err := c.varint()
	if err != nil {
		return nil, err
	}
	hasPositions := ix.Flags&flagHasPositions != 0
	hasFreqs := ix.Flags&flagHasFreqs != 0
	// Every entry takes at least a byte.
	if entryCount > uint64(c.remaining()) {
		return nil, errTruncated
	}
	entries := make([]TermEntry, 0, entryCount)
	var key []byte
	var postingsOffset, positionsOffset uint64
	// One more than the last document an implied or inline entry named: what
	// an inline entry's first document is written against (SPEC.md §5.3.4).
	expected := firstTermID
	for i := uint64(0); i < entryCount; i++ {
		if key, err = c.key(key); err != nil {
			return nil, err
		}
		termID := firstTermID + i
		docFreq, err := c.varint()
		if err != nil {
			return nil, err
		}
		delta, err := c.varint()
		if err != nil {
			return nil, err
		}
		if postingsOffset, err = addOffset(postingsOffset, delta); err != nil {
			return nil, err
		}
		stored, err := c.varint()
		if err != nil {
			return nil, err
		}
		entry := TermEntry{Term: key, TermID: termID, DocFreq: docFreq, PostingsOffset: postingsOffset, PostingsLength: stored}
		switch stored {
		case postingsImplied:
			expected = termID + 1
		case postingsInline:
			entry.PostingsLength = 0
			if entry.Inline, err = ix.inlinePostings(&c, docFreq, expected, hasFreqs); err != nil {
				return nil, err
			}
			expected = uint64(entry.Inline.Docs[len(entry.Inline.Docs)-1]) + 1
		}
		if hasPositions {
			// Two further varints an entry carries only when the file has
			// positions, which is why the flag is read before any entry is.
			gap, err := c.varint()
			if err != nil {
				return nil, err
			}
			if positionsOffset, err = addOffset(positionsOffset, gap); err != nil {
				return nil, err
			}
			entry.PositionsOffset = positionsOffset
			if entry.PositionsLength, err = c.varint(); err != nil {
				return nil, err
			}
		}
		entries = append(entries, entry)
	}
	return entries, nil
}

func addOffset(offset, delta uint64) (uint64, error) {
	if delta > math.MaxUint64-offset {
		return 0, malformed("an offset overflows 64 bits")
	}
	return offset + delta, nil
}

// inlinePostings reads the postings a dictionary entry carries itself.
func (ix *Index) inlinePostings(c *cursor, docFreq, expected uint64, hasFreqs bool) (*Postings, error) {
	missing := malformed("inline postings name a missing document")
	if docFreq == 0 {
		return nil, malformed("inline postings name no document")
	}
	if docFreq > uint64(c.remaining()) {
		return nil, errTruncated
	}
	v, err := c.varint()
	if err != nil {
		return nil, err
	}
	step := unzigzag(v)
	if expected > math.MaxInt64 || (step > 0 && int64(expected) > math.MaxInt64-step) {
		return nil, missing
	}
	first := int64(expected) + step
	if first < 0 || first >= int64(ix.numDocs) {
		return nil, missing
	}
	docs := make([]int, 1, docFreq)
	docs[0] = int(first)
	for len(docs) < int(docFreq) {
		gap, err := c.varint()
		if err != nil {
			return nil, err
		}
		if gap >= uint64(ix.numDocs) {
			return nil, missing
		}
		doc := docs[len(docs)-1] + int(gap) + 1
		if doc >= ix.numDocs {
			return nil, missing
		}
		docs = append(docs, doc)
	}
	freqs, err := frequencies(c, len(docs), hasFreqs)
	if err != nil {
		return nil, err
	}
	return &Postings{Docs: docs, Freqs: freqs}, nil
}

// frequencies reads one varint, less one, for each document; or, in a file
// without frequencies, says one each.
func frequencies(c *cursor, n int, hasFreqs bool) ([]uint64, error) {
	freqs := make([]uint64, n)
	for i := range freqs {
		if !hasFreqs {
			freqs[i] = 1
			continue
		}
		v, err := c.varint()
		if err != nil {
			return nil, err
		}
		if v == math.MaxUint64 {
			return nil, malformed("a frequency overflows 64 bits")
		}
		freqs[i] = v + 1
	}
	return freqs, nil
}

// --- the resident set ------------------------------------------------------

// Column is one named column of per-document numbers, or nil.
//
// Resident, so this costs no read: the point of the section is that a scorer
// can weigh many documents before deciding which payloads are worth
// fetching.
func (ix *Index) Column(name string) *Column {
	return ix.Columns[name]
}

// BlockFor is the dictionary block a term would be in, from the resident
// index alone. Block 0 for a term before every block, as the TypeScript
// reader answers; Exact knows such a term is absent without reading it.
func (ix *Index) BlockFor(term []byte) int {
	i := sort.Search(len(ix.Boundaries), func(i int) bool {
		return bytes.Compare(ix.Boundaries[i], term) > 0
	})
	return max(i-1, 0)
}

// beforeEveryBlock is SPEC.md §5.3.2: a term that precedes the first
// boundary key is absent, and the resident index has said so already.
func (ix *Index) beforeEveryBlock(term []byte) bool {
	return len(ix.Boundaries) == 0 || bytes.Compare(term, ix.Boundaries[0]) < 0
}

// nothingBeginsWith says, from the resident index alone, that no term can
// begin with prefix. The first boundary key is the smallest term; when it
// sorts after the prefix without beginning with it, every term does, and
// none can begin with it either.
func (ix *Index) nothingBeginsWith(prefix []byte) bool {
	if len(ix.Boundaries) == 0 {
		return true
	}
	first := ix.Boundaries[0]
	return bytes.Compare(prefix, first) < 0 && !bytes.HasPrefix(first, prefix)
}

// --- terms -----------------------------------------------------------------

// Positions is where a term occurs in each document its postings name.
//
// One read, and only for a caller that asks. The run is decoded from the
// front and the frequencies say how many values belong to each document, so
// the postings have to be in hand first.
func (ix *Index) Positions(ctx context.Context, entry *TermEntry, postings Postings) ([][]uint64, error) {
	if ix.Flags&flagHasPositions == 0 || entry.PositionsLength == 0 {
		return [][]uint64{}, nil
	}
	s := ix.Sections[SectionPositions]
	offset, length := entry.PositionsOffset, entry.PositionsLength
	if offset < s.Offset || offset > s.Offset+s.Length || length > s.Offset+s.Length-offset {
		return nil, malformed("a positions run lies outside its section")
	}
	data, err := ix.readExact(ctx, offset, length)
	if err != nil {
		return nil, err
	}
	c := cursor{data: data}
	out := make([][]uint64, 0, len(postings.Freqs))
	for _, freq := range postings.Freqs {
		values, err := c.values(freq)
		if err != nil {
			return nil, err
		}
		// The first is a position; the rest are gaps less one, so that
		// adjacent occurrences pack to nothing.
		places := make([]uint64, len(values))
		for i, v := range values {
			if i > 0 {
				prev := places[i-1]
				if v >= math.MaxUint64-prev {
					return nil, malformed("a position overflows 64 bits")
				}
				v = prev + v + 1
			}
			places[i] = v
		}
		out = append(out, places)
	}
	if c.remaining() != 0 {
		return nil, malformed("trailing bytes in positions run")
	}
	return out, nil
}

// findIn is the entry for term in a decoded block, or nil.
func findIn(entries []TermEntry, term []byte) *TermEntry {
	for i := range entries {
		switch bytes.Compare(entries[i].Term, term) {
		case 0:
			return &entries[i]
		case 1:
			return nil
		}
	}
	return nil
}

// Exact is one term, exactly, or nil.
//
// A prefix scan would answer this, and would cost more: `xn76ur` is a prefix
// of every cell inside it, so completing it fetches postings for all of them
// when the caller wanted one. At most one block is read.
func (ix *Index) Exact(ctx context.Context, term string) (*TermEntry, error) {
	key := []byte(term)
	if ix.beforeEveryBlock(key) {
		return nil, nil
	}
	entries, err := ix.TermBlock(ctx, ix.BlockFor(key))
	if err != nil {
		return nil, err
	}
	return findIn(entries, key), nil
}

// ExactMany looks up several terms at once, each exactly, in one read per
// dictionary block. An absent term is nil.
//
// Cells one geohash precision apart are neighbours in the dictionary, so the
// lookups a point needs usually land in the same block. Asking for them
// together is the difference between reading it once and reading it twice.
func (ix *Index) ExactMany(ctx context.Context, terms []string) ([]*TermEntry, error) {
	blockOf := make([]int, len(terms))
	var ordinals []int
	for i, term := range terms {
		blockOf[i] = -1
		if key := []byte(term); !ix.beforeEveryBlock(key) {
			blockOf[i] = ix.BlockFor(key)
			ordinals = append(ordinals, blockOf[i])
		}
	}
	blocks, err := ix.TermBlocks(ctx, ordinals)
	if err != nil {
		return nil, err
	}
	out := make([]*TermEntry, len(terms))
	for i, term := range terms {
		if blockOf[i] >= 0 {
			out[i] = findIn(blocks[blockOf[i]], []byte(term))
		}
	}
	return out, nil
}

// Prefix is the terms beginning with prefix, at most limit of them; a limit
// of zero or less means all.
func (ix *Index) Prefix(ctx context.Context, prefix string, limit int) ([]TermEntry, error) {
	key := []byte(prefix)
	found := []TermEntry{}
	if ix.nothingBeginsWith(key) {
		return found, nil
	}
	start := ix.BlockFor(key)
	for ordinal := start; ordinal < len(ix.Boundaries); ordinal++ {
		// Whether the range continues is answerable from the resident index
		// alone, so only the first block is fetched on faith.
		if ordinal > start && !bytes.HasPrefix(ix.Boundaries[ordinal], key) {
			break
		}
		entries, err := ix.TermBlock(ctx, ordinal)
		if err != nil {
			return nil, err
		}
		for _, entry := range entries {
			if bytes.HasPrefix(entry.Term, key) {
				found = append(found, entry)
				if len(found) == limit {
					return found, nil
				}
			} else if bytes.Compare(entry.Term, key) > 0 && len(found) > 0 {
				return found, nil
			}
		}
	}
	return found, nil
}

// PrefixMany is the terms beginning with each of several prefixes, every one
// of them.
//
// Which blocks each prefix spans is answerable from the resident index, so
// all of them are known before any is read, and they are fetched together: a
// request for each run of neighbouring blocks, however many prefixes fall in
// it. This is what a query over an area needs (the cells covering it are
// many short prefixes, mostly in blocks next to one another), where Prefix,
// which walks one prefix a block at a time, would make a request per block
// per prefix.
func (ix *Index) PrefixMany(ctx context.Context, prefixes []string) ([][]TermEntry, error) {
	type span struct{ start, end int }
	spans := make([]*span, len(prefixes))
	var ordinals []int
	for i, prefix := range prefixes {
		key := []byte(prefix)
		if ix.nothingBeginsWith(key) {
			continue
		}
		s := &span{ix.BlockFor(key), ix.BlockFor(key)}
		for s.end+1 < len(ix.Boundaries) && bytes.HasPrefix(ix.Boundaries[s.end+1], key) {
			s.end++
		}
		spans[i] = s
		for o := s.start; o <= s.end; o++ {
			ordinals = append(ordinals, o)
		}
	}
	blocks, err := ix.TermBlocks(ctx, ordinals)
	if err != nil {
		return nil, err
	}
	out := make([][]TermEntry, len(prefixes))
	for i, prefix := range prefixes {
		key := []byte(prefix)
		out[i] = []TermEntry{}
		if s := spans[i]; s != nil {
			for o := s.start; o <= s.end; o++ {
				for _, entry := range blocks[o] {
					if bytes.HasPrefix(entry.Term, key) {
						out[i] = append(out[i], entry)
					}
				}
			}
		}
	}
	return out, nil
}

// Range is the terms from low to high, both inclusive, at most limit of them;
// a limit of zero or less means all.
//
// The same walk as Prefix, with a different test for where it stops: the
// dictionary is sorted, so a range of keys is a run of neighbouring entries,
// and whether the run continues past a block is answerable from the resident
// index alone.
func (ix *Index) Range(ctx context.Context, low, high string, limit int) ([]TermEntry, error) {
	lo, hi := []byte(low), []byte(high)
	found := []TermEntry{}
	if len(ix.Boundaries) == 0 || bytes.Compare(hi, ix.Boundaries[0]) < 0 {
		return found, nil
	}
	start := ix.BlockFor(lo)
	for ordinal := start; ordinal < len(ix.Boundaries); ordinal++ {
		if ordinal > start && bytes.Compare(ix.Boundaries[ordinal], hi) > 0 {
			break
		}
		entries, err := ix.TermBlock(ctx, ordinal)
		if err != nil {
			return nil, err
		}
		for _, entry := range entries {
			if bytes.Compare(entry.Term, hi) > 0 {
				return found, nil
			}
			if bytes.Compare(entry.Term, lo) >= 0 {
				found = append(found, entry)
				if len(found) == limit {
					return found, nil
				}
			}
		}
	}
	return found, nil
}

// --- postings and payload --------------------------------------------------

// Implied is the postings a dictionary entry states itself (SPEC.md §5.3.4):
// inline, or implied, when the term names one document, whose ID is the
// term's own, once.
func (ix *Index) Implied(entry *TermEntry) (Postings, error) {
	if entry.Inline != nil {
		return *entry.Inline, nil
	}
	if entry.DocFreq != 1 {
		return Postings{}, malformed("implied postings need a doc_freq of 1")
	}
	if entry.TermID >= ix.NumDocs {
		return Postings{}, malformed("implied postings name a missing document")
	}
	return Postings{Docs: []int{int(entry.TermID)}, Freqs: []uint64{1}}, nil
}

// PostingsSpan reads the postings of a contiguous range of entries, in one
// request, or none.
func (ix *Index) PostingsSpan(ctx context.Context, entries []TermEntry) ([]Postings, error) {
	out := make([]Postings, 0, len(entries))
	if len(entries) == 0 {
		return out, nil
	}
	// Entries whose postings are implied or inline contribute no bytes, so an
	// index of a dictionary spends no request here at all.
	var first, last *TermEntry
	for i := range entries {
		if entries[i].PostingsLength > 0 {
			if first == nil {
				first = &entries[i]
			}
			last = &entries[i]
		}
	}
	var start uint64
	var span []byte
	if first != nil {
		start = first.PostingsOffset
		end, err := addOffset(last.PostingsOffset, last.PostingsLength)
		if err != nil {
			return nil, err
		}
		s := ix.Sections[SectionPostings]
		if start < s.Offset || start > end || end > s.Offset+s.Length {
			return nil, malformed("a postings run lies outside its section")
		}
		if span, err = ix.readExact(ctx, start, end-start); err != nil {
			return nil, err
		}
	}

	hasFreqs := ix.Flags&flagHasFreqs != 0
	for i := range entries {
		entry := &entries[i]
		if entry.PostingsLength == 0 {
			p, err := ix.Implied(entry)
			if err != nil {
				return nil, err
			}
			out = append(out, p)
			continue
		}
		if entry.PostingsOffset < start || entry.PostingsOffset-start > uint64(len(span)) ||
			entry.PostingsLength > uint64(len(span))-(entry.PostingsOffset-start) {
			return nil, malformed("postings runs are not in term order")
		}
		at := entry.PostingsOffset - start
		p, err := ix.decodeRun(span[at:at+entry.PostingsLength], entry.DocFreq, hasFreqs)
		if err != nil {
			return nil, err
		}
		out = append(out, p)
	}
	return out, nil
}

func (ix *Index) decodeRun(run []byte, docFreq uint64, hasFreqs bool) (Postings, error) {
	c := cursor{data: run}
	codec, err := c.byte()
	if err != nil {
		return Postings{}, err
	}
	if codec != 0 {
		return Postings{}, malformed("unsupported postings codec")
	}
	count, err := c.varint()
	if err != nil {
		return Postings{}, err
	}
	// The dictionary and the postings are separate sections, and nothing but
	// this ties them together.
	if count != docFreq {
		return Postings{}, malformed("doc_freq and the postings run disagree")
	}
	values, err := c.values(count)
	if err != nil {
		return Postings{}, err
	}
	missing := malformed("a postings run names a missing document")
	// The first value is a document ID; the rest are gaps less one.
	docs := make([]int, len(values))
	for i, v := range values {
		if v >= uint64(ix.numDocs) {
			return Postings{}, missing
		}
		if i > 0 {
			v += uint64(docs[i-1]) + 1
			if v >= uint64(ix.numDocs) {
				return Postings{}, missing
			}
		}
		docs[i] = int(v)
	}
	freqs := make([]uint64, len(docs))
	if hasFreqs {
		raw, err := c.values(count)
		if err != nil {
			return Postings{}, err
		}
		for i, f := range raw {
			if f == math.MaxUint64 {
				return Postings{}, malformed("a frequency overflows 64 bits")
			}
			freqs[i] = f + 1
		}
	} else {
		for i := range freqs {
			freqs[i] = 1
		}
	}
	if c.remaining() != 0 {
		return Postings{}, malformed("trailing bytes in a postings run")
	}
	return Postings{Docs: docs, Freqs: freqs}, nil
}

// PayloadSpan is the payload of each of docs, as text: opaque bytes decoded,
// or integers written out against the scale the file declares. Empty strings
// when the file has no payload.
func (ix *Index) PayloadSpan(ctx context.Context, docs []int) ([]string, error) {
	out := make([]string, len(docs))
	if ix.payloadOffsets == nil || len(docs) == 0 {
		return out, nil
	}
	perBlock := ix.docsPerBlock
	// Every block the documents fall in, read once each. Neighbouring blocks
	// are one range of the file and read as one.
	ordinals := make([]int, len(docs))
	for i, doc := range docs {
		if doc < 0 {
			return nil, malformed("there is no document %d", doc)
		}
		ordinals[i] = doc / perBlock
	}
	raw, err := ix.blockRuns(ctx, ordinals, ix.payloadBlockLocation)
	if err != nil {
		return nil, err
	}
	codec := ix.Sections[SectionPayload].Codec
	blocks := make(map[int][]byte, len(raw))
	for ordinal, data := range raw {
		if blocks[ordinal], err = unit(data, codec); err != nil {
			return nil, err
		}
	}

	for i, doc := range docs {
		c := cursor{data: blocks[doc/perBlock]}
		count, err := c.varint()
		if err != nil {
			return nil, err
		}
		index := doc % perBlock
		if uint64(index) >= count {
			return nil, malformed("payload block is shorter than expected")
		}
		switch ix.payloadCodec {
		case payloadOpaque:
			for j := 0; j <= index; j++ {
				length, err := c.varint()
				if err != nil {
					return nil, err
				}
				value, err := c.take(length)
				if err != nil {
					return nil, err
				}
				if j == index {
					out[i] = lossy(value)
				}
			}
		case payloadDeltaInts:
			// Every entry up to the wanted one has to be decoded, because each
			// is a difference from the one before it. A block is what gets
			// fetched anyway, and it is sized so that this is cheap.
			values := make([]int64, ix.valuesPerDoc)
			for j := 0; j <= index; j++ {
				for k := range values {
					v, err := c.varint()
					if err != nil {
						return nil, err
					}
					values[k] += unzigzag(v)
				}
			}
			parts := make([]string, len(values))
			for k, v := range values {
				parts[k] = scaled(v, ix.scaleExponent)
			}
			out[i] = strings.Join(parts, ",")
		default:
			return nil, malformed("payload codec %d is not implemented here", ix.payloadCodec)
		}
	}
	return out, nil
}
