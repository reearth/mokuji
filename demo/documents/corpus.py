#!/usr/bin/env python3
"""Turn the PLATEAU specifications into a corpus `mokuji build` can index.

The other demo indexes a dictionary: one term names one record, and a query is
a prefix. This one is the opposite shape — thousands of terms per record, and a
query is a ranked intersection — on the same file layout.

Output is one section per line: the text, a tab, then `document/section|title`.

    git clone https://github.com/eukarya-inc/plateau-spec
    python3 corpus.py --docs plateau-spec/docs --out documents.tsv
    mokuji build documents.tsv documents.mokuji --tokenizer bigram
    mokuji search documents.mokuji 建築物 -n 10

A section is one line of the corpus, and its text is its own. Each section's
Markdown file holds its subsections as well, so indexing the files counts a
subsection once for every section above it and lists a section beside its
parent in every result. The JSON saved beside them, `json/<section>.json`,
holds each section's own content as a tree, and the text is read from that.
Only the text: an image's `src` there is a `data:` URI, and one figure in the
procedure is a million and a half characters of SVG.
"""

import argparse
import json
import pathlib
import re
import sys

# Nodes that are blocks of their own, so that their text does not run into
# the text beside them. Everything else — a span, a link, a footnote cue — is
# inline, and joins its neighbours as written: `LOD` and a span holding `2`
# are `LOD2`.
BLOCKS = {
    "paragraph", "title", "table_cell", "table_header", "list_item",
    "linebreak", "figCaption", "dt", "dd", "bibitem", "footnote",
    "admonition", "example", "definition", "term", "source_listing",
}


def text_of(node, out):
    """Appends the text under `node` to `out`, a space around each block."""
    if isinstance(node, list):
        for child in node:
            text_of(child, out)
        return
    if not isinstance(node, dict):
        return
    block = node.get("type") in BLOCKS
    if block:
        out.append(" ")
    if isinstance(node.get("text"), str):
        out.append(node["text"])
    text_of(node.get("content"), out)
    if block:
        out.append(" ")


def sections(chapters):
    """Every section in the outline, parents before their children."""
    for chapter in chapters:
        yield chapter
        yield from sections(chapter.get("children", []))


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--docs", required=True, help="plateau-spec/docs")
    parser.add_argument("--out", default="documents.tsv")
    args = parser.parse_args()

    root = pathlib.Path(args.docs)
    outlines = sorted(root.glob("*/index.json"))
    if not outlines:
        sys.exit(f"no */index.json under {root}")

    out = pathlib.Path(args.out)
    documents = characters = 0
    with out.open("w", encoding="utf-8") as handle:
        for outline in outlines:
            kind = outline.parent.name
            for section in sections(json.loads(outline.read_text(encoding="utf-8"))["chapters"]):
                page = outline.parent / "json" / f"{section['path']}.json"
                parts = []
                text_of(json.loads(page.read_text(encoding="utf-8"))["content"], parts)
                # One section per line, so every run of whitespace becomes one
                # space. The tokenizer decides what a term is; this only has to
                # deliver the text intact.
                flat = re.sub(r"\s+", " ", "".join(parts)).strip()
                title = re.sub(r"\s+", " ", section["title"]).strip()
                handle.write(f"{flat}\t{kind}/{section['path']}|{title}\n")
                documents += 1
                characters += len(flat)

    print(
        f"{documents:,} sections, {characters:,} characters "
        f"-> {out} ({out.stat().st_size:,} bytes)",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
