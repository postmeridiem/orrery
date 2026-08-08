//! Osculating elements looked up from JPL Horizons, and propagation from them.
//!
//! The built-in tables in [`crate::ephemeris`] carry mean elements at J2000
//! plus a linear rate per century. That is a *fit* across 250 years, so it
//! cannot represent the periodic perturbations planets exert on each other —
//! most visibly the Jupiter–Saturn great inequality, which leaves Saturn up to
//! 0.11° from its true position.
//!
//! An almanac fixes that by asking Horizons for the real osculating orbit at a
//! series of epochs and propagating from whichever epoch is nearest. Because
//! the propagation interval is then at most a couple of weeks rather than
//! decades, the perturbations barely have time to accumulate.
//!
//! Measured against Horizons state vectors at four dates across the bundled
//! almanac's year (`tests/almanac_accuracy.rs`, which prints the numbers),
//! worst-case heliocentric direction error falls from 0.074° to 2.2e-5° —
//! 0.079 arcseconds. The tables' own worst case over their full 1990–2044
//! validity span is a little higher, 0.11° (`ephemeris.rs` tests).
//!
//! One annual network request per body covers a whole year, because Horizons
//! returns every requested epoch in a single response.

use std::collections::BTreeMap;

use glam::DVec3;
use serde::{Deserialize, Serialize};

use crate::ephemeris::{Kepler, Planet};
use crate::time::JulianDate;

/// Nominal spacing between epochs, in days. One month is comfortably fine
/// enough: the residual error is dominated by the look-up itself, not the gap.
pub const EPOCH_SPACING_DAYS: f64 = 30.4375;

/// How far outside its epoch range an almanac is still considered usable.
/// Beyond this the built-in tables take over.
pub const COVERAGE_MARGIN_DAYS: f64 = 45.0;

/// A single osculating element set: the orbit a body is instantaneously on at
/// `epoch`, as opposed to a long-term mean orbit.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Osculating {
    /// Julian Date the elements were computed for.
    pub epoch: f64,
    /// Semi-major axis, AU.
    pub a: f64,
    /// Eccentricity.
    pub e: f64,
    /// Inclination to the ecliptic, degrees.
    pub inclination: f64,
    /// Longitude of the ascending node, Ω, degrees.
    pub ascending_node: f64,
    /// Argument of perihelion, ω, degrees.
    pub argument_of_perihelion: f64,
    /// Mean anomaly at `epoch`, degrees.
    pub mean_anomaly: f64,
    /// Mean motion, degrees per day.
    pub mean_motion: f64,
}

impl Osculating {
    /// Elements propagated to `at`, in the form the rest of the code uses.
    ///
    /// Only the mean longitude advances: the orbit's shape and orientation are
    /// held at their `epoch` values, which is exactly the approximation whose
    /// error the module documentation quantifies.
    pub fn elements_at(&self, at: JulianDate) -> Kepler {
        let longitude_of_perihelion = self.argument_of_perihelion + self.ascending_node;
        let mean_anomaly = self.mean_anomaly + self.mean_motion * (at.0 - self.epoch);
        Kepler {
            a: self.a,
            e: self.e,
            i: self.inclination,
            mean_longitude: mean_anomaly + longitude_of_perihelion,
            longitude_of_perihelion,
            longitude_of_ascending_node: self.ascending_node,
        }
    }

    /// Heliocentric position at `at`, in AU, J2000 ecliptic frame.
    pub fn position(&self, at: JulianDate) -> DVec3 {
        self.elements_at(at).position()
    }

    /// How far `at` is from this element set's epoch, in days.
    pub fn distance_from_epoch(&self, at: JulianDate) -> f64 {
        (at.0 - self.epoch).abs()
    }
}

/// Element sets for every tracked body, from one annual look-up.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Almanac {
    /// Julian Date the almanac was retrieved, used to decide when it is stale.
    pub retrieved: f64,
    /// Keyed by planet name so the file stays readable and stable.
    pub bodies: BTreeMap<String, Vec<Osculating>>,
}

impl Almanac {
    pub fn is_empty(&self) -> bool {
        self.bodies.values().all(|sets| sets.is_empty())
    }

