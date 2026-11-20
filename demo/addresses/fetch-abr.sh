#!/bin/sh
# Fetch the Address Base Registry tables this demo needs.
#
# Japan's authoritative address dataset, published by the Digital Agency under
# CC BY 4.0: https://dataset.address-br.digital.go.jp/
#
# Town level, nationwide, from three tables:
#   mt_town_fullset_all       the address text, nationwide (11 MB compressed)
#   mt_town_pos_pref01..47    representative coordinates   (4 MB compressed)
#   mt_city_pos_all           municipalities' coordinates  (24 KB compressed)
#
# Town coordinates are published per prefecture only, so the second is 47
# files. The third places the addresses the second has no point for; see
# abr.py.
#
# Pass a prefecture number as the second argument to fetch residence level for
# it as well — the level a street address is actually written at:
#   mt_rsdtdsp_rsdt_pref<NN>      text with block and residence numbers
#   mt_rsdtdsp_rsdt_pos_pref<NN>  representative coordinates
#
#   ./fetch-abr.sh abr 13     # Tokyo
#
# Each archive is kept beside what is unpacked from it, with a `.source` record
# of its URL, the registry's Last-Modified, and when it was fetched, so that a
# build can say how current it is (`../provenance.py`, `SPEC.md` §6).
set -eu

BASE="https://data.address-br.digital.go.jp"
DIR="${1:-abr}"
PREF="${2:-}"
FETCH="python3 $(dirname "$0")/../provenance.py fetch"

mkdir -p "$DIR/pos"
echo "fetching the nationwide town master"
$FETCH "$BASE/mt_town_fullset/mt_town_fullset_all.csv.zip" "$DIR/mt_town_fullset_all.csv.zip"

echo "fetching representative points for 47 prefectures"
for i in $(seq -w 1 47); do
  $FETCH "$BASE/mt_town_pos/pref/mt_town_pos_pref${i}.csv.zip" "$DIR/pos/mt_town_pos_pref${i}.csv.zip"
done

echo "fetching representative points for municipalities"
$FETCH "$BASE/mt_city_pos/mt_city_pos_all.csv.zip" "$DIR/mt_city_pos_all.csv.zip"

echo "unpacking"
(cd "$DIR" && unzip -oq mt_town_fullset_all.csv.zip && unzip -oq mt_city_pos_all.csv.zip)
(cd "$DIR/pos" && for z in *.zip; do unzip -oq "$z"; done)

if [ -n "$PREF" ]; then
  echo "fetching residence level for prefecture $PREF"
  $FETCH "$BASE/mt_rsdtdsp_rsdt/pref/mt_rsdtdsp_rsdt_pref${PREF}.csv.zip" \
    "$DIR/mt_rsdtdsp_rsdt_pref${PREF}.csv.zip"
  $FETCH "$BASE/mt_rsdtdsp_rsdt_pos/pref/mt_rsdtdsp_rsdt_pos_pref${PREF}.csv.zip" \
    "$DIR/mt_rsdtdsp_rsdt_pos_pref${PREF}.csv.zip"
  (cd "$DIR" && unzip -oq "mt_rsdtdsp_rsdt_pref${PREF}.csv.zip" \
                       && unzip -oq "mt_rsdtdsp_rsdt_pos_pref${PREF}.csv.zip")
fi

echo "done: $DIR"
