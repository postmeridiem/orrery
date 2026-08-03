# Where the data comes from, and under what terms

Everything shipped here is compatible with this project's MIT licence. That
constraint did real work: the obvious sources for constellation figures and
deep-sky catalogues are copyleft or non-commercial, and were rejected.

## Stars — `stars.csv`

Bright Star Catalogue, 5th Revised Ed. (Hoffleit & Warren, 1991), NSSDC/ADC,
retrieved via the VizieR catalogue access tool, CDS, Strasbourg, France
(catalogue `V/50`, DOI 10.26093/cds/vizier).

The `V/50` ReadMe asserts no copyright and the catalogue originates as a US
Government work; positions and magnitudes are uncopyrightable fact. CDS asks
that the origin be cited, which is what this file does.

*Rejected:* the HYG database is the convenient single-CSV choice and is
CC BY-SA 4.0 — ShareAlike, so a derived file would not be MIT.

## Constellation figures — `constellations.csv`

Authored for this project. The IAU standardises constellation *boundaries* but
not the stick figures, so every published set is an artistic work carrying its
author's licence:

- Stellarium's `modern` / `western`: CC BY-SA 4.0
- Stellarium's `modern_st`: CC BY-SA 2.0, derived from Sky & Telescope's data
- Stellarium's `modern_iau`: labelled CC BY-SA 4.0 "Stellarium's team", but the
  line data is a byte-identical copy of the Sky & Telescope set
- `stellarium-skycultures` repository root: AGPL-3.0

None can be redistributed under MIT. The figures here are therefore simple
asterisms built from the conventional Bayer-letter patterns, with star
references resolved programmatically from the Yale catalogue's own Bayer
designations rather than typed by hand. Twenty constellations, deliberately —
the brief was the major ones, not all 88.

*Permissive alternative, if a fuller set is ever wanted:* d3-celestial's
`constellations.lines.json` is BSD-3-Clause and traces to the IAU charts
(CC BY 4.0). It bakes RA/Dec rather than star identifiers, so its lines do not
terminate exactly on catalogue stars.

## Deep-sky objects — `deep_sky.csv`

Positions and angular sizes verified against SIMBAD, operated at CDS,
Strasbourg, France. The object selection is ours; magnitudes are deliberately
not used (see the file header).

*Rejected:* VizieR `VII/118` (NGC 2000.0) carries an explicit notice that the
data is "for scientific research purposes only" and "should not be used for
commercial purposes without the explicit permission of Sky Publishing
Corporation" — incompatible with MIT, which permits commercial use. OpenNGC is
CC BY-SA 4.0. Steinicke's Revised NGC/IC requires written permission.

## Almanac — `almanac.toml`

Osculating elements from the JPL Horizons system, Solar System Dynamics Group,
Jet Propulsion Laboratory. Regenerate with `orrery --refresh-ephemeris`.
