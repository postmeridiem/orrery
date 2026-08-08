//! Turning an instant in time into everything the renderer needs to draw.
//!
//! This is the boundary between astronomy and graphics: [`Scene::build`] takes
//! a [`Config`] and a [`JulianDate`] and produces plain geometry in scene
//! units. Nothing here touches the GPU, so the layout is testable on its own.
//!
//! **Coordinate convention.** The ephemeris works in the ecliptic frame, where
//! +Z is the ecliptic north pole. The renderer wants Y up. The mapping is
//!
//! ```text
//! scene.x =  ecliptic.x
//! scene.y =  ecliptic.z
//! scene.z = -ecliptic.y
//! ```
//!
//! which is a rotation, not a reflection — so the planets still orbit
//! anticlockwise seen from the north, as they do in reality.

use std::sync::Arc;

use glam::{DVec3, Mat4, Quat, Vec2, Vec3};

use crate::bodies::{self, BodyData, Rings};
use crate::config::Config;
use crate::ephemeris::{self, Planet};
use crate::lookup::Lookup;
use crate::scale::RadialScale;
use crate::time::JulianDate;

/// Convert an ecliptic-frame vector to the renderer's Y-up frame.
pub fn ecliptic_to_scene(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.z as f32, -v.y as f32)
}

/// The planets drawn, innermost first.
///
/// Pluto is deliberately absent: it is not a planet, and its 17° inclination
/// sits it awkwardly outside the plane of the others. [`crate::ephemeris`] still
/// carries it — whether a body is *drawn* is a scene question, not an ephemeris
/// one — so adding it back is a one-line change here.
pub const DRAWN_PLANETS: [Planet; 8] = [
    Planet::Mercury,
    Planet::Venus,
    Planet::Earth,
    Planet::Mars,
    Planet::Jupiter,
    Planet::Saturn,
    Planet::Uranus,
    Planet::Neptune,
];

/// Segments per orbit ring. Already sub-pixel at 4K; higher buys nothing.
const ORBIT_SEGMENTS: u32 = 512;

/// How far the Moon is pushed from Earth, as a multiple of its true separation
/// under the current scale. It is 60 Earth radii out, which compresses to
/// nothing — so the distance is exaggerated while the *direction* stays real,
/// and the phase and the side it sits on read correctly.
const MOON_DISTANCE_BOOST: f32 = 3.5;

/// Particles in the asteroid belt.
const BELT_PARTICLES: u32 = 6_000;

/// Particles in the Kuiper belt: 1.6× the main belt, being both wider and
/// more populous.
const KUIPER_PARTICLES: u32 = BELT_PARTICLES * 8 / 5;

/// One drawn body.
#[derive(Debug, Clone)]
pub struct BodyInstance {
    pub name: &'static str,
    /// Centre, in scene units.
    pub position: Vec3,
    /// Drawn radius, in scene units.
    pub radius: f32,
    /// Base albedo, linear sRGB.
    pub color: [f32; 3],
    /// Orientation: spin about the body's own axis, with that axis tilted by
    /// the body's obliquity.
    pub orientation: Quat,
    pub flattening: f32,
    pub rings: Option<Rings>,
    /// The Sun is lit by nothing and lights everything else.
    pub emissive: bool,
    /// Stable index used by the renderer to vary procedural surface detail.
    pub surface_seed: u32,
}

/// One drawn orbit, as a closed polyline in scene units.
///
/// The points are shared rather than owned because the geometry changes only
/// when the osculating elements do — monthly under an almanac — while a scene
/// is built every frame. Only `body_fraction` is per-frame.
#[derive(Debug, Clone)]
pub struct OrbitRing {
    pub points: Arc<[Vec3]>,
    pub color: [f32; 3],
    /// Where along `points` the body currently sits, in `0..1`. The renderer
    /// uses this to brighten the ring just behind the planet.
    pub body_fraction: f32,
}

/// One particle of a debris belt.
#[derive(Debug, Clone, Copy)]
pub struct BeltParticle {
    pub position: Vec3,
    /// Drawn radius in scene units.
    pub size: f32,
    pub brightness: f32,
}

/// A ring of debris: the main asteroid belt, or the Kuiper belt.
///
/// Drawn as individual particles rather than a solid annulus, because that is
/// what they are -- and because a torus of points reads as depth in a way a
/// flat band does not.
#[derive(Debug, Clone)]
pub struct Belt {
    pub name: &'static str,
    pub particles: Vec<BeltParticle>,
    pub color: [f32; 3],
}

/// Deterministic hash, so a belt looks identical from frame to frame.
fn hash_u32(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

fn random_unit(seed: u32, index: u32, stream: u32) -> f64 {
    hash_u32(seed ^ hash_u32(index.wrapping_mul(0x9E37_79B9) ^ stream)) as f64 / u32::MAX as f64
}

/// Everything that shapes one debris belt, so [`build_belt`] takes a
/// description rather than nine loose arguments.
struct BeltSpec {
    name: &'static str,
    inner_au: f64,
    outer_au: f64,
    thickness_au: f64,
    count: u32,
    seed: u32,
    color: [f32; 3],
    brightness: f32,
}

/// Scatter `spec.count` particles through the torus the spec describes.
fn build_belt(spec: &BeltSpec, scale: &RadialScale) -> Belt {
    let particles = (0..spec.count)
        .map(|index| {
            // Radius is biased toward the middle of the belt, which is roughly
            // how the real population is distributed.
            let u = random_unit(spec.seed, index, 0);
            let v = random_unit(spec.seed, index, 1);
            let bias = (u + v) * 0.5;
            let radius = spec.inner_au + (spec.outer_au - spec.inner_au) * bias;

            let angle = random_unit(spec.seed, index, 2) * std::f64::consts::TAU;
            // Two samples summed approximate a normal distribution, so the belt
            // is concentrated near the ecliptic with a scattered tail.
            let height = (random_unit(spec.seed, index, 3) + random_unit(spec.seed, index, 4)
                - 1.0)
                * spec.thickness_au;

            let position_au = DVec3::new(radius * angle.cos(), radius * angle.sin(), height);
            let scaled = scale.apply_to_position(position_au);

            // Faint at the edges, brighter through the middle.
            let edge = ((bias - 0.5).abs() * 2.0).clamp(0.0, 1.0);
            let brightness = (1.0 - edge * edge) as f32;

            BeltParticle {
                position: ecliptic_to_scene(scaled),
                size: (0.0016 + 0.0022 * random_unit(spec.seed, index, 5)) as f32,
                brightness: (0.25 + 0.75 * brightness) * spec.brightness,
            }
        })
        .collect();

    Belt {
        name: spec.name,
        particles,
        color: spec.color,
    }
}

/// Camera placement for a frame.
#[derive(Debug, Clone, Copy)]
pub struct CameraState {
    pub eye: Vec3,
    pub target: Vec3,
    pub up: Vec3,
    pub fov_y_radians: f32,
    pub near: f32,
    pub far: f32,
    /// Lens shift, in normalised device coordinates: how far the image sits
    /// from centre once projected. `(0.0, 0.2)` lifts everything a tenth of
    /// the screen height, putting the Sun two fifths from the top.
    ///
    /// This is a shift of the *image*, not of the camera. Moving the camera
    /// instead -- which is what the old `offset_y` did -- changes the angle the
    /// ecliptic is seen at, and at a shallow tilt that makes the near arc of
    /// the outer orbit diverge violently rather than simply sliding down.
    pub lens_shift: Vec2,
}

impl CameraState {
    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye, self.target, self.up)
    }

    /// Reversed-Z infinite perspective. Reversed Z buys float depth precision
    /// across the enormous near/far ratio an orrery spans; the renderer pairs
    /// it with a `GreaterEqual` depth test and a clear value of 0.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        let projection = glam::camera::rh::proj::directx::perspective_infinite_reverse(
            self.fov_y_radians,
            aspect,
            self.near,
        );

        if self.lens_shift == Vec2::ZERO {
            return projection;
        }

        // Shear in clip space: `y += shift.y * w`, so after the perspective
        // divide every point lands `shift.y` further up the screen. Depth is
        // untouched, which keeps the reversed-Z arrangement intact.
        let mut shift = Mat4::IDENTITY;
        shift.w_axis.x = self.lens_shift.x;
        shift.w_axis.y = self.lens_shift.y;
        shift * projection
    }

    pub fn view_projection(&self, aspect: f32) -> Mat4 {
        self.projection(aspect) * self.view()
    }
}

