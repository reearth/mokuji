#!/usr/bin/env python3
"""Writes the published site into OUT: the page as it is at /, in English,
and the same page at /ja/, in Japanese.

Both languages are in index.html already, and the stylesheet shows one; what
differs is what a crawler reads without running the page: the html element's
lang, the title, the description and the link preview. GitHub Pages cannot
choose a language from Accept-Language, so the choice is two addresses, and
the English page sends on a browser that prefers Japanese (see index.html).

    python3 site/build.py _site
    python3 -m http.server -d _site
"""

import re
import shutil
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
# Not published: this script, the notes, and the templates the images in
# img/ are rendered from.
SKIP = {"build.py", "DESIGN.md", "brand", "__pycache__"}

# What is English in index.html, and what /ja/ says instead. Each must be
# found exactly once, so a change to the page that misses this fails the
# build rather than publishing English under /ja/.
JA = [
    ('<html lang="en" data-lang="en">', '<html lang="ja" data-lang="ja">'),
    (
        "<title>Mokuji — search, without a server</title>",
        "<title>Mokuji — サーバーなしで、検索</title>",
    ),
    (
        'content="Mokuji turns a search index into one file you upload. Each query reads only the few pieces it needs, by HTTP range requests."\n    />\n    <meta name="color-scheme"',
        'content="Mokuji は、検索インデックスを置くだけのファイル1つにします。検索のたびに読むのは、そのうち必要な数か所だけ。"\n    />\n    <meta name="color-scheme"',
    ),
    (
        '<link rel="canonical" href="https://reearth.github.io/mokuji/" />',
        '<link rel="canonical" href="https://reearth.github.io/mokuji/ja/" />',
    ),
    (
        '<meta property="og:url" content="https://reearth.github.io/mokuji/" />',
        '<meta property="og:url" content="https://reearth.github.io/mokuji/ja/" />',
    ),
    (
        '<meta property="og:title" content="Mokuji — search, without a server" />',
        '<meta property="og:title" content="Mokuji — サーバーなしで、検索" />',
    ),
    (
        'property="og:description"\n      content="Mokuji turns a search index into one file you upload. Each query reads only the few pieces it needs, by HTTP range requests."',
        'property="og:description"\n      content="Mokuji は、検索インデックスを置くだけのファイル1つにします。検索のたびに読むのは、そのうち必要な数か所だけ。"',
    ),
    ("/img/og-en.png", "/img/og-ja.png"),
    (
        'content="Search, without a server: 3.1 KB of a 4.5 MB file, in 2 range reads"',
        'content="サーバーなしで、検索。4.5 MB のファイルから、2 回の読み出しで 3.1 KB"',
    ),
    ('<meta property="og:locale" content="en_US" />', '<meta property="og:locale" content="ja_JP" />'),
    (
        '<meta property="og:locale:alternate" content="ja_JP" />',
        '<meta property="og:locale:alternate" content="en_US" />',
    ),
    # The language switch: from /ja/, English is one level up, and Japanese
    # is this page.
    ('<a href="./" hreflang="en"', '<a href="../" hreflang="en"'),
    ('<a href="./ja/" hreflang="ja"', '<a href="./" hreflang="ja"'),
]


def japanese(page: str) -> str:
    for en, ja in JA:
        count = page.count(en)
        if count != 1:
            sys.exit(f"build.py: expected one {en[:60]!r} in index.html, found {count}")
        page = page.replace(en, ja)
    # Files beside the page are one level up from /ja/. "./" alone is the
    # page itself (the logo, back to top), and stays.
    return re.sub(r'(href|src)="\./(?=[^"])', r'\1="../', page)


def main() -> None:
    if len(sys.argv) != 2:
        sys.exit("usage: build.py OUT")
    out = Path(sys.argv[1])
    if out.exists():
        shutil.rmtree(out)
    out.mkdir(parents=True)
    for item in HERE.iterdir():
        if item.name in SKIP or item.name.startswith("."):
            continue
        # mokuji is a link to ../js/dist; copied as files, followed.
        if item.is_dir():
            shutil.copytree(item, out / item.name, ignore=shutil.ignore_patterns("__pycache__"))
        else:
            shutil.copy2(item, out / item.name)
    page = (HERE / "index.html").read_text(encoding="utf-8")
    (out / "ja").mkdir()
    (out / "ja" / "index.html").write_text(japanese(page), encoding="utf-8")
    print(f"wrote {out}/index.html and {out}/ja/index.html")


if __name__ == "__main__":
    main()