    /// Element sets for `planet`, if the almanac carries any.
    pub fn sets_for(&self, planet: Planet) -> Option<&[Osculating]> {
        self.bodies
            .get(planet.name())
            .map(|v| v.as_slice())
            .filter(|v| !v.is_empty())
    }

    /// The element set whose epoch is closest to `at`.
    ///
    /// Choosing the nearest epoch rather than the first is what keeps the
    /// propagation interval down to weeks; propagating a whole year from a
    /// single anchor is roughly a hundred times worse.
    pub fn nearest(&self, planet: Planet, at: JulianDate) -> Option<&Osculating> {
        self.sets_for(planet)?.iter().min_by(|left, right| {
            left.distance_from_epoch(at)
                .total_cmp(&right.distance_from_epoch(at))
        })
    }

    /// Position of `planet` at `at`, or `None` if this almanac cannot answer.
    pub fn position(&self, planet: Planet, at: JulianDate) -> Option<DVec3> {
        let set = self.nearest(planet, at)?;
        (set.distance_from_epoch(at) <= EPOCH_SPACING_DAYS / 2.0 + COVERAGE_MARGIN_DAYS)
            .then(|| set.position(at))
    }

    /// Does this almanac cover `at` for every body it claims to know?
    pub fn covers(&self, at: JulianDate) -> bool {
        !self.is_empty()
            && Planet::ALL.iter().all(|planet| {
                self.sets_for(*planet).is_none_or(|_| {
                    self.nearest(*planet, at).is_some_and(|set| {
                        set.distance_from_epoch(at)
                            <= EPOCH_SPACING_DAYS / 2.0 + COVERAGE_MARGIN_DAYS
                    })
                })
            })
    }

    /// Age in days at `now`. Negative if the clock has moved backwards.
    pub fn age_days(&self, now: JulianDate) -> f64 {
        now.0 - self.retrieved
    }

    pub fn to_toml(&self) -> Result<String, AlmanacError> {
        toml::to_string_pretty(self).map_err(|e| AlmanacError::Serialize(e.to_string()))
    }

    pub fn from_toml(text: &str) -> Result<Self, AlmanacError> {
        let almanac: Almanac =
            toml::from_str(text).map_err(|e| AlmanacError::Parse(e.to_string()))?;
        almanac.validate()?;
        Ok(almanac)
    }

    /// Reject an almanac that would render nonsense, so a truncated download or
    /// a hand-edited file is reported rather than drawn.
    pub fn validate(&self) -> Result<(), AlmanacError> {
        for (name, sets) in &self.bodies {
            if Planet::from_name(name).is_none() {
                return Err(AlmanacError::UnknownBody(name.clone()));
            }
            for set in sets {
                let sane = set.epoch.is_finite()
                    && set.a.is_finite()
                    && set.a > 0.0
                    && set.e.is_finite()
                    && (0.0..1.0).contains(&set.e)
                    && set.inclination.is_finite()
                    && set.mean_motion.is_finite()
                    && set.mean_motion > 0.0;
                if !sane {
                    return Err(AlmanacError::Implausible(name.clone(), set.epoch));
                }
            }
        }
        Ok(())
    }
}

/// Parse the `$$SOE`/`$$EOE` block of a Horizons `EPHEM_TYPE='ELEMENTS'`
/// response into element sets.
///
/// The response is fixed-format text, one record per epoch:
///
/// ```text
/// 2461041.500000000 = A.D. 2026-Jan-01 00:00:00.0000 TDB
///  EC= 5.5e-02 QR= 9.01e+00 IN= 2.48e+00
///  OM= 1.13e+02 W = 3.38e+02 Tp=  2463568.62
///  N = 3.34e-02 MA= 2.82e+02 TA= 2.76e+02
///  A = 9.53e+00 AD= 1.00e+01 PR= 1.07e+04
/// ```
/// The element fields `flush` consumes. Everything else in a Horizons record
/// is surplus and may safely fail to parse.
const REQUIRED_FIELDS: [&str; 7] = ["A", "EC", "IN", "OM", "W", "MA", "N"];

