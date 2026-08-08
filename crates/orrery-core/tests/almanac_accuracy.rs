//! What the annual Horizons look-up actually buys, measured against truth.
//!
//! The bundled `data/almanac.toml` is real data fetched from JPL Horizons, and
//! the reference vectors below are Horizons state vectors at four dates spread
//! across that almanac's coverage. Neither is synthetic, so this test measures
//! the real end-to-end accuracy of the shipped ephemeris rather than the
//! self-consistency of the maths.

use glam::DVec3;
use orrery_core::almanac::Almanac;
use orrery_core::ephemeris::{self, Planet};
use orrery_core::lookup::{Lookup, Source};
use orrery_core::time::JulianDate;

/// Heliocentric ecliptic-J2000 position vectors (AU) of the planetary
/// barycentres, from the JPL Horizons API.
#[rustfmt::skip]
const REFERENCE: &[(Planet, f64, DVec3)] = &[
    (Planet::Mercury, 2461255.5, DVec3::new(0.336033850, 0.064131619, -0.025578796)),
    (Planet::Mercury, 2461390.5, DVec3::new(-0.267611805, -0.370470297, -0.005732107)),
    (Planet::Mercury, 2461525.5, DVec3::new(0.225270066, 0.221855156, -0.002529225)),
    (Planet::Mercury, 2461660.5, DVec3::new(-0.150394707, -0.440495528, -0.022206784)),
    (Planet::Venus, 2461255.5, DVec3::new(-0.125233365, -0.715485138, -0.002604109)),
    (Planet::Venus, 2461390.5, DVec3::new(-0.309058383, 0.648244871, 0.026741320)),
    (Planet::Venus, 2461525.5, DVec3::new(0.640133862, -0.343962825, -0.041662537)),
    (Planet::Venus, 2461660.5, DVec3::new(-0.710724644, -0.112731164, 0.039458753)),
    (Planet::Earth, 2461255.5, DVec3::new(0.656786275, -0.773514385, 0.000042534)),
    (Planet::Earth, 2461390.5, DVec3::new(0.108827299, 0.978199251, -0.000061796)),
    (Planet::Earth, 2461525.5, DVec3::new(-0.781950439, -0.634685449, 0.000044451)),
    (Planet::Earth, 2461660.5, DVec3::new(0.986934887, -0.198264488, 0.000006944)),
    (Planet::Mars, 2461255.5, DVec3::new(0.862949615, 1.205119055, 0.004095019)),
    (Planet::Mars, 2461390.5, DVec3::new(-0.849321158, 1.397182017, 0.050104777)),
    (Planet::Mars, 2461525.5, DVec3::new(-1.650017183, -0.023341429, 0.039964340)),
    (Planet::Mars, 2461660.5, DVec3::new(-0.624902420, -1.376638899, -0.013533736)),
    (Planet::Jupiter, 2461255.5, DVec3::new(-3.137952193, 4.255260176, 0.052530982)),
    (Planet::Jupiter, 2461390.5, DVec3::new(-3.908769885, 3.624229072, 0.072398629)),
    (Planet::Jupiter, 2461525.5, DVec3::new(-4.540785008, 2.864287371, 0.089696190)),
    (Planet::Jupiter, 2461660.5, DVec3::new(-5.014936463, 2.004632657, 0.103876640)),
    (Planet::Saturn, 2461255.5, DVec3::new(9.333423601, 1.443189064, -0.396660376)),
    (Planet::Saturn, 2461390.5, DVec3::new(9.146769989, 2.180141809, -0.402066662)),
    (Planet::Saturn, 2461525.5, DVec3::new(8.901068005, 2.902848868, -0.404874151)),
    (Planet::Saturn, 2461660.5, DVec3::new(8.597180903, 3.606442770, -0.405030703)),
    (Planet::Uranus, 2461255.5, DVec3::new(9.138296187, 17.171671629, -0.054715651)),
    (Planet::Uranus, 2461390.5, DVec3::new(8.661555951, 17.389706166, -0.047721950)),
    (Planet::Uranus, 2461525.5, DVec3::new(8.178553345, 17.594789419, -0.040695182)),
    (Planet::Uranus, 2461660.5, DVec3::new(7.689640973, 17.786773060, -0.033640329)),
    (Planet::Neptune, 2461255.5, DVec3::new(29.847098903, 1.194153300, -0.712404952)),
    (Planet::Neptune, 2461390.5, DVec3::new(29.823610017, 1.619627580, -0.720634188)),
    (Planet::Neptune, 2461525.5, DVec3::new(29.794198627, 2.044630986, -0.728719479)),
    (Planet::Neptune, 2461660.5, DVec3::new(29.758888166, 2.469129111, -0.736659046)),
    (Planet::Pluto, 2461255.5, DVec3::new(19.805887488, -29.432195395, -2.578688709)),
    (Planet::Pluto, 2461390.5, DVec3::new(20.167536135, -29.288906750, -2.698886781)),
    (Planet::Pluto, 2461525.5, DVec3::new(20.526901857, -29.142275451, -2.818765613)),
    (Planet::Pluto, 2461660.5, DVec3::new(20.883980173, -28.992294522, -2.938313477)),
];

