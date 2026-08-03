//! Choosing where a position comes from.
//!
//! Two sources, in preference order:
//!
//! 1. An [`Almanac`] of osculating elements looked up from JPL Horizons, good
//!    to about 0.18 arcseconds while it covers the moment being drawn.
//! 2. The built-in Standish tables, good to 0.11° in the worst case, always
//!    available and needing no network.
//!
//! The fallback is what makes the network optional rather than required: with
//! no almanac, a stale one, or no connectivity ever, the orrery still draws the
//! right sky — just slightly less precisely.

use glam::DVec3;

use crate::almanac::Almanac;
use crate::ephemeris::{self, Kepler, Planet};
use crate::time::JulianDate;

/// Where a given position actually came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Propagated from Horizons osculating elements.
    Almanac,
    /// The built-in Keplerian tables.
    BuiltIn,
}

/// Resolves positions from the best source available.
#[derive(Debug, Clone, Default)]
pub struct Lookup {
    almanac: Option<Almanac>,
}

impl Lookup {
    /// The built-in tables only. This is what the renderer uses until an
    /// almanac has been loaded or fetched.
    pub fn builtin() -> Self {
        Self { almanac: None }
    }

    pub fn with_almanac(almanac: Almanac) -> Self {
        Self {
            almanac: Some(almanac),
        }
    }

    pub fn set_almanac(&mut self, almanac: Option<Almanac>) {
        self.almanac = almanac;
    }

    pub fn almanac(&self) -> Option<&Almanac> {
        self.almanac.as_ref()
    }

    /// Which source would answer for `planet` at `at`.
    pub fn source(&self, planet: Planet, at: JulianDate) -> Source {
        match &self.almanac {
            Some(almanac) if almanac.position(planet, at).is_some() => Source::Almanac,
            _ => Source::BuiltIn,
        }
    }

    /// Heliocentric position, AU, J2000 ecliptic frame.
    pub fn position(&self, planet: Planet, at: JulianDate) -> DVec3 {
        self.almanac
            .as_ref()
            .and_then(|almanac| almanac.position(planet, at))
            .unwrap_or_else(|| ephemeris::heliocentric_position(planet, at))
    }

    /// Elements to draw this body's orbit ring from.
    ///
    /// When an almanac is in use these are the true osculating elements, so the
    /// drawn ellipse is the orbit the planet is actually on right now rather
    /// than a long-term mean.
    pub fn elements(&self, planet: Planet, at: JulianDate) -> Kepler {
        self.almanac
            .as_ref()
            .filter(|almanac| almanac.position(planet, at).is_some())
            .and_then(|almanac| almanac.nearest(planet, at))
            .map(|set| set.elements_at(at))
            .unwrap_or_else(|| ephemeris::elements_at(planet, at))
    }

    /// Is every drawn body being served by the almanac at `at`?
    pub fn fully_covered(&self, at: JulianDate) -> bool {
        self.almanac
            .as_ref()
            .is_some_and(|almanac| Planet::ALL.iter().all(|p| almanac.position(*p, at).is_some()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::almanac::Osculating;
    use std::collections::BTreeMap;

    /// Osculating elements taken from the built-in tables, so the two sources
    /// must agree closely and any difference is attributable to propagation.
    fn almanac_from_builtin(planet: Planet, epochs: &[f64]) -> Almanac {
        let sets = epochs
            .iter()
            .map(|epoch| {
                let k = ephemeris::elements_at(planet, JulianDate(*epoch));
                Osculating {
                    epoch: *epoch,
                    a: k.a,
                    e: k.e,
                    inclination: k.i,
                    ascending_node: k.longitude_of_ascending_node,
                    argument_of_perihelion: k.argument_of_perihelion(),
                    mean_anomaly: k.mean_anomaly(),
                    // Kepler's third law, in degrees per day.
                    mean_motion: 0.985_608_/ k.a.powf(1.5),
                }
            })
            .collect();
        Almanac {
            retrieved: epochs[0],
            bodies: BTreeMap::from([(planet.name().to_owned(), sets)]),
        }
    }

    #[test]
    fn falls_back_to_builtin_without_an_almanac() {
        let lookup = Lookup::builtin();
        let at = JulianDate(2_461_255.5);
        assert_eq!(lookup.source(Planet::Mars, at), Source::BuiltIn);
        assert_eq!(
            lookup.position(Planet::Mars, at),
            ephemeris::heliocentric_position(Planet::Mars, at)
        );
    }

    #[test]
    fn uses_the_almanac_where_it_covers_and_falls_back_outside() {
        let epochs = [2_461_041.5, 2_461_071.9, 2_461_102.4];
        let lookup = Lookup::with_almanac(almanac_from_builtin(Planet::Earth, &epochs));

        let inside = JulianDate(2_461_060.0);
        assert_eq!(lookup.source(Planet::Earth, inside), Source::Almanac);

        // Well outside the epoch range, and a body the almanac never carried.
        assert_eq!(
            lookup.source(Planet::Earth, JulianDate(2_462_500.0)),
            Source::BuiltIn
        );
        assert_eq!(lookup.source(Planet::Mars, inside), Source::BuiltIn);
    }

    /// An almanac derived from the built-in tables must reproduce them, or the
    /// propagation itself is wrong.
    #[test]
    fn almanac_propagation_reproduces_its_own_source() {
        let epochs = [2_461_041.5, 2_461_071.9, 2_461_102.4];
        for planet in Planet::ALL {
            let lookup = Lookup::with_almanac(almanac_from_builtin(planet, &epochs));
            for day in 0..60 {
                let at = JulianDate(2_461_041.5 + day as f64);
                let from_almanac = lookup.position(planet, at);
                let from_tables = ephemeris::heliocentric_position(planet, at);
                let separation = from_almanac
                    .normalize()
                    .dot(from_tables.normalize())
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees();
                assert!(
                    separation < 0.01,
                    "{} day {day}: {separation}° apart",
                    planet.name()
                );
            }
        }
    }

    #[test]
    fn switching_the_almanac_changes_the_answer() {
        let mut lookup = Lookup::builtin();
        let at = JulianDate(2_461_060.0);
        assert!(!lookup.fully_covered(at));

        lookup.set_almanac(Some(almanac_from_builtin(Planet::Earth, &[2_461_041.5])));
        assert_eq!(lookup.source(Planet::Earth, at), Source::Almanac);
        // Only Earth is covered, so the scene as a whole is not.
        assert!(!lookup.fully_covered(at));

        lookup.set_almanac(None);
        assert_eq!(lookup.source(Planet::Earth, at), Source::BuiltIn);
    }
}
