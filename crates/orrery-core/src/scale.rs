//! Mapping the real solar system onto something you can actually look at.
//!
//! Two independent problems, two independent laws:
//!
//! * **Orbits.** Neptune is 78× further out than Mercury. Drawn to scale with
//!   Neptune on screen, Mercury's orbit is a few pixels across.
//! * **Bodies.** Earth's radius is 4.3e-5 AU. Drawn to scale at any orbit
//!   framing, every planet is sub-pixel.
//!
//! Both laws only ever rescale a *radius*. Direction is never touched, so
//! heliocentric longitude and latitude — and therefore every conjunction,
//! opposition and alignment — stay exactly as [`crate::ephemeris`] computed
//! them. The picture is compressed, not falsified.

use serde::{Deserialize, Serialize};

/// Earth's equatorial radius in km, the reference for [`BodyScale`]. Taken
/// from the body table rather than restated, so one physical constant has one
/// home.
const EARTH_RADIUS_KM: f64 = crate::bodies::data(crate::ephemeris::Planet::Earth).radius_km;

/// The floor below which a heliocentric distance is treated as "at the Sun".
/// A magnitude, unlike `f64::EPSILON` — which is relative spacing at 1.0, not
/// a smallness threshold. 1e-12 AU is a sixth of a millimetre.
const NEGLIGIBLE_AU: f64 = 1e-12;

/// How orbital distance in AU becomes distance in scene units.
///
/// `Deserialize` is written by hand below: serde does not honour
/// `deny_unknown_fields` on internally tagged enums, so the derived form let
/// `[scale.orbit]` silently swallow typos and leftover keys — the one section
/// of the config that broke the promise in [`crate::config`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "law", rename_all = "snake_case")]
pub enum RadialScale {
    /// True to scale. Honest, and unusable unless you are framing the inner
    /// system: with Neptune in shot the terrestrial planets are a smudge.
    Linear { units_per_au: f64 },

    /// `r^exponent`, scaled. A power law is the classic orrery compression —
    /// smooth, self-similar, and with no special radius where behaviour
    /// changes. `exponent: 1.0` degenerates to [`RadialScale::Linear`].
    Power { units_per_au: f64, exponent: f64 },

    /// `softness · ln(1 + r/softness)`, scaled. More aggressive than a power
    /// law in the outer system; `softness` sets the radius below which the
    /// mapping is roughly linear.
    Logarithmic { units_per_au: f64, softness: f64 },
}

impl Default for RadialScale {
    /// Compressed by default, per the design brief: every planet visible at
    /// once, all angles exact.
    fn default() -> Self {
        RadialScale::Power {
            units_per_au: 1.0,
            exponent: 0.45,
        }
    }
}

impl<'de> Deserialize<'de> for RadialScale {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error;

        /// The raw shape, which — being a struct — *does* honour
        /// `deny_unknown_fields`, so `expoennt = 0.45` is an error naming the
        /// stray key rather than a silently applied default.
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields, rename_all = "snake_case")]
        struct Raw {
            law: Law,
            units_per_au: f64,
            exponent: Option<f64>,
            softness: Option<f64>,
        }
        #[derive(Deserialize, Clone, Copy)]
        #[serde(rename_all = "snake_case")]
        enum Law {
            Linear,
            Power,
            Logarithmic,
        }

        let raw = Raw::deserialize(deserializer)?;
        // A parameter belonging to a different law is a mistake worth naming:
        // a leftover `exponent` under `law = "linear"` almost certainly means
        // the law was changed and the intent did not follow.
        let forbid = |value: Option<f64>, key: &str, law: &str| match value {
            None => Ok(()),
            Some(_) => Err(D::Error::custom(format!(
                "`{key}` does not apply to law = \"{law}\""
            ))),
        };
        match raw.law {
            Law::Linear => {
                forbid(raw.exponent, "exponent", "linear")?;
                forbid(raw.softness, "softness", "linear")?;
                Ok(RadialScale::Linear {
                    units_per_au: raw.units_per_au,
                })
            }
            Law::Power => {
                forbid(raw.softness, "softness", "power")?;
                Ok(RadialScale::Power {
                    units_per_au: raw.units_per_au,
                    exponent: raw
                        .exponent
                        .ok_or_else(|| D::Error::custom("law = \"power\" needs `exponent`"))?,
                })
            }
            Law::Logarithmic => {
                forbid(raw.exponent, "exponent", "logarithmic")?;
                Ok(RadialScale::Logarithmic {
                    units_per_au: raw.units_per_au,
                    softness: raw.softness.ok_or_else(|| {
                        D::Error::custom("law = \"logarithmic\" needs `softness`")
                    })?,
                })
            }
        }
    }
}