pub fn parse_horizons_elements(text: &str) -> Result<Vec<Osculating>, AlmanacError> {
    let body = text
        .split_once("$$SOE")
        .and_then(|(_, rest)| rest.split_once("$$EOE"))
        .map(|(block, _)| block)
        .ok_or(AlmanacError::NoDataBlock)?;

    let mut sets = Vec::new();
    let mut epoch: Option<f64> = None;
    let mut fields: BTreeMap<&str, f64> = BTreeMap::new();

    // Records are delimited by the epoch line, so a record is complete when the
    // next epoch line arrives or the block ends.
    let flush = |epoch: &mut Option<f64>,
                 fields: &mut BTreeMap<&str, f64>,
                 sets: &mut Vec<Osculating>|
     -> Result<(), AlmanacError> {
        let Some(epoch) = epoch.take() else {
            return Ok(());
        };
        let get = |key: &str| {
            fields
                .get(key)
                .copied()
                .ok_or(AlmanacError::MissingField(key.to_owned()))
        };
        sets.push(Osculating {
            epoch,
            a: get("A")?,
            e: get("EC")?,
            inclination: get("IN")?,
            ascending_node: get("OM")?,
            argument_of_perihelion: get("W")?,
            mean_anomaly: get("MA")?,
            mean_motion: get("N")?,
        });
        fields.clear();
        Ok(())
    };

    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // An epoch line looks like "2461041.5 = A.D. ...".
        if let Some((value, _)) = trimmed.split_once('=')
            && let Ok(parsed) = value.trim().parse::<f64>()
            && trimmed.contains("A.D.")
        {
            flush(&mut epoch, &mut fields, &mut sets)?;
            epoch = Some(parsed);
            continue;
        }

        // Otherwise it is up to three "KEY= value" pairs.
        for pair in split_key_value_pairs(trimmed) {
            let Some((key, value)) = pair else { continue };
            match value.parse::<f64>() {
                Ok(number) => {
                    fields.insert(key, number);
                }
                // A required field we cannot read must be an error naming the
                // real problem here; dropping it would surface later as a
                // misleading `MissingField`. Unknown keys stay ignored —
                // Horizons emits plenty we never consume.
                Err(_) if REQUIRED_FIELDS.contains(&key) => {
                    return Err(AlmanacError::UnparseableValue {
                        key: key.to_owned(),
                        value: value.to_owned(),
                    });
                }
                Err(_) => {}
            }
        }
    }
    flush(&mut epoch, &mut fields, &mut sets)?;

    if sets.is_empty() {
        return Err(AlmanacError::NoRecords);
    }
    Ok(sets)
}

/// Split a Horizons data line into its `KEY= value` pairs.
///
/// Keys are one or two characters and may be padded (`W =`, `N =`), and values
/// are in Fortran-ish exponential notation, so this cannot be a simple split on
/// whitespace.
fn split_key_value_pairs(line: &str) -> impl Iterator<Item = Option<(&str, &str)>> {
    line.match_indices('=').map(move |(equals, _)| {
        let key = line[..equals].trim_end().rsplit(' ').next()?.trim();
        let rest = line[equals + 1..].trim_start();
        let value = rest.split_whitespace().next()?;
        (!key.is_empty()).then_some((key, value))
    })
}

