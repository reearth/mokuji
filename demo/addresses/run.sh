#!/bin/sh
# Build an address index and query it. Run from this directory.
set -eu

COUNT="${1:-300000}"
COLEX="${COLEX:-cargo run --release -q -p colex-cli --}"

python3 generate.py --count "$COUNT" --out addresses.tsv
$COLEX build addresses.tsv addresses.colex --tokenizer identity
$COLEX verify addresses.colex --source addresses.tsv
$COLEX prefix addresses.colex "東京都上足立区丸の内一丁目"