impl RadialScale {
    /// Map a heliocentric distance in AU to scene units.
    ///
    /// Monotonic and continuous for every legal parameter set, so orbits never
    /// cross and the outward ordering of the planets is preserved.
    pub fn apply(&self, distance_au: f64) -> f64 {
        let r = distance_au.max(0.0);
        match *self {
            RadialScale::Linear { units_per_au } => units_per_au * r,
            RadialScale::Power {
                units_per_au,
                exponent,
            } => units_per_au * r.powf(exponent),
            RadialScale::Logarithmic {
                units_per_au,
                softness,
            } => {
                // `validate` rejects non-positive softness; this floor is
                // insurance for direct construction, not a correction.
                let s = softness.max(NEGLIGIBLE_AU);
                units_per_au * s * (1.0 + r / s).ln()
            }
        }
    }

    /// Rescale a heliocentric position, preserving its direction exactly.
    pub fn apply_to_position(&self, position_au: glam::DVec3) -> glam::DVec3 {
        let r = position_au.length();
        if r <= NEGLIGIBLE_AU {
            return glam::DVec3::ZERO;
        }
        position_au * (self.apply(r) / r)
    }

    /// Reject parameter sets that would produce a non-monotonic or degenerate
    /// mapping, so a typo in the config file is reported rather than rendered.
    pub fn validate(&self) -> Result<(), ScaleError> {
        let units_per_au = match *self {
            RadialScale::Linear { units_per_au }
            | RadialScale::Power { units_per_au, .. }
            | RadialScale::Logarithmic { units_per_au, .. } => units_per_au,
        };
        if !(units_per_au.is_finite() && units_per_au > 0.0) {
            return Err(ScaleError::UnitsPerAu(units_per_au));
        }
        match *self {
            RadialScale::Power { exponent, .. } if !(exponent.is_finite() && exponent > 0.0) => {
                Err(ScaleError::Exponent(exponent))
            }
            RadialScale::Logarithmic { softness, .. }
                if !(softness.is_finite() && softness > 0.0) =>
            {
                Err(ScaleError::Softness(softness))
            }
            _ => Ok(()),
        }
    }
}

/// How a body's true radius becomes its drawn radius, in scene units.
///
/// Also a power law, for the same reason: it keeps the *ordering* and a
/// recognisable sense of relative size (Jupiter clearly dwarfs Earth, Earth
/// clearly dwarfs Mercury) while lifting everything above one pixel.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct BodyScale {
    /// Drawn radius of an Earth-sized body, in scene units.
    pub earth_radius_units: f64,
    /// Compression exponent. 1.0 keeps true relative sizes (Jupiter 11× Earth);
    /// lower values pull the extremes together.
    pub exponent: f64,
    /// Extra multiplier applied to the Sun alone. Physically the Sun is 109
    /// Earth radii, which even compressed tends to dominate the frame; this
    /// lets it be dialled back without touching the planets.
    pub sun_multiplier: f64,
}

impl Default for BodyScale {
    fn default() -> Self {
        Self {
            earth_radius_units: 0.054,
            exponent: 0.4,
            sun_multiplier: 0.85,
        }
    }
}

impl BodyScale {
    /// Drawn radius, in scene units, for a body of the given true radius.
    pub fn apply(&self, radius_km: f64) -> f64 {
        self.earth_radius_units * (radius_km.max(0.0) / EARTH_RADIUS_KM).powf(self.exponent)
    }

