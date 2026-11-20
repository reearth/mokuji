package mokuji

import (
	"bytes"
	"compress/flate"
	"encoding/binary"
	"fmt"
	"io"
	"math"
	"math/big"
	"strconv"
	"strings"
	"unicode/utf8"
)

// FormatError is a file that is not one this reader can read, or that is
// malformed.
type FormatError struct {
	Msg string
}

func (e *FormatError) Error() string { return "mokuji: " + e.Msg }

func malformed(format string, args ...any) error {
	return &FormatError{Msg: fmt.Sprintf(format, args...)}
}

var errTruncated = &FormatError{Msg: "truncated"}

// maxUnit is the largest unit this reader will decompress. SPEC.md §5.6 lets a
// reader refuse a declared length it is unwilling to allocate, and §8 asks it
// to: the length is read from the file before any of the unit has been
// checked.
const maxUnit = 1 << 30

// maxDocs is the most documents this reader accepts. A document ID is a Go
// int, and the sum of two of them has to stay one while it is checked.
const maxDocs = 1 << 62

func unzigzag(v uint64) int64 {
	return int64(v>>1) ^ -int64(v&1)
}

// scaled writes value * 10^exponent in full, keeping the zeros the file
// claims.
func scaled(value int64, exponent int) string {
	if exponent >= 0 {
		return strconv.FormatInt(value, 10) + strings.Repeat("0", exponent)
	}
	digits := -exponent
	sign := ""
	magnitude := uint64(value)
	if value < 0 {
		sign = "-"
		magnitude = -magnitude
	}
	s := strconv.FormatUint(magnitude, 10)
	if len(s) <= digits {
		s = strings.Repeat("0", digits+1-len(s)) + s
	}
	return sign + s[:len(s)-digits] + "." + s[len(s)-digits:]
}

// lossy decodes UTF-8, each invalid byte becoming U+FFFD.
func lossy(b []byte) string {
	if utf8.Valid(b) {
		return string(b)
	}
	return string([]rune(string(b)))
}

type cursor struct {
	data []byte
	pos  int
}

func (c *cursor) remaining() int { return len(c.data) - c.pos }

func (c *cursor) byte() (byte, error) {
	if c.pos >= len(c.data) {
		return 0, errTruncated
	}
	b := c.data[c.pos]
	c.pos++
	return b, nil
}

func (c *cursor) take(length uint64) ([]byte, error) {
	if length > uint64(c.remaining()) {
		return nil, errTruncated
	}
	out := c.data[c.pos : c.pos+int(length)]
	c.pos += int(length)
	return out, nil
}

// varint reads an unsigned LEB128 integer: SPEC.md §5.3.1, at most ten bytes,
// and nothing past 64 bits.
func (c *cursor) varint() (uint64, error) {
	var value uint64
	for i := 0; i < 10; i++ {
		b, err := c.byte()
		if err != nil {
			return 0, err
		}
		if i == 9 && b&0x7F > 1 && b&0x80 == 0 {
			return 0, malformed("varint overflows 64 bits")
		}
		value |= uint64(b&0x7F) << (7 * i)
		if b&0x80 == 0 {
			return value, nil
		}
	}
	return 0, malformed("varint longer than ten bytes")
}

// key reads a front-coded key, decoded against the previous one (SPEC.md
// §5.3.1).
func (c *cursor) key(previous []byte) ([]byte, error) {
	shared, err := c.varint()
	if err != nil {
		return nil, err
	}
	length, err := c.varint()
	if err != nil {
		return nil, err
	}
	if shared > uint64(len(previous)) {
		return nil, malformed("shared prefix too long")
	}
	suffix, err := c.take(length)
	if err != nil {
		return nil, err
	}
	key := make([]byte, 0, int(shared)+len(suffix))
	return append(append(key, previous[:shared]...), suffix...), nil
}

// group reads one bit-packed group: minimal width, least significant bit
// first.
func (c *cursor) group(count int, out []uint64) ([]uint64, error) {
	width, err := c.byte()
	if err != nil {
		return nil, err
	}
	if width > 64 {
		return nil, malformed("bit width %d exceeds 64", width)
	}
	packed, err := c.take((uint64(count)*uint64(width) + 7) / 8)
	if err != nil {
		return nil, err
	}
	w := uint(width)
	for i := 0; i < count; i++ {
		var v uint64
		at := uint(i) * w
		for got := uint(0); got < w; {
			bit := (at + got) % 8
			n := min(8-bit, w-got)
			v |= uint64(packed[(at+got)/8]>>bit) & (1<<n - 1) << got
			got += n
		}
		out = append(out, v)
	}
	return out, nil
}

// values reads count values, in groups of at most 128.
func (c *cursor) values(count uint64) ([]uint64, error) {
	// A group costs at least its width byte, so the data in hand bounds how
	// many values there can be: a count the file merely claims is not
	// allocated up front.
	out := make([]uint64, 0, min(count, uint64(c.remaining())*128))
	for uint64(len(out)) < count {
		var err error
		out, err = c.group(int(min(128, count-uint64(len(out)))), out)
		if err != nil {
			return nil, err
		}
	}
	return out, nil
}

// inflate undoes the deflate codec: a varint of the decompressed length, then
// raw deflate (SPEC.md §3.4).
func inflate(unit []byte) ([]byte, error) {
	c := cursor{data: unit}
	length, err := c.varint()
	if err != nil {
		return nil, err
	}
	if length > maxUnit {
		return nil, malformed("a unit declares %d bytes, more than this reader allocates", length)
	}
	r := flate.NewReader(bytes.NewReader(unit[c.pos:]))
	defer func() { _ = r.Close() }()
	var out bytes.Buffer
	// One byte more than declared, so that a unit that would decompress to
	// more is caught without decompressing all of it.
	if _, err := io.CopyN(&out, r, int64(length)+1); err != nil && err != io.EOF {
		return nil, malformed("a unit does not decompress: %v", err)
	}
	if uint64(out.Len()) != length {
		return nil, malformed("unit does not match its declared length")
	}
	// The stream has to end where the length says it does.
	if _, err := r.Read(make([]byte, 1)); err != io.EOF {
		return nil, malformed("unit does not match its declared length")
	}
	return out.Bytes(), nil
}

