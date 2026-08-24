#!/bin/sh
# Fetch the Address Base Registry tables this demo needs.
#
# Japan's authoritative address dataset, published by the Digital Agency under
# CC BY 4.0: https://dataset.address-br.digital.go.jp/
#
# Town level, nationwide, from two tables joined on (lg_code, machiaza_id):
#   mt_town_fullset_all       the address text, nationwide (12 MB compressed)
#   mt_town_pos_pref01..47    representative coordinates   (4 MB compressed)
#
# Coordinates are published per prefecture only, so the second is 47 files.
#
# Pass a prefecture number as the second argument to fetch residence level for
# it as well — the level a street address is actually written at:
#   mt_rsdtdsp_rsdt_pref<NN>      text with block and residence numbers
#   mt_rsdtdsp_rsdt_pos_pref<NN>  representative coordinates
#
#   ./fetch-abr.sh abr 13     # Tokyo
set -eu

BASE="https://data.address-br.digital.go.jp"
DIR="${1:-abr}"
PREF="${2:-}"

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

if [ -n "$PREF" ]; then
  echo "fetching residence level for prefecture $PREF"
  curl -sS -o "$DIR/mt_rsdtdsp_rsdt_pref${PREF}.csv.zip" \
    "$BASE/mt_rsdtdsp_rsdt/pref/mt_rsdtdsp_rsdt_pref${PREF}.csv.zip"
  curl -sS -o "$DIR/mt_rsdtdsp_rsdt_pos_pref${PREF}.csv.zip" \
    "$BASE/mt_rsdtdsp_rsdt_pos/pref/mt_rsdtdsp_rsdt_pos_pref${PREF}.csv.zip"
  (cd "$DIR" && unzip -oq "mt_rsdtdsp_rsdt_pref${PREF}.csv.zip" \
                       && unzip -oq "mt_rsdtdsp_rsdt_pos_pref${PREF}.csv.zip")
fi

echo "done: $DIR"
