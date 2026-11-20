#!/bin/sh
# Fetch the small-area boundary layer this demo covers.
#
# The 2020 Census small-area boundaries, published by Japan's Statistics Bureau
# through e-Stat: one polygon per 町丁目, the same unit the town-level address
# index in ../addresses is keyed by.
#
#   ./fetch-boundaries.sh 13     # Tokyo, into boundaries/boundaries.geojson
#   ./fetch-boundaries.sh all    # all 47, into boundaries/pref01..47.geojson
#
# Each archive is kept as e-Stat served it, with a `.source` record of its URL
# and when it was fetched (`../provenance.py`). e-Stat sends no Last-Modified,
# so the date recorded as the publisher's is that of the newest file in the
# archive: 2022-06-23 for every prefecture of the 2020 layer.
#
# Needs ogr2ogr (GDAL) to reproject: the layer is published in JGD2000, and the
# covering works in WGS84 because that is what a GPS fix is in.
set -eu

PREF="${1:-13}"
DIR="${2:-boundaries}"
SURVEY="A002005212020"
PROVENANCE="python3 $(dirname "$0")/../provenance.py"

fetch() {
  code="$1"
  out="$2"
  echo "fetching small-area boundaries for prefecture $code"
  $PROVENANCE fetch --archive-date \
    "https://www.e-stat.go.jp/gis/statmap-search/data?dlserveyId=${SURVEY}&code=${code}&coordSys=1&format=shape&downloadType=5" \
    "$DIR/pref${code}.zip"
  rm -rf "$DIR/pref${code}"
  mkdir -p "$DIR/pref${code}"
  (cd "$DIR/pref${code}" && unzip -oq "../pref${code}.zip")
  shp=$(ls "$DIR/pref${code}"/*.shp | head -1)
  ogr2ogr -f GeoJSON -t_srs EPSG:4326 -lco COORDINATE_PRECISION=6 "$out" "$shp"
}

mkdir -p "$DIR"
if [ "$PREF" = all ]; then
  for code in $(seq -w 1 47); do
    fetch "$code" "$DIR/pref${code}.geojson"
  done
else
  fetch "$PREF" "$DIR/boundaries.geojson"
fi

echo "done: $DIR"