// ByteString is a CBOR byte string used as a map key, kept apart from a text
// string of the same bytes.
type ByteString string

// decodeCBOR reads one CBOR item (RFC 8949), returning it and the position
// after it.
//
// The metadata section is canonical CBOR (SPEC.md §6), and a reader must
// tolerate keys it does not know, so every type is decoded, not only the ones
// the keys this reader uses have. Like the other readers this does not check
// that the encoding is canonical; that is `mokuji verify`'s job.
//
// Unsigned integers come back as uint64, negative ones as int64 (or *big.Int
// below its range), byte strings as []byte, text as string, arrays as []any,
// maps as map[any]any, floats as float64, and booleans and null as themselves.
// A map key that is a byte string becomes a ByteString, and one of a type that
// cannot be a Go map key becomes its printed form.
func decodeCBOR(data []byte, pos, depth int) (any, int, error) {
	if depth > 64 {
		return nil, 0, malformed("metadata is nested too deeply")
	}
	if pos >= len(data) {
		return nil, 0, malformed("truncated metadata")
	}
	initial := data[pos]
	major := initial >> 5
	small := initial & 0x1F
	pos++

	if major == 7 {
		// Simple values and floats carry their value in the argument bytes.
		switch {
		case small == 20:
			return false, pos, nil
		case small == 21:
			return true, pos, nil
		case small == 22 || small == 23:
			return nil, pos, nil
		case small < 24:
			return uint64(small), pos, nil
		case small == 24:
			if pos >= len(data) {
				return nil, 0, malformed("truncated metadata")
			}
			return uint64(data[pos]), pos + 1, nil
		case small >= 25 && small <= 27:
			size := 1 << (small - 24)
			if pos+size > len(data) {
				return nil, 0, malformed("truncated metadata")
			}
			b := data[pos : pos+size]
			switch size {
			case 2:
				return halfFloat(binary.BigEndian.Uint16(b)), pos + size, nil
			case 4:
				return float64(math.Float32frombits(binary.BigEndian.Uint32(b))), pos + size, nil
			default:
				return math.Float64frombits(binary.BigEndian.Uint64(b)), pos + size, nil
			}
		}
		return nil, 0, malformed("metadata uses an indefinite or reserved length")
	}

	var argument uint64
	switch {
	case small < 24:
		argument = uint64(small)
	case small <= 27:
		size := 1 << (small - 24)
		if pos+size > len(data) {
			return nil, 0, malformed("truncated metadata")
		}
		for _, b := range data[pos : pos+size] {
			argument = argument<<8 | uint64(b)
		}
		pos += size
	default:
		return nil, 0, malformed("metadata uses an indefinite or reserved length")
	}

	switch major {
	case 0:
		return argument, pos, nil
	case 1:
		if argument <= math.MaxInt64 {
			return -1 - int64(argument), pos, nil
		}
		n := new(big.Int).SetUint64(argument)
		return n.Sub(new(big.Int).SetInt64(-1), n), pos, nil
	case 2, 3:
		if argument > uint64(len(data)-pos) {
			return nil, 0, malformed("truncated metadata")
		}
		raw := data[pos : pos+int(argument)]
		pos += int(argument)
		if major == 2 {
			return bytes.Clone(raw), pos, nil
		}
		return lossy(raw), pos, nil
	case 4:
		// Every item takes at least a byte, so the bytes left bound the count.
		if argument > uint64(len(data)-pos) {
			return nil, 0, malformed("truncated metadata")
		}
		items := make([]any, 0, argument)
		for i := uint64(0); i < argument; i++ {
			item, next, err := decodeCBOR(data, pos, depth+1)
			if err != nil {
				return nil, 0, err
			}
			items = append(items, item)
			pos = next
		}
		return items, pos, nil
	case 5:
		if argument > uint64(len(data)-pos) {
			return nil, 0, malformed("truncated metadata")
		}
		mapping := make(map[any]any, argument)
		for i := uint64(0); i < argument; i++ {
			key, next, err := decodeCBOR(data, pos, depth+1)
			if err != nil {
				return nil, 0, err
			}
			value, after, err := decodeCBOR(data, next, depth+1)
			if err != nil {
				return nil, 0, err
			}
			mapping[mapKey(key)] = value
			pos = after
		}
		return mapping, pos, nil
	}
	// A tag: what it says about the value is not something this reader needs.
	return decodeCBOR(data, pos, depth+1)
}

func mapKey(key any) any {
	switch k := key.(type) {
	case string, uint64, int64, bool, float64, nil:
		return k
	case []byte:
		return ByteString(k)
	default:
		return fmt.Sprint(k)
	}
}

// halfFloat widens an IEEE 754 half-precision float.
func halfFloat(h uint16) float64 {
	sign := 1.0
	if h&0x8000 != 0 {
		sign = -1
	}
	exponent := int(h>>10) & 0x1F
	fraction := float64(h & 0x3FF)
	switch exponent {
	case 0:
		return sign * math.Ldexp(fraction, -24)
	case 0x1F:
		if fraction == 0 {
			return math.Inf(int(sign))
		}
		return math.NaN()
	}
	return sign * math.Ldexp(fraction+1024, exponent-25)
}
