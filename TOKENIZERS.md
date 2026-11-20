# Mokuji tokenizer registry

**Version:** 0.1 (draft)
Licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

Tokenization is outside the Mokuji specification (`SPEC.md` §1). This registry
does not change that. It assigns stable identifiers to tokenizers so that
`tokenizer.id` in the metadata section (`SPEC.md` §6) means the same thing to
everyone, and an index can be rebuilt from its source and compared.

## Why identifiers and not a mandate

Which analyzer is correct is a property of the language and the corpus.
Japanese alone admits both morphological analysis and character n-grams, and
neither can be ruled out without excluding a legitimate use. The same
standardization pressure that keeps scoring out of the format keeps
tokenization out of it.

What provenance actually requires is weaker and achievable: that the choice be
stated, and that stating it be enough to repeat it. A registry delivers that.

## Registering

An identifier is a lowercase ASCII string of letters, digits, and hyphens. An
entry must define the token boundary precisely enough that two implementations
produce the same tokens for the same input, and must name every parameter that
changes the output. Identifiers are never redefined; a change in behaviour is a
new identifier.

That rule binds from version 1.0. Until then an identifier may still change in
place, and `ja-address` has been changed once that way — variant folding was
added to it — rather than being registered twice.

Identifiers beginning `x-` are private and will never be registered here.

## Registered

### `space-split`

Splits on U+0020, discarding empty tokens. No normalization: no case folding,
no Unicode normalization, no trimming of punctuation. Tokens are the byte
sequences between spaces.

Parameters: none.

Deliberately minimal. It exists so that a corpus that has already been
tokenized upstream can be indexed without this format pretending to have
opinions about text it has not seen.

### `identity`

The whole input is one token. No splitting and no normalization.

Parameters: none.

This is the tokenizer for a dictionary rather than a document collection: an
address, a path, or a statute number is looked up whole or by prefix, not
matched by its parts. Combined with the sorted term dictionary it turns a Mokuji
file into an ordered key-value store with a stated I/O bound, which is what
forward geocoding actually needs.

### `ja-address`

The whole input is one term, after normalization. Intended for Japanese address
dictionaries, where a record is looked up whole or by prefix.

Parameters: none.

Normalization, in order:

1. Unicode Normalization Form KC (NFKC).
2. Strip leading and trailing whitespace.
3. Fold the variant spellings below.
4. Rewrite each maximal run of the characters
   `〇零一二三四五六七八九十百千` that is **immediately followed by `丁目`**
   as its decimal value in ASCII digits. A run that cannot be read as a numeral
   by the rule below is left exactly as it was.

The numeral rule: `十`, `百`, and `千` multiply the digit before them, or one
when there is none, so `十二` is 12 and `二十` is 20 and `百二十三` is 123. Two
bare digits in sequence are positional notation, which this rule does not claim
to read, and such a run is left alone.

#### The variant folding

| From | To | Condition |
|---|---|---|
| `ノ` | `の` | neither neighbour is in U+30A0–U+30FF |
| `ケ` | `ヶ` | neither neighbour is in U+30A0–U+30FF |
| `ヵ` | `ヶ` | neither neighbour is in U+30A0–U+30FF |
| `之` | `の` | always |

A character with no neighbour on one side — at either end of the input — is
treated as having a non-katakana neighbour there.

`丸ノ内` and `丸の内` are the same place, and the Address Base Registry spells
both; so are `茅ケ崎` and `茅ヶ崎`, and `二之瀬` and `二の瀬`. Unfolded they sort
in two places and neither is where the other's reader looks, which is the
argument that already put the chome rewriting in this entry.

The katakana condition is why this is a table and not a character map. Inside a
katakana word these are letters, not connectives: `マルノウチ` is not
`マルのウチ` and `ケーキ` is not `ヶーキ`. `之` is a kanji and never inside one,
so it needs no condition.

Each member was measured, on 725,855 nationwide addresses spelled the other way
and looked up again with `ja-address-bigram`:

| Folding | Terms it changes | Top-1 |
|---|---|---|
| none | 0 | 70% |
| `ノ` | 45,709 | 88% |
| `ノ` `ケ` `ヵ` | 61,159 | 92% |
| `ノ` `ケ` `ヵ` `之` | 71,003 | 94% |

Two candidates were measured and left out. Hiragana `が` is written for the same
connective in `自由が丘`, and folding it moved nothing — a `が`/`ヶ` swap already
scores what any single substitution scores, 81%, and `が` is an ordinary
particle everywhere outside a place name. Katakana `ツ` in `四ツ谷` would fold
`ハイツ` as well, and `ハイツ` is a word.

Folding rewrites the key, so a reader sees the folded spelling unless the corpus
carries the original — which is what the payload is for.

#### Why only `丁目`

Because it is the only suffix where a numeral is unambiguously a numbered
sequence rather than part of a name. `千代田区一番町` is a place, not block one;
`五条` is a street in Kyoto, not the fifth of anything. Rewriting those would
damage the very keys the index exists to find. Extending the rule to `番` and
`号` for block-level addresses needs a way to tell the two apart, and that is
parsing rather than normalization.

#### Why at all

The Address Base Registry writes chome both ways — `一丁目` in one municipality
and `１丁目` in the next. Without normalization the same concept sorts in two
different places, and neither is where someone typing an address would look.

It does not make numbering sort correctly. `10丁目` still falls between `1丁目`
and `2丁目`, because prefix order is byte order. Zero-padding would fix the
ordering and break the matching, since nobody types `01丁目`. Getting both means
parsing the address into fields, which is a geocoder's job and not an index's.

### `bigram`

