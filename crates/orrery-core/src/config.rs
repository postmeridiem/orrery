//! User configuration, loaded from TOML.
//!
//! Every field has a default, so a partial config file — or none at all — is
//! valid and yields the shipped look. `deny_unknown_fields` means a typo is
//! reported instead of silently ignored, which matters when the app runs as a
//! wallpaper with no visible console.

use serde::{Deserialize, Serialize};

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
    /// Vertical field of view, degrees.
    pub fov_deg: f32,
    /// Heliocentric radius, in AU, that lands on the left and right edges of the
    /// frame. This is the whole of the framing: the camera distance follows from
    /// it in closed form, with no search and so nothing that can quietly settle
    /// somewhere other than where it was asked to.
    ///
    /// The shipped `35.33` sits just outside Neptune's orbit, which is what
    /// leaves the Kuiper belt running off the sides rather than boxed inside the
    /// frame. Smaller crops in; larger pulls back.
    ///
    /// It replaced `zoom`, `fill` and `fit_width` — three knobs for one
    /// quantity, which interacted: `fill` was measured before `zoom` was
    /// applied, so cropping in changed what the solver thought it was fitting.
    pub frame_radius_au: f32,
    /// Minutes for the camera to travel once around the Sun. Zero holds still.
    ///
    /// This is what makes the constellations legible: the visible patch of sky
    /// sits behind the Sun, so circling the Sun walks that patch through every
    /// ecliptic longitude. An hour shows the whole band; set it to 1.0 to
    /// evaluate a full circuit quickly.
    ///
    /// The orrery turns with it, since the camera really is orbiting.
    pub rotation_period_minutes: f64,
    /// Shift the picture within the frame, in fractions of the viewport.
    ///
    /// `offset_y` is measured from the *nearer* edge: it lifts the picture on a
    /// landscape screen and lowers it on a portrait one, so the shipped `0.27`
    /// puts the Sun 23% from the top on a wide monitor and 23% from the bottom
    /// on a tall one. See [`crate::scene`] for why that flip happens at square.
    ///
    /// This is a lens shift: the image moves, the camera does not. It used to
    /// move the camera, which at a shallow tilt dropped it toward the ecliptic
    /// and made the outer orbit diverge rather than simply slide -- so asking
    /// for a nudge also resized the whole system.
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            elevation_deg: 16.0,
            azimuth_deg: 0.0,
            fov_deg: 55.0,
            frame_radius_au: 35.33,
            rotation_period_minutes: 60.0,
            offset_x: 0.0,
            offset_y: 0.27,
        }
    }
}

/// Which instant to draw, and how fast time runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Time {
    /// `"live"` tracks the system clock. `"fixed"` freezes at [`Time::date`].
    pub mode: TimeMode,
    /// Starting date, `YYYY-MM-DD` or `YYYY-MM-DDTHH:MM:SS`, UTC.
    ///
    /// When set, it is the starting epoch in *every* mode: fixed time freezes
    /// there, and live time runs from there at [`Time::days_per_second`].
    /// Unset, both modes start from the moment of launch.
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
    /// The Julian Date this configuration starts from: the configured date in
    /// every mode, else the moment this is called — so fixed mode with no
    /// date freezes at launch time.
    pub fn start_epoch(&self) -> Result<JulianDate, ConfigError> {
        match &self.date {
            Some(text) => parse_date(text),
            None => Ok(JulianDate::now()),
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
    let year: i32 = fields
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let month: u32 = fields
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    let day: u32 = fields
        .next()
        .ok_or_else(invalid)?
        .parse()
        .map_err(|_| invalid())?;
    if fields.next().is_some()
        || !(1..=12).contains(&month)
        || day < 1
        || day > days_in(year, month)
    {
        return Err(invalid());
    }

    let day_fraction = match time_part {
        None => 0.0,
        Some(t) => {
            let mut hms = t.split(':');
            let hours: f64 = hms
                .next()
                .ok_or_else(invalid)?
                .parse()
                .map_err(|_| invalid())?;
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

/// Days in a Gregorian month. Without this, `2026-02-31` would be accepted
/// and quietly rolled into March by the calendar arithmetic downstream.
fn days_in(year: i32, month: u32) -> u32 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
            if leap { 29 } else { 28 }
        }
        _ => 0,
    }
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Scale {
    pub orbit: RadialScale,
    pub body: BodyScale,
}

/// The sky behind the orrery: 8,404 catalogued stars at their true positions,
/// over a procedural haze that supplies the sub-naked-eye background.
///
/// Nine further fields lived here — the generator seed, star density, nebula
/// and ambient levels, a sky rotation, a magnitude cut-off, and switches for the
/// real stars and the constellations. They were tuning that has settled, and
/// they are now constants in [`orrery_render`] at the point each is used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Sky {
    /// Overall star brightness.
    pub star_brightness: f32,
    /// Intensity of the galactic band. Placed from the real IAU galactic pole,
    /// so it crosses the solar system at its true 60° to the ecliptic.
    pub milky_way: f32,
    /// How strongly the constellation figures are drawn. Deliberately very low:
    /// the lines are meant to be found by someone looking for them, not to turn
    /// the desktop into a star chart. Raise toward 0.5 to actually study them.
    pub constellation_opacity: f32,
}

impl Default for Sky {
    fn default() -> Self {
        Self {
            star_brightness: 1.0,
            milky_way: 0.40,
            constellation_opacity: 0.10,
        }
    }
}

/// The drawn orbit rings.
///
/// Line width, the trail that brightens the ring behind each planet, and the
/// segment count are constants now — see [`orrery_render`] and
/// [`crate::scene`]. How *visible* the rings are is the one thing worth a knob.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Orbits {
    /// `0` hides the rings entirely and leaves the planets on the bare sky.
    pub opacity: f32,
}

