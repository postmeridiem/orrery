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

use glam::{DVec3, Mat4, Quat, Vec3};

use crate::bodies::{self, BodyData, Rings};
use crate::config::Config;
use crate::ephemeris::{self, Planet};
use crate::lookup::Lookup;
use crate::time::JulianDate;

/// Convert an ecliptic-frame vector to the renderer's Y-up frame.
pub fn ecliptic_to_scene(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.z as f32, -v.y as f32)
}

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
#[derive(Debug, Clone)]
pub struct OrbitRing {
    pub points: Vec<Vec3>,
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

/// Scatter `count` particles through a torus between `inner_au` and `outer_au`.
fn build_belt(
    name: &'static str,
    inner_au: f64,
    outer_au: f64,
    thickness_au: f64,
    count: u32,
    seed: u32,
    color: [f32; 3],
    brightness: f32,
    scale: &crate::scale::RadialScale,
) -> Belt {
    let brightness_scale = brightness;
    let particles = (0..count)
        .map(|index| {
            // Radius is biased toward the middle of the belt, which is roughly
            // how the real population is distributed.
            let u = random_unit(seed, index, 0);
            let v = random_unit(seed, index, 1);
            let bias = (u + v) * 0.5;
            let radius = inner_au + (outer_au - inner_au) * bias;

            let angle = random_unit(seed, index, 2) * std::f64::consts::TAU;
            // Two samples summed approximate a normal distribution, so the belt
            // is concentrated near the ecliptic with a scattered tail.
            let height = (random_unit(seed, index, 3) + random_unit(seed, index, 4) - 1.0)
                * thickness_au;

            let position_au = DVec3::new(
                radius * angle.cos(),
                radius * angle.sin(),
                height,
            );
            let scaled = scale.apply_to_position(position_au);

            // Faint at the edges, brighter through the middle.
            let edge = ((bias - 0.5).abs() * 2.0).clamp(0.0, 1.0);
            let brightness = (1.0 - edge * edge) as f32;

            BeltParticle {
                position: ecliptic_to_scene(scaled),
                size: (0.0016 + 0.0022 * random_unit(seed, index, 5)) as f32,
                brightness: (0.25 + 0.75 * brightness) * brightness_scale,
            }
        })
        .collect();

    Belt {
        name,
        particles,
        color,
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
}

impl CameraState {
    pub fn view(&self) -> Mat4 {
        glam::camera::rh::view::look_at_mat4(self.eye, self.target, self.up)
    }

    /// Reversed-Z infinite perspective. Reversed Z buys float depth precision
    /// across the enormous near/far ratio an orrery spans; the renderer pairs
    /// it with a `GreaterEqual` depth test and a clear value of 0.
    pub fn projection(&self, aspect: f32) -> Mat4 {
        glam::camera::rh::proj::directx::perspective_infinite_reverse(
            self.fov_y_radians,
            aspect,
            self.near,
        )
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
    /// Radius of the outermost drawn orbit, in scene units.
    pub extent: f32,
    /// Debris belts, outermost last.
    pub belts: Vec<Belt>,
}

impl Scene {
    /// Build the scene for `epoch` under `config`, framed for `aspect`
    /// (width / height).
    ///
    /// `lookup` decides where positions come from: a Horizons almanac when one
    /// covers this instant, the built-in tables otherwise.
    pub fn build(config: &Config, lookup: &Lookup, epoch: JulianDate, aspect: f32) -> Self {
        let planets = config.bodies.resolve().unwrap_or_default();
        let orbit_scale = &config.scale.orbit;
        let body_scale = &config.scale.body;

        let mut bodies_out = Vec::with_capacity(planets.len() + 1);
        let mut orbits_out = Vec::with_capacity(planets.len());
        let mut extent: f32 = 0.0;

        for (index, planet) in planets.iter().copied().enumerate() {
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

            if config.orbits.enabled {
                let ring = build_orbit_ring(
                    &elements,
                    config.orbits.segments,
                    orbit_scale,
                    data.color,
                );
                extent = extent.max(
                    ring.points
                        .iter()
                        .fold(0.0_f32, |acc, p| acc.max(p.length())),
                );
                orbits_out.push(ring);
            }
            extent = extent.max(position.length());

            // The Moon rides along with Earth.
            if planet == Planet::Earth && config.bodies.moon {
                let moon_offset_au = ephemeris::geocentric_moon_position(epoch);
                // Under any useful compression the true separation collapses to
                // nothing, so it is deliberately exaggerated. The *direction* is
                // still the real one, so the phase and the side it sits on read
                // correctly.
                let boost = config.bodies.moon_distance_boost.max(0.0) as f64;
                let offset = ecliptic_to_scene(
                    orbit_scale.apply_to_position(position_au + moon_offset_au) -
                        orbit_scale.apply_to_position(position_au),
                ) * boost.max(1.0) as f32;
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

        // Guard against a config that draws nothing at all.
        if extent <= f32::EPSILON {
            extent = sun.radius.max(0.1) * 4.0;
        }

        let mut belts = Vec::new();
        if config.bodies.asteroid_belt {
            // The main belt runs roughly 2.1 to 3.3 AU, between Mars and Jupiter.
            belts.push(build_belt(
                "Asteroid Belt",
                2.1,
                3.3,
                0.10,
                config.bodies.belt_particles,
                0xA57E_201D,
                [0.72, 0.66, 0.56],
                // The real main belt is invisible from anywhere. Drawn dense
                // and additive it piles up into a solid glowing ring that
                // out-shouts the Sun, so it is kept to a suggestion.
                0.16,
                orbit_scale,
            ));
        }
        if config.bodies.kuiper_belt {
            // The classical Kuiper belt runs from Neptune's orbit out to the
            // 2:1 resonance at about 48 AU, and is far thicker than the main
            // belt.
            let kuiper = build_belt(
                "Kuiper Belt",
                30.0,
                48.0,
                2.4,
                (config.bodies.belt_particles as f32 * 1.6) as u32,
                0x4B1D_9E37,
                [0.62, 0.70, 0.82],
                // Spread over a far larger area, so it survives being brighter.
                0.55,
                orbit_scale,
            );
            for particle in &kuiper.particles {
                extent = extent.max(particle.position.length());
            }
            belts.push(kuiper);
        }

        // Everything the framing has to keep on screen. Bodies contribute the
        // extremes of their disc, not just their centre, so a planet is never
        // half off the edge.
        let framing_points: Vec<Vec3> = orbits_out
            .iter()
            .flat_map(|ring| ring.points.iter().copied())
            // Belts are deliberately absent. Their near edge sits far closer to
            // the camera than anything else and projects enormously, so making
            // the framing contain it retreats the camera until the planets are
            // specks. They are meant to run off the edges.
            .chain(bodies_out.iter().flat_map(|body| {
                [Vec3::X, Vec3::Y, Vec3::Z]
                    .into_iter()
                    .flat_map(move |axis| {
                        [
                            body.position + axis * body.radius,
                            body.position - axis * body.radius,
                        ]
                    })
            }))
            .collect();

        let camera = frame_camera(config, &framing_points, extent, aspect);

        Scene {
            epoch,
            sun,
            bodies: bodies_out,
            orbits: orbits_out,
            camera,
            extent,
            belts,
        }
    }
}

/// Sample a full orbit by sweeping eccentric anomaly, which distributes points
/// evenly around the *ellipse* rather than clustering them at perihelion.
fn build_orbit_ring(
    elements: &ephemeris::Kepler,
    segments: u32,
    scale: &crate::scale::RadialScale,
    color: [f32; 3],
) -> OrbitRing {
    let segments = segments.max(16);
    let points = (0..segments)
        .map(|i| {
            let eccentric_anomaly = 360.0 * i as f64 / segments as f64;
            ecliptic_to_scene(
                scale.apply_to_position(
                    elements.position_at_eccentric_anomaly(eccentric_anomaly),
                ),
            )
        })
        .collect();

    let body_fraction =
        (ephemeris::solve_kepler(elements.mean_anomaly(), elements.e).rem_euclid(360.0) / 360.0)
            as f32;

    OrbitRing {
        points,
        color,
        body_fraction,
    }
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
    let spin = if period_days.abs() > f64::EPSILON {
        (std::f64::consts::TAU * days / period_days).rem_euclid(std::f64::consts::TAU)
    } else {
        0.0
    };
    // Tilt about scene +X (ecliptic longitude 0), then spin about the tilted
    // pole, which after the tilt is the body's own +Y.
    let tilt = Quat::from_rotation_x(data.axial_tilt_deg.to_radians() as f32);
    tilt * Quat::from_rotation_y(spin as f32)
}

/// Place the camera so the system fills the configured fraction of the frame.
///
/// The framing distance is solved numerically rather than in closed form. A
/// closed form has to approximate the solar system as a flat disc of radius
/// `extent`, which ignores perspective foreshortening — the near side of an
/// orbit projects larger than the far side, so the approximation overflows the
/// frame on wide aspect ratios at shallow elevations. Measuring the real
/// projected geometry instead makes `fill` an exact promise at every aspect
/// ratio and camera angle.
///
/// Projected size falls off as roughly `1/distance`, so scaling the distance by
/// the measured overshoot is very nearly a Newton step and converges in a
/// handful of iterations.
fn frame_camera(config: &Config, points: &[Vec3], extent: f32, aspect: f32) -> CameraState {
    let camera = &config.camera;
    let aspect = aspect.max(f32::EPSILON);
    let fill = camera.fill.clamp(0.05, 1.0);

    // The elevation is used exactly as configured. Nothing here adjusts it.
    //
    // There used to be a search that stepped the elevation down until the scene
    // fitted vertically. It meant the configured angle was not the angle you
    // got -- the picture ended up at whichever step the loop stopped on, that
    // step moved as the camera orbited, and cropping in changed it again. The
    // distance adapts to fit the frame; the tilt is the user's to set.
    solve_at_elevation(config, points, extent, aspect, camera.elevation_deg, fill).0
}

/// Place the camera at a given elevation and solve its distance.
///
/// Returns the camera plus the widest horizontal and vertical extents the
/// geometry reaches, in normalised device coordinates.
fn solve_at_elevation(
    config: &Config,
    points: &[Vec3],
    extent: f32,
    aspect: f32,
    elevation_deg: f32,
    fill: f32,
) -> (CameraState, f32, f32) {
    const MAX_ITERATIONS: u32 = 40;
    const TOLERANCE: f32 = 1e-3;

    let camera = &config.camera;
    let elevation = elevation_deg.to_radians();
    let azimuth = camera.azimuth_deg.to_radians();
    let fov_y = camera.fov_deg.to_radians();

    let direction = Vec3::new(
        elevation.cos() * azimuth.cos(),
        elevation.sin(),
        elevation.cos() * azimuth.sin(),
    )
    .normalize_or(Vec3::Y);

    let forward = -direction;
    let right = forward.cross(Vec3::Y).normalize_or(Vec3::X);
    let up = right.cross(forward).normalize_or(Vec3::Y);
    let target = right * (camera.offset_x * extent) + up * (camera.offset_y * extent);
    let roll = Quat::from_axis_angle(forward, camera.roll_deg.to_radians());

    let minimum_distance = extent * 1.05;
    let at_distance = |distance: f32| CameraState {
        eye: target + direction * distance,
        target,
        up: roll * up,
        fov_y_radians: fov_y,
        near: (distance * 0.001).max(1e-4),
        far: distance * 10.0,
    };

    /// Widest the geometry reaches on each axis, or `None` if any is behind.
    fn measure(camera: &CameraState, points: &[Vec3], aspect: f32) -> Option<(f32, f32)> {
        let view_projection = camera.view_projection(aspect);
        let (mut widest_x, mut widest_y) = (0.0_f32, 0.0_f32);
        for point in points {
            let clip = view_projection * point.extend(1.0);
            if clip.w <= 1e-6 {
                return None;
            }
            let ndc = clip.truncate() / clip.w;
            widest_x = widest_x.max(ndc.x.abs());
            widest_y = widest_y.max(ndc.y.abs());
        }
        Some((widest_x, widest_y))
    }

    let half_fov_y = (fov_y * 0.5).tan();
    let half_fov_x = half_fov_y * aspect;
    let mut distance = (extent / (fill * half_fov_x))
        .max(extent * elevation.sin().abs().max(0.05) / (fill * half_fov_y))
        .max(minimum_distance);

    if points.is_empty() {
        return (at_distance(distance * camera.zoom.max(0.01)), 0.0, 0.0);
    }

    // Which axis the fit targets. Fitting the width uses only the horizontal
    // extent; otherwise whichever axis binds first.
    let measure_target =
        |x: f32, y: f32| if config.camera.fit_width { x } else { x.max(y) };

    for _ in 0..MAX_ITERATIONS {
        match measure(&at_distance(distance), points, aspect) {
            None => distance = (distance * 1.5).max(minimum_distance),
            Some((x, y)) => {
                let widest = measure_target(x, y);
                if widest <= f32::EPSILON {
                    break;
                }
                let correction = widest / fill;
                if (correction - 1.0).abs() < TOLERANCE {
                    break;
                }
                distance = (distance * correction).max(minimum_distance);
            }
        }
    }

    // Expand until it genuinely fits, so nothing is ever clipped.
    //
    // Scaled by the measured overshoot rather than by a fixed small step: a 1%
    // step can only grow the distance by about half over the iteration budget
    // and then gives up silently, which left the scene clipped at steep
    // elevations on very wide screens.
    for _ in 0..MAX_ITERATIONS {
        let Some((x, y)) = measure(&at_distance(distance), points, aspect) else {
            distance = (distance * 1.5).max(minimum_distance);
            continue;
        };
        let overshoot = (measure_target(x, y) / fill).max(x).max(y);
        if overshoot <= 1.0 + TOLERANCE {
            break;
        }
        distance = (distance * overshoot * 1.001).max(minimum_distance);
    }

    // Measure the framing *before* zoom, and apply zoom only to the camera
    // handed back.
    //
    // The elevation search above reacts to these numbers: if it sees the
    // zoomed-in extents it reads them as the scene not fitting and keeps
    // flattening the angle to compensate, so cropping in silently changes the
    // tilt. Cropping must not do that -- it is a scale change and nothing else.
    let framed = at_distance(distance);
    let (x, y) = measure(&framed, points, aspect).unwrap_or((0.0, 0.0));
    (at_distance(distance * camera.zoom.max(0.01)), x, y)
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
        let planets: Vec<_> = Config::default().bodies.resolve().unwrap();
        for (planet, ring) in planets.iter().zip(&scene.orbits) {
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
            let chord = 2.0 * std::f32::consts::PI * body.position.length()
                / Config::default().orbits.segments as f32;
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
        let planets: Vec<_> = Config::default().bodies.resolve().unwrap();
        for (planet, ring) in planets.iter().zip(&scene.orbits) {
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

    /// The whole system must actually be inside the frustum, at a range of
    /// aspect ratios and camera angles. This is the test that catches framing
    /// regressions on ultrawide and portrait monitors.
    #[test]
    fn everything_is_inside_the_frustum() {
        for aspect in [32.0 / 9.0, 16.0 / 9.0, 4.0 / 3.0, 1.0, 9.0 / 16.0] {
            for elevation in [5.0, 27.0, 60.0, 89.0] {
                let mut config = Config::default();
                config.camera.elevation_deg = elevation;
                // This tests the *framing*, which promises the scene fits. Zoom
                // is applied on top of that and crops in deliberately, so it is
                // held at 1 here.
                config.camera.zoom = 1.0;
                let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, aspect);
                let view_projection = scene.camera.view_projection(aspect);

                let mut checked = 0;
                for point in scene
                    .orbits
                    .iter()
                    .flat_map(|o| o.points.iter().copied())
                    .chain(scene.bodies.iter().map(|b| b.position))
                {
                    let clip = view_projection * point.extend(1.0);
                    assert!(clip.w > 0.0, "behind the camera at aspect {aspect}");
                    let ndc = clip.truncate() / clip.w;
                    assert!(
                        ndc.x.abs() <= 1.0 && ndc.y.abs() <= 1.0,
                        "aspect {aspect}, elevation {elevation}: {ndc:?} outside the frame"
                    );
                    checked += 1;
                }
                assert!(checked > 100);
            }
        }
    }

    /// `fill` is a promise about how much of the frame gets used; a scene that
    /// only fills a tenth of the screen is as wrong as one that overflows.
    ///
    /// Bounded to ordinary aspect ratios. The elevation is now used exactly as
    /// configured -- nothing adjusts it to make things fit -- so on a very wide
    /// screen a steep angle spreads the scene vertically and the camera has to
    /// retreat to contain it, leaving width unused. That is the deliberate
    /// trade: the tilt is the user's to set, and the distance adapts around it.
    #[test]
    fn framing_actually_fills_the_frame() {
        for aspect in [3440.0 / 1440.0, 16.0 / 9.0, 1.0, 9.0 / 16.0] {
            let config = Config::default();
            let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, aspect);
            let view_projection = scene.camera.view_projection(aspect);
            // Belts count: with the Kuiper belt on it, not Neptune's orbit, is
            // the outermost thing in the scene.
            let widest = scene
                .orbits
                .iter()
                .flat_map(|o| o.points.iter().copied())
                .chain(
                    scene
                        .belts
                        .iter()
                        .flat_map(|b| b.particles.iter().map(|p| p.position)),
                )
                .map(|p| {
                    let clip = view_projection * p.extend(1.0);
                    let ndc = clip.truncate() / clip.w;
                    ndc.x.abs().max(ndc.y.abs())
                })
                .fold(0.0_f32, f32::max);
            assert!(
                widest > 0.8,
                "aspect {aspect}: the scene only reaches {widest} of the frame"
            );
        }
    }

    #[test]
    fn zoom_pulls_the_camera_back() {
        let mut config = Config::default();
        let near = Scene::build(&config, &Lookup::builtin(), EPOCH, 1.6).camera.eye.length();
        config.camera.zoom = 2.0;
        let far = Scene::build(&config, &Lookup::builtin(), EPOCH, 1.6).camera.eye.length();
        assert!(far > near * 1.9, "{far} vs {near}");
    }

    #[test]
    fn disabling_orbits_and_moon_is_respected() {
        let mut config = Config::default();
        config.orbits.enabled = false;
        config.bodies.moon = false;
        config.bodies.asteroid_belt = false;
        config.bodies.kuiper_belt = false;
        let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, 1.6);
        assert!(scene.orbits.is_empty());
        assert!(!scene.bodies.iter().any(|b| b.name == "Moon"));
        assert!(scene.belts.is_empty());
        // With no rings to frame against, the planets themselves must still fit.
        assert!(scene.extent > 0.0);
    }

    /// An empty body list must not produce a degenerate camera.
    #[test]
    fn empty_scene_is_still_well_formed() {
        let mut config = Config::default();
        config.bodies.show = Vec::new();
        config.bodies.moon = false;
        let scene = Scene::build(&config, &Lookup::builtin(), EPOCH, 1.6);
        assert!(scene.bodies.is_empty());
        assert!(scene.extent.is_finite() && scene.extent > 0.0);
        assert!(scene.camera.eye.is_finite());
        assert!(scene.camera.eye.length() > scene.sun.radius);
    }

    #[test]
    fn planets_advance_along_their_orbits_over_time() {
        let now = Scene::build(&Config::default(), &Lookup::builtin(), EPOCH, 1.6);
        let later = Scene::build(&Config::default(), &Lookup::builtin(), JulianDate(EPOCH.0 + 30.0), 1.6);
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
            assert!(body.position.is_finite() && body.radius > 0.0, "{}", body.name);
        }
    }
}
