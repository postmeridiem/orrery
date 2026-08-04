//! User configuration, loaded from TOML.
//!
//! Every field has a default, so a partial config file — or none at all — is
//! valid and yields the shipped look. `deny_unknown_fields` means a typo is
//! reported instead of silently ignored, which matters when the app runs as a
//! wallpaper with no visible console.

use serde::{Deserialize, Serialize};

use crate::ephemeris::Planet;
use crate::scale::{BodyScale, RadialScale, ScaleError};
use crate::time::JulianDate;

/// The complete configuration.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub camera: Camera,
    pub time: Time,
    pub scale: Scale,
    pub sky: Sky,
    pub orbits: Orbits,
    pub render: Render,
    pub bodies: Bodies,
    pub ephemeris: Ephemeris,
    pub lighting: Lighting,
}

/// How a body's unlit side is drawn.
///
/// The physical answer is "black", but a wallpaper where half the planets are
/// invisible silhouettes reads badly. So the terminator is modelled as mainly a
/// *saturation* gradient rather than a brightness one: the night side keeps
/// most of its luminance and loses most of its colour, so you can still see the
/// planet while sunlight is still obviously what picks out its hue.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Lighting {
    /// Luminance of the fully unlit side, as a fraction of the lit side.
    /// `1.0` flattens the terminator away entirely; `0.0` is physically honest
    /// and mostly black.
    pub night_brightness: f32,
    /// How much of the body's colour survives on the unlit side. Low values
    /// drain it toward grey, which is what makes the terminator read as
    /// desaturation rather than as darkness.
    pub night_saturation: f32,
    /// Chroma of the fully lit side. Above `1.0` pushes past the body's raw
    /// albedo.
    ///
    /// This is what makes the effect work on the gas giants: their true
    /// colours are low-chroma creams, so draining saturation from them barely
    /// registers and they just go pale. Boosting the lit side instead gives the
    /// terminator something to actually contrast against.
    pub day_saturation: f32,
    /// How brightly the Sun's own surface emits, in HDR units.
    ///
    /// It is the only emissive body in the scene, so this also sets how far
    /// its bloom reaches. Turned down, the inner planets stop being washed out
    /// by the glare of something they orbit very close to.
    pub sun_intensity: f32,
}

impl Default for Lighting {
    fn default() -> Self {
        Self {
            night_brightness: 0.30,
            night_saturation: 0.15,
            day_saturation: 1.35,
            sun_intensity: 6.0,
        }
    }
}

/// Where positions come from.
///
/// The built-in tables are always available and need no network. Looking real
/// osculating elements up from JPL Horizons once a year and propagating from
/// the nearest of them takes the worst-case error from 0.11° to well under an
/// arcsecond -- see [`crate::almanac`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ephemeris {
    /// Fetch osculating elements from JPL Horizons.
    ///
    /// The request contains only a body number and a list of dates -- no
    /// identifying information. It happens on a background thread, at most
    /// once per `refresh_days`, and any failure falls back silently to the
    /// built-in tables, so the orrery renders identically offline.
    pub online: bool,
    /// How old the cached almanac may get before it is refreshed.
    pub refresh_days: f64,
}

impl Default for Ephemeris {
    fn default() -> Self {
        Self {
            online: true,
            refresh_days: 365.0,
        }
    }
}