impl Default for Orbits {
    fn default() -> Self {
        Self { opacity: 0.34 }
    }
}

/// Renderer and output settings.
///
/// Multisampling, exposure and bloom strength are fixed in [`orrery_render`]:
/// they are part of the look, not preferences, and the look is what the
/// composition was signed off against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Render {
    /// Frame rate cap. A wallpaper has no business running at 240 Hz.
    pub fps: u32,
    pub vsync: bool,
}

impl Default for Render {
    fn default() -> Self {
        Self {
            fps: 30,
            vsync: true,
        }
    }
}

/// The optional company the planets keep.
///
/// Which planets are drawn is no longer configurable: it is the eight, always,
/// in [`crate::scene::DRAWN_PLANETS`]. A list you could edit invited a scene
/// with two planets in it, which every framing decision in this project assumes
/// away. These three are genuine taste — some people want the belts, some find
/// them noise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Bodies {
    /// Draw the Moon around Earth. Its separation is exaggerated to stay
    /// visible, though its direction is the real one.
    pub moon: bool,
    /// Draw the asteroid belt between Mars and Jupiter.
    pub asteroid_belt: bool,
    /// Draw the Kuiper belt beyond Neptune. It deliberately runs off the sides
    /// of the frame rather than being contained by it.
    pub kuiper_belt: bool,
}

