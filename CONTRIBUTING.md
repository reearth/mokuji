# Contributing

How to run the tests is in the README, under
[Developing](README.md#developing). This file is the rules a change follows.

## One feature, every language

The library exists four times: the Rust crate (`src/`), the TypeScript
package (`js/`), the Python package (`python/`) and the Go module (`go/`).
They are one API in four languages, and what one of them can do, all of them
can do.

- **A feature lands in every language at once.** A change that adds to or
  changes what a library does, in reading, writing, tokenizing or a query
  built on them, changes all four in the same pull request. "Port it later"
  is how the gaps open.
- **Names follow the language; behaviour does not.** `lookupPrefix`,
  `lookup_prefix` and `LookupPrefix` are one function. Given the same file
  and the same arguments, the four return the same answer, refuse the same
  input, and cost the same reads.
- **The tests ask the same questions.** A feature's tests in one language
  have a counterpart in each of the others, with the same fixtures and the
  same expected answers. What can be stated as a case goes into
  `conformance/cases.toml`, which all four already answer.
- **Each language documents it.** The language's README shows it, in that
  language's names. A Rust module goes into the table in the root README.
- **A deliberate gap is written down, with its reason.** Some differences are
  the point: a reader that fetches over HTTP needs a client, and a language may
  leave that to the caller. Such a difference is listed in the README of the
  language that lacks it, under "Where it differs", saying why. A gap that is
  written down nowhere is a bug.

The `mokuji` command is built on the Rust library and has no counterpart in
the other languages. A subcommand need not exist elsewhere; a library
function it calls does, unless only building from a corpus needs it.

### The differences that are meant

- **Rust has no HTTP client.** The crate reads through `RangeReader` (async)
  or `BlockingRangeReader` (blocking), and a caller plugs in the client they
  already use, so that the crate pulls in no HTTP stack. The other three
  fetch a URL themselves, since their standard libraries can.
- **Rust keeps its byte layer public.** `mokuji::read::Index` takes terms as
  bytes already normalized, and has `search` over bytes and `read_payload`
  for one document. The other languages have one layer, where `rank` and
  `payloadSpan` do the same work.
- **Rust checks the metadata into types.** `mokuji::meta::Meta` parses
  `ContentHash` and RFC 3339 dates. The other languages expose the decoded
  metadata and the fields a reader acts on: sources, generator, tokenizer,
  generation, attrs, license, attribution.
- **A manifest's JSON is read by each language's own parser.** They agree on
  every manifest MANIFEST.md describes, and part at the edges of JSON
  itself: a key written twice, integers past 2^53 or 2^63, `1.0` for `1`,
  a lone surrogate. Each language's README says where its parser parts from
  Rust's.
- **What only a build needs stays with the command.** Parsing a corpus, and
  `Tokenizer::is_key`, which decides whether two lines with the same key are
  a duplicate, are Rust's. The writers in the other languages take terms
  already split.
