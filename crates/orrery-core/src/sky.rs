//! The real sky: catalogued stars, constellation figures and deep-sky objects.
//!
//! Everything here is at its true position. Stars come from the Yale Bright
//! Star Catalogue down to visual magnitude 6.5 — the whole naked-eye sky —
//! with real magnitudes and colours. Deep-sky positions and angular sizes come
//! from SIMBAD.
//!
//! Catalogue coordinates are equatorial J2000; the renderer works in an
//! ecliptic frame with Y up. The conversion is a single rotation about the
//! vernal equinox by the obliquity, so it preserves every angular separation
//! exactly — the constellations keep their shapes.
//!
//! Precession is not modelled. It moves the whole sky by about 0.014° a year,
//! so roughly a third of a degree since J2000, applied uniformly. For a
//! backdrop with no horizon to reference it against, that is invisible.

use glam::Vec3;
use serde::{Deserialize, Serialize};

/// Obliquity of the ecliptic at J2000.0, degrees (IAU 2006).
pub const OBLIQUITY_J2000_DEG: f64 = 23.439_291_1;

/// Convert equatorial J2000 right ascension and declination, in degrees, to a
/// unit vector in the renderer's Y-up scene frame.
pub fn equatorial_to_scene(right_ascension_deg: f64, declination_deg: f64) -> Vec3 {
    let (sin_ra, cos_ra) = right_ascension_deg.to_radians().sin_cos();
    let (sin_dec, cos_dec) = declination_deg.to_radians().sin_cos();
    let (sin_obliquity, cos_obliquity) = OBLIQUITY_J2000_DEG.to_radians().sin_cos();

    // Equatorial unit vector.
    let (x, y, z) = (cos_dec * cos_ra, cos_dec * sin_ra, sin_dec);

    // Rotate about the vernal equinox (+X) by the obliquity, giving ecliptic
    // coordinates. This is a rotation, so separations are preserved.
    let ecliptic_y = y * cos_obliquity + z * sin_obliquity;
    let ecliptic_z = -y * sin_obliquity + z * cos_obliquity;

    // Ecliptic -> scene, matching `scene::ecliptic_to_scene`.
    Vec3::new(x as f32, ecliptic_z as f32, -ecliptic_y as f32)
}

/// A catalogued star.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Star {
    /// Harvard Revised number, the identifier constellation figures reference.
    pub hr: u32,
    /// Unit vector toward the star, scene frame.
    pub direction: Vec3,
    /// Visual magnitude. Lower is brighter; Sirius is -1.46.
    pub magnitude: f32,
    /// Linear sRGB tint from the star's colour index.
    pub color: [f32; 3],
}

/// A constellation figure: the conventional lines joining its stars.
#[derive(Debug, Clone, PartialEq)]
pub struct Constellation {
    /// Three-letter IAU abbreviation, e.g. `Ori`.
    pub abbreviation: String,
    pub name: String,
    /// Endpoint pairs, already resolved to directions.
    pub segments: Vec<(Vec3, Vec3)>,
}

/// What a deep-sky object is, which drives how it is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeepSkyKind {
    /// Glowing hydrogen, drawn warm and diffuse: Orion, Lagoon, Carina.
    EmissionNebula,
    /// A shed stellar envelope, drawn small and teal: the Ring, the Dumbbell.
    PlanetaryNebula,
    /// A dense ancient ball of stars, drawn as a tight warm glow.
    GlobularCluster,
    /// A loose young group, drawn blue and sparse: the Pleiades.
    OpenCluster,
    /// Another galaxy, drawn as an elongated pale glow.
    Galaxy,
    /// An expanding remnant, drawn ragged: the Crab.
    SupernovaRemnant,
}

/// A deep-sky object.
#[derive(Debug, Clone, PartialEq)]
pub struct DeepSky {
    pub name: String,
    /// Unit vector toward the object, scene frame.
    pub direction: Vec3,
    /// Apparent radius on the sky, degrees.
    pub angular_radius_deg: f32,
    pub kind: DeepSkyKind,
    /// How strongly to draw it, 0..1.
    ///
    /// Deliberately *not* the integrated magnitude: that is a poor predictor of
    /// how striking an extended object looks. M31 is brighter than M42 on paper
    /// and far less arresting in a photograph. This is a curated weighting.
    pub prominence: f32,
}

/// Everything drawn behind the solar system.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Catalog {
    pub stars: Vec<Star>,
    pub constellations: Vec<Constellation>,
    pub deep_sky: Vec<DeepSky>,
}