/// Where the camera sits and how it is framed.
///
/// The orrery is heliocentric, so there is no meaningful "north" to align a
/// monitor against — these are framing controls, not geographic ones.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Camera {
    /// Angle above the ecliptic plane, degrees. 90° looks straight down on the
    /// solar system; 0° is edge-on. The default is the oblique three-quarter
    /// view that makes the orbits read as ellipses.
    pub elevation_deg: f32,
    /// Rotation around the ecliptic pole, degrees. Spins the whole system in
    /// the frame.
    pub azimuth_deg: f32,
    /// Roll about the view axis, degrees. Tilts the horizon.
    pub roll_deg: f32,
    /// Vertical field of view, degrees.
    pub fov_deg: f32,
    /// Multiplier on the framing distance. Below 1 crops in, above 1 pulls back.
    ///
    /// The framing fits the whole scene inside the frame, which leaves the
    /// Kuiper belt sitting exactly at the edges. `0.785` crops in by 1.274x so
    /// the belt runs off the sides instead.
    pub zoom: f32,
    /// Minutes for the camera to travel once around the Sun. Zero holds still.
    ///
    /// This is what makes the constellations legible: the visible patch of sky
    /// sits behind the Sun, so circling the Sun walks that patch through every
    /// ecliptic longitude. An hour shows the whole band; set it to 1.0 to
    /// evaluate a full circuit quickly.
    ///
    /// The orrery turns with it, since the camera really is orbiting.
    pub rotation_period_minutes: f64,
    /// Days for the camera to rise above the ecliptic and sink below it again.
    /// Zero holds the elevation fixed.
    ///
    /// A fixed elevation only ever sees one band of sky -- from above the plane
    /// that band is southern, so the northern constellations can never appear.
    /// Letting the viewpoint drift below the plane and back brings the rest of
    /// the sky into reach, slowly.
    pub elevation_cycle_days: f64,
    /// How far above and below `elevation_deg` that cycle travels, in degrees.
    pub elevation_cycle_deg: f32,
    /// Fraction of the frame the outermost drawn orbit should span.
    pub fill: f32,
    /// Fit the scene to the frame's *width* rather than to whichever axis binds
    /// first.
    ///
    /// On a wide monitor the solar system is much wider than it is tall, so
    /// fitting both axes leaves it floating in the middle with the width
    /// unused. Filling the width instead means the vertical extent has to fit
    /// too, which is what `elevation_deg` controls -- and on an ultrawide the
    /// elevation genuinely has to come down, so this lowers it automatically
    /// rather than letting the scene overflow.
    pub fit_width: bool,
    /// Offset of the system's centre within the frame, in fractions of the
    /// viewport. Useful for keeping the Sun clear of desktop icons.
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            elevation_deg: 16.0,
            azimuth_deg: 0.0,
            roll_deg: 0.0,
            fov_deg: 55.0,
            zoom: 0.578,
            rotation_period_minutes: 60.0,
            elevation_cycle_days: 0.0,
            elevation_cycle_deg: 40.0,
            fill: 0.94,
            fit_width: true,
            offset_x: 0.0,
            offset_y: 0.0,
        }
    }
}

/// Which instant to draw, and how fast time runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Time {
    /// `"live"` tracks the system clock. `"fixed"` freezes at [`Time::date`].
    pub mode: TimeMode,
    /// Starting date, `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS`, UTC. Ignored in
    /// live mode unless `speed` is non-default.
    pub date: Option<String>,
    /// Simulated days per real second.
    ///
    /// `0` is real time: the orrery then shows the true configuration of the
    /// solar system at this moment, continuously. Motion at that rate is
    /// imperceptible over a glance — the visible movement comes from the slow
    /// camera drift and from the planets' own rotation.
    ///
    /// Raise it to watch the system actually turn: `1` advances a day a second,
    /// so Mercury laps its orbit in about a minute and a half. Anything
    /// non-zero means what is on screen is the real solar system at some *other*
    /// time, which is why the default is 0.
    pub days_per_second: f64,
}