/// Everything needed to draw one frame.
#[derive(Debug, Clone)]
pub struct Scene {
    pub epoch: JulianDate,
    pub sun: BodyInstance,
    pub bodies: Vec<BodyInstance>,
    pub orbits: Vec<OrbitRing>,
    pub camera: CameraState,
    /// Debris belts, outermost last. Shared for the same reason as
    /// [`OrbitRing::points`]: the particles are a pure function of the config.
    pub belts: Arc<[Belt]>,
    /// Which build of the belts this is. Increments when the cache rebuilds
    /// them; 0 means "uncached — treat as new every time". The renderer skips
    /// re-uploading a generation it has already sent to the GPU.
    pub belts_generation: u64,
    /// Which build of the orbit ring geometry this is; same convention.
    pub orbits_generation: u64,
}

/// Reused state between [`Scene::build_cached`] calls.
///
/// The belts depend only on the config, and the orbit ring geometry only on
/// the config and the osculating elements — which change monthly under an
/// almanac. Rebuilding both every frame was, by a wide margin, the largest
/// CPU cost in the whole application, spent computing bytes identical to the
/// previous frame's.
#[derive(Debug, Default)]
pub struct SceneCache {
    belts: Option<CachedBelts>,
    orbits: Option<CachedOrbits>,
    /// Monotonic stamp for cache rebuilds. Starts at 1 so a generation of 0
    /// can mean "uncached" everywhere downstream.
    next_generation: u64,
}

#[derive(Debug)]
struct CachedBelts {
    key: (bool, bool, RadialScale),
    belts: Arc<[Belt]>,
    generation: u64,
}

#[derive(Debug)]
struct CachedOrbits {
    scale: RadialScale,
    epoch: f64,
    /// Ring points per drawn planet, in [`DRAWN_PLANETS`] order.
    points: Vec<Arc<[Vec3]>>,
    generation: u64,
}

/// How far the epoch may move before cached ring geometry is rebuilt. The
/// osculating elements drift over weeks, not hours; a quarter day of drift
/// moves a ring by far less than a pixel, while live time at real rate takes
/// six hours to cross it.
const ORBIT_CACHE_TOLERANCE_DAYS: f64 = 0.25;

impl SceneCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forget everything. Call when the position source itself changes — an
    /// almanac refresh landing — which no cache key here can observe.
    pub fn invalidate(&mut self) {
        self.belts = None;
        self.orbits = None;
    }

    fn stamp(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    fn belts_for(&mut self, config: &Config) -> (Arc<[Belt]>, u64) {
        let key = (
            config.bodies.asteroid_belt,
            config.bodies.kuiper_belt,
            config.scale.orbit,
        );
        if let Some(cached) = &self.belts
            && cached.key == key
        {
            return (Arc::clone(&cached.belts), cached.generation);
        }
        let belts: Arc<[Belt]> = build_belts(config).into();
        let generation = self.stamp();
        self.belts = Some(CachedBelts {
            key,
            belts: Arc::clone(&belts),
            generation,
        });
        (belts, generation)
    }

    fn orbit_points_for(
        &mut self,
        scale: RadialScale,
        lookup: &Lookup,
        epoch: JulianDate,
    ) -> (Vec<Arc<[Vec3]>>, u64) {
        if let Some(cached) = &self.orbits
            && cached.scale == scale
            && (cached.epoch - epoch.0).abs() <= ORBIT_CACHE_TOLERANCE_DAYS
        {
            return (cached.points.clone(), cached.generation);
        }
        let points = build_ring_points(&scale, lookup, epoch);
        let generation = self.stamp();
        self.orbits = Some(CachedOrbits {
            scale,
            epoch: epoch.0,
            points: points.clone(),
            generation,
        });
        (points, generation)
    }
}

/// Sample every drawn planet's orbit, in [`DRAWN_PLANETS`] order.
fn build_ring_points(scale: &RadialScale, lookup: &Lookup, epoch: JulianDate) -> Vec<Arc<[Vec3]>> {
    DRAWN_PLANETS
        .into_iter()
        .map(|planet| orbit_points(&lookup.elements(planet, epoch), scale))
        .collect()
}

/// The debris belts the config asks for, outermost last.
fn build_belts(config: &Config) -> Vec<Belt> {
    let orbit_scale = &config.scale.orbit;
    let mut belts = Vec::new();
    if config.bodies.asteroid_belt {
        // The main belt runs roughly 2.1 to 3.3 AU, between Mars and Jupiter.
        belts.push(build_belt(
            &BeltSpec {
                name: "Asteroid Belt",
                inner_au: 2.1,
                outer_au: 3.3,
                thickness_au: 0.10,
                count: BELT_PARTICLES,
                seed: 0xA57E_201D,
                color: [0.72, 0.66, 0.56],
                // The real main belt is invisible from anywhere. Drawn dense
                // and additive it piles up into a solid glowing ring that
                // out-shouts the Sun, so it is kept to a suggestion.
                brightness: 0.16,
            },
            orbit_scale,
        ));
    }
    if config.bodies.kuiper_belt {
        // The classical Kuiper belt runs from Neptune's orbit out to the
        // 2:1 resonance at about 48 AU, and is far thicker than the main
        // belt.
        belts.push(build_belt(
            &BeltSpec {
                name: "Kuiper Belt",
                inner_au: 30.0,
                outer_au: 48.0,
                thickness_au: 2.4,
                count: KUIPER_PARTICLES,
                seed: 0x4B1D_9E37,
                color: [0.62, 0.70, 0.82],
                // Spread over a far larger area, so it survives being brighter.
                brightness: 0.55,
            },
            orbit_scale,
        ));
    }
    belts
}

impl Scene {
    /// Build the scene for `epoch` under `config`, framed for `aspect`
    /// (width / height).
    ///
    /// `lookup` decides where positions come from: a Horizons almanac when one
    /// covers this instant, the built-in tables otherwise.
    ///
    /// Everything is built from scratch. The render loop uses
    /// [`Scene::build_cached`] instead; this entry point serves one-shot
    /// callers — tests and screenshots — where a cache would be dead weight.
    pub fn build(config: &Config, lookup: &Lookup, epoch: JulianDate, aspect: f32) -> Self {
        Self::assemble(
            config,
            lookup,
            epoch,
            aspect,
            config.camera.azimuth_deg,
            None,
        )
    }