impl Catalog {
    /// The catalogue compiled into the binary. See `data/SOURCES.md`.
    ///
    /// Parsing ~8,400 stars takes well under a millisecond, so this is done at
    /// start-up rather than baked into a binary blob, which keeps the data
    /// readable and reviewable in the repository.
    pub fn embedded() -> Result<Self, CatalogError> {
        let stars = parse_stars(include_str!("../../../data/stars.csv"))?;
        let constellations =
            parse_constellations(include_str!("../../../data/constellations.csv"), &stars)?;
        let deep_sky = parse_deep_sky(include_str!("../../../data/deep_sky.csv"))?;
        Ok(Self {
            stars,
            constellations,
            deep_sky,
        })
    }

    /// Stars brighter than `limit`, brightest first.
    pub fn stars_to_magnitude(&self, limit: f32) -> impl Iterator<Item = &Star> {
        self.stars.iter().filter(move |s| s.magnitude <= limit)
    }

    /// Look a star up by its Harvard Revised number.
    pub fn star(&self, hr: u32) -> Option<&Star> {
        self.stars
            .binary_search_by_key(&hr, |s| s.hr)
            .ok()
            .map(|index| &self.stars[index])
    }
}

/// Effective temperature in kelvin from the B-V colour index.
///
/// Ballesteros' formula, which treats a star as two blackbodies seen through
/// the B and V passbands. Good to a few percent across the main sequence.
pub fn temperature_from_color_index(b_minus_v: f64) -> f64 {
    4600.0 * (1.0 / (0.92 * b_minus_v + 1.70) + 1.0 / (0.92 * b_minus_v + 0.62))
}

/// Approximate linear-sRGB colour of a blackbody at `kelvin`.
///
/// Helland's piecewise fit to the Planckian locus, converted from the sRGB
/// values it produces back to linear. Normalised so the brightest channel is 1,
/// because magnitude carries the brightness and this carries only the hue.
pub fn blackbody_color(kelvin: f64) -> [f32; 3] {
    let t = (kelvin / 100.0).clamp(10.0, 400.0);

    let red = if t <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (t - 60.0).powf(-0.133_204_759_2)
    };
    let green = if t <= 66.0 {
        99.470_802_586 * t.ln() - 161.119_568_166
    } else {
        288.122_169_528 * (t - 60.0).powf(-0.075_514_849_2)
    };
    let blue = if t >= 66.0 {
        255.0
    } else if t <= 19.0 {
        0.0
    } else {
        138.517_731_223 * (t - 10.0).ln() - 305.044_792_730
    };

    let channels = [red, green, blue].map(|c| (c / 255.0).clamp(0.0, 1.0));
    let peak = channels.iter().copied().fold(f64::MIN, f64::max).max(1e-6);
    // sRGB transfer function -> linear.
    channels.map(|c| {
        let normalised = c / peak;
        let linear = if normalised <= 0.040_45 {
            normalised / 12.92
        } else {
            ((normalised + 0.055) / 1.055).powf(2.4)
        };
        linear as f32
    })
}

/// Linear-sRGB tint for a star of the given colour index.
pub fn color_from_index(b_minus_v: f64) -> [f32; 3] {
    blackbody_color(temperature_from_color_index(b_minus_v))
}

/// Whether a constellation figure is worth drawing this frame.
///
/// One test: is most of it *behind the Sun*? The viewpoint is out in the Oort
/// cloud looking down at the Sun, so the far side of the Sun is the middle of
/// the picture and the only place a figure can read clearly.
///
/// There is deliberately no "must fit on screen" test. Demanding whole figures
/// sounds right and is far too strict in practice: the band of sky in view is
/// only a few tens of degrees tall, so almost every constellation is larger
/// than the frame and nothing was ever drawn.
pub fn figure_is_visible(
    segments: &[(Vec3, Vec3)],
    forward: Vec3,
    min_behind_sun: f32,
) -> bool {
    if segments.is_empty() {
        return false;
    }
    let mut behind = 0usize;
    let mut total = 0usize;
    for (a, b) in segments {
        for endpoint in [a, b] {
            total += 1;
            if endpoint.dot(forward) > 0.0 {
                behind += 1;
            }
        }
    }
    behind as f32 / total as f32 >= min_behind_sun
}

#[derive(Debug, thiserror::Error)]
pub enum CatalogError {
    #[error("{file} line {line}: {reason}")]
    Malformed {
        file: &'static str,
        line: usize,
        reason: String,
    },
    #[error("constellation {constellation} references unknown star HR {hr}")]
    UnknownStar { constellation: String, hr: u32 },
}

/// Strip comments and the header row.
fn data_lines(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .skip(1)
}

