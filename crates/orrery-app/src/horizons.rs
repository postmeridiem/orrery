//! Fetching osculating elements from JPL Horizons, and caching them on disk.
//!
//! One request per body returns every epoch we ask for, so a whole year of
//! coverage costs nine requests, once a year.
//!
//! The request carries a body number and a list of dates and nothing else. It
//! always runs off the render thread, and every failure path falls back to the
//! built-in tables, so the orrery is fully functional with the network disabled
//! or unavailable — just slightly less precise.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use orrery_core::almanac::{Almanac, EPOCH_SPACING_DAYS, Osculating, parse_horizons_elements};
use orrery_core::ephemeris::Planet;
use orrery_core::time::JulianDate;

const API: &str = "https://ssd.jpl.nasa.gov/api/horizons.api";

/// How far back the epochs start, relative to the moment of the fetch. A little
/// history means a cached almanac stays usable if the clock steps backwards.
const EPOCHS_BEFORE_DAYS: f64 = 31.0;

/// How far ahead the epochs run. Slightly over a year so an annual refresh
/// never leaves a gap.
const EPOCHS_AFTER_DAYS: f64 = 400.0;

/// Horizons asks for no more than about one request per second.
const REQUEST_SPACING: Duration = Duration::from_millis(1200);

const TIMEOUT: Duration = Duration::from_secs(45);

/// Horizons body identifier. These are the planet–satellite *barycentres*,
/// which is what heliocentric planetary elements are properly expressed for.
fn horizons_id(planet: Planet) -> &'static str {
    match planet {
        Planet::Mercury => "1",
        Planet::Venus => "2",
        Planet::Earth => "3",
        Planet::Mars => "4",
        Planet::Jupiter => "5",
        Planet::Saturn => "6",
        Planet::Uranus => "7",
        Planet::Neptune => "8",
        Planet::Pluto => "9",
    }
}

/// Where the cached almanac lives.
pub fn cache_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(|home| PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(base.join("orrery").join("almanac.toml"))
}

/// Load a cached almanac, if there is a valid one.
pub fn load_cached(path: &std::path::Path) -> Option<Almanac> {
    let text = std::fs::read_to_string(path).ok()?;
    match Almanac::from_toml(&text) {
        Ok(almanac) => {
            log::info!("loaded almanac from {}", path.display());
            Some(almanac)
        }
        Err(error) => {
            // A corrupt cache must not be fatal; the built-in tables still work.
            log::warn!("ignoring unusable almanac at {}: {error}", path.display());
            None
        }
    }
}

/// The temporary path `save` writes before renaming into place.
///
/// The name carries the process id because Plasma runs one orrery per output:
/// on a shared stale cache they all fetch at once, and two processes writing
/// the same temp file interleave — the loser's rename then publishes torn
/// content. Distinct names keep every write private until its atomic rename.
fn temp_path(path: &std::path::Path) -> PathBuf {
    path.with_extension(format!("toml.tmp.{}", std::process::id()))
}