    /// [`Scene::build`], but reusing the belts and orbit geometry held in
    /// `cache` when their inputs have not changed.
    ///
    /// `azimuth_deg` replaces the configured azimuth, so the camera drift does
    /// not need a mutated copy of the whole `Config` — which would both
    /// allocate every frame and destabilise the cache keys.
    pub fn build_cached(
        config: &Config,
        lookup: &Lookup,
        epoch: JulianDate,
        aspect: f32,
        azimuth_deg: f32,
        cache: &mut SceneCache,
    ) -> Self {
        Self::assemble(config, lookup, epoch, aspect, azimuth_deg, Some(cache))
    }

    fn assemble(
        config: &Config,
        lookup: &Lookup,
        epoch: JulianDate,
        aspect: f32,
        azimuth_deg: f32,
        mut cache: Option<&mut SceneCache>,
    ) -> Self {
        let orbit_scale = &config.scale.orbit;
        let body_scale = &config.scale.body;

        let (ring_points, orbits_generation) = match cache.as_deref_mut() {
            Some(cache) => cache.orbit_points_for(*orbit_scale, lookup, epoch),
            None => (build_ring_points(orbit_scale, lookup, epoch), 0),
        };

        let mut bodies_out = Vec::with_capacity(DRAWN_PLANETS.len() + 1);
        let mut orbits_out = Vec::with_capacity(DRAWN_PLANETS.len());

        for (index, planet) in DRAWN_PLANETS.into_iter().enumerate() {
            let elements = lookup.elements(planet, epoch);
            let data = bodies::data(planet);

            let position_au = lookup.position(planet, epoch);
            let position = ecliptic_to_scene(orbit_scale.apply_to_position(position_au));
            let radius = body_scale.apply(data.radius_km) as f32;

            bodies_out.push(BodyInstance {
                name: planet.name(),
                position,
                radius,
                color: data.color,
                orientation: spin_orientation(&data, epoch),
                flattening: data.flattening as f32,
                rings: data.rings,
                emissive: false,
                surface_seed: index as u32 + 1,
            });

            // The geometry may be cached; where the body sits on it is always
            // this frame's.
            orbits_out.push(OrbitRing {
                points: Arc::clone(&ring_points[index]),
                color: data.color,
                body_fraction: body_fraction(&elements),
            });

            // The Moon rides along with Earth.
            if planet == Planet::Earth && config.bodies.moon {
                let moon_offset_au = ephemeris::geocentric_moon_position(epoch);
                let offset = ecliptic_to_scene(
                    orbit_scale.apply_to_position(position_au + moon_offset_au)
                        - orbit_scale.apply_to_position(position_au),
                ) * MOON_DISTANCE_BOOST;
                let moon_offset = if offset.length() < radius * 1.8 {
                    offset.normalize_or_zero() * radius * 1.8
                } else {
                    offset
                };
                bodies_out.push(BodyInstance {
                    name: "Moon",
                    position: position + moon_offset,
                    radius: body_scale.apply(bodies::MOON.radius_km) as f32,
                    color: bodies::MOON.color,
                    orientation: spin_orientation(&bodies::MOON, epoch),
                    flattening: bodies::MOON.flattening as f32,
                    rings: None,
                    emissive: false,
                    surface_seed: 100,
                });
            }
        }

        let sun = BodyInstance {
            name: "Sun",
            position: Vec3::ZERO,
            radius: body_scale.apply_to_sun(bodies::SUN_RADIUS_KM) as f32,
            color: bodies::SUN.color,
            orientation: spin_orientation(&bodies::SUN, epoch),
            flattening: 0.0,
            rings: None,
            emissive: true,
            surface_seed: 0,
        };

        let (belts, belts_generation) = match cache {
            Some(cache) => cache.belts_for(config),
            None => (build_belts(config).into(), 0),
        };

        // The framing is a closed form over one configured radius, so it needs
        // nothing from the scene it is framing.
        let camera = frame_camera_with_azimuth(config, aspect, azimuth_deg);

        Scene {
            epoch,
            sun,
            bodies: bodies_out,
            orbits: orbits_out,
            camera,
            belts,
            belts_generation,
            orbits_generation,
        }
    }
}

/// Sample a full orbit by sweeping eccentric anomaly, which distributes points
/// evenly around the *ellipse* rather than clustering them at perihelion.
fn orbit_points(elements: &ephemeris::Kepler, scale: &RadialScale) -> Arc<[Vec3]> {
    (0..ORBIT_SEGMENTS)
        .map(|i| {
            let eccentric_anomaly = 360.0 * f64::from(i) / f64::from(ORBIT_SEGMENTS);
            ecliptic_to_scene(
                scale.apply_to_position(elements.position_at_eccentric_anomaly(eccentric_anomaly)),
            )
        })
        .collect()
}

/// Where along its sampled ring a body currently sits, in `0..1`.
fn body_fraction(elements: &ephemeris::Kepler) -> f32 {
    (ephemeris::solve_kepler(elements.mean_anomaly(), elements.e).rem_euclid(360.0) / 360.0) as f32
}

/// Orientation of a body: obliquity applied to the ecliptic pole, then spin.
///
/// The *azimuth* the axis leans toward is not modelled — that would need each
/// body's pole right ascension, which the JPL approximate-position tables do
/// not carry. Every axis therefore leans toward ecliptic longitude 0. The tilt
/// magnitude and the spin rate are real; only the lean direction is arbitrary,
/// which is visible on Uranus and nowhere else.
fn spin_orientation(data: &BodyData, epoch: JulianDate) -> Quat {
    let days = epoch.days_since_j2000();
    let period_days = data.rotation_period_hours / 24.0;
    // The threshold is a magnitude — a tenth of a second per rotation — not
    // `f64::EPSILON`, which is relative spacing at 1.0 and means nothing as a
    // "no rotation" cutoff. Every real body in the table is far above it.
    const MIN_PERIOD_DAYS: f64 = 1e-6;
    let spin = if period_days.abs() > MIN_PERIOD_DAYS {
        (std::f64::consts::TAU * days / period_days).rem_euclid(std::f64::consts::TAU)
    } else {
        0.0
    };
    // Tilt about scene +X (ecliptic longitude 0), then spin about the tilted
    // pole, which after the tilt is the body's own +Y.
    let tilt = Quat::from_rotation_x(data.axial_tilt_deg.to_radians() as f32);
    tilt * Quat::from_rotation_y(spin as f32)
}

/// `offset_y` measured from the *nearer* edge: it lifts the picture on a
/// landscape screen and lowers it on a portrait one.
///
/// At a shallow tilt the near arc of the outermost orbit hangs well below the
/// Sun, so a wide frame has to lift the picture to keep that arc on screen. A
/// tall frame has the opposite problem — the system is small in it, and pinning
/// the Sun near the top leaves the whole lower half empty, so the orrery hangs
/// like a chandelier instead of sitting like a foundation.
///
/// Flipping below aspect 1 is not a chosen threshold; it is where the geometry
/// changes sign. Bottom-aligned, the lowest planet lands 127 px over the edge at
/// 4:3 and 78 px over at 5:4, clears by 6 px at exactly 1:1, and only improves
/// from there. Square is the last shape with no room, so square is the boundary.
fn vertical_offset(offset_y: f32, aspect: f32) -> f32 {
    if aspect < 1.0 { -offset_y } else { offset_y }
}

