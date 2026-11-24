# Toward 1.0

Mokuji is a draft: `SPEC.md`, `MANIFEST.md` and `TOKENIZERS.md` are version
0.1 and may still change incompatibly. This file says what 1.0 will promise,
and what has to be true before it is declared. Each condition can be checked;
1.0 is declared when all of them hold, not on a date.

## What 1.0 promises

- **A 1.x file is read by every 1.x reader.** After 1.0 the format grows only
  through the extension points it already has: `spec_minor` (§5.1), new
  section IDs (§5.2), flags, and registered codecs and tokenizers. A change
  that a 1.0 reader cannot ignore is 2.0.
- **Tokenizer identifiers are never redefined.** `TOKENIZERS.md` already
  states this rule as binding from 1.0.
- **Conformance cases are only added.** A minor version adds cases to
  `conformance/`; it does not change or remove one, read counts included.

## Conditions

| | Condition | Status |
|---|---|---|
| 1 | An independent reader | Not yet |
| 2 | A public comment period, closed | Not yet |
| 3 | Data from another publisher | Not yet |
| 4 | The I/O bound measured on large real data | Met |

1. **An independent reader.** Someone outside Re:Earth writes a reader from
   `SPEC.md` and `conformance/` alone, and it passes every case, read counts
   included. Every point where they had to ask or guess is then fixed in the
   text. The four implementations in this repository were written by one
   team and share its reading of the specification, so they cannot show that
   the text is enough on its own.

2. **A public comment period, closed.** A pinned issue calls for comments
   on the draft: what to review, how to comment (an issue labelled `spec`),
   and a closing date at least eight weeks out. The call is announced where
   people who build or use such formats will see it, the cloud-native
   geospatial community among them, and sent directly to a few people who
   maintain range-read formats or search engines. When it closes, every
   `spec` issue is closed with a change or with a decision recorded in
   Appendix A of `SPEC.md`, and the changes are summarized in one place.

3. **Data from another publisher.** A dataset other than Re:Earth Places is
   published as Mokuji files by its own publisher. A format with one
   publisher has not yet met the questions a second one brings.

4. **The I/O bound measured on large real data.** Met: the bound is measured
   on 12,854,245 places (`tools/README.md`), and the measurements are in
   `SPEC.md` §4.1.1 and `MANIFEST.md`.
