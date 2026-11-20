package mokuji

import (
	"bytes"
	"fmt"
	"slices"
)

// Metadata is the metadata section (SPEC.md §6), for a writer: what an index
// was derived from and how, which together are required to be sufficient to
// rebuild it byte for byte (§3.4).
type Metadata struct {
	// Monotonic generation number. A file is immutable; a new index is a new
	// generation, not an edit.
	Generation uint64
	// The sources the index was derived from. ContentHash is required, as
	// `<algorithm>:<lowercase hex>`; the other fields are recorded when not
	// empty.
	Sources []Provenance
	// What built it: GeneratorName and Version for this writer.
	GeneratorName    string
	GeneratorVersion string
	// The full build parameters, such as WriteOptions.Params gives. Anything
	// the writer branches on belongs here.
	GeneratorParams map[string]any
	// How it was tokenized: an identifier from TOKENIZERS.md, and its
	// parameters.
	TokenizerID     string
	TokenizerParams map[string]any
	// Application-defined additions; recorded when not nil.
	Attrs map[string]any
	// The license the index itself is published under, as an SPDX
	// expression, and what a consumer shows alongside its results; recorded
	// when not empty.
	License     string
	Attribution string
}

// Encode writes the section as canonical CBOR (RFC 8949 §4.2): definite
// lengths, shortest-form integers, and map keys sorted by their encoded
// bytes, so that one input has one encoding.
//
// Values in the parameter and attribute maps may be unsigned or non-negative
// integers, text, slices of those, or further maps with text keys: the subset
// the Rust writer reads back. Anything else is an error.
func (m *Metadata) Encode() ([]byte, error) {
	sources := make([]any, len(m.Sources))
	for i, s := range m.Sources {
		if s.ContentHash == "" {
			return nil, buildError("a source has no content hash")
		}
		entry := map[string]any{"content_hash": s.ContentHash}
		for key, value := range map[string]string{"uri": s.URI, "retrieved": s.Retrieved, "modified": s.Modified} {
			if value != "" {
				entry[key] = value
			}
		}
		sources[i] = entry
	}
	root := map[string]any{
		"generation": m.Generation,
		"source":     sources,
		"generator": map[string]any{
			"name":    m.GeneratorName,
			"version": m.GeneratorVersion,
			"params":  orEmptyMap(m.GeneratorParams),
		},
		"tokenizer": map[string]any{
			"id":     m.TokenizerID,
			"params": orEmptyMap(m.TokenizerParams),
		},
	}
	if m.Attrs != nil {
		root["attrs"] = m.Attrs
	}
	if m.License != "" {
		root["license"] = m.License
	}
	if m.Attribution != "" {
		root["attribution"] = m.Attribution
	}
	return EncodeCBOR(root)
}

func orEmptyMap(m map[string]any) map[string]any {
	if m == nil {
		return map[string]any{}
	}
	return m
}

// EncodeCBOR writes one value as canonical CBOR, from the subset Metadata
// uses: unsigned or non-negative integers, text, slices, and maps with text
// keys.
func EncodeCBOR(value any) ([]byte, error) {
	return appendCBOR(nil, value)
}

func cborHead(out []byte, major byte, v uint64) []byte {
	major <<= 5
	switch {
	case v < 24:
		return append(out, major|byte(v))
	case v <= 0xFF:
		return append(out, major|24, byte(v))
	case v <= 0xFFFF:
		return append(out, major|25, byte(v>>8), byte(v))
	case v <= 0xFFFF_FFFF:
		return append(out, major|26, byte(v>>24), byte(v>>16), byte(v>>8), byte(v))
	}
	out = append(out, major|27)
	for shift := 56; shift >= 0; shift -= 8 {
		out = append(out, byte(v>>shift))
	}
	return out
}

func appendCBOR(out []byte, value any) ([]byte, error) {
	switch v := value.(type) {
	case uint64:
		return cborHead(out, 0, v), nil
	case uint:
		return cborHead(out, 0, uint64(v)), nil
	case uint32:
		return cborHead(out, 0, uint64(v)), nil
	case uint8:
		return cborHead(out, 0, uint64(v)), nil
	case int:
		return appendCBOR(out, int64(v))
	case int32:
		return appendCBOR(out, int64(v))
	case int64:
		if v < 0 {
			return nil, buildError("metadata holds only non-negative integers")
		}
		return cborHead(out, 0, uint64(v)), nil
	case string:
		return append(cborHead(out, 3, uint64(len(v))), v...), nil
	case []string:
		out = cborHead(out, 4, uint64(len(v)))
		for _, item := range v {
			out = append(cborHead(out, 3, uint64(len(item))), item...)
		}
		return out, nil
	case []any:
		out = cborHead(out, 4, uint64(len(v)))
		for _, item := range v {
			var err error
			if out, err = appendCBOR(out, item); err != nil {
				return nil, err
			}
		}
		return out, nil
	case map[string]any:
		type pair struct{ key, value []byte }
		pairs := make([]pair, 0, len(v))
		for key, item := range v {
			value, err := appendCBOR(nil, item)
			if err != nil {
				return nil, err
			}
			pairs = append(pairs, pair{append(cborHead(nil, 3, uint64(len(key))), key...), value})
		}
		slices.SortFunc(pairs, func(a, b pair) int { return bytes.Compare(a.key, b.key) })
		out = cborHead(out, 5, uint64(len(pairs)))
		for _, p := range pairs {
			out = append(append(out, p.key...), p.value...)
		}
		return out, nil
	}
	return nil, buildError(fmt.Sprintf("metadata cannot hold a %T", value))
}

// Split is the terms a document's text becomes under a tokenizer from
// TOKENIZERS.md, as a writer builds them; false for a tokenizer this package
// does not know. A key tokenizer, `identity` or `ja-address`, makes the
// whole text one term.
func Split(tokenizerID, text string) ([]string, bool) {
	t, ok := tokenizers[tokenizerID]
	if !ok {
		return nil, false
	}
	return t.split(text), true
}