/// The aspect floor for a degenerate viewport. A magnitude with a meaning —
/// one pixel of width per thousand of height — where `f32::EPSILON` was the
/// spacing of floats at 1.0 and no useful floor at all.
const MIN_ASPECT: f32 = 1e-3;

/// Place the camera so a chosen heliocentric radius lands on the left and right
/// edges of the frame.
///
/// One parameter, solved in closed form. There is no search, no iteration and
/// no convergence budget, so there is nothing that can quietly give up and leave
/// the picture wherever a loop happened to stop. The distance does not depend on
/// the azimuth either, so circling the Sun cannot re-frame the scene — which is
/// what `one_rotation_does_not_re_frame_the_scene` exists to hold. The azimuth
/// itself is a parameter rather than read from the config, which is how the
/// per-frame camera drift avoids mutating a clone of the whole `Config`.
///
/// # The widest point of an orbit is not the one beside the Sun
///
/// The obvious closed form, `scale(r) / tan(fov_x / 2)`, puts the point square
/// to the view direction on the frame edge. That is only the answer looking
/// straight down. Seen from a shallow angle the near half of an orbit is closer
/// to the camera and projects larger, so the extreme left and right of the
/// ellipse sit round towards the viewer. Parameterise a circle of scene radius
/// `s` in the ecliptic plane by `t`:
///
/// ```text
/// sideways offset   s·sin t
/// depth             d − s·cos(elevation)·cos t
/// ```
///
/// Their ratio peaks at `cos t = s·cos(elevation) / d`, where it equals
/// `s / sqrt(d² − s²·cos²(elevation))`. Requiring that to be exactly the frame
/// half-width and solving for `d` gives the expression below.
///
/// Dropping `cos(elevation)` recovers the naive form. At the shipped 16° that
/// lands the camera at 4.00 units where the honest answer is 6.23, and Uranus
/// and Neptune leave the frame entirely — verified by rendering it. This is the
/// perspective foreshortening an earlier comment here claimed no closed form
/// could account for.
fn frame_camera_with_azimuth(config: &Config, aspect: f32, azimuth_deg: f32) -> CameraState {
    let camera = &config.camera;
    let aspect = aspect.max(MIN_ASPECT);

    // The elevation is used exactly as configured. Nothing here adjusts it.
    //
    // There used to be a search that stepped the elevation down until the scene
    // fitted vertically. It meant the configured angle was not the angle you
    // got -- the picture ended up at whichever step the loop stopped on, and
    // that step moved as the camera orbited. The distance does the framing; the
    // tilt is the user's to set.
    let elevation = camera.elevation_deg.to_radians();
    let fov_y = camera.fov_deg.to_radians();

    // fov_x = 2·atan(aspect·tan(fov_y / 2)), so this is tan(fov_x / 2).
    let tan_half_fov_x = aspect * (fov_y * 0.5).tan();
    let edge = config.scale.orbit.apply(camera.frame_radius_au as f64) as f32;
    let cos_elevation = elevation.cos();

    let distance = edge
        * (1.0 + tan_half_fov_x * tan_half_fov_x * cos_elevation * cos_elevation).sqrt()
        / tan_half_fov_x;
    // A radius of zero would put the eye on the target and make the view matrix
    // NaN. `Config::validate` rejects that, but `Scene::build` is also called
    // directly, so this is cheap insurance rather than a correction.
    let distance = distance.max(1e-3);

    let azimuth = azimuth_deg.to_radians();
    let direction = Vec3::new(
        cos_elevation * azimuth.cos(),
        elevation.sin(),
        cos_elevation * azimuth.sin(),
    )
    .normalize_or(Vec3::Y);
    let forward = -direction;
    let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
    let up = right.cross(forward).normalize_or(Vec3::Y);

    CameraState {
        eye: direction * distance,
        target: Vec3::ZERO,
        up,
        fov_y_radians: fov_y,
        near: (distance * 0.001).max(1e-4),
        far: distance * 10.0,
        // Applied *after* the framing, deliberately. Shifting during it would
        // let the solve pull back to compensate, shrinking the system -- so
        // asking to move the picture would silently also resize it. Offsets are
        // fractions of the full viewport; normalised device coordinates span two
        // of those per axis.
        lens_shift: Vec2::new(
            camera.offset_x * 2.0,
            vertical_offset(camera.offset_y, aspect) * 2.0,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    const EPOCH: JulianDate = JulianDate(2_461_255.5);

    fn scene() -> Scene {
        Scene::build(&Config::default(), &Lookup::builtin(), EPOCH, 16.0 / 9.0)
    }

    /// The ecliptic-to-scene map must be a rotation, not a reflection, or every
    /// planet would orbit backwards.
    #[test]
    fn ecliptic_to_scene_preserves_handedness() {
        let x = ecliptic_to_scene(DVec3::X);
        let y = ecliptic_to_scene(DVec3::Y);
        let z = ecliptic_to_scene(DVec3::Z);
        assert!((x.cross(y) - z).length() < 1e-6, "not right-handed");
        for v in [x, y, z] {
            assert!((v.length() - 1.0).abs() < 1e-6, "not orthonormal");
        }
    }

    #[test]
    fn ecliptic_north_becomes_scene_up() {
        assert!((ecliptic_to_scene(DVec3::Z) - Vec3::Y).length() < 1e-6);
    }

    #[test]
    fn default_scene_has_every_configured_body_plus_sun_and_moon() {
        let scene = scene();
        assert_eq!(scene.sun.name, "Sun");
        assert!(scene.sun.emissive);
        // Eight planets (Pluto is off by default) plus the Moon.
        assert_eq!(scene.bodies.len(), 9);
        assert!(scene.bodies.iter().any(|b| b.name == "Moon"));
        assert!(scene.bodies.iter().all(|b| !b.emissive));
        assert_eq!(scene.orbits.len(), 8);
    }

    /// Every body must sit on its own orbit ring — this is the check that the
    /// ring geometry and the body position come from the same elements.
    #[test]
    fn bodies_lie_on_their_orbit_rings() {
        let scene = scene();
        for (planet, ring) in DRAWN_PLANETS.iter().zip(&scene.orbits) {
            let body = scene
                .bodies
                .iter()
                .find(|b| b.name == planet.name())
                .expect("body present");
            let closest = ring
                .points
                .iter()
                .map(|p| (*p - body.position).length())
                .fold(f32::INFINITY, f32::min);
            // Within one segment's chord length of the ring.
            let chord = 2.0 * std::f32::consts::PI * body.position.length() / ORBIT_SEGMENTS as f32;
            assert!(
                closest < chord * 1.5,
                "{} is {closest} from its ring (chord {chord})",
                planet.name()
            );
        }
    }

    /// `body_fraction` must point at the ring vertex nearest the planet, or the
    /// trail highlight trails the wrong part of the orbit.
    #[test]
    fn body_fraction_indexes_the_right_ring_vertex() {
        let scene = scene();
        for (planet, ring) in DRAWN_PLANETS.iter().zip(&scene.orbits) {
            let body = scene
                .bodies
                .iter()
                .find(|b| b.name == planet.name())
                .unwrap();
            let index = (ring.body_fraction * ring.points.len() as f32).round() as usize
                % ring.points.len();
            let at_fraction = (ring.points[index] - body.position).length();
            let nearest = ring
                .points
                .iter()
                .map(|p| (*p - body.position).length())
                .fold(f32::INFINITY, f32::min);
            assert!(
                (at_fraction - nearest).abs() < body.position.length() * 0.02,
                "{}: fraction points {at_fraction} away, nearest is {nearest}",
                planet.name()
            );
        }
    }

    #[test]
    fn moon_sits_next_to_earth_and_clear_of_its_surface() {
        let scene = scene();
        let earth = scene.bodies.iter().find(|b| b.name == "Earth").unwrap();
        let moon = scene.bodies.iter().find(|b| b.name == "Moon").unwrap();
        let separation = (moon.position - earth.position).length();
        assert!(
            separation > earth.radius + moon.radius,
            "the Moon is inside the Earth"
        );
        assert!(
            separation < earth.position.length() * 0.25,
            "the Moon has left the Earth system"
        );
    }

    /// The composition on seven screen shapes, chosen from renders by Jeroen on
    /// 2026-08-04 and pinned here as numbers.
    ///
    /// This replaced a test that asserted the whole system fits at every aspect
    /// ratio and elevation. It could, when the framing searched for a distance
    /// that contained everything; it cannot now that one parameter fixes the
    /// left and right edges and the vertical follows. Rather than weaken that
    /// into something vague, the shapes were rendered, looked at, and signed
    /// off — so what is asserted here is a composition someone approved, not a
    /// property someone assumed.
    ///
    /// The clearances are what they are. 32:9 loses Neptune off the bottom and
    /// 21:9 grazes it; both were judged acceptable against the picture. Any
    /// change to the camera has to reproduce these or say why.
    #[test]
    fn the_golden_screen_shapes() {
        // Shape, width, height, lowest planet's clearance above the bottom edge
        // in pixels — negative meaning it hangs over.
        const GOLDENS: [(&str, u32, u32, f32); 7] = [
            ("32:9", 5120, 1440, -838.0),
            ("21:9", 3440, 1440, -16.0),
            ("16:9", 1920, 1080, 282.0),
            ("16:10", 1920, 1200, 400.0),
            ("3:2", 2256, 1504, 560.0),
            ("4:3", 1600, 1200, 521.0),
            // Portrait alone anchors the Sun to the bottom, so the system sits
            // low with sky above rather than hanging from the top edge.
            ("9:16", 1080, 1920, 237.0),
        ];

        for (shape, width, height, expected) in GOLDENS {
            let aspect = width as f32 / height as f32;
            let mut config = Config::default();
            config.camera.rotation_period_minutes = 0.0;
            let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, aspect);
            let view_projection = scene.camera.view_projection(aspect);
            let ndc = |p: Vec3| {
                let clip = view_projection * p.extend(1.0);
                (clip.w > 1e-6).then(|| clip.truncate() / clip.w)
            };

            // The Sun sits 23% from the nearer edge: the top in landscape, the
            // bottom in portrait.
            let sun_y = ndc(Vec3::ZERO).expect("the Sun in front of the camera").y;
            let from_nearer_edge = if aspect < 1.0 {
                (1.0 + sun_y) / 2.0
            } else {
                (1.0 - sun_y) / 2.0
            };
            assert!(
                (from_nearer_edge - 0.23).abs() < 0.003,
                "{shape}: the Sun is {:.1}% from the nearer edge, not 23%",
                from_nearer_edge * 100.0
            );

            // The lowest planet, measured as the real projected silhouette --
            // sampled over the sphere's surface. `radius / distance` is the
            // on-axis small-angle approximation and understates an off-axis disc
            // by about half, which is how the old measurement reported Neptune
            // clearing by 16px when it in fact hangs 16px over.
            let (mut lowest, mut widest) = (f32::MAX, 0.0f32);
            for body in &scene.bodies {
                for i in 0..512 {
                    let around = (i as f32 / 512.0) * std::f32::consts::TAU;
                    let along = ((i * 7 % 512) as f32 / 512.0) * std::f32::consts::PI;
                    let direction = Vec3::new(
                        along.sin() * around.cos(),
                        along.cos(),
                        along.sin() * around.sin(),
                    );
                    if let Some(v) = ndc(body.position + direction * body.radius) {
                        lowest = lowest.min(v.y);
                        widest = widest.max(v.x.abs());
                    }
                }
            }

            let clearance_px = (lowest + 1.0) * height as f32 / 2.0;
            assert!(
                (clearance_px - expected).abs() < 8.0,
                "{shape} at {width}x{height}: the lowest planet sits {clearance_px:.0}px \
                 above the bottom edge, golden is {expected:.0}px"
            );
            // No shape has ever clipped a planet at the sides, and none should.
            assert!(
                widest < 1.0,
                "{shape}: a planet reaches {widest:.3} across, past the side edge"
            );
        }
    }

    /// The camera must never be upside down, and the near side of an orbit must
    /// project *below* the Sun.
    ///
    /// This guards a bug that actually happened: a `right` vector with the wrong
    /// sign negates `up` with it and rotates the whole picture 180 degrees. On a
    /// scene this close to symmetric that is easy to miss -- it does not look
    /// like a flipped image, it looks like the outer orbits drifting above the
    /// ecliptic. Asserting `up.y > 0` alone is not enough to catch a basis that
    /// is only slightly wrong; the second assertion is the one with teeth.
    #[test]
    fn the_camera_is_the_right_way_up() {
        for azimuth in [0.0, 45.0, 90.0, 180.0, 270.0, 330.0] {
            for elevation in [2.0, 6.0, 27.0, 60.0] {
                let mut config = Config::default();
                config.camera.azimuth_deg = azimuth;
                config.camera.elevation_deg = elevation;
                let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, 16.0 / 9.0);

                assert!(
                    scene.camera.up.y > 0.0,
                    "azimuth {azimuth}, elevation {elevation}: up is {:?}",
                    scene.camera.up
                );

                // Looking down on the ecliptic means a point in the plane on the
                // camera's side of the Sun appears *below* the Sun, and one on
                // the far side appears above it.
                //
                // Probe with constructed points rather than orbit vertices. A
                // vertex of the outermost ring can fall behind the camera at a
                // tight zoom, and dividing by a negative `w` flips the sign --
                // which reads as an inversion when the basis is in fact fine.
                let view_projection = scene.camera.view_projection(16.0 / 9.0);
                let ndc_y = |point: Vec3| {
                    let clip = view_projection * point.extend(1.0);
                    assert!(
                        clip.w > 0.0,
                        "azimuth {azimuth}, elevation {elevation}: probe {point:?} \
                         landed behind the camera"
                    );
                    (clip.truncate() / clip.w).y
                };

                // Halfway from the Sun towards the camera, in the plane.
                let towards_camera = Vec3::new(scene.camera.eye.x, 0.0, scene.camera.eye.z) * 0.5;
                let sun_y = ndc_y(Vec3::ZERO);
                let near_y = ndc_y(towards_camera);
                let far_y = ndc_y(-towards_camera);

                assert!(
                    near_y < sun_y,
                    "azimuth {azimuth}, elevation {elevation}: the near side of the \
                     ecliptic projected at y={near_y}, not below the Sun at y={sun_y}"
                );
                assert!(
                    far_y > sun_y,
                    "azimuth {azimuth}, elevation {elevation}: the far side of the \
                     ecliptic projected at y={far_y}, not above the Sun at y={sun_y}"
                );
            }
        }
    }

    /// The composition Jeroen locked on 2026-08-04, asserted as numbers.
    ///
    /// See `docs/TARGET-COMPOSITION.md` for the reference render and for why
    /// the outermost orbit deliberately overflows the bottom edge. Two rules
    /// this test exists to enforce:
    ///
    /// * It measures **orbits only**. Its deleted predecessor counted Kuiper
    ///   belt particles, which fill the frame on their own, and so reported a
    ///   healthy 0.86 while the orbits sat at 0.28 and the picture was wrong.
    /// * It excludes points already off the top or bottom, because the near arc
    ///   of the outer ellipse diverges as the camera closes in. It is the
    ///   visible silhouette that reads as "how big the system is".
    ///
    /// If the camera is re-parameterised, update how the numbers are *produced*
    /// but not the numbers themselves -- they are the target.
    #[test]
    fn the_locked_composition_still_holds() {
        const ASPECT: f32 = 3440.0 / 1440.0;

        let mut config = Config::default();
        config.camera.elevation_deg = 16.0;
        config.camera.frame_radius_au = 35.33;
        config.camera.offset_y = 0.27;
        config.camera.rotation_period_minutes = 0.0;

        let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, ASPECT);
        let view_projection = scene.camera.view_projection(ASPECT);
        let ndc_of = |point: Vec3| {
            let clip = view_projection * point.extend(1.0);
            (clip.w > 0.0).then(|| clip.truncate() / clip.w)
        };

        let sun_y = ndc_of(Vec3::ZERO)
            .expect("the Sun in front of the camera")
            .y;

        let outermost = scene.orbits.last().expect("an outermost orbit");
        let (mut visible_half_width, mut far_edge, mut near_arc) = (0.0f32, 0.0f32, 0.0f32);
        for point in outermost.points.iter() {
            let Some(ndc) = ndc_of(*point) else { continue };
            if ndc.y.abs() <= 1.0 {
                visible_half_width = visible_half_width.max(ndc.x.abs());
            }
            if ndc.y < sun_y {
                near_arc = near_arc.max(-ndc.y);
            } else {
                far_edge = far_edge.max(ndc.y);
            }
        }

        let close = |actual: f32, target: f32, tolerance: f32, what: &str| {
            assert!(
                (actual - target).abs() <= tolerance,
                "{what}: {actual:.3}, target {target:.3} +/- {tolerance}"
            );
        };
        close(
            (1.0 - sun_y) / 2.0,
            0.230,
            0.003,
            "Sun's distance from the top",
        );
        close(visible_half_width, 0.851, 0.02, "visible half-width");
        close(far_edge, 0.790, 0.02, "far edge above centre");
        close(near_arc, 0.902, 0.02, "near arc below centre");
        close(
            scene.camera.eye.distance(scene.camera.target),
            6.2334,
            0.01,
            "camera distance",
        );

        // Where the lowest *planet* falls is asserted by `the_golden_screen_shapes`,
        // which measures the same 3440x1440 frame against a silhouette sampled
        // over the sphere. This test used to check it here with
        // `radius / distance`, the on-axis small-angle approximation, and so
        // reported Neptune clearing by 16px when it in fact hangs 16px over.
    }

    /// Turning the system must be a rigid rotation, not a re-framing.
    ///
    /// Stick a pin through the Sun perpendicular to the ecliptic and turn it:
    /// a near-circular orbit projects to the same ellipse whatever the azimuth,
    /// and only the planets travel along it. Nothing should appear or disappear.
    ///
    /// It used to. The distance was re-solved every frame against a quantity
    /// that diverges as the camera closes in, so it settled somewhere different
    /// at every azimuth -- the camera crept 18% in and out over one rotation and
    /// swung whole stretches of the outer orbit off the bottom of the frame.
    #[test]
    fn one_rotation_does_not_re_frame_the_scene() {
        const ASPECT: f32 = 3440.0 / 1440.0;

        let (mut nearest, mut furthest) = (f32::MAX, f32::MIN);
        let (mut lowest_arc, mut worst_body, mut worst_azimuth) = (f32::MAX, f32::MAX, 0.0f32);

        for azimuth in (0..360).step_by(3).map(|d| d as f32) {
            let mut config = Config::default();
            config.camera.azimuth_deg = azimuth;
            config.camera.rotation_period_minutes = 0.0;

            let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, ASPECT);
            let distance = scene.camera.eye.distance(scene.camera.target);
            nearest = nearest.min(distance);
            furthest = furthest.max(distance);

            let view_projection = scene.camera.view_projection(ASPECT);
            let ndc_of = |point: Vec3| {
                let clip = view_projection * point.extend(1.0);
                (clip.w > 0.0).then(|| clip.truncate() / clip.w)
            };

            for point in scene
                .orbits
                .last()
                .expect("an outermost orbit")
                .points
                .iter()
            {
                if let Some(ndc) = ndc_of(*point) {
                    lowest_arc = lowest_arc.min(ndc.y);
                }
            }

            // The real projected silhouette, sampled over each sphere's surface.
            // `radius / distance` is the on-axis small-angle approximation and
            // understates a disc this far off-axis by a third of its own size.
            for body in &scene.bodies {
                for i in 0..256 {
                    let around = (i as f32 / 256.0) * std::f32::consts::TAU;
                    let along = ((i * 7 % 256) as f32 / 256.0) * std::f32::consts::PI;
                    let direction = Vec3::new(
                        along.sin() * around.cos(),
                        along.cos(),
                        along.sin() * around.sin(),
                    );
                    let Some(ndc) = ndc_of(body.position + direction * body.radius) else {
                        continue;
                    };
                    if ndc.y < worst_body {
                        worst_body = ndc.y;
                        worst_azimuth = azimuth;
                    }
                }
            }
        }

        assert!(
            furthest - nearest < 1e-3,
            "the camera distance moved between {nearest:.4} and {furthest:.4} over one \
             rotation; it must be solved once and reused, never re-solved per azimuth"
        );
        assert!(
            lowest_arc > -1.0,
            "the outermost orbit reached {lowest_arc:.4}, off the bottom of the frame"
        );

        // Neptune's disc overhangs the bottom edge slightly, and how far varies
        // with azimuth: 16px at azimuth 0, 19.9px at its worst around 357. That
        // graze was looked at in a native-resolution crop and accepted -- the
        // orbit line itself stays comfortably on frame, which is what carries
        // the composition. The bound is here to catch it growing, not to demand
        // it disappear.
        let overhang_px = -(worst_body + 1.0) * 720.0;
        assert!(
            overhang_px <= 24.0,
            "at azimuth {worst_azimuth} the lowest planet hangs {overhang_px:.1}px over the \
             bottom edge; up to 24px is the accepted graze"
        );
    }

    /// `frame_radius_au` must mean what it says: the heliocentric radius whose
    /// orbit is exactly as wide as the frame.
    ///
    /// Measured against the projected geometry by bisection, not against the
    /// formula that produced the camera -- otherwise this only asserts that the
    /// arithmetic was copied consistently, which is precisely the mistake that
    /// let the previous attempt optimise invented metrics.
    #[test]
    fn frame_radius_au_is_the_radius_that_lands_on_the_frame_edge() {
        for aspect in [3440.0 / 1440.0, 16.0 / 9.0, 4.0 / 3.0, 1.0] {
            for elevation in [5.0, 16.0, 45.0, 89.0] {
                for radius_au in [12.0, 35.33, 60.0] {
                    let mut config = Config::default();
                    config.camera.elevation_deg = elevation;
                    config.camera.frame_radius_au = radius_au;
                    config.camera.offset_x = 0.0;
                    config.camera.offset_y = 0.0;

                    let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, aspect);
                    let view_projection = scene.camera.view_projection(aspect);
                    // Widest this circle of radius `au` reaches across the frame.
                    let widest = |au: f64| {
                        let s = config.scale.orbit.apply(au) as f32;
                        (0..2048)
                            .map(|i| {
                                let t = std::f32::consts::TAU * i as f32 / 2048.0;
                                let clip = view_projection
                                    * Vec3::new(s * t.cos(), 0.0, s * t.sin()).extend(1.0);
                                if clip.w > 1e-6 {
                                    (clip.x / clip.w).abs()
                                } else {
                                    0.0
                                }
                            })
                            .fold(0.0f32, f32::max)
                    };
                    let (mut low, mut high) = (0.5, 500.0);
                    for _ in 0..50 {
                        let middle = 0.5 * (low + high);
                        if widest(middle) < 1.0 {
                            low = middle
                        } else {
                            high = middle
                        }
                    }
                    let found = 0.5 * (low + high);
                    assert!(
                        (found - radius_au as f64).abs() < radius_au as f64 * 0.005,
                        "aspect {aspect}, elevation {elevation}: asked for {radius_au} AU at \
                         the frame edge, measured {found:.3} AU"
                    );
                }
            }
        }
    }

    /// Larger radius, further back -- and by the amount the scale law implies,
    /// not merely in the right direction.
    #[test]
    fn a_larger_frame_radius_pulls_the_camera_back() {
        let mut config = Config::default();
        let near = Scene::build(&config, &Lookup::builtin(), EPOCH, 1.6)
            .camera
            .eye
            .length();
        config.camera.frame_radius_au *= 4.0;
        let far = Scene::build(&config, &Lookup::builtin(), EPOCH, 1.6)
            .camera
            .eye
            .length();
        // The distance is linear in `scale(frame_radius_au)`, and the shipped
        // scale law is r^0.45, so four times the radius is 4^0.45 = 1.866x.
        let expected = near * 4.0_f32.powf(0.45);
        assert!(
            (far - expected).abs() < expected * 0.001,
            "{far} vs the {expected} the scale law implies"
        );
    }

    #[test]
    fn switching_off_the_moon_and_the_belts_is_respected() {
        let mut config = Config::default();
        config.bodies.moon = false;
        config.bodies.asteroid_belt = false;
        config.bodies.kuiper_belt = false;
        let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, 1.6);
        assert!(!scene.bodies.iter().any(|b| b.name == "Moon"));
        assert!(scene.belts.is_empty());
        // The eight planets are not optional and are still all here.
        assert_eq!(scene.bodies.len(), DRAWN_PLANETS.len());
        assert_eq!(scene.orbits.len(), DRAWN_PLANETS.len());
    }

    /// The framing no longer reads the scene at all, so it cannot be made
    /// degenerate by what is in it — but it can still be handed a nonsense
    /// radius by a caller that skipped `Config::validate`.
    #[test]
    fn a_degenerate_frame_radius_does_not_produce_a_broken_camera() {
        for radius in [0.0, -1.0, f32::NAN] {
            let mut config = Config::default();
            config.camera.frame_radius_au = radius;
            let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, 1.6);
            assert!(
                scene.camera.eye.is_finite() && scene.camera.up.is_finite(),
                "frame_radius_au = {radius} gave eye {:?}",
                scene.camera.eye
            );
            assert!(scene.camera.view_projection(1.6).is_finite());
            // ...and `validate` is what stops it reaching here in the first place.
            assert!(config.validate().is_err(), "{radius} should be rejected");
        }
    }

    /// The cached path must be an optimisation and nothing else: same inputs,
    /// same scene, whether or not a cache sits in the middle.
    #[test]
    fn cached_and_uncached_scenes_are_identical() {
        let config = Config::default();
        let lookup = Lookup::builtin();
        let mut cache = SceneCache::new();

        let plain = Scene::build(&config, &lookup, EPOCH, 1.6);
        // Twice, so the second pass exercises the cache-hit path too.
        for _ in 0..2 {
            let cached = Scene::build_cached(
                &config,
                &lookup,
                EPOCH,
                1.6,
                config.camera.azimuth_deg,
                &mut cache,
            );
            for (a, b) in plain.bodies.iter().zip(&cached.bodies) {
                assert_eq!(a.name, b.name);
                assert_eq!(a.position, b.position, "{} moved", a.name);
                assert_eq!(a.orientation, b.orientation, "{} turned", a.name);
            }
            for (a, b) in plain.orbits.iter().zip(&cached.orbits) {
                assert_eq!(a.body_fraction, b.body_fraction);
                assert_eq!(a.points[..], b.points[..], "ring geometry differs");
            }
            for (a, b) in plain.belts.iter().zip(cached.belts.iter()) {
                assert_eq!(a.name, b.name);
                assert_eq!(a.particles.len(), b.particles.len());
                for (pa, pb) in a.particles.iter().zip(&b.particles) {
                    assert_eq!(pa.position, pb.position);
                }
            }
            assert_eq!(plain.camera.eye, cached.camera.eye);
        }
    }

    /// A cache hit must be a shared pointer, not a fresh copy — otherwise the
    /// cache saves the arithmetic but keeps the allocation churn.
    #[test]
    fn a_second_frame_shares_rather_than_rebuilds() {
        let config = Config::default();
        let lookup = Lookup::builtin();
        let mut cache = SceneCache::new();

        let first = Scene::build_cached(&config, &lookup, EPOCH, 1.6, 0.0, &mut cache);
        let second = Scene::build_cached(&config, &lookup, EPOCH, 1.6, 90.0, &mut cache);

        assert!(
            Arc::ptr_eq(&first.belts, &second.belts),
            "belts were rebuilt"
        );
        for (a, b) in first.orbits.iter().zip(&second.orbits) {
            assert!(
                Arc::ptr_eq(&a.points, &b.points),
                "ring points were rebuilt"
            );
        }
        assert_eq!(first.belts_generation, second.belts_generation);
        assert_eq!(first.orbits_generation, second.orbits_generation);
        // Generation 0 is reserved for the uncached path.
        assert!(first.belts_generation > 0 && first.orbits_generation > 0);
        // The azimuth override reached the camera even on the shared path.
        assert_ne!(first.camera.eye, second.camera.eye);
    }

    /// Moving the epoch far enough must rebuild the ring geometry — the orbits
    /// genuinely change as elements drift — while the belts, which depend on
    /// nothing time-varying, stay shared.
    #[test]
    fn epoch_movement_rebuilds_orbits_but_not_belts() {
        let config = Config::default();
        let lookup = Lookup::builtin();
        let mut cache = SceneCache::new();

        let now = Scene::build_cached(&config, &lookup, EPOCH, 1.6, 0.0, &mut cache);
        let later = Scene::build_cached(
            &config,
            &lookup,
            JulianDate(EPOCH.0 + 30.0),
            1.6,
            0.0,
            &mut cache,
        );

        assert_ne!(now.orbits_generation, later.orbits_generation);
        assert_eq!(now.belts_generation, later.belts_generation);
        assert!(Arc::ptr_eq(&now.belts, &later.belts));

        // Within the tolerance nothing rebuilds.
        let barely = Scene::build_cached(
            &config,
            &lookup,
            JulianDate(EPOCH.0 + 30.0 + ORBIT_CACHE_TOLERANCE_DAYS * 0.5),
            1.6,
            0.0,
            &mut cache,
        );
        assert_eq!(later.orbits_generation, barely.orbits_generation);
    }

    #[test]
    fn config_changes_rebuild_what_they_touch() {
        let mut config = Config::default();
        let lookup = Lookup::builtin();
        let mut cache = SceneCache::new();

        let before = Scene::build_cached(&config, &lookup, EPOCH, 1.6, 0.0, &mut cache);
        config.bodies.kuiper_belt = false;
        let after = Scene::build_cached(&config, &lookup, EPOCH, 1.6, 0.0, &mut cache);

        assert_ne!(before.belts_generation, after.belts_generation);
        assert_eq!(after.belts.len(), 1, "the Kuiper belt should be gone");
        // The ring geometry saw no relevant change.
        assert_eq!(before.orbits_generation, after.orbits_generation);
    }

    /// `invalidate` is the hook for when the position source itself changes —
    /// an almanac landing — which no key in the cache can see.
    #[test]
    fn invalidate_forces_a_full_rebuild() {
        let config = Config::default();
        let lookup = Lookup::builtin();
        let mut cache = SceneCache::new();

        let before = Scene::build_cached(&config, &lookup, EPOCH, 1.6, 0.0, &mut cache);
        cache.invalidate();
        let after = Scene::build_cached(&config, &lookup, EPOCH, 1.6, 0.0, &mut cache);

        assert_ne!(before.belts_generation, after.belts_generation);
        assert_ne!(before.orbits_generation, after.orbits_generation);
    }

    #[test]
    fn planets_advance_along_their_orbits_over_time() {
        let now = Scene::build(&Config::default(), &Lookup::builtin(), EPOCH, 1.6);
        let later = Scene::build(
            &Config::default(),
            &Lookup::builtin(),
            JulianDate(EPOCH.0 + 30.0),
            1.6,
        );
        let mercury_now = now.bodies.iter().find(|b| b.name == "Mercury").unwrap();
        let mercury_later = later.bodies.iter().find(|b| b.name == "Mercury").unwrap();
        // Mercury's year is 88 days, so 30 days is a large fraction of an orbit.
        assert!((mercury_later.position - mercury_now.position).length() > 0.1);
    }

    #[test]
    fn orientation_is_a_unit_quaternion_for_every_body() {
        let scene = scene();
        for body in std::iter::once(&scene.sun).chain(&scene.bodies) {
            assert!(
                (body.orientation.length() - 1.0).abs() < 1e-5,
                "{} has a non-unit orientation",
                body.name
            );
            assert!(
                body.position.is_finite() && body.radius > 0.0,
                "{}",
                body.name
            );
        }
    }
}