impl Default for Time {
    fn default() -> Self {
        Self {
            mode: TimeMode::Live,
            date: None,
            days_per_second: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeMode {
    Live,
    Fixed,
}

impl Time {
    /// The Julian Date this configuration starts from.
    pub fn start_epoch(&self) -> Result<JulianDate, ConfigError> {
        match (&self.date, self.mode) {
            (Some(text), _) => parse_date(text),
            (None, TimeMode::Live) => Ok(JulianDate::now()),
            (None, TimeMode::Fixed) => Ok(JulianDate::now()),
        }
    }
}

/// Parse `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS`, treated as UTC.
fn parse_date(text: &str) -> Result<JulianDate, ConfigError> {
    let invalid = || ConfigError::Date(text.to_owned());
    let (date_part, time_part) = match text.split_once(['T', ' ']) {
        Some((d, t)) => (d, Some(t.trim_end_matches('Z'))),
        None => (text, None),
    };

    let mut fields = date_part.split('-');
    let year: i32 = fields.next().ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
    let month: u32 = fields.next().ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
    let day: u32 = fields.next().ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
    if fields.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err(invalid());
    }

    let day_fraction = match time_part {
        None => 0.0,
        Some(t) => {
            let mut hms = t.split(':');
            let hours: f64 = hms.next().ok_or_else(invalid)?.parse().map_err(|_| invalid())?;
            let minutes: f64 = hms.next().unwrap_or("0").parse().map_err(|_| invalid())?;
            let seconds: f64 = hms.next().unwrap_or("0").parse().map_err(|_| invalid())?;
            if hours >= 24.0 || minutes >= 60.0 || seconds >= 60.0 {
                return Err(invalid());
            }
            (hours * 3600.0 + minutes * 60.0 + seconds) / 86_400.0
        }
    };

    Ok(JulianDate::from_gregorian_utc(
        year,
        month,
        day as f64 + day_fraction,
    ))
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scale {
    pub orbit: RadialScale,
    pub body: BodyScale,
}

/// The starfield and nebulae behind the orrery. All procedural — no assets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sky {
    /// Changes the entire generated sky. Any value is as valid as any other.
    pub seed: u32,
    /// Relative number of stars.
    pub star_density: f32,
    /// Overall star brightness.
    pub star_brightness: f32,
    /// Intensity of the galactic band.
    pub milky_way: f32,
    /// Intensity of the coloured nebulosity.
    pub nebula: f32,
    /// Lifts the darkest parts of the sky off pure black.
    pub ambient: f32,
    /// Degrees to rotate the generated sky about the ecliptic pole.
    pub rotation_deg: f32,

    /// Draw the real catalogued stars at their true positions.
    ///
    /// With this off the sky is entirely procedural: still pretty, but the
    /// constellations are not there to be found.
    pub real_stars: bool,
    /// Faintest catalogued star to draw. The catalogue runs to 6.5, which is
    /// roughly the naked-eye limit under a dark sky.
    pub magnitude_limit: f32,
    /// Draw constellation figures.
    pub constellations: bool,
    /// How strongly to draw them. Deliberately very low by default: the lines
    /// are meant to be found by someone looking for them, not to be a diagram.
    pub constellation_opacity: f32,
    /// Fraction of a figure's stars that must lie behind the Sun before it is
    /// drawn. The viewpoint looks down at the Sun from outside, so the far side
    /// of it is the middle of the picture.
    pub constellation_min_behind_sun: f32,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            seed: 0x0B17_5EED,
            star_density: 1.0,
            star_brightness: 1.0,
            milky_way: 0.40,
            nebula: 0.35,
            ambient: 0.018,
            rotation_deg: 0.0,
            real_stars: true,
            magnitude_limit: 6.5,
            constellations: true,
            constellation_opacity: 0.10,
            constellation_min_behind_sun: 0.8,
        }
    }
}

/// The drawn orbit rings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Orbits {
    pub enabled: bool,
    /// Line width in physical pixels.
    pub width_px: f32,
    pub opacity: f32,
    /// How much brighter the ring is just behind each planet, giving a sense of
    /// travel direction. `0` draws a uniform ring.
    pub trail_strength: f32,
    /// Fraction of the orbit the trail spans.
    pub trail_length: f32,
    /// Segments per ring. Higher is smoother; 512 is already sub-pixel at 4K.
    pub segments: u32,
}

impl Default for Orbits {
    fn default() -> Self {
        Self {
            enabled: true,
            width_px: 1.4,
            opacity: 0.34,
            trail_strength: 2.2,
            trail_length: 0.22,
            segments: 512,
        }
    }
}

/// Renderer and output settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Render {
    /// Frame rate cap. A wallpaper has no business running at 240 Hz.
    pub fps: u32,
    /// Multisample count: 1, 2, 4 or 8.
    pub msaa: u32,
    /// Exposure applied before tone mapping, in stops.
    pub exposure_stops: f32,
    pub bloom_intensity: f32,
    pub vsync: bool,
}

impl Default for Render {
    fn default() -> Self {
        Self {
            fps: 30,
            msaa: 4,
            exposure_stops: 0.0,
            bloom_intensity: 0.09,
            vsync: true,
        }
    }
}

/// Which bodies to draw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bodies {
    /// Planet names, case-insensitive. Order is irrelevant.
    pub show: Vec<String>,
    /// Draw the Moon around Earth. Its orbit is exaggerated to stay visible.
    pub moon: bool,
    /// How far to push the Moon from Earth, as a multiple of the true
    /// separation under the current scale. The Moon is 60 Earth radii out,
    /// which compresses to nothing.
    pub moon_distance_boost: f32,
    /// Draw the asteroid belt between Mars and Jupiter.
    pub asteroid_belt: bool,
    /// Draw the Kuiper belt beyond Neptune. This is the outermost thing in the
    /// scene, so switching it on pulls the camera back and shrinks everything
    /// else.
    pub kuiper_belt: bool,
    /// Particles in the asteroid belt. The Kuiper belt gets 1.6x this, being
    /// both wider and more populous.
    pub belt_particles: u32,
}

