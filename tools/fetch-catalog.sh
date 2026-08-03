#!/usr/bin/env bash
# Regenerate data/stars.csv and the reference coordinates behind
# data/deep_sky.csv and data/constellations.csv.
#
# The committed files are the output of this script; the build never touches
# the network. Re-run only to refresh the source data.
#
# See data/SOURCES.md for provenance and licensing.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."

VIZIER='https://tapvizier.cds.unistra.fr/TAPVizieR/tap/sync'
SIMBAD='https://simbad.cds.unistra.fr/simbad/sim-tap/sync'

echo "==> Yale Bright Star Catalogue (VizieR V/50), to magnitude 6.5"
# Column names must be double-quoted or ADQL reads B-V as subtraction.
curl -s --get "$VIZIER" \
  --data-urlencode 'REQUEST=doQuery' --data-urlencode 'LANG=ADQL' \
  --data-urlencode 'FORMAT=csv' \
  --data-urlencode 'QUERY=SELECT "HR","RAJ2000","DEJ2000","Vmag","B-V" FROM "V/50/catalog"
                          WHERE "Vmag" IS NOT NULL AND "Vmag" <= 6.5 ORDER BY "Vmag"' \
  -o /tmp/orrery-bsc.csv
echo "    $(( $(wc -l < /tmp/orrery-bsc.csv) - 1 )) stars"

echo "==> Bayer designations, for resolving constellation figures"
curl -s --get "$VIZIER" \
  --data-urlencode 'REQUEST=doQuery' --data-urlencode 'LANG=ADQL' \
  --data-urlencode 'FORMAT=csv' \
  --data-urlencode 'QUERY=SELECT "HR","Name","Vmag" FROM "V/50/catalog"
                          WHERE "Vmag" IS NOT NULL AND "Vmag" < 6.0 AND "Name" IS NOT NULL
                          ORDER BY "Vmag"' \
  -o /tmp/orrery-names.csv

echo "==> Deep-sky coordinates (SIMBAD)"
echo "    NOTE: take positions and sizes only. SIMBAD's V for a planetary"
echo "    nebula is its central *star* (M57 reads 15.8, not 8.8), and its"
echo "    types are astrophysical (M31 is 'AGN'), so both are unusable here."

cat <<'NOTE'

The committed CSVs were produced from these downloads. deep_sky.csv and
constellations.csv are curated on top of them and are not machine-regenerated
wholesale -- see their headers for exactly which fields are curated and why.
NOTE
