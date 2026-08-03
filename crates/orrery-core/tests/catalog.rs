//! Checks against the real embedded catalogue.
//!
//! These assert astronomical facts, not just that the parser runs -- a
//! transposed coordinate or a mismatched frame would still parse cleanly.

use orrery_core::sky::{Catalog, DeepSkyKind};

fn catalog() -> Catalog {
    Catalog::embedded().expect("the embedded catalogue must load")
}

fn separation_deg(a: glam::Vec3, b: glam::Vec3) -> f32 {
    a.dot(b).clamp(-1.0, 1.0).acos().to_degrees()
}

#[test]
fn the_catalogue_is_the_naked_eye_sky() {
    let catalog = catalog();
    assert_eq!(catalog.stars.len(), 8404);
    assert!(catalog.stars.iter().all(|s| s.magnitude <= 6.5));
    assert!(
        catalog
            .stars
            .iter()
            .all(|s| (s.direction.length() - 1.0).abs() < 1e-4),
        "every direction must be a unit vector"
    );

    // HR 2491 is Sirius, and nothing in the sky is brighter.
    let brightest = catalog
        .stars
        .iter()
        .min_by(|a, b| a.magnitude.total_cmp(&b.magnitude))
        .unwrap();
    assert_eq!(brightest.hr, 2491);
    assert!((brightest.magnitude + 1.46).abs() < 0.01);
}

#[test]
fn constellation_figures_are_well_formed() {
    let catalog = catalog();
    assert_eq!(catalog.constellations.len(), 20);

    let mut segments = 0;
    for figure in &catalog.constellations {
        assert!(!figure.segments.is_empty(), "{} is empty", figure.name);
        for (a, b) in &figure.segments {
            let span = separation_deg(*a, *b);
            // No real constellation figure joins stars a third of the sky apart;
            // a wrong star reference would show up here.
            assert!(
                span > 0.0 && span < 35.0,
                "{}: a {span}° segment is not a constellation line",
                figure.abbreviation
            );
            segments += 1;
        }
    }
    assert_eq!(segments, 122);
}

#[test]
fn deep_sky_objects_are_well_formed() {
    let catalog = catalog();
    assert_eq!(catalog.deep_sky.len(), 23);
    for object in &catalog.deep_sky {
        assert!(
            (object.direction.length() - 1.0).abs() < 1e-4,
            "{}: not a unit direction",
            object.name
        );
        assert!(
            (0.0..=3.0).contains(&object.angular_radius_deg),
            "{}: implausible radius {}",
            object.name,
            object.angular_radius_deg
        );
        assert!((0.0..=1.0).contains(&object.prominence), "{}", object.name);
    }
    // The Andromeda Galaxy is the largest thing on the list after the LMC.
    let largest = catalog
        .deep_sky
        .iter()
        .max_by(|a, b| a.angular_radius_deg.total_cmp(&b.angular_radius_deg))
        .unwrap();
    assert_eq!(largest.name, "Large Magellanic Cloud");
}

/// The load-bearing cross-check: stars and deep-sky objects are parsed from
/// separate files but must land in the same frame. The Orion Nebula sits just
/// below Orion's belt, so it has to be within a few degrees of the constellation
/// figure. If either file's coordinate convention were wrong, this fails.
#[test]
fn the_orion_nebula_sits_inside_orion() {
    let catalog = catalog();
    let nebula = catalog
        .deep_sky
        .iter()
        .find(|o| o.name == "Orion Nebula")
        .expect("Orion Nebula present");
    assert_eq!(nebula.kind, DeepSkyKind::EmissionNebula);

    let orion = catalog
        .constellations
        .iter()
        .find(|c| c.abbreviation == "Ori")
        .expect("Orion present");

    let closest = orion
        .segments
        .iter()
        .flat_map(|(a, b)| [*a, *b])
        .map(|star| separation_deg(star, nebula.direction))
        .fold(f32::INFINITY, f32::min);

    assert!(
        closest < 6.0,
        "the Orion Nebula is {closest}° from the nearest star in Orion"
    );
}

/// Likewise for the Pleiades, which lie in Taurus.
#[test]
fn the_pleiades_sit_in_taurus() {
    let catalog = catalog();
    let cluster = catalog
        .deep_sky
        .iter()
        .find(|o| o.name == "Pleiades")
        .unwrap();
    let taurus = catalog
        .constellations
        .iter()
        .find(|c| c.abbreviation == "Tau")
        .unwrap();
    let closest = taurus
        .segments
        .iter()
        .flat_map(|(a, b)| [*a, *b])
        .map(|star| separation_deg(star, cluster.direction))
        .fold(f32::INFINITY, f32::min);
    assert!(closest < 15.0, "the Pleiades are {closest}° from Taurus");
}