impl Default for Bodies {
    fn default() -> Self {
        Self {
            // Pluto omitted by default: it is not a planet, and its 17°
            // inclination sits it awkwardly outside the plane of the others.
            show: Planet::ALL
                .iter()
                .filter(|p| **p != Planet::Pluto)
                .map(|p| p.name().to_owned())
                .collect(),
            moon: true,
            moon_distance_boost: 3.5,
            asteroid_belt: true,
            kuiper_belt: true,
            belt_particles: 6000,
        }
    }
}

impl Bodies {
    /// Resolve [`Bodies::show`] into planets, rejecting unknown names.
    pub fn resolve(&self) -> Result<Vec<Planet>, ConfigError> {
        let mut resolved = Vec::with_capacity(self.show.len());
        for name in &self.show {
            let planet = Planet::from_name(name)
                .ok_or_else(|| ConfigError::UnknownBody(name.clone()))?;
            if !resolved.contains(&planet) {
                resolved.push(planet);
            }
        }
        resolved.sort();
        Ok(resolved)
    }
}

impl Config {
    /// Parse a configuration from TOML text.
    pub fn from_toml(text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(text)?;
        config.validate()?;
        Ok(config)
    }

    /// Reject configurations that would render nonsense, so mistakes surface as
    /// a log line rather than a black screen.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.scale.orbit.validate()?;
        self.scale.body.validate()?;
        self.bodies.resolve()?;
        self.time.start_epoch()?;