/// Parse `data/stars.csv`: `hr,ra,dec,v,bv`.
pub fn parse_stars(text: &str) -> Result<Vec<Star>, CatalogError> {
    let mut stars = Vec::with_capacity(9000);
    for (line_number, line) in data_lines(text) {
        let fail = |reason: &str| CatalogError::Malformed {
            file: "stars.csv",
            line: line_number,
            reason: reason.to_owned(),
        };
        let mut fields = line.split(',');
        let mut next = |what: &str| -> Result<f64, CatalogError> {
            fields
                .next()
                .ok_or_else(|| fail(&format!("missing {what}")))?
                .trim()
                .parse::<f64>()
                .map_err(|_| fail(&format!("unparseable {what}")))
        };

        let hr = next("hr")? as u32;
        let right_ascension = next("ra")?;
        let declination = next("dec")?;
        let magnitude = next("v")?;
        let color_index = next("bv")?;

        if !(0.0..360.0).contains(&right_ascension) || !(-90.0..=90.0).contains(&declination) {
            return Err(fail("coordinates out of range"));
        }

        stars.push(Star {
            hr,
            direction: equatorial_to_scene(right_ascension, declination),
            magnitude: magnitude as f32,
            color: color_from_index(color_index),
        });
    }
    stars.sort_by_key(|s| s.hr);
    Ok(stars)
}

/// Parse `data/deep_sky.csv`: `name,ra,dec,radius_deg,kind,prominence`.
pub fn parse_deep_sky(text: &str) -> Result<Vec<DeepSky>, CatalogError> {
    let mut objects = Vec::new();
    for (line_number, line) in data_lines(text) {
        let fail = |reason: &str| CatalogError::Malformed {
            file: "deep_sky.csv",
            line: line_number,
            reason: reason.to_owned(),
        };
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let [name, ra, dec, radius, kind, prominence] = fields[..] else {
            return Err(fail("expected 6 fields"));
        };

        let kind = match kind {
            "emission_nebula" => DeepSkyKind::EmissionNebula,
            "planetary_nebula" => DeepSkyKind::PlanetaryNebula,
            "globular_cluster" => DeepSkyKind::GlobularCluster,
            "open_cluster" => DeepSkyKind::OpenCluster,
            "galaxy" => DeepSkyKind::Galaxy,
            "supernova_remnant" => DeepSkyKind::SupernovaRemnant,
            other => return Err(fail(&format!("unknown kind {other:?}"))),
        };

        objects.push(DeepSky {
            name: name.to_owned(),
            direction: equatorial_to_scene(
                ra.parse().map_err(|_| fail("unparseable ra"))?,
                dec.parse().map_err(|_| fail("unparseable dec"))?,
            ),
            angular_radius_deg: radius.parse().map_err(|_| fail("unparseable radius"))?,
            kind,
            prominence: prominence.parse().map_err(|_| fail("unparseable prominence"))?,
        });
    }
    Ok(objects)
}

