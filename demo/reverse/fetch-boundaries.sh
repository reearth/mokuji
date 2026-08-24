#!/bin/sh
# Fetch the small-area boundary layer this demo covers.
#
# The 2020 Census small-area boundaries, published by Japan's Statistics Bureau
# through e-Stat: one polygon per 町丁目, the same unit the town-level address
# index in ../addresses is keyed by.
#
#   ./fetch-boundaries.sh 13     # Tokyo
#
# Needs ogr2ogr (GDAL) to reproject: the layer is published in JGD2000, and the
# covering works in WGS84 because that is what a GPS fix is in.
set -eu

PREF="${1:-13}"
DIR="${2:-boundaries}"
SURVEY="A002005212020"

mkdir -p "$DIR"
echo "fetching small-area boundaries for prefecture $PREF"
curl -sS -L -o "$DIR/pref${PREF}.zip" \
  "https://www.e-stat.go.jp/gis/statmap-search/data?dlserveyId=${SURVEY}&code=${PREF}&coordSys=1&format=shape&downloadType=5"

(cd "$DIR" && unzip -oq "pref${PREF}.zip")
SHP=$(ls "$DIR"/*.shp | head -1)

echo "reprojecting to WGS84"
ogr2ogr -f GeoJSON -t_srs EPSG:4326 -lco COORDINATE_PRECISION=6 \
  "$DIR/boundaries.geojson" "$SHP"

echo "done: $DIR/boundaries.geojson"
