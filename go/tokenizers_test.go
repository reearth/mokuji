package mokuji

// The tokenizers against TOKENIZERS.md, with the vectors the other readers'
// tests use. A tokenizer is outside the format, but an index is unreachable
// if two implementations disagree about it.

import (
	"slices"
	"testing"
)

func TestJaAddressAgreesWithTheOtherImplementations(t *testing.T) {
	for given, want := range map[string]string{
		"東京都千代田区丸の内一丁目": "東京都千代田区丸の内1丁目",
		"東京都千代田区丸の内１丁目": "東京都千代田区丸の内1丁目",
		"札幌市北区北十一丁目":    "札幌市北区北11丁目",
		"二十丁目":          "20丁目",
		"百丁目":           "100丁目",
		"千代田区一番町":       "千代田区一番町",
		"京都市下京区五条":      "京都市下京区五条",
		"一二丁目":          "一二丁目",
		"  東京都　":        "東京都",
		"ﾏﾙﾉｳﾁ":         "マルノウチ",
	} {
		if got := JaAddress(given); got != want {
			t.Errorf("%q: %q, want %q", given, got, want)
		}
	}
}

func TestJaAddressFoldsTheSpellingsOfAPlaceNameOntoOne(t *testing.T) {
	// The registry writes both, so a reader that folded only one of them
	// would find the address it was not given.
	for _, pair := range [][2]string{
		{"東京都千代田区丸ノ内一丁目", "東京都千代田区丸の内1丁目"},
		{"神奈川県茅ケ崎市", "神奈川県茅ヶ崎市"},
		{"三重県いなべ市北勢町二之瀬", "三重県いなべ市北勢町二の瀬"},
	} {
		if JaAddress(pair[0]) != JaAddress(pair[1]) {
			t.Errorf("%q and %q differ", pair[0], pair[1])
		}
	}
	// Left alone on purpose: `が` is an ordinary particle, and `ハイツ` is a
	// word.
	if JaAddress("三重県亀山市みずきが丘") == JaAddress("三重県亀山市みずきヶ丘") {
		t.Error("folded a particle")
	}
	// Inside a katakana word these are letters, not connectives.
	for _, word := range []string{"三重県亀山市関町富士ハイツ", "マルノウチ", "ケーキ"} {
		if got := JaAddress(word); got != word {
			t.Errorf("%q became %q", word, got)
		}
	}
}

func TestJaAddressStripsWhiteSpaceAndNothingElse(t *testing.T) {
	// `White_Space`, as Rust's `trim` reads it: U+0085 is whitespace, and
	// U+001F is not.
	if got := JaAddress("\u0085東京都　"); got != "東京都" {
		t.Error(got)
	}
	if got := JaAddress("\u001f東京都"); got != "\u001f東京都" {
		t.Error(got)
	}
}

func TestBigramSplitsAsTokenizersMdSays(t *testing.T) {
	for given, want := range map[string][]string{
		"建築物":     {"建築", "築物"},
		"CityGML": {"citygml"},
		"3D都市":    {"3d", "都市"},
		// Split where ASCII meets CJK, after composition, and only there.
		"LOD2の建物": {"lod2", "の建", "建物"},
		"ＬＯＤ２の":   {"lod2", "の"},
		"第3章":     {"第", "3", "章"},
		"Aー":      {"a", "ー"},
		"Zürich":  {"zü", "ür", "ri", "ic", "ch"},
		"都":       {"都"},
		// Devanagari vowel signs are combining marks by category and
		// `Alphabetic` by property, so `हिंदी` is one run, not three.
		"हिंदी": {"हि", "िं", "ंद", "दी"},
		"":      {},
	} {
		if got := Bigram(given); !slices.Equal(got, want) {
			t.Errorf("%q: %q, want %q", given, got, want)
		}
	}
	if got, _ := Split("ja-address-bigram", "丸の内一丁目"); !slices.Equal(got, []string{"丸の", "の内", "内1", "1丁", "丁目"}) {
		t.Error(got)
	}
}

func TestNameListAgreesWithTheOtherImplementations(t *testing.T) {
	for given, want := range map[string]string{
		"  Black  Sea ": "black sea",
		"ＴＯＫＹＯ":         "tokyo",
		// The full lowercase mapping: two characters, and a final sigma.
		"İstanbul": "i̇stanbul",
		"ΟΔΟΣ":     "οδος",
	} {
		if got := NameList(given); got != want {
			t.Errorf("%q: %q, want %q", given, got, want)
		}
	}
}

func TestTheNameListSeparatorIsNotASpace(t *testing.T) {
	got := tokenizers["name-list"].split("Tokyo\u001f東京\u001f  ")
	if !slices.Equal(got, []string{"tokyo", "東京"}) {
		t.Error(got)
	}
}