Splits on everything that is not a letter or a digit. Each run is lowercased;
a run that is then all ASCII becomes one term, and any other run becomes its
character bigrams — or the single character, if that is all there is.

`建築物` becomes `建築`, `築物`. `CityGML` becomes `citygml`. `3D都市` is not
all ASCII, so it becomes `3d`, `d都`, `都市`; the rule does not split at a
script boundary, which costs a bigram straddling it and saves a great deal of
specification.

Parameters: none.

#### Why bigrams rather than morphology

Morphological analysis is more precise and produces a much smaller index. It
also needs a dictionary — tens of megabytes for IPA or UniDic — and it needs it
**at query time as well as at index time**, because a query has to be analyzed
the same way the terms were. A reader with no server is exactly the reader that
cannot be assumed to have one.

Bigrams need nothing. The rule above is a few lines in any language, which is
what makes an index built with it readable by a page.

The cost is real and worth stating: an index of bigrams is larger than an index
of words, and a two-character query matches substrings rather than words, so
`京都` matches `東京都`. Where a dictionary can be relied on — a server, a Go
program, a Worker that can call `Intl.Segmenter` — a morphological or ICU-based
tokenizer will do better, and belongs in this registry under its own
identifier.

### `ja-address-bigram`

The normalization of `ja-address`, then the token boundary of `bigram`.

Parameters: none.

`東京都千代田区丸の内一丁目` normalizes to `東京都千代田区丸の内1丁目` and
becomes `東京`, `京都`, `都千`, … `内1`, `1丁`, `丁目`.

Two entries with the same normalization and different token boundaries, because
an address is asked for in two ways. Looked up whole or by prefix it wants
`ja-address`, and three range reads answer it exactly. Typed with a character
wrong, or without the prefecture, it wants this: one character wrong leaves
every bigram intact but two, so the union of the query's terms still ranks the
address first. See `demo/fuzzy`.

The cost is the cost of `bigram` — a larger index, and substring matches that a
word tokenizer would not make — plus a read count that grows with the length of
the query rather than staying at three.

### `name-list`

The input is a list of names separated by U+001F (INFORMATION SEPARATOR ONE).
Each name becomes one term, after normalization:

1. Unicode Normalization Form KC (NFKC).
2. Lowercase by the Unicode default case conversion (`toLowercase`, Unicode
   §3.13): the full mapping, with no locale.
3. Replace each maximal run of `White_Space` characters with one U+0020, and
   strip leading and trailing ones.

A name that is empty after normalization produces no term.

Parameters: none.

A query is normalized by the same three steps and not split, since what was
typed is a prefix of one name.

`Black Sea` becomes `black sea`, one term, space included. `東京タワー` is
unchanged. A place called `Tokyo` and `東京` and `Tōkyō` is a document with
three terms, all of which complete to it.

#### Why a name and not its words

This is the tokenizer for completion. What someone types is a finished word and
an unfinished one, and under a word tokenizer that is an intersection of a term
with a prefix — whose best results are not among the best of either, so no
precomputed answer covers it. As one term per name the same input is one prefix,
which a completions section (`SPEC.md` §5.3.5) can answer ahead of time.
Measured on 12,854,245 places, completing a second word cost a median of 29
range reads by word and at most 3 by name (`tools/README.md`).

What it gives up is a word in the middle: `sea` does not complete to
`Black Sea`. That is a search, not a completion, and it belongs in a second
index with a word tokenizer.

#### Why U+001F

A separator has to be something no name contains. U+0020 is inside names,
commas are inside names, and a tab is the corpus field separator. U+001F is a
control character designed for exactly this, and it does not occur once in the
13.4 million rows of GeoNames.

#### Why the default case conversion

Because it has no locale, and a registry entry cannot depend on one. Turkish
`I` lowercases to `i` here rather than `ı`, which is wrong for Turkish and the
same in every implementation — the second matters more for an identifier that
has to mean one thing. JavaScript's `toLowerCase`, Rust's `to_lowercase` and
Python's `lower` all implement this mapping.

The mapping is not folding. `İstanbul` lowercases to `i` followed by U+0307, so
`istanbul` does not complete it. A corpus that wants it to supplies the name
spelled that way as well; which spellings a place has is the pipeline's
decision, not the tokenizer's.

## Why there is no ICU or morphological entry

There is an obvious candidate for a better Japanese tokenizer than `bigram`.
V8 ships ICU, so `Intl.Segmenter("ja", {granularity: "word"})` segments Japanese
in every browser and in Cloudflare Workers with nothing bundled, and ICU4X does
the same in Rust. It looks like a tokenizer that could be registered here and
implemented everywhere.

Measured, the two do not agree. Over 4,000 lines of the PLATEAU specification:

| | |
|---|---|
| Lines segmented identically | 82.8% |
| Terms produced by V8 and not by ICU4X on the same line | 3.3% |
| Shared vocabulary | 94% of the union |

Most of the disagreement is grammatical fragments, but not all of it: V8 gives
`ユース` `ケース` where ICU4X gives `ユースケース`, and a technical corpus is
full of katakana compounds. A term that one implementation produces and the
other does not is a term the second cannot find.

A registry entry has to define the token boundary precisely enough that two
implementations produce the same tokens (see the registration rules above), and
"whatever ICU does" does not, because ICU is a moving target with versions and
data behind it. The same argument rules out naming a morphological analyzer:
`kuromoji` means a particular dictionary at a particular version.

None of this stops a pipeline from using them. It means the identifier is
private — `x-icu-word-v8`, say — which is exactly what a private identifier is
for: it says the index is reachable by readers that share the implementation,
and honestly says nothing more.

## Reserved

Nothing is currently reserved.