fn bundled_almanac() -> Almanac {
    let text = include_str!("../../../data/almanac.toml");
    Almanac::from_toml(text).expect("the bundled almanac must be valid")
}

fn separation_deg(a: DVec3, b: DVec3) -> f64 {
    a.normalize()
        .dot(b.normalize())
        .clamp(-1.0, 1.0)
        .acos()
        .to_degrees()
}

/// The bundled almanac must actually be in use at these dates -- otherwise the
/// accuracy assertions below would silently be measuring the built-in tables.
#[test]
fn the_almanac_is_the_active_source_across_its_window() {
    let lookup = Lookup::with_almanac(bundled_almanac());
    for &(planet, jd, _) in REFERENCE {
        assert_eq!(
            lookup.source(planet, JulianDate(jd)),
            Source::Almanac,
            "{} at JD {jd} fell back to the built-in tables",
            planet.name()
        );
    }
}

/// The headline claim: propagating from the nearest monthly osculating element
/// set is accurate to well under an arcsecond.
#[test]
fn almanac_positions_are_sub_arcsecond() {
    // 0.0005 deg is 1.8 arcseconds -- comfortably above the ~0.2 arcsec
    // actually observed, but far below anything the built-in tables achieve.
    const TOLERANCE_DEG: f64 = 0.0005;

    let lookup = Lookup::with_almanac(bundled_almanac());
    let mut worst = 0.0_f64;
    for &(planet, jd, truth) in REFERENCE {
        let error = separation_deg(lookup.position(planet, JulianDate(jd)), truth);
        assert!(
            error < TOLERANCE_DEG,
            "{} at JD {jd}: {error:.6} deg",
            planet.name()
        );
        worst = worst.max(error);
    }
    assert!(worst > 0.0, "the comparison must be doing something");
    println!(
        "worst almanac error: {worst:.7} deg = {:.3} arcsec",
        worst * 3600.0
    );
}

/// And it must beat the built-in tables, which is the entire reason the
/// look-up exists. Saturn is the case that matters: the Jupiter-Saturn great
/// inequality is what the linear element rates cannot represent.
#[test]
fn the_almanac_beats_the_built_in_tables() {
    let lookup = Lookup::with_almanac(bundled_almanac());

    let mut worst_almanac = 0.0_f64;
    let mut worst_builtin = 0.0_f64;
    let mut saturn_almanac = 0.0_f64;
    let mut saturn_builtin = 0.0_f64;

    for &(planet, jd, truth) in REFERENCE {
        let at = JulianDate(jd);
        let almanac_error = separation_deg(lookup.position(planet, at), truth);
        let builtin_error = separation_deg(ephemeris::heliocentric_position(planet, at), truth);
        worst_almanac = worst_almanac.max(almanac_error);
        worst_builtin = worst_builtin.max(builtin_error);
        if planet == Planet::Saturn {
            saturn_almanac = saturn_almanac.max(almanac_error);
            saturn_builtin = saturn_builtin.max(builtin_error);
        }
    }

    assert!(
        worst_almanac * 20.0 < worst_builtin,
        "expected a large improvement, got {worst_almanac:.6} vs {worst_builtin:.6} deg"
    );
    assert!(
        saturn_almanac < saturn_builtin / 20.0,
        "Saturn: almanac {saturn_almanac:.6} vs built-in {saturn_builtin:.6} deg"
    );
    println!("worst overall: almanac {worst_almanac:.7} deg, built-in {worst_builtin:.7} deg");
    println!("Saturn:        almanac {saturn_almanac:.7} deg, built-in {saturn_builtin:.7} deg");
}

/// Outside the almanac's epochs the built-in tables must take over rather than
/// the elements being extrapolated indefinitely.
#[test]
fn coverage_ends_gracefully() {
    let lookup = Lookup::with_almanac(bundled_almanac());
    let far_future = JulianDate(2_469_000.0);
    assert_eq!(lookup.source(Planet::Saturn, far_future), Source::BuiltIn);
    assert_eq!(
        lookup.position(Planet::Saturn, far_future),
        ephemeris::heliocentric_position(Planet::Saturn, far_future),
    );
}
