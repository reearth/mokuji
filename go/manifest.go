package mokuji

import (
	"bytes"
	"encoding/json"
	"math"
	"slices"
	"strconv"
	"strings"
)

// A manifest is read with the checks the Rust crate's Manifest::parse makes
// (src/manifest/document.rs), and written as its Manifest::to_json writes
// one, byte for byte, so that a collection is written down the same way
// whichever language wrote it.

func manifestError(format string, args ...any) error {
	return malformed("manifest: "+format, args...)
}

func segmentError(path, detail string) error {
	return malformed("segment %s: %s", path, detail)
}

// ParseManifest reads and checks a manifest (MANIFEST.md §2): JSON of the
// shape the specification gives, a major version this reader implements, at
// least one segment, and for each a path, a content hash in
// `<algorithm>:<lowercase hex>`, strictly increasing deletes, and a region,
// if any, of distinct ISO 3166-1 alpha-2 codes. Keys it does not know are
// ignored.
func ParseManifest(data []byte) (*Manifest, error) {
	var top map[string]json.RawMessage
	if err := json.Unmarshal(data, &top); err != nil || top == nil {
		return nil, manifestError("not a JSON object")
	}
	m := &Manifest{}
	var err error
	if m.MokujiManifest, err = jsonString(top, "mokuji_manifest", true); err != nil {
		return nil, err
	}
	if m.Generation, err = jsonCount(top, "generation"); err != nil {
		return nil, err
	}
	raw, ok := top["segments"]
	if !ok {
		return nil, manifestError("missing field `segments`")
	}
	var segments []map[string]json.RawMessage
	if isNull(raw) || json.Unmarshal(raw, &segments) != nil {
		return nil, manifestError("`segments` is not a list of objects")
	}
	for _, fields := range segments {
		if fields == nil {
			return nil, manifestError("`segments` is not a list of objects")
		}
		var s ManifestSegment
		if s.Path, err = jsonString(fields, "path", true); err != nil {
			return nil, err
		}
		if s.ContentHash, err = jsonString(fields, "content_hash", true); err != nil {
			return nil, err
		}
		if s.Length, err = jsonCount(fields, "length"); err != nil {
			return nil, err
		}
		if raw, ok := fields["deletes"]; ok {
			var deletes []uint64
			if isNull(raw) || json.Unmarshal(raw, &deletes) != nil {
				return nil, manifestError("`deletes` is not a list of document IDs")
			}
			for _, d := range deletes {
				if d > math.MaxInt64 {
					return nil, manifestError("a delete is larger than this reader supports")
				}
				s.Deletes = append(s.Deletes, int(d))
			}
		}
		// A region may be null, which is none: serde reads an absent and a
		// null optional field alike.
		if raw, ok := fields["region"]; ok && !isNull(raw) {
			var region map[string]json.RawMessage
			if json.Unmarshal(raw, &region) != nil || region == nil {
				return nil, manifestError("`region` is not an object")
			}
			countries, ok := region["countries"]
			if !ok {
				return nil, manifestError("missing field `countries`")
			}
			s.Region = &Region{}
			if isNull(countries) || json.Unmarshal(countries, &s.Region.Countries) != nil {
				return nil, manifestError("`countries` is not a list of codes")
			}
			if s.Region.Countries == nil {
				s.Region.Countries = []string{}
			}
		}
		m.Segments = append(m.Segments, s)
	}
	if err := m.check(); err != nil {
		return nil, err
	}
	return m, nil
}

func isNull(raw json.RawMessage) bool {
	return bytes.Equal(bytes.TrimSpace(raw), []byte("null"))
}

func jsonString(fields map[string]json.RawMessage, key string, required bool) (string, error) {
	raw, ok := fields[key]
	if !ok {
		if required {
			return "", manifestError("missing field `%s`", key)
		}
		return "", nil
	}
	var s string
	if isNull(raw) || json.Unmarshal(raw, &s) != nil {
		return "", manifestError("`%s` is not a string", key)
	}
	return s, nil
}

func jsonCount(fields map[string]json.RawMessage, key string) (int64, error) {
	raw, ok := fields[key]
	if !ok {
		return 0, manifestError("missing field `%s`", key)
	}
	var n uint64
	if isNull(raw) || json.Unmarshal(raw, &n) != nil {
		return 0, manifestError("`%s` is not a non-negative integer", key)
	}
	if n > math.MaxInt64 {
		return 0, manifestError("`%s` is larger than this reader supports", key)
	}
	return int64(n), nil
}

// check is what ParseManifest checks beyond the JSON's shape, and what
// NewCollection checks of a manifest it is handed already parsed.
func (m *Manifest) check() error {
	if major, _, _ := strings.Cut(m.MokujiManifest, "."); major != "0" {
		return manifestError("version %s is not one this reader implements", m.MokujiManifest)
	}
	if m.Generation < 0 {
		return manifestError("`generation` is not a non-negative integer")
	}
	if len(m.Segments) == 0 {
		return manifestError("a collection has no segments")
	}
	for _, s := range m.Segments {
		if s.Path == "" {
			return manifestError("a segment has no path")
		}
		if !validContentHash(s.ContentHash) {
			return segmentError(s.Path, "content_hash is not <algorithm>:<hex>")
		}
		if s.Length < 0 {
			return manifestError("`length` is not a non-negative integer")
		}
		for i, d := range s.Deletes {
			if d < 0 {
				return manifestError("`deletes` is not a list of document IDs")
			}
			if i > 0 && d <= s.Deletes[i-1] {
				return segmentError(s.Path, "deletes are not strictly increasing")
			}
		}
		if s.Region != nil {
			if len(s.Region.Countries) == 0 {
				return segmentError(s.Path, "a region names no country")
			}
			seen := map[string]bool{}
			for _, code := range s.Region.Countries {
				if len(code) != 2 || !isUpper(code[0]) || !isUpper(code[1]) {
					return segmentError(s.Path, "a region's countries are ISO 3166-1 alpha-2 codes")
				}
				if seen[code] {
					return segmentError(s.Path, "a region names a country twice")
				}
				seen[code] = true
			}
		}
	}
	return nil
}