/// Does any planet ever fall further off the bottom of the frame than the graze
/// already accepted? Swept over a full Neptune orbit and a full camera rotation.
///
/// `docs/TARGET-COMPOSITION.md` used to say the planet clearance was verified
/// across azimuth but not across dates, and that a planet reaching the deepest
/// point of the outer orbit would sit tangent to the edge. Both are settled
/// here: it does reach it, and it hangs 35.6 px over rather than sitting tangent.
#[cfg(test)]
mod epoch_sweep {
    use super::*;
    use crate::config::Config;

    const ASPECT: f32 = 3440.0 / 1440.0;
    const HALF_HEIGHT_PX: f32 = 720.0;
    /// Neptune's year is 164.8 years, so this is one full circuit and a little.
    const YEARS: f64 = 170.0;
    const EPOCH: JulianDate = JulianDate(2_461_255.5);

    /// Lowest point of a body's projected silhouette.
    ///
    /// Minimising `(Q·up) / (Q·forward)` over the sphere puts the extremum where
    /// `Q − centre` lies in the plane spanned by the camera's up and forward
    /// axes. The whole answer is therefore on a single great circle, and
    /// sampling that beats scattering points over the sphere — which is what
    /// makes a sweep this size affordable.
    ///
    /// `radius / distance` would be cheaper still and is what this project used
    /// to do. It is the on-axis small-angle approximation, and understates a
    /// disc sitting 37° off axis by a third of its own size.
    fn lowest_edge(body: &BodyInstance, camera: &CameraState, view_projection: &Mat4) -> f32 {
        let forward = (camera.target - camera.eye).normalize_or(Vec3::NEG_Z);
        let mut lowest = f32::MAX;
        for i in 0..48 {
            let phi = std::f32::consts::TAU * i as f32 / 48.0;
            let point = body.position + (camera.up * phi.cos() + forward * phi.sin()) * body.radius;
            let clip = *view_projection * point.extend(1.0);
            if clip.w > 1e-6 {
                lowest = lowest.min(clip.y / clip.w);
            }
        }
        lowest
    }

