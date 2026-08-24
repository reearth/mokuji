#!/usr/bin/env python3
"""Turn a directory of Markdown into a corpus `mokuji build` can index.

The other demo indexes a dictionary: one term names one record, and a query is
a prefix. This one is the opposite shape — thousands of terms per record, and a
query is a ranked intersection — on the same file layout.

Output is one document per line: the text, a tab, then `path|title`.

    python3 corpus.py --docs path/to/markdown --out documents.tsv
    mokuji build documents.tsv documents.mokuji --tokenizer bigram
    mokuji search documents.mokuji 建築物 -n 10

Measured on the PLATEAU specifications, which are Markdown and public:

    git clone https://github.com/eukarya-inc/plateau-spec
    python3 corpus.py --docs plateau-spec/docs --out documents.tsv
"""

import argparse
import pathlib
import re
import sys


def title_of(text, fallback):
    """The first heading, which is what a result list wants to show."""
    for line in text.splitlines():
        if line.startswith("#"):
            return line.lstrip("# ").strip()
    return fallback


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--docs", required=True, help="directory of .md files")
    parser.add_argument("--out", default="documents.tsv")
    args = parser.parse_args()

    root = pathlib.Path(args.docs)
    paths = sorted(root.rglob("*.md"))
    if not paths:
        sys.exit(f"no .md under {root}")

    out = pathlib.Path(args.out)
    documents = characters = 0
    with out.open("w", encoding="utf-8") as handle:
        for path in paths:
            text = path.read_text(encoding="utf-8", errors="replace")
            # One document per line, so every space in the file becomes one
            # space. The tokenizer decides what a term is; this only has to
            # deliver the text intact.
            flat = re.sub(r"\s+", " ", text).strip()
            if not flat:
                continue
            rel = path.relative_to(root)
            handle.write(f"{flat}\t{rel}|{title_of(text, str(rel))}\n")
            documents += 1
            characters += len(flat)

    print(
        f"{documents:,} documents, {characters:,} characters "
        f"-> {out} ({out.stat().st_size:,} bytes)",
        file=sys.stderr,
    )


if __name__ == "__main__":
    main()