pub fn save(almanac: &Almanac, path: &std::path::Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    // Write then rename, so a crash mid-write cannot leave a half-file that
    // the next run would have to reject.
    let temporary = temp_path(path);
    std::fs::write(&temporary, almanac.to_toml()?)
        .with_context(|| format!("writing {}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("renaming into {}", path.display()));
    }
    Ok(())
}

/// Should we go and fetch a fresh almanac?
pub fn needs_refresh(almanac: Option<&Almanac>, now: JulianDate, refresh_days: f64) -> bool {
    match almanac {
        None => true,
        Some(almanac) => {
            // Refresh when stale, and also when it no longer covers now --
            // which catches a clock that has jumped far forward.
            almanac.age_days(now).abs() >= refresh_days || !almanac.covers(now)
        }
    }
}

/// The epochs to request, monthly around `now`.
fn epochs(now: JulianDate) -> Vec<f64> {
    let start = now.0 - EPOCHS_BEFORE_DAYS;
    let count = ((EPOCHS_BEFORE_DAYS + EPOCHS_AFTER_DAYS) / EPOCH_SPACING_DAYS).ceil() as usize;
    (0..=count)
        .map(|k| start + k as f64 * EPOCH_SPACING_DAYS)
        .collect()
}

/// Fetch elements for every planet. Blocking; call this off the render thread.
pub fn fetch(now: JulianDate) -> Result<Almanac> {
    let epochs = epochs(now);
    let time_list = epochs
        .iter()
        .map(|jd| format!("{jd:.6}"))
        .collect::<Vec<_>>()
        .join(" ");

    let mut bodies = BTreeMap::new();
    for (index, planet) in Planet::ALL.into_iter().enumerate() {
        if index > 0 {
            std::thread::sleep(REQUEST_SPACING);
        }
        let sets = fetch_one(planet, &time_list)
            .with_context(|| format!("fetching elements for {}", planet.name()))?;
        log::debug!("{}: {} element sets", planet.name(), sets.len());
        bodies.insert(planet.name().to_owned(), sets);
    }

    let almanac = Almanac {
        retrieved: now.0,
        bodies,
    };
    almanac.validate().context("Horizons returned implausible elements")?;
    Ok(almanac)
}

fn fetch_one(planet: Planet, time_list: &str) -> Result<Vec<Osculating>> {
    let response = ureq::get(API)
        .config()
        .timeout_global(Some(TIMEOUT))
        .build()
        .query_pairs([
            ("format", "text"),
            ("COMMAND", &format!("'{}'", horizons_id(planet))),
            ("OBJ_DATA", "'NO'"),
            ("MAKE_EPHEM", "'YES'"),
            ("EPHEM_TYPE", "'ELEMENTS'"),
            // Heliocentric, so the Sun sits at the origin as the renderer assumes.
            ("CENTER", "'500@10'"),
            ("REF_PLANE", "'ECLIPTIC'"),
            ("REF_SYSTEM", "'J2000'"),
            ("OUT_UNITS", "'AU-D'"),
            ("TLIST", time_list),
        ])
        .call()?
        .body_mut()
        .read_to_string()?;

    if response.contains("$$SOE") {
        Ok(parse_horizons_elements(&response)?)
    } else {
        // Horizons reports errors as prose in a 200 response.
        let detail = response.lines().take(6).collect::<Vec<_>>().join(" | ");
        bail!("Horizons returned no ephemeris: {detail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epochs_span_more_than_a_year_and_are_monthly() {
        let now = JulianDate(2_461_255.5);
        let epochs = epochs(now);
        assert!(epochs.first().unwrap() < &now.0, "must start before now");
        assert!(
            epochs.last().unwrap() - now.0 >= 365.0,
            "must cover a full year ahead"
        );
        for pair in epochs.windows(2) {
            assert!((pair[1] - pair[0] - EPOCH_SPACING_DAYS).abs() < 1e-9);
        }
    }

    #[test]
    fn refresh_is_due_when_missing_stale_or_uncovered() {
        let now = JulianDate(2_461_255.5);
        assert!(needs_refresh(None, now, 365.0), "no almanac at all");

        let fresh = Almanac {
            retrieved: now.0 - 10.0,
            bodies: BTreeMap::from([(
                "Earth".to_owned(),
                vec![Osculating {
                    epoch: now.0,
                    a: 1.0,
                    e: 0.017,
                    inclination: 0.0,
                    ascending_node: 0.0,
                    argument_of_perihelion: 102.9,
                    mean_anomaly: 0.0,
                    mean_motion: 0.9856,
                }],
            )]),
        };
        assert!(!needs_refresh(Some(&fresh), now, 365.0), "recently fetched");

        let stale = Almanac {
            retrieved: now.0 - 400.0,
            ..fresh.clone()
        };
        assert!(needs_refresh(Some(&stale), now, 365.0), "older than the interval");

        // Not stale by age, but its epochs no longer bracket the moment.
        assert!(
            needs_refresh(Some(&fresh), JulianDate(now.0 + 300.0), 3650.0),
            "no longer covers now"
        );
    }

    #[test]
    fn temp_path_is_distinct_per_process_and_never_the_target() {
        let target = std::path::Path::new("/tmp/orrery-test/almanac.toml");
        let temporary = temp_path(target);
        assert_ne!(temporary, target);
        assert_eq!(temporary.parent(), target.parent(), "rename must stay on one filesystem");
        let name = temporary.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            name.ends_with(&format!(".{}", std::process::id())),
            "{name} should end with this process id, so concurrent per-output \
             processes never write the same temp file"
        );
    }

    #[test]
    fn save_leaves_no_temp_file_behind() {
        let directory = std::env::temp_dir().join(format!("orrery-save-{}", std::process::id()));
        let path = directory.join("almanac.toml");
        let almanac = Almanac {
            retrieved: 2_461_255.5,
            bodies: BTreeMap::from([(
                "Earth".to_owned(),
                vec![Osculating {
                    epoch: 2_461_255.5,
                    a: 1.0,
                    e: 0.017,
                    inclination: 0.0,
                    ascending_node: 0.0,
                    argument_of_perihelion: 102.9,
                    mean_anomaly: 0.0,
                    mean_motion: 0.9856,
                }],
            )]),
        };
        save(&almanac, &path).expect("save should succeed");
        assert!(path.is_file(), "the almanac must exist at the target path");
        let leftovers: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path() != path)
            .collect();
        assert!(leftovers.is_empty(), "no temp files may remain: {leftovers:?}");
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[test]
    fn every_planet_has_a_horizons_id() {
        for planet in Planet::ALL {
            let id = horizons_id(planet);
            assert!(
                id.parse::<u32>().is_ok_and(|n| (1..=9).contains(&n)),
                "{} has id {id:?}",
                planet.name()
            );
        }
    }

    /// Hits the network. Run explicitly with:
    /// `cargo test -p orrery-app -- --ignored live_horizons`
    #[test]
    #[ignore = "requires network access to ssd.jpl.nasa.gov"]
    fn live_horizons_fetch_is_usable() {
        let now = JulianDate::now();
        let almanac = fetch(now).expect("fetch should succeed");
        for planet in Planet::ALL {
            let sets = almanac.sets_for(planet).expect("every planet present");
            assert!(sets.len() >= 13, "{}: only {} epochs", planet.name(), sets.len());
            assert!(
                almanac.position(planet, now).is_some(),
                "{} does not cover now",
                planet.name()
            );
        }
        assert!(almanac.covers(now));
    }
}
