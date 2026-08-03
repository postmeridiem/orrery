//! Physical and visual properties of the bodies we draw.
//!
//! Sizes and rotation states are IAU/NASA planetary-fact-sheet values. The
//! colours are hand-picked approximations of each body's true appearance in
//! visible light, used as the base albedo the shaders tint their procedural
//! surface detail with.

use crate::ephemeris::Planet;

/// One astronomical unit, in kilometres (IAU 2012 definition — exact).
pub const AU_KM: f64 = 149_597_870.7;

/// Equatorial radius of the Sun, in kilometres.
pub const SUN_RADIUS_KM: f64 = 695_700.0;

/// Static properties of a rendered body.
#[derive(Debug, Clone, Copy)]
pub struct BodyData {
    /// Equatorial radius, kilometres.
    pub radius_km: f64,
    /// Polar flattening, `(r_eq − r_pol) / r_eq`. Visible on the gas giants.
    pub flattening: f64,
    /// Sidereal rotation period in hours; negative means retrograde.
    pub rotation_period_hours: f64,
    /// Obliquity of the rotation axis to the orbital plane, degrees. Values
    /// above 90° mean the body spins retrograde (Venus, Uranus, Pluto).
    pub axial_tilt_deg: f64,
    /// Base albedo colour, linear sRGB.
    pub color: [f32; 3],
    /// Rings, if any.
    pub rings: Option<Rings>,
}

/// A ring system, with radii expressed in units of the parent's equatorial radius.
#[derive(Debug, Clone, Copy)]
pub struct Rings {
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub color: [f32; 3],
    /// Broad optical depth; drives how much starlight the rings block.
    pub opacity: f32,
}

impl BodyData {
    /// Radius in astronomical units — the unit the scene graph works in.
    pub fn radius_au(&self) -> f64 {
        self.radius_km / AU_KM
    }
}

/// The Sun.
pub const SUN: BodyData = BodyData {
    radius_km: SUN_RADIUS_KM,
    flattening: 0.000_009,
    rotation_period_hours: 609.12,
    axial_tilt_deg: 7.25,
    color: [1.0, 0.95, 0.84],
    rings: None,
};

/// Earth's Moon.
pub const MOON: BodyData = BodyData {
    radius_km: 1_737.4,
    flattening: 0.0012,
    rotation_period_hours: 655.72,
    axial_tilt_deg: 6.68,
    color: [0.62, 0.60, 0.58],
    rings: None,
};

/// Physical data for `planet`.
pub const fn data(planet: Planet) -> BodyData {
    match planet {
        Planet::Mercury => BodyData {
            radius_km: 2_439.7,
            flattening: 0.0,
            rotation_period_hours: 1_407.6,
            axial_tilt_deg: 0.034,
            color: [0.58, 0.55, 0.52],
            rings: None,
        },
        Planet::Venus => BodyData {
            radius_km: 6_051.8,
            flattening: 0.0,
            rotation_period_hours: -5_832.6,
            axial_tilt_deg: 177.36,
            color: [0.94, 0.87, 0.70],
            rings: None,
        },
        Planet::Earth => BodyData {
            radius_km: 6_378.1,
            flattening: 0.003_353,
            rotation_period_hours: 23.934_5,
            axial_tilt_deg: 23.44,
            color: [0.25, 0.42, 0.68],
            rings: None,
        },
        Planet::Mars => BodyData {
            radius_km: 3_396.2,
            flattening: 0.005_89,
            rotation_period_hours: 24.622_9,
            axial_tilt_deg: 25.19,
            color: [0.76, 0.42, 0.26],
            rings: None,
        },
        Planet::Jupiter => BodyData {
            radius_km: 71_492.0,
            flattening: 0.064_87,
            rotation_period_hours: 9.925,
            axial_tilt_deg: 3.13,
            color: [0.83, 0.73, 0.60],
            rings: None,
        },
        Planet::Saturn => BodyData {
            radius_km: 60_268.0,
            flattening: 0.097_96,
            rotation_period_hours: 10.656,
            axial_tilt_deg: 26.73,
            color: [0.90, 0.80, 0.62],
            rings: Some(Rings {
                // C-ring inner edge (74 658 km) out to the A-ring outer edge
                // (136 775 km), in units of Saturn's equatorial radius.
                inner_radius: 1.239,
                outer_radius: 2.269,
                color: [0.86, 0.80, 0.70],
                opacity: 0.62,
            }),
        },
        Planet::Uranus => BodyData {
            radius_km: 25_559.0,
            flattening: 0.022_9,
            rotation_period_hours: -17.24,
            axial_tilt_deg: 97.77,
            color: [0.62, 0.83, 0.86],
            rings: Some(Rings {
                // The narrow ε-ring system: faint, nearly edge-on from Earth.
                inner_radius: 1.64,
                outer_radius: 2.00,
                color: [0.55, 0.60, 0.62],
                opacity: 0.14,
            }),
        },
        Planet::Neptune => BodyData {
            radius_km: 24_764.0,
            flattening: 0.017_1,
            rotation_period_hours: 16.11,
            axial_tilt_deg: 28.32,
            color: [0.33, 0.47, 0.83],
            rings: None,
        },
        Planet::Pluto => BodyData {
            radius_km: 1_188.3,
            flattening: 0.0,
            rotation_period_hours: -153.292_8,
            axial_tilt_deg: 122.53,
            color: [0.75, 0.68, 0.60],
            rings: None,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_planet_has_sane_physical_data() {
        for planet in Planet::ALL {
            let d = data(planet);
            assert!(d.radius_km > 0.0, "{}", planet.name());
            assert!((0.0..0.2).contains(&d.flattening), "{}", planet.name());
            assert!(d.rotation_period_hours.abs() > 1.0, "{}", planet.name());
            assert!(
                (0.0..=180.0).contains(&d.axial_tilt_deg),
                "{}",
                planet.name()
            );
            assert!(
                d.color.iter().all(|c| (0.0..=1.0).contains(c)),
                "{}",
                planet.name()
            );
            if let Some(r) = d.rings {
                assert!(
                    r.inner_radius > 1.0 && r.outer_radius > r.inner_radius,
                    "{}",
                    planet.name()
                );
            }
        }
    }

    /// Jupiter is the largest planet and Mercury the smallest; a transposed
    /// digit in the radius table would break this ordering.
    #[test]
    fn radius_ordering_is_right() {
        let radius = |p| data(p).radius_km;
        assert!(radius(Planet::Jupiter) > radius(Planet::Saturn));
        assert!(radius(Planet::Saturn) > radius(Planet::Uranus));
        assert!(radius(Planet::Uranus) > radius(Planet::Neptune));
        assert!(radius(Planet::Neptune) > radius(Planet::Earth));
        assert!(radius(Planet::Earth) > radius(Planet::Venus));
        assert!(radius(Planet::Venus) > radius(Planet::Mars));
        assert!(radius(Planet::Mars) > radius(Planet::Mercury));
        assert!(radius(Planet::Mercury) > radius(Planet::Pluto));
        assert!(SUN.radius_km > 100.0 * radius(Planet::Earth));
    }

    #[test]
    fn retrograde_rotators_are_flagged_consistently() {
        // A tilt past 90° and a negative period are two statements of the same
        // fact; they must agree.
        for planet in Planet::ALL {
            let d = data(planet);
            assert_eq!(
                d.axial_tilt_deg > 90.0,
                d.rotation_period_hours < 0.0,
                "{} disagrees about which way it spins",
                planet.name()
            );
        }
    }
}