impl Default for Bodies {
    fn default() -> Self {
        Self {
            moon: true,
            asteroid_belt: true,
            kuiper_belt: true,
        }
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
        self.time.start_epoch()?;

        if !(1.0..=179.0).contains(&self.camera.fov_deg) {
            return Err(ConfigError::Range("camera.fov_deg", "1 to 179"));
        }
        if !(-90.0..=90.0).contains(&self.camera.elevation_deg) {
            return Err(ConfigError::Range("camera.elevation_deg", "-90 to 90"));
        }
        if !(self.camera.frame_radius_au.is_finite() && self.camera.frame_radius_au > 0.0) {
            return Err(ConfigError::Range(
                "camera.frame_radius_au",
                "greater than 0",
            ));
        }
        if !(0.0..=1.0).contains(&self.orbits.opacity) {
            return Err(ConfigError::Range("orbits.opacity", "0.0 to 1.0"));
        }
        if !(0.0..=1.0).contains(&self.lighting.night_brightness) {
            return Err(ConfigError::Range(
                "lighting.night_brightness",
                "0.0 to 1.0",
            ));
        }
        if !(0.0..=1.0).contains(&self.lighting.night_saturation) {
            return Err(ConfigError::Range(
                "lighting.night_saturation",
                "0.0 to 1.0",
            ));
        }
        if !(0.0..=3.0).contains(&self.lighting.day_saturation) {
            return Err(ConfigError::Range("lighting.day_saturation", "0.0 to 3.0"));
        }
        if !(0.0..=100.0).contains(&self.lighting.sun_intensity) {
            return Err(ConfigError::Range("lighting.sun_intensity", "0.0 to 100.0"));
        }
        if !(0.0..=10_080.0).contains(&self.camera.rotation_period_minutes) {
            return Err(ConfigError::Range(
                "camera.rotation_period_minutes",
                "0 to 10080",
            ));
        }
        if !(1.0..=3650.0).contains(&self.ephemeris.refresh_days) {
            return Err(ConfigError::Range("ephemeris.refresh_days", "1 to 3650"));
        }
        if !(1..=240).contains(&self.render.fps) {
            return Err(ConfigError::Range("render.fps", "1 to 240"));
        }
        if !self.camera.azimuth_deg.is_finite() {
            return Err(ConfigError::Range("camera.azimuth_deg", "finite"));
        }
        // The offsets are fractions of the viewport; a whole viewport in
        // either direction already moves the picture entirely off screen.
        if !(-1.0..=1.0).contains(&self.camera.offset_x) {
            return Err(ConfigError::Range("camera.offset_x", "-1.0 to 1.0"));
        }
        if !(-1.0..=1.0).contains(&self.camera.offset_y) {
            return Err(ConfigError::Range("camera.offset_y", "-1.0 to 1.0"));
        }
        // These three go straight to the shader, where NaN would paint the
        // whole sky with it. `contains` on a range rejects NaN by itself for
        // the bounded ones; the explicit bound here also keeps "brightness
        // 1000000" from being an accepted way to white out the screen.
        if !(0.0..=10.0).contains(&self.sky.star_brightness) {
            return Err(ConfigError::Range("sky.star_brightness", "0.0 to 10.0"));
        }
        if !(0.0..=10.0).contains(&self.sky.milky_way) {
            return Err(ConfigError::Range("sky.milky_way", "0.0 to 10.0"));
        }
        if !(0.0..=1.0).contains(&self.sky.constellation_opacity) {
            return Err(ConfigError::Range(
                "sky.constellation_opacity",
                "0.0 to 1.0",
            ));
        }
        // A century per second laps Neptune's orbit twice a minute; anything
        // beyond that is surely a typo, and NaN or infinity would poison the
        // epoch arithmetic for good.
        if !(self.time.days_per_second.is_finite() && self.time.days_per_second.abs() <= 36_500.0) {
            return Err(ConfigError::Range(
                "time.days_per_second",
                "-36500 to 36500",
            ));
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

    /// A field that was removed must be reported by name, not ignored. This is
    /// how `install.sh` detects a config from an older version: it runs
    /// `--check-config`, and on failure moves the file aside with the offending
    /// key quoted back to the user.
    #[test]
    fn fields_removed_in_the_prune_are_rejected_by_name() {
        for (section, key) in [
            ("camera", "zoom = 0.578"),
            ("camera", "fill = 0.94"),
            ("camera", "fit_width = true"),
            ("camera", "roll_deg = 0.0"),
            ("sky", "seed = 186081005"),
            ("sky", "magnitude_limit = 6.5"),
            ("orbits", "segments = 512"),
            ("render", "msaa = 4"),
            ("bodies", "show = [\"Mars\"]"),
            ("bodies", "belt_particles = 6000"),
        ] {
            let text = format!("[{section}]\n{key}\n");
            let err = Config::from_toml(&text).unwrap_err();
            let name = key.split(' ').next().unwrap();
            assert!(
                matches!(&err, ConfigError::Toml(e) if e.to_string().contains(name)),
                "removed key {name} was not reported by name: {err}"
            );
        }
    }

    #[test]
    fn scale_law_round_trips_through_toml() {
        let config = Config::from_toml(
            "[scale.orbit]\nlaw = \"logarithmic\"\nunits_per_au = 1.0\nsoftness = 0.4\n",
        )
        .unwrap();
        assert_eq!(
            config.scale.orbit,
            RadialScale::Logarithmic {
                units_per_au: 1.0,
                softness: 0.4
            }
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
            "[camera]\nframe_radius_au = 0.0\n",
            "[orbits]\nopacity = 1.5\n",
            "[scale.orbit]\nlaw = \"power\"\nunits_per_au = 1.0\nexponent = -1.0\n",
            "[render]\nfps = 0\n",
            "[render]\nfps = 1000\n",
            "[camera]\noffset_y = 2.0\n",
            "[camera]\nazimuth_deg = inf\n",
            "[sky]\nstar_brightness = nan\n",
            "[sky]\nmilky_way = -1.0\n",
            "[time]\ndays_per_second = nan\n",
            "[time]\ndays_per_second = 1e9\n",
        ] {
            assert!(Config::from_toml(bad).is_err(), "accepted {bad:?}");
        }
    }

    /// `[scale.orbit]` is the config section most likely to be hand-edited,
    /// and — as an internally tagged enum — the one where derived serde
    /// silently ignored unknown keys. The hand-written `Deserialize` closes
    /// that: a typo is an error naming the stray key, and a parameter left
    /// over from a different law is called out rather than dropped.
    #[test]
    fn scale_orbit_rejects_stray_and_misplaced_keys() {
        let typo = "[scale.orbit]\nlaw = \"power\"\nunits_per_au = 1.0\nexpoennt = 0.45\n";
        let err = Config::from_toml(typo).unwrap_err();
        assert!(
            err.to_string().contains("expoennt"),
            "typo not named: {err}"
        );

        let leftover = "[scale.orbit]\nlaw = \"linear\"\nunits_per_au = 1.0\nexponent = 0.45\n";
        let err = Config::from_toml(leftover).unwrap_err();
        assert!(
            err.to_string().contains("exponent") && err.to_string().contains("linear"),
            "leftover key not explained: {err}"
        );

        let missing = "[scale.orbit]\nlaw = \"power\"\nunits_per_au = 1.0\n";
        let err = Config::from_toml(missing).unwrap_err();
        assert!(
            err.to_string().contains("exponent"),
            "missing key not named: {err}"
        );
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
            // Days that don't exist in their month must not roll over into
            // the next one.
            "2026-02-29",
            "2026-02-31",
            "2026-04-31",
            "2100-02-29", // divisible by 100: not a leap year
        ] {
            assert!(parse_date(bad).is_err(), "accepted {bad:?}");
        }
        // ...while genuine leap days parse.
        assert!(parse_date("2024-02-29").is_ok());
        assert!(
            parse_date("2000-02-29").is_ok(),
            "divisible by 400 is a leap year"
        );
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
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
        let from_file = Config::from_toml(&text).expect("shipped config must be valid");
        assert_eq!(
            from_file,
            Config::default(),
            "config/orrery.toml has drifted from the built-in defaults"
        );
    }
}