        if !(1.0..=179.0).contains(&self.camera.fov_deg) {
            return Err(ConfigError::Range("camera.fov_deg", "1 to 179"));
        }
        if !(-90.0..=90.0).contains(&self.camera.elevation_deg) {
            return Err(ConfigError::Range("camera.elevation_deg", "-90 to 90"));
        }
        if self.camera.zoom <= 0.0 || !self.camera.zoom.is_finite() {
            return Err(ConfigError::Range("camera.zoom", "greater than 0"));
        }
        if !matches!(self.render.msaa, 1 | 2 | 4 | 8) {
            return Err(ConfigError::Range("render.msaa", "1, 2, 4 or 8"));
        }
        if self.orbits.segments < 16 {
            return Err(ConfigError::Range("orbits.segments", "at least 16"));
        }
        if !(0.0..=1.0).contains(&self.lighting.night_brightness) {
            return Err(ConfigError::Range("lighting.night_brightness", "0.0 to 1.0"));
        }
        if !(0.0..=1.0).contains(&self.lighting.night_saturation) {
            return Err(ConfigError::Range("lighting.night_saturation", "0.0 to 1.0"));
        }
        if !(0.0..=3.0).contains(&self.lighting.day_saturation) {
            return Err(ConfigError::Range("lighting.day_saturation", "0.0 to 3.0"));
        }
        if !(0.0..=100.0).contains(&self.lighting.sun_intensity) {
            return Err(ConfigError::Range("lighting.sun_intensity", "0.0 to 100.0"));
        }
        if !(0.0..=1.0).contains(&self.sky.constellation_min_behind_sun) {
            return Err(ConfigError::Range("sky.constellation_min_behind_sun", "0.0 to 1.0"));
        }
        if !(0.0..=10_080.0).contains(&self.camera.rotation_period_minutes) {
            return Err(ConfigError::Range("camera.rotation_period_minutes", "0 to 10080"));
        }
        if !(0.0..=3650.0).contains(&self.camera.elevation_cycle_days) {
            return Err(ConfigError::Range("camera.elevation_cycle_days", "0 to 3650"));
        }
        if !(0.0..=90.0).contains(&self.camera.elevation_cycle_deg) {
            return Err(ConfigError::Range("camera.elevation_cycle_deg", "0 to 90"));
        }
        if !(-2.0..=6.5).contains(&self.sky.magnitude_limit) {
            return Err(ConfigError::Range("sky.magnitude_limit", "-2.0 to 6.5"));
        }
        if !(1.0..=3650.0).contains(&self.ephemeris.refresh_days) {
            return Err(ConfigError::Range("ephemeris.refresh_days", "1 to 3650"));
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("could not parse config: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("invalid scale: {0}")]
    Scale(#[from] ScaleError),
    #[error("unknown body {0:?}; expected one of Mercury..Pluto")]
    UnknownBody(String),
    #[error("could not parse date {0:?}; expected YYYY-MM-DD or YYYY-MM-DDTHH:MM:SS")]
    Date(String),
    #[error("{0} must be {1}")]
    Range(&'static str, &'static str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_is_the_default_config() {
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
    }

    #[test]
    fn defaults_are_valid() {
        Config::default().validate().unwrap();
    }

    #[test]
    fn partial_config_keeps_other_defaults() {
        let config = Config::from_toml("[camera]\nelevation_deg = 45.0\n").unwrap();
        assert_eq!(config.camera.elevation_deg, 45.0);
        assert_eq!(config.camera.fov_deg, Camera::default().fov_deg);
        assert_eq!(config.render, Render::default());
    }

    /// A typo in a wallpaper's config file must not silently do nothing.
    #[test]
    fn unknown_keys_are_rejected() {
        let err = Config::from_toml("[camera]\nelevaton_deg = 45.0\n").unwrap_err();
        assert!(matches!(err, ConfigError::Toml(_)), "{err}");
    }

    #[test]
    fn unknown_body_is_rejected() {
        let err = Config::from_toml("[bodies]\nshow = [\"Vulcan\"]\n").unwrap_err();
        assert!(matches!(err, ConfigError::UnknownBody(name) if name == "Vulcan"));
    }

    #[test]
    fn body_names_are_case_insensitive_and_deduplicated() {
        let config =
            Config::from_toml("[bodies]\nshow = [\"mars\", \"MARS\", \"Earth\"]\n").unwrap();
        assert_eq!(
            config.bodies.resolve().unwrap(),
            vec![Planet::Earth, Planet::Mars]
        );
    }

    #[test]
    fn scale_law_round_trips_through_toml() {
        let config = Config::from_toml(
            "[scale.orbit]\nlaw = \"logarithmic\"\nunits_per_au = 1.0\nsoftness = 0.4\n",
        )
        .unwrap();
        assert_eq!(
            config.scale.orbit,
            RadialScale::Logarithmic { units_per_au: 1.0, softness: 0.4 }
        );
        let text = toml::to_string(&config).unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), config);
    }

    #[test]
    fn default_config_round_trips_through_toml() {
        let config = Config::default();
        let text = toml::to_string(&config).unwrap();
        assert_eq!(Config::from_toml(&text).unwrap(), config);
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        for bad in [
            "[camera]\nfov_deg = 0.0\n",
            "[camera]\nelevation_deg = 120.0\n",
            "[camera]\nzoom = 0.0\n",
            "[render]\nmsaa = 3\n",
            "[orbits]\nsegments = 4\n",
            "[scale.orbit]\nlaw = \"power\"\nunits_per_au = 1.0\nexponent = -1.0\n",
        ] {
            assert!(Config::from_toml(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn dates_parse_in_both_forms() {
        let date_only = parse_date("2026-08-03").unwrap();
        let with_time = parse_date("2026-08-03T00:00:00Z").unwrap();
        assert!((date_only.0 - with_time.0).abs() < 1e-12);

        let midday = parse_date("2026-08-03T12:00:00").unwrap();
        assert!((midday.0 - date_only.0 - 0.5).abs() < 1e-12);

        // A space separator is accepted too, since TOML users will write it.
        assert!((parse_date("2026-08-03 12:00:00").unwrap().0 - midday.0).abs() < 1e-12);
    }

    #[test]
    fn malformed_dates_are_rejected() {
        for bad in [
            "not-a-date",
            "2026-13-01",
            "2026-08-32",
            "2026-08",
            "2026-08-03-01",
            "2026-08-03T25:00:00",
            "2026-08-03T12:61:00",
        ] {
            assert!(parse_date(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn fixed_mode_uses_the_configured_date() {
        let config =
            Config::from_toml("[time]\nmode = \"fixed\"\ndate = \"2026-08-03\"\n").unwrap();
        let epoch = config.time.start_epoch().unwrap();
        assert!((epoch.0 - 2_461_255.5).abs() < 1e-3);
    }
}

/// The shipped `config/orrery.toml` documents every default in prose. Prose
/// drifts, so this checks it against the real thing.
#[cfg(test)]
mod shipped_config {
    use super::Config;

    #[test]
    fn shipped_file_parses_and_equals_the_defaults() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../config/orrery.toml");
        let text = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("reading {path}: {e}"));
        let from_file = Config::from_toml(&text).expect("shipped config must be valid");
        assert_eq!(
            from_file,
            Config::default(),
            "config/orrery.toml has drifted from the built-in defaults"
        );
    }
}
