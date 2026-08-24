#!/bin/sh
# Fetch the Address Base Registry tables this demo needs.
#
# Japan's authoritative address dataset, published by the Digital Agency under
# CC BY 4.0: https://dataset.address-br.digital.go.jp/
#
# Two tables, joined on (lg_code, machiaza_id):
#   mt_town_fullset_all       the address text, nationwide (12 MB compressed)
#   mt_town_pos_pref01..47    representative coordinates   (4 MB compressed)
#
# Coordinates are published per prefecture only, so the second is 47 files.
set -eu

BASE="https://data.address-br.digital.go.jp"
DIR="${1:-abr}"

mkdir -p "$DIR/pos"
echo "fetching the nationwide town master"
curl -sS -o "$DIR/mt_town_fullset_all.csv.zip" \
  "$BASE/mt_town_fullset/mt_town_fullset_all.csv.zip"

echo "fetching representative points for 47 prefectures"
for i in $(seq -w 1 47); do
  curl -sS -o "$DIR/pos/mt_town_pos_pref${i}.csv.zip" \
    "$BASE/mt_town_pos/pref/mt_town_pos_pref${i}.csv.zip"
done

echo "unpacking"
(cd "$DIR" && unzip -oq mt_town_fullset_all.csv.zip)
(cd "$DIR/pos" && for z in *.zip; do unzip -oq "$z"; done)

echo "done: $DIR"
