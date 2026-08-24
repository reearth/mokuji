# COLex tokenizer registry

**Version:** 0.1 (draft)
Licensed under [CC BY 4.0](https://creativecommons.org/licenses/by/4.0/).

Tokenization is outside the COLex specification (`SPEC.md` §1). This registry
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
matched by its parts. Combined with the sorted term dictionary it turns a COLex
file into an ordered key-value store with a stated I/O bound, which is what
forward geocoding actually needs.

## Reserved

`ja-address` is reserved for the address normalizer used by the Japanese
address index, and will be registered when that index is published. It is named
here so that the identifier is not taken by something else in the meantime.