#[derive(Debug, thiserror::Error)]
pub enum AlmanacError {
    #[error("the response has no $$SOE/$$EOE data block")]
    NoDataBlock,
    #[error("the response contained no element records")]
    NoRecords,
    #[error("element record is missing field {0}")]
    MissingField(String),
    #[error("element field {key} has unparseable value {value:?}")]
    UnparseableValue { key: String, value: String },
    #[error("unknown body {0:?} in almanac")]
    UnknownBody(String),
    #[error("implausible elements for {0} at epoch {1}")]
    Implausible(String, f64),
    #[error("could not parse almanac: {0}")]
    Parse(String),
    #[error("could not serialise almanac: {0}")]
    Serialize(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real Horizons response for Saturn at two consecutive monthly epochs,
    /// captured verbatim. It has to be real data: the continuity test below is
    /// only meaningful if the two element sets genuinely describe the same
    /// orbit at different times.
    const SATURN_RESPONSE: &str = "\
API VERSION: 1.2
*******************************************************************************
$$SOE
2461041.500000000 = A.D. 2026-Jan-01 00:00:00.0000 TDB 
 EC= 5.535357040578952E-02 QR= 9.017558491623657E+00 IN= 2.487910232325189E+00
 OM= 1.136324066054327E+02 W = 3.381382522589528E+02 Tp=  2463550.550155367237
 N = 3.342232234177665E-02 MA= 2.761417169356263E+02 TA= 2.698010300076631E+02
 A = 9.545961546159983E+00 AD= 1.007436460069631E+01 PR= 1.077124432942272E+04
2461071.937500000 = A.D. 2026-Jan-31 10:30:00.0000 TDB 
 EC= 5.534445298568243E-02 QR= 9.016800557151956E+00 IN= 2.487908518326133E+00
 OM= 1.136323053055297E+02 W = 3.382342939415772E+02 Tp=  2463553.086352110375
 N = 3.342702048633953E-02 MA= 2.770625864908416E+02 TA= 2.707276679052988E+02
 A = 9.545067073019887E+00 AD= 1.007333358888782E+01 PR= 1.076973043849719E+04
$$EOE
*******************************************************************************
";

    #[test]
    fn parses_every_field_of_a_real_response() {
        let sets = parse_horizons_elements(SATURN_RESPONSE).unwrap();
        assert_eq!(sets.len(), 2);

        let first = sets[0];
        assert_eq!(first.epoch, 2_461_041.5);
        assert!((first.a - 9.545_961_546_159_983).abs() < 1e-12);
        assert!((first.e - 0.055_353_570_405_789_52).abs() < 1e-15);
        assert!((first.inclination - 2.487_910_232_325_189).abs() < 1e-12);
        assert!((first.ascending_node - 113.632_406_605_432_7).abs() < 1e-10);
        assert!((first.argument_of_perihelion - 338.138_252_258_952_8).abs() < 1e-10);
        assert!((first.mean_anomaly - 276.141_716_935_626_3).abs() < 1e-10);
        assert!((first.mean_motion - 0.033_422_322_341_776_65).abs() < 1e-15);

        assert_eq!(sets[1].epoch, 2_461_071.937_5);
    }

    /// `W =` and `N =` are padded, and `A =` shares a prefix with `AD=`. A
    /// naive whitespace split gets all three wrong.
    #[test]
    fn padded_and_prefixed_keys_are_not_confused() {
        let sets = parse_horizons_elements(SATURN_RESPONSE).unwrap();
        // AD (aphelion, ~10.06) must not have been read as A (~9.54).
        assert!(sets[0].a < 9.6, "A was confused with AD: {}", sets[0].a);
        // W (~338.7) must not have been read as something else.
        assert!(sets[0].argument_of_perihelion > 300.0);
        // N (~0.033) must not have been read from a longer key.
        assert!(sets[0].mean_motion < 0.1);
    }

    #[test]
    fn rejects_responses_with_no_data() {
        assert!(matches!(
            parse_horizons_elements("no markers here"),
            Err(AlmanacError::NoDataBlock)
        ));
        assert!(matches!(
            parse_horizons_elements("$$SOE\n$$EOE"),
            Err(AlmanacError::NoRecords)
        ));
    }

    #[test]
    fn rejects_a_truncated_record() {
        let truncated = "$$SOE\n2461041.5 = A.D. 2026-Jan-01 00:00:00.0000 TDB\n EC= 0.05\n$$EOE";
        assert!(matches!(
            parse_horizons_elements(truncated),
            Err(AlmanacError::MissingField(_))
        ));
    }

    /// A required field whose value cannot be read must fail naming that
    /// field. If it were silently dropped — say Horizons switched to Fortran
    /// `D`-exponents — the error would be a baffling `MissingField` instead.
    #[test]
    fn names_the_field_when_a_required_value_is_unparseable() {
        let doctored =
            SATURN_RESPONSE.replace("EC= 5.535357040578952E-02", "EC= 5.535357040578952D-02");
        match parse_horizons_elements(&doctored) {
            Err(AlmanacError::UnparseableValue { key, value }) => {
                assert_eq!(key, "EC");
                assert!(
                    value.contains("D-02"),
                    "the offending token is quoted: {value:?}"
                );
            }
            other => panic!("expected UnparseableValue for EC, got {other:?}"),
        }
        // Surplus fields Horizons emits are still free to be strange.
        let surplus = SATURN_RESPONSE.replace("Tp=  2463550.550155367237", "Tp=  not-a-number");
        assert!(parse_horizons_elements(&surplus).is_ok());
    }

    fn saturn_almanac() -> Almanac {
        let sets = parse_horizons_elements(SATURN_RESPONSE).unwrap();
        Almanac {
            retrieved: 2_461_041.5,
            bodies: BTreeMap::from([("Saturn".to_owned(), sets)]),
        }
    }

    #[test]
    fn nearest_picks_the_closest_epoch() {
        let almanac = saturn_almanac();
        // Just after the first epoch.
        let near_first = almanac
            .nearest(Planet::Saturn, JulianDate(2_461_045.0))
            .unwrap();
        assert_eq!(near_first.epoch, 2_461_041.5);
        // Just before the second.
        let near_second = almanac
            .nearest(Planet::Saturn, JulianDate(2_461_070.0))
            .unwrap();
        assert_eq!(near_second.epoch, 2_461_071.937_5);
    }

    #[test]
    fn propagation_is_continuous_across_the_epoch_handover() {
        // Switching from one element set to the next must not make the planet
        // jump, or the wallpaper would visibly twitch once a month.
        let almanac = saturn_almanac();
        let midpoint = (2_461_041.5 + 2_461_071.937_5) / 2.0;
        let before = almanac
            .position(Planet::Saturn, JulianDate(midpoint - 1e-4))
            .unwrap();
        let after = almanac
            .position(Planet::Saturn, JulianDate(midpoint + 1e-4))
            .unwrap();
        let separation = (before.normalize().dot(after.normalize()).clamp(-1.0, 1.0))
            .acos()
            .to_degrees();
        assert!(separation < 1e-3, "jumped {separation}° at the handover");
    }

    #[test]
    fn at_its_own_epoch_propagation_is_the_identity() {
        let sets = parse_horizons_elements(SATURN_RESPONSE).unwrap();
        let elements = sets[0].elements_at(JulianDate(sets[0].epoch));
        // Horizons reports mean anomaly in 0..360; Kepler wraps it to +/-180,
        // so the comparison has to be modular.
        let difference = (elements.mean_anomaly() - sets[0].mean_anomaly).rem_euclid(360.0);
        assert!(
            difference < 1e-9 || (360.0 - difference) < 1e-9,
            "mean anomaly {} vs {}",
            elements.mean_anomaly(),
            sets[0].mean_anomaly
        );
        assert!((elements.a - sets[0].a).abs() < 1e-12);
    }

    #[test]
    fn coverage_is_bounded() {
        let almanac = saturn_almanac();
        assert!(
            almanac
                .position(Planet::Saturn, JulianDate(2_461_050.0))
                .is_some()
        );
        // Far outside the epoch range, the almanac must decline rather than
        // extrapolate, so the built-in tables can take over.
        assert!(
            almanac
                .position(Planet::Saturn, JulianDate(2_462_000.0))
                .is_none()
        );
        assert!(
            almanac
                .position(Planet::Saturn, JulianDate(2_460_000.0))
                .is_none()
        );
        // A body it knows nothing about is always declined.
        assert!(
            almanac
                .position(Planet::Mars, JulianDate(2_461_050.0))
                .is_none()
        );
    }

    #[test]
    fn round_trips_through_toml() {
        let almanac = saturn_almanac();
        let text = almanac.to_toml().unwrap();
        assert_eq!(Almanac::from_toml(&text).unwrap(), almanac);
    }

    #[test]
    fn validation_rejects_corrupt_data() {
        let mut almanac = saturn_almanac();
        almanac.bodies.get_mut("Saturn").unwrap()[0].e = 1.5;
        assert!(matches!(
            almanac.validate(),
            Err(AlmanacError::Implausible(_, _))
        ));

        let mut unknown = saturn_almanac();
        unknown.bodies.insert("Vulcan".to_owned(), Vec::new());
        assert!(matches!(
            unknown.validate(),
            Err(AlmanacError::UnknownBody(_))
        ));
    }

    #[test]
    fn age_is_measured_from_retrieval() {
        let almanac = saturn_almanac();
        assert!((almanac.age_days(JulianDate(2_461_041.5 + 400.0)) - 400.0).abs() < 1e-9);
    }
}