    #[test]
    fn no_planet_falls_further_off_frame_over_a_whole_neptune_orbit() {
        let mut config = Config::default();
        // Belts are never measured for framing and are the bulk of the cost of
        // building a scene. Switching them off changes nothing else: the camera
        // is a closed form over one configured radius and never reads the scene.
        config.bodies.asteroid_belt = false;
        config.bodies.kuiper_belt = false;

        let (mut worst, mut worst_year, mut worst_azimuth, mut worst_body) =
            (f32::MAX, 0.0, 0.0, "");

        let mut year = 0.0;
        while year < YEARS {
            let epoch = JulianDate(EPOCH.0 + year * 365.25);
            // One scene per epoch. Azimuth moves the camera, not the planets,
            // so every azimuth reuses it.
            let scene = Scene::build(&config, &Lookup::builtin(), epoch, ASPECT);

            let mut azimuth = 0.0f32;
            while azimuth < 360.0 {
                let camera = frame_camera_with_azimuth(&config, ASPECT, azimuth);
                let view_projection = camera.view_projection(ASPECT);

                for body in &scene.bodies {
                    let clip = view_projection * body.position.extend(1.0);
                    // A disc spans at most 0.13 in normalised device coordinates,
                    // so anything centred above the middle cannot reach the
                    // bottom edge and does not need its silhouette walked.
                    if clip.w <= 1e-6 || clip.y / clip.w > 0.0 {
                        continue;
                    }
                    let lowest = lowest_edge(body, &camera, &view_projection);
                    if lowest < worst {
                        worst = lowest;
                        worst_year = year;
                        worst_azimuth = azimuth;
                        worst_body = body.name;
                    }
                }
                azimuth += 5.0;
            }
            year += 1.0;
        }

        let overhang_px = -(worst + 1.0) * HALF_HEIGHT_PX;
        assert!(
            overhang_px <= 42.0,
            "over {YEARS:.0} years and a full rotation, {worst_body} hangs {overhang_px:.1}px \
             over the bottom edge at year +{worst_year:.0}, azimuth {worst_azimuth} -- worse \
             than the 42px this composition was signed off for. Do not widen the framing to \
             hide it; report the number."
        );
        // ...and it really does get that close, so the bound has teeth.
        assert!(
            overhang_px >= 28.0,
            "the worst overhang is only {overhang_px:.1}px, where 35.6px was measured. \
             Something has changed the framing -- check before relaxing this."
        );
    }
}
