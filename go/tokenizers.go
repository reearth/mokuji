package mokuji

// Query-side normalization for the tokenizers in TOKENIZERS.md.
//
// A query has to go through whatever the terms went through when the index
// was built, or it will not match them. The index records which tokenizer
// that was; these are the ones a reader can apply to a query without a
// dictionary.
//
// The same rules as the Rust, TypeScript and Python ones, written against the
// Unicode properties the registry names: `White_Space` rather than
// unicode.IsSpace, which differs on U+0085 and U+FEFF, and the full default
// lowercase mapping rather than unicode.ToLower, which maps `İ` to one
// character and every `Σ` to `σ`.

import (
	"sort"
	"strconv"
	"strings"
	"unicode"
	"unicode/utf8"

	"golang.org/x/text/cases"
	"golang.org/x/text/language"
	"golang.org/x/text/unicode/norm"
)

// tokenizer is what a reader does with a query for one tokenizer identifier.
type tokenizer struct {
	// What a query goes through before it is looked up as a prefix.
	normalize func(string) string
	// The terms a query is looked up as. A search, unlike a prefix, puts the
	// query through the tokenizer whole: a tokenizer that splits turns what
	// was typed into several terms.
	split func(string) []string
}

func as(text string) string { return text }

func one(text string) []string { return []string{text} }

var tokenizers = map[string]tokenizer{
	"space-split": {as, func(q string) []string {
		out := []string{}
		for _, t := range strings.Split(q, " ") {
			if t != "" {
				out = append(out, t)
			}
		}
		return out
	}},
	"identity": {as, one},
	"bigram":   {as, Bigram},
	"ja-address": {JaAddress, func(q string) []string {
		return []string{JaAddress(q)}
	}},
	// The same normalization as `ja-address`; only the token boundary
	// differs, and a prefix is not split in either.
	"ja-address-bigram": {JaAddress, func(q string) []string {
		return bigrams(JaAddress(q), false)
	}},
	// What was typed is a prefix of one name, so it is normalized, not split.
	"name-list": {NameList, func(q string) []string {
		out := []string{}
		for _, part := range strings.Split(q, "\u001f") {
			if name := NameList(part); name != "" {
				out = append(out, name)
			}
		}
		return out
	}},
}

// KnownTokenizer says whether this reader can normalize a query for the
// tokenizer an index records. An index whose tokenizer it does not know is
// still readable, with queries used as typed.
func KnownTokenizer(id string) bool {
	_, ok := tokenizers[id]
	return ok
}

// Normalize is what a query goes through, before it is looked up as a
// prefix, for the tokenizer id names: what Index.Normalize does for an index
// that records it. False for an id this reader does not know, with text
// returned as typed.
func Normalize(tokenizerID, text string) (string, bool) {
	t, ok := tokenizers[tokenizerID]
	if !ok {
		return text, false
	}
	return t.normalize(text), true
}

func isWhiteSpace(r rune) bool { return unicode.Is(unicode.White_Space, r) }

// isAlphanumeric is `\p{Alphabetic}` or `\p{N}`, what `bigram` keeps.
func isAlphanumeric(r rune) bool {
	if unicode.IsLetter(r) || unicode.IsNumber(r) {
		return true
	}
	at := sort.Search(len(otherAlphabetic), func(i int) bool { return otherAlphabetic[i][0] > r }) - 1
	return at >= 0 && r <= otherAlphabetic[at][1]
}

func lower(text string) string {
	// A Caser keeps state between calls, so one is made for each.
	return cases.Lower(language.Und).String(text)
}

var jaNumerals = map[rune]int{
	'〇': 0, '零': 0, '一': 1, '二': 2, '三': 3, '四': 4, '五': 5,
	'六': 6, '七': 7, '八': 8, '九': 9, '十': 10, '百': 100, '千': 1000,
}

// parseNumeral reads a run of numerals as a number.
//
// See TOKENIZERS.md. 十 百 千 multiply the digit before them, or one when
// there is none; two bare digits in sequence are positional notation, which
// this rule does not claim to read.
func parseNumeral(run []rune) (int, bool) {
	total := 0
	digit := -1
	for _, r := range run {
		value := jaNumerals[r]
		if value >= 10 {
			multiplier := 1
			if digit >= 0 {
				multiplier = digit
			}
			total += multiplier * value
			digit = -1
		} else {
			if digit >= 0 {
				return 0, false
			}
			digit = value
		}
	}
	return total + max(digit, 0), true
}

// Characters a Japanese place name is written with in more than one way. See
// TOKENIZERS.md: `丸ノ内` and `丸の内` are the same place and the registry
// spells both, so both have to fold onto one before anything is compared.
var jaVariants = map[rune]rune{'ノ': 'の', 'ケ': 'ヶ', 'ヵ': 'ヶ'}