/// Parse `data/constellations.csv`: `abbreviation,name,hr_a,hr_b`, one line per
/// segment. Endpoints are resolved against `stars`.
pub fn parse_constellations(
    text: &str,
    stars: &[Star],
) -> Result<Vec<Constellation>, CatalogError> {
    let find = |hr: u32| {
        stars
            .binary_search_by_key(&hr, |s| s.hr)
            .ok()
            .map(|index| stars[index].direction)
    };

    let mut constellations: Vec<Constellation> = Vec::new();
    for (line_number, line) in data_lines(text) {
        let fail = |reason: &str| CatalogError::Malformed {
            file: "constellations.csv",
            line: line_number,
            reason: reason.to_owned(),
        };
        let fields: Vec<&str> = line.split(',').map(str::trim).collect();
        let [abbreviation, name, a, b] = fields[..] else {
            return Err(fail("expected 4 fields"));
        };
        let a: u32 = a.parse().map_err(|_| fail("unparseable hr_a"))?;
        let b: u32 = b.parse().map_err(|_| fail("unparseable hr_b"))?;

        let resolve = |hr| {
            find(hr).ok_or_else(|| CatalogError::UnknownStar {
                constellation: abbreviation.to_owned(),
                hr,
            })
        };
        let segment = (resolve(a)?, resolve(b)?);

        match constellations
            .iter_mut()
            .find(|c| c.abbreviation == abbreviation)
        {
            Some(existing) => existing.segments.push(segment),
            None => constellations.push(Constellation {
                abbreviation: abbreviation.to_owned(),
                name: name.to_owned(),
                segments: vec![segment],
            }),
        }
    }
    Ok(constellations)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Angular separation between two unit vectors, in degrees.
    fn separation_deg(a: Vec3, b: Vec3) -> f64 {
        (a.dot(b).clamp(-1.0, 1.0) as f64).acos().to_degrees()
    }

    /// Separation computed directly in equatorial coordinates, for comparison.
    fn equatorial_separation_deg(a: (f64, f64), b: (f64, f64)) -> f64 {
        let (ra1, dec1) = (a.0.to_radians(), a.1.to_radians());
        let (ra2, dec2) = (b.0.to_radians(), b.1.to_radians());
        (dec1.sin() * dec2.sin() + dec1.cos() * dec2.cos() * (ra1 - ra2).cos())
            .clamp(-1.0, 1.0)
            .acos()
            .to_degrees()
    }

    #[test]
    fn transform_produces_unit_vectors() {
        for ra in (0..360).step_by(17) {
            for dec in (-90..=90).step_by(13) {
                let v = equatorial_to_scene(ra as f64, dec as f64);
                assert!((v.length() - 1.0).abs() < 1e-5, "ra {ra} dec {dec}");
            }
        }
    }

    /// The whole point: the transform is a rotation, so constellations keep
    /// their shapes. Any separation must survive it untouched.
    #[test]
    fn transform_preserves_angular_separation() {
        let samples = [
            (0.0, 0.0),
            (101.287, -16.716),  // Sirius
            (79.172, 45.998),    // Capella
            (213.915, 19.182),   // Arcturus
            (279.235, 38.784),   // Vega
            (37.954, 89.264),    // Polaris
            (180.0, -60.0),
        ];
        for (index, a) in samples.iter().enumerate() {
            for b in &samples[index + 1..] {
                let direct = equatorial_separation_deg(*a, *b);
                let transformed =
                    separation_deg(equatorial_to_scene(a.0, a.1), equatorial_to_scene(b.0, b.1));
                assert!(
                    (direct - transformed).abs() < 1e-3,
                    "{a:?} to {b:?}: {direct} vs {transformed}"
                );
            }
        }
    }

    /// The vernal equinox is the shared origin of both frames, so it must land
    /// on the scene +X axis exactly.
    #[test]
    fn vernal_equinox_is_the_scene_x_axis() {
        let v = equatorial_to_scene(0.0, 0.0);
        assert!((v - Vec3::X).length() < 1e-6, "{v:?}");
    }

    /// The north celestial pole sits one obliquity away from the ecliptic pole,
    /// which is the scene's up axis.
    #[test]
    fn celestial_pole_is_one_obliquity_from_up() {
        let pole = equatorial_to_scene(0.0, 90.0);
        let separation = separation_deg(pole, Vec3::Y);
        assert!(
            (separation - OBLIQUITY_J2000_DEG).abs() < 1e-3,
            "celestial pole is {separation}° from scene up"
        );
    }

    /// The ecliptic pole must map to scene up, so the solar system lies flat.
    #[test]
    fn ecliptic_pole_is_scene_up() {
        // The ecliptic north pole is at RA 18h (270°), Dec 90 - obliquity.
        let v = equatorial_to_scene(270.0, 90.0 - OBLIQUITY_J2000_DEG);
        assert!((v - Vec3::Y).length() < 1e-5, "{v:?}");
    }

    #[test]
    fn star_colours_run_blue_through_white_to_red() {
        let hot = color_from_index(-0.30); // a B-type star
        let sun = color_from_index(0.65); // the Sun
        let cool = color_from_index(1.85); // Betelgeuse

        assert!(hot[2] > hot[0], "a hot star must be bluer than it is red");
        assert!(cool[0] > cool[2], "a cool star must be redder than it is blue");
        // The Sun sits between the two.
        assert!(sun[0] > hot[0] && sun[2] > cool[2]);
        for colour in [hot, sun, cool] {
            assert!(colour.iter().all(|c| (0.0..=1.0).contains(c)), "{colour:?}");
            assert!(colour.iter().any(|c| *c > 0.9), "must be normalised");
        }
    }

    /// Ballesteros' formula against published effective temperatures.
    #[test]
    fn temperatures_are_plausible() {
        // Vega, B-V 0.00, Teff about 9600 K.
        let vega = temperature_from_color_index(0.0);
        assert!((8000.0..11000.0).contains(&vega), "Vega came out at {vega} K");
        // Betelgeuse, B-V 1.85, Teff about 3600 K.
        let betelgeuse = temperature_from_color_index(1.85);
        assert!(
            (3000.0..4000.0).contains(&betelgeuse),
            "Betelgeuse came out at {betelgeuse} K"
        );
        // Hotter stars must always come out hotter.
        let mut previous = f64::MAX;
        let mut index = -0.3;
        while index < 2.0 {
            let t = temperature_from_color_index(index);
            assert!(t < previous, "temperature must fall as B-V rises");
            previous = t;
            index += 0.05;
        }
    }

    #[test]
    fn parses_a_star_file() {
        let text = "# comment\nhr,ra,dec,v,bv\n2491,101.2871,-16.7161,-1.46,0.00\n7001,279.2347,38.7837,0.03,0.00\n";
        let stars = parse_stars(text).unwrap();
        assert_eq!(stars.len(), 2);
        let sirius = stars.iter().find(|s| s.hr == 2491).unwrap();
        assert!((sirius.magnitude + 1.46).abs() < 1e-6);
        assert!((sirius.direction.length() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn rejects_malformed_star_data() {
        for bad in [
            "hr,ra,dec,v,bv\n2491,999.0,-16.7,-1.46,0.0\n",   // ra out of range
            "hr,ra,dec,v,bv\n2491,101.2,-99.0,-1.46,0.0\n",   // dec out of range
            "hr,ra,dec,v,bv\n2491,101.2,-16.7\n",             // truncated
            "hr,ra,dec,v,bv\n2491,abc,-16.7,-1.46,0.0\n",     // unparseable
        ] {
            assert!(parse_stars(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn constellation_segments_resolve_against_the_stars() {
        // Rigel and Betelgeuse, exactly as they appear in data/stars.csv.
        let stars = parse_stars(
            "hr,ra,dec,v,bv\n1713,78.6346,-8.2017,0.12,-0.03\n2061,88.7929,7.4069,0.50,1.85\n",
        )
        .unwrap();
        let figures =
            parse_constellations("abbr,name,a,b\nOri,Orion,1713,2061\n", &stars).unwrap();
        assert_eq!(figures.len(), 1);
        assert_eq!(figures[0].segments.len(), 1);

        // Rigel to Betelgeuse spans 18.606 degrees of sky.
        let (a, b) = figures[0].segments[0];
        let separation = separation_deg(a, b);
        assert!(
            (separation - 18.606).abs() < 0.01,
            "Rigel to Betelgeuse came out {separation}°"
        );
    }

    #[test]
    fn constellations_referencing_missing_stars_are_rejected() {
        let stars = parse_stars("hr,ra,dec,v,bv\n1713,78.6346,-8.2017,0.12,-0.03\n").unwrap();
        assert!(matches!(
            parse_constellations("abbr,name,a,b\nOri,Orion,1713,9999\n", &stars),
            Err(CatalogError::UnknownStar { hr: 9999, .. })
        ));
    }

    #[test]
    fn figures_are_drawn_only_when_behind_the_sun() {
        let forward = Vec3::NEG_Z;
        let ahead = [(
            Vec3::new(-0.05, 0.0, -1.0).normalize(),
            Vec3::new(0.05, 0.0, -1.0).normalize(),
        )];
        assert!(figure_is_visible(&ahead, forward, 0.8));

        let behind = [(
            Vec3::new(-0.05, 0.0, 1.0).normalize(),
            Vec3::new(0.05, 0.0, 1.0).normalize(),
        )];
        assert!(!figure_is_visible(&behind, forward, 0.8));
    }

    /// A figure straddling the horizon must fail the threshold rather than be
    /// drawn half off the back of the sky.
    #[test]
    fn figures_straddling_the_horizon_are_dropped() {
        let forward = Vec3::NEG_Z;
        let straddling = [
            (Vec3::NEG_Z, Vec3::new(0.0, 0.2, -1.0).normalize()),
            (Vec3::Z, Vec3::new(0.0, 0.2, 1.0).normalize()),
        ];
        // Half in front, half behind: below any sensible threshold.
        assert!(!figure_is_visible(&straddling, forward, 0.8));
        // ...but a bare majority test would let it through.
        assert!(figure_is_visible(&straddling, forward, 0.5));
    }

    #[test]
    fn an_empty_figure_is_never_visible() {
        assert!(!figure_is_visible(&[], Vec3::NEG_Z, 0.0));
    }

    #[test]
    fn parses_deep_sky_objects() {
        let text = "name,ra,dec,radius,kind,prominence\n\
                    Orion Nebula,83.8221,-5.3911,0.55,emission_nebula,1.0\n";
        let objects = parse_deep_sky(text).unwrap();
        assert_eq!(objects.len(), 1);
        assert_eq!(objects[0].kind, DeepSkyKind::EmissionNebula);
        assert!((objects[0].direction.length() - 1.0).abs() < 1e-5);
    }
}