    /// Drawn radius for the Sun, including [`BodyScale::sun_multiplier`].
    pub fn apply_to_sun(&self, radius_km: f64) -> f64 {
        self.apply(radius_km) * self.sun_multiplier
    }

    pub fn validate(&self) -> Result<(), ScaleError> {
        if !(self.earth_radius_units.is_finite() && self.earth_radius_units > 0.0) {
            return Err(ScaleError::EarthRadiusUnits(self.earth_radius_units));
        }
        if !(self.exponent.is_finite() && self.exponent > 0.0) {
            return Err(ScaleError::Exponent(self.exponent));
        }
        if !(self.sun_multiplier.is_finite() && self.sun_multiplier > 0.0) {
            return Err(ScaleError::SunMultiplier(self.sun_multiplier));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum ScaleError {
    #[error("units_per_au must be finite and positive, got {0}")]
    UnitsPerAu(f64),
    #[error("exponent must be finite and positive, got {0}")]
    Exponent(f64),
    #[error("softness must be finite and positive, got {0}")]
    Softness(f64),
    #[error("earth_radius_units must be finite and positive, got {0}")]
    EarthRadiusUnits(f64),
    #[error("sun_multiplier must be finite and positive, got {0}")]
    SunMultiplier(f64),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ephemeris::{Planet, elements_at};
    use crate::time::JulianDate;

    const ALL_LAWS: [RadialScale; 4] = [
        RadialScale::Linear { units_per_au: 1.0 },
        RadialScale::Power {
            units_per_au: 1.0,
            exponent: 0.45,
        },
        RadialScale::Power {
            units_per_au: 2.5,
            exponent: 1.0,
        },
        RadialScale::Logarithmic {
            units_per_au: 1.0,
            softness: 0.3,
        },
    ];

    /// The whole point of rescaling only the radius: direction survives.
    #[test]
    fn direction_is_never_altered() {
        let jd = JulianDate(2_461_255.5);
        for law in ALL_LAWS {
            for planet in Planet::ALL {
                let original = crate::ephemeris::heliocentric_position(planet, jd);
                let scaled = law.apply_to_position(original);
                let cosine = original.normalize().dot(scaled.normalize());
                assert!(
                    (cosine - 1.0).abs() < 1e-12,
                    "{law:?} rotated {}",
                    planet.name()
                );
            }
        }
    }

    /// Orbits must never cross: a body further out in reality must be further
    /// out on screen, under every law.
    #[test]
    fn outward_ordering_is_preserved() {
        let jd = JulianDate(2_461_255.5);
        for law in ALL_LAWS {
            let mut previous = 0.0;
            for planet in Planet::ALL {
                let scaled = law.apply(elements_at(planet, jd).a);
                assert!(
                    scaled > previous,
                    "{law:?}: {} maps to {scaled}, inside the previous orbit at {previous}",
                    planet.name()
                );
                previous = scaled;
            }
        }
    }

    /// Strictly increasing all the way from the Sun's centre outward past
    /// Pluto's aphelion, so orbits can never cross or invert.
    #[test]
    fn scaling_is_monotonic() {
        for law in ALL_LAWS {
            let mut previous = law.apply(0.0);
            let mut r = 0.0;
            while r < 50.0 {
                r += 0.01;
                let current = law.apply(r);
                assert!(current > previous, "{law:?} not increasing at r={r}");
                previous = current;
            }
        }
    }

    /// No jumps across the range anything actually occupies.
    ///
    /// The lower bound matters: a power law has unbounded slope as r → 0, which
    /// is real behaviour and not a defect — but only the Sun sits near the
    /// origin, and Mercury's perihelion is 0.307 AU. Testing from 0.05 AU
    /// outward covers every radius a body can ever have while still catching a
    /// genuine discontinuity.
    #[test]
    fn scaling_is_smooth_over_the_occupied_range() {
        const STEP: f64 = 0.01;
        for law in ALL_LAWS {
            let mut r = 0.05;
            let mut previous = law.apply(r);
            while r < 50.0 {
                r += STEP;
                let current = law.apply(r);
                assert!(
                    current - previous < 0.05,
                    "{law:?} jumps by {} across a {STEP} AU step at r={r}",
                    current - previous
                );
                previous = current;
            }
        }
    }

    #[test]
    fn origin_maps_to_origin() {
        for law in ALL_LAWS {
            assert_eq!(law.apply(0.0), 0.0, "{law:?}");
            assert_eq!(law.apply_to_position(glam::DVec3::ZERO), glam::DVec3::ZERO);
        }
    }

    /// A power law with exponent 1 is exactly the linear law.
    #[test]
    fn power_of_one_is_linear() {
        let linear = RadialScale::Linear { units_per_au: 3.0 };
        let power = RadialScale::Power {
            units_per_au: 3.0,
            exponent: 1.0,
        };
        for r in [0.1, 0.5, 1.0, 5.2, 30.1] {
            assert!((linear.apply(r) - power.apply(r)).abs() < 1e-12);
        }
    }

    /// The default compression has to actually solve the problem it exists for:
    /// every planet visible at once, with Mercury not vanishing next to Neptune.
    #[test]
    fn default_compression_keeps_every_orbit_visible() {
        let law = RadialScale::default();
        let jd = JulianDate(2_461_255.5);
        let mercury = law.apply(elements_at(Planet::Mercury, jd).a);
        let neptune = law.apply(elements_at(Planet::Neptune, jd).a);
        let true_ratio = elements_at(Planet::Neptune, jd).a / elements_at(Planet::Mercury, jd).a;
        assert!(true_ratio > 75.0, "sanity: the real ratio is huge");
        assert!(
            neptune / mercury < 10.0,
            "compressed ratio {} is still too extreme",
            neptune / mercury
        );
    }

    #[test]
    fn body_scale_preserves_size_ordering() {
        let scale = BodyScale::default();
        let radius = |p| scale.apply(crate::bodies::data(p).radius_km);
        assert!(radius(Planet::Jupiter) > radius(Planet::Neptune));
        assert!(radius(Planet::Neptune) > radius(Planet::Earth));
        assert!(radius(Planet::Earth) > radius(Planet::Mercury));
        assert!(radius(Planet::Mercury) > radius(Planet::Pluto));
    }

    /// Compressed bodies must be big enough to see but not so big they swallow
    /// their own orbit — Mercury's disc must fit well inside Mercury's orbit.
    #[test]
    fn default_bodies_fit_inside_their_orbits() {
        let orbit = RadialScale::default();
        let body = BodyScale::default();
        let jd = JulianDate(2_461_255.5);
        for planet in Planet::ALL {
            let orbit_units = orbit.apply(elements_at(planet, jd).a);
            let body_units = body.apply(crate::bodies::data(planet).radius_km);
            assert!(
                body_units < orbit_units * 0.25,
                "{} is drawn at {body_units} inside an orbit of {orbit_units}",
                planet.name()
            );
            assert!(body_units > 0.005, "{} is too small to see", planet.name());
        }
        let sun = body.apply_to_sun(crate::bodies::SUN_RADIUS_KM);
        let mercury_orbit = orbit.apply(elements_at(Planet::Mercury, jd).a);
        assert!(
            sun < mercury_orbit * 0.5,
            "the Sun at {sun} crowds Mercury's orbit at {mercury_orbit}"
        );
    }

    #[test]
    fn validation_rejects_nonsense() {
        assert!(RadialScale::default().validate().is_ok());
        assert!(BodyScale::default().validate().is_ok());
        assert!(
            RadialScale::Linear { units_per_au: 0.0 }
                .validate()
                .is_err()
        );
        assert!(
            RadialScale::Power {
                units_per_au: 1.0,
                exponent: -0.5
            }
            .validate()
            .is_err()
        );
        assert!(
            RadialScale::Logarithmic {
                units_per_au: 1.0,
                softness: 0.0
            }
            .validate()
            .is_err()
        );
        assert!(
            RadialScale::Power {
                units_per_au: f64::NAN,
                exponent: 1.0
            }
            .validate()
            .is_err()
        );
    }
}