func isKatakana(r rune) bool { return r >= '゠' && r <= 'ヿ' }

// foldVariants folds the variant spellings, but only where they are
// connectives. Inside a katakana word these characters are letters:
// `マルノウチ` is not `マルのウチ`, and `ケーキ` is not `ヶーキ`.
func foldVariants(chars []rune) []rune {
	out := make([]rune, len(chars))
	for i, r := range chars {
		// A kanji is never part of a katakana word, so it needs no guard.
		if r == '之' {
			out[i] = 'の'
			continue
		}
		folded, ok := jaVariants[r]
		if !ok || (i > 0 && isKatakana(chars[i-1])) || (i+1 < len(chars) && isKatakana(chars[i+1])) {
			out[i] = r
		} else {
			out[i] = folded
		}
	}
	return out
}

// JaAddress is the `ja-address` tokenizer, applied to a query: NFKC, the
// variant spellings of a connective folded, and a number of chōme written in
// kanji written in digits.
func JaAddress(text string) string {
	chars := foldVariants([]rune(strings.TrimFunc(norm.NFKC.String(text), isWhiteSpace)))
	var out strings.Builder
	for i := 0; i < len(chars); {
		start := i
		for i < len(chars) {
			if _, ok := jaNumerals[chars[i]]; !ok {
				break
			}
			i++
		}
		if i == start {
			out.WriteRune(chars[i])
			i++
			continue
		}
		run := chars[start:i]
		chome := i+1 < len(chars) && chars[i] == '丁' && chars[i+1] == '目'
		if value, ok := parseNumeral(run); chome && ok {
			out.WriteString(strconv.Itoa(value))
		} else {
			out.WriteString(string(run))
		}
	}
	return out.String()
}

// NameList is one name as `name-list` normalizes it (TOKENIZERS.md): NFKC,
// the default lowercase mapping, and every run of White_Space one space.
func NameList(text string) string {
	return strings.Join(strings.FieldsFunc(lower(norm.NFKC.String(text)), isWhiteSpace), " ")
}

// Bigram is the terms `bigram` makes of a text (TOKENIZERS.md): compatibility
// composition, a run at everything that is not a letter or a digit, each run
// split again where ASCII meets CJK, each piece lowercased, and a piece that
// is all ASCII kept whole while any other becomes its character bigrams, or
// its one character.
func Bigram(text string) []string { return bigrams(text, true) }

// isCJK is the blocks `bigram` splits from ASCII: CJK symbols, kana, Hangul,
// and the unified and compatibility ideographs. Blocks rather than the Script
// property, because `ー` is Common by script and `々` sits among symbols.
func isCJK(r rune) bool {
	switch {
	case r >= 0x1100 && r <= 0x11FF, r >= 0x3000 && r <= 0x30FF,
		r >= 0x3130 && r <= 0x318F, r >= 0x31F0 && r <= 0x31FF,
		r >= 0x3400 && r <= 0x4DBF, r >= 0x4E00 && r <= 0x9FFF,
		r >= 0xAC00 && r <= 0xD7AF, r >= 0xF900 && r <= 0xFAFF,
		r >= 0x20000 && r <= 0x3FFFF:
		return true
	}
	return false
}

// bigrams is `bigram`, with the split where ASCII meets CJK only when
// splitCJK. `ja-address-bigram` does not split there: `1丁` is a bigram an
// address query leans on.
func bigrams(text string, splitCJK bool) []string {
	out := []string{}
	pieces := strings.FieldsFunc(norm.NFKC.String(text), func(r rune) bool { return !isAlphanumeric(r) })
	for _, piece := range pieces {
		chars := []rune(piece)
		start := 0
		for i := 1; i <= len(chars); i++ {
			if i == len(chars) || splitCJK &&
				(chars[i-1] < utf8.RuneSelf && isCJK(chars[i]) || isCJK(chars[i-1]) && chars[i] < utf8.RuneSelf) {
				out = appendBigrams(out, string(chars[start:i]))
				start = i
			}
		}
	}
	return out
}

func appendBigrams(out []string, piece string) []string {
	run := lower(piece)
	if isASCII(run) || utf8.RuneCountInString(run) == 1 {
		return append(out, run)
	}
	chars := []rune(run)
	for i := 0; i+1 < len(chars); i++ {
		out = append(out, string(chars[i:i+2]))
	}
	return out
}

func isASCII(s string) bool {
	for i := 0; i < len(s); i++ {
		if s[i] >= utf8.RuneSelf {
			return false
		}
	}
	return true
}