func isUpper(b byte) bool { return 'A' <= b && b <= 'Z' }

// validContentHash is `<algorithm>:<lowercase hex>`, both parts non-empty.
func validContentHash(text string) bool {
	algorithm, hex, ok := strings.Cut(text, ":")
	if !ok || algorithm == "" || hex == "" {
		return false
	}
	for i := 0; i < len(hex); i++ {
		if c := hex[i]; !('0' <= c && c <= '9' || 'a' <= c && c <= 'f') {
			return false
		}
	}
	return true
}

// Within is the collection within these countries: every segment whose
// region includes one of them, and every segment with no region, in the same
// order and with the same deletes (MANIFEST.md §7).
//
// Restricting keeps collection order, so a restricted collection answers as
// the whole one would with the other regions' documents deleted.
func (m *Manifest) Within(countries []string) *Manifest {
	out := *m
	out.Segments = nil
	for _, s := range m.Segments {
		if s.Region == nil || slices.ContainsFunc(s.Region.Countries, func(c string) bool {
			return slices.Contains(countries, c)
		}) {
			// A copy all the way down, so that changing one manifest does
			// not change the other.
			s.Deletes = slices.Clone(s.Deletes)
			if s.Region != nil {
				s.Region = &Region{Countries: slices.Clone(s.Region.Countries)}
			}
			out.Segments = append(out.Segments, s)
		}
	}
	return &out
}

// JSON is the manifest as the Rust crate writes it: two-space indentation,
// keys in the specification's order, an empty delete list and an absent
// region left out, and a final newline. One collection is written down one
// way, so a manifest that has not changed does not differ.
func (m *Manifest) JSON() []byte {
	var b bytes.Buffer
	b.WriteString("{\n")
	b.WriteString(`  "mokuji_manifest": ` + jsonQuote(m.MokujiManifest) + ",\n")
	b.WriteString(`  "generation": ` + strconv.FormatInt(m.Generation, 10) + ",\n")
	b.WriteString(`  "segments": `)
	if len(m.Segments) == 0 {
		b.WriteString("[]")
	} else {
		b.WriteString("[\n")
		for i, s := range m.Segments {
			b.WriteString("    {\n")
			b.WriteString(`      "path": ` + jsonQuote(s.Path) + ",\n")
			b.WriteString(`      "content_hash": ` + jsonQuote(s.ContentHash) + ",\n")
			b.WriteString(`      "length": ` + strconv.FormatInt(s.Length, 10))
			if len(s.Deletes) > 0 {
				b.WriteString(",\n      \"deletes\": [\n")
				for j, d := range s.Deletes {
					b.WriteString("        " + strconv.Itoa(d))
					if j < len(s.Deletes)-1 {
						b.WriteString(",")
					}
					b.WriteString("\n")
				}
				b.WriteString("      ]")
			}
			if s.Region != nil {
				b.WriteString(",\n      \"region\": {\n        \"countries\": ")
				if len(s.Region.Countries) == 0 {
					b.WriteString("[]")
				} else {
					b.WriteString("[\n")
					for j, c := range s.Region.Countries {
						b.WriteString("          " + jsonQuote(c))
						if j < len(s.Region.Countries)-1 {
							b.WriteString(",")
						}
						b.WriteString("\n")
					}
					b.WriteString("        ]")
				}
				b.WriteString("\n      }")
			}
			b.WriteString("\n    }")
			if i < len(m.Segments)-1 {
				b.WriteString(",")
			}
			b.WriteString("\n")
		}
		b.WriteString("  ]")
	}
	b.WriteString("\n}\n")
	return b.Bytes()
}

// jsonQuote escapes a string as serde_json does: `"` and `\` escaped, the
// control characters as \b \f \n \r \t or \u00xx in lowercase hex, and
// everything else, U+2028 and U+2029 included, as itself. Go's encoder
// would also escape <, > and & and those two separators.
func jsonQuote(s string) string {
	const hex = "0123456789abcdef"
	var b strings.Builder
	b.WriteByte('"')
	for _, r := range s {
		switch r {
		case '"':
			b.WriteString(`\"`)
		case '\\':
			b.WriteString(`\\`)
		case '\b':
			b.WriteString(`\b`)
		case '\f':
			b.WriteString(`\f`)
		case '\n':
			b.WriteString(`\n`)
		case '\r':
			b.WriteString(`\r`)
		case '\t':
			b.WriteString(`\t`)
		default:
			if r < 0x20 {
				b.WriteString(`\u00`)
				b.WriteByte(hex[r>>4])
				b.WriteByte(hex[r&0xF])
			} else {
				b.WriteRune(r)
			}
		}
	}
	b.WriteByte('"')
	return b.String()
}
