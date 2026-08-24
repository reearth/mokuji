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
3. Rewrite each maximal run of the characters
   `〇零一二三四五六七八九十百千` that is **immediately followed by `丁目`**
   as its decimal value in ASCII digits. A run that cannot be read as a numeral
   by the rule below is left exactly as it was.

The numeral rule: `十`, `百`, and `千` multiply the digit before them, or one
when there is none, so `十二` is 12 and `二十` is 20 and `百二十三` is 123. Two
bare digits in sequence are positional notation, which this rule does not claim
to read, and such a run is left alone.

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

## Reserved

Nothing is currently reserved.
