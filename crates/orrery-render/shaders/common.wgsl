// Shared uniforms and noise, textually prepended to every other shader.

struct Globals {
    view_projection: mat4x4<f32>,
    inverse_view_projection: mat4x4<f32>,
    // xyz = camera position in scene units, w = elapsed seconds.
    camera: vec4<f32>,
    // xyz = the Sun's position (the origin), w = its drawn radius.
    sun: vec4<f32>,
    // star density, star brightness, milky way intensity, nebula intensity.
    sky_a: vec4<f32>,
    // ambient floor, cos(sky rotation), sin(sky rotation), seed.
    sky_b: vec4<f32>,
    // width, height, 1/width, 1/height.
    viewport: vec4<f32>,
    // exposure multiplier, bloom intensity, bloom radius, orbit width in pixels.
    post: vec4<f32>,
    // angular size of one pixel in radians, star core radius in pixels,
    // constellation opacity, deep-sky opacity.
    sky_c: vec4<f32>,
    // night-side brightness, night-side saturation, day-side saturation,
    // Sun emission intensity.
    lighting: vec4<f32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

const PI: f32 = 3.14159265359;
const TAU: f32 = 6.28318530718;

// The galactic north pole and galactic centre, expressed in the renderer's
// Y-up scene frame.
//
// Derived from the IAU values -- pole at RA 192.859 deg, Dec 27.128 deg;
// centre at RA 266.405 deg, Dec -28.936 deg -- rotated into ecliptic
// coordinates using an obliquity of 23.4393 deg, then mapped through the
// scene's (x, z, -y) convention. The upshot is that the Milky Way crosses the
// solar system at its true angle of about 60 degrees to the ecliptic, rather
// than at whatever angle looked nice.
const GALACTIC_POLE: vec3<f32> = vec3<f32>(-0.867654, 0.497155, 0.000427);
const GALACTIC_CENTRE: vec3<f32> = vec3<f32>(-0.054899, -0.096794, 0.993747);

// Hashing is integer-based rather than the usual `fract(sin(x) * 43758.5)`.
//
// That trick relies on sin() staying well-behaved for large arguments, and it
// does not: AMD's hardware sine has a limited input range, so once coordinates
// reach a few thousand it degenerates and returns the same value everywhere.
// This is not a subtle quality issue -- it silently produced a starfield with
// no stars in it. PCG on the integer lattice has no such range limit and is
// identical on every backend.

fn pcg3d(input: vec3<u32>) -> vec3<u32> {
    var v = input * 1664525u + vec3<u32>(1013904223u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v ^= v >> vec3<u32>(16u);
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return v;
}

const INVERSE_U32: f32 = 2.3283064e-10; // 1 / 2^32

/// Three uniform randoms in `0..1` for an integer lattice cell.
fn hash_cell3(cell: vec3<i32>, seed: u32) -> vec3<f32> {
    let keyed = bitcast<vec3<u32>>(cell)
        + vec3<u32>(seed, seed * 747796405u + 2891336453u, seed ^ 0x9E3779B9u);
    return vec3<f32>(pcg3d(keyed)) * INVERSE_U32;
}

/// One uniform random in `0..1` for an integer lattice cell.
fn hash_cell1(cell: vec3<i32>, seed: u32) -> f32 {
    return hash_cell3(cell, seed).x;
}

fn hash31(p: vec3<f32>) -> f32 {
    return hash_cell1(vec3<i32>(floor(p)), 0u);
}

fn hash33(p: vec3<f32>) -> vec3<f32> {
    return hash_cell3(vec3<i32>(floor(p)), 0u);
}

// Value noise with a smoothstep-interpolated lattice.
fn noise3(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);

    let n000 = hash31(i + vec3<f32>(0.0, 0.0, 0.0));
    let n100 = hash31(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash31(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash31(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash31(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash31(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash31(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash31(i + vec3<f32>(1.0, 1.0, 1.0));

    let x00 = mix(n000, n100, u.x);
    let x10 = mix(n010, n110, u.x);
    let x01 = mix(n001, n101, u.x);
    let x11 = mix(n011, n111, u.x);
    return mix(mix(x00, x10, u.y), mix(x01, x11, u.y), u.z);
}

fn fbm(p: vec3<f32>, octaves: i32) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var frequency = p;
    for (var i = 0; i < octaves; i = i + 1) {
        value = value + amplitude * noise3(frequency);
        frequency = frequency * 2.02;
        amplitude = amplitude * 0.5;
    }
    return value;
}

// Ridged noise, for the filamentary structure in nebulae.
fn ridged(p: vec3<f32>, octaves: i32) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var frequency = p;
    for (var i = 0; i < octaves; i = i + 1) {
        value = value + amplitude * (1.0 - abs(noise3(frequency) * 2.0 - 1.0));
        frequency = frequency * 2.1;
        amplitude = amplitude * 0.5;
    }
    return value;
}

// Rotate about the scene's up axis, so the generated sky can be spun without
// regenerating it.
fn rotate_about_y(v: vec3<f32>, cos_angle: f32, sin_angle: f32) -> vec3<f32> {
    return vec3<f32>(
        v.x * cos_angle + v.z * sin_angle,
        v.y,
        -v.x * sin_angle + v.z * cos_angle,
    );
}

// A very rough blackbody colour for a normalised temperature in 0..1, running
// from cool orange-red through white to hot blue.
fn star_tint(t: f32) -> vec3<f32> {
    let cool = vec3<f32>(1.0, 0.62, 0.38);
    let warm = vec3<f32>(1.0, 0.89, 0.76);
    let neutral = vec3<f32>(1.0, 1.0, 1.0);
    let hot = vec3<f32>(0.72, 0.80, 1.0);
    if (t < 0.35) {
        return mix(cool, warm, t / 0.35);
    } else if (t < 0.7) {
        return mix(warm, neutral, (t - 0.35) / 0.35);
    }
    return mix(neutral, hot, (t - 0.7) / 0.3);
}

// A full-screen triangle, larger than the viewport so it needs no clipping.
fn fullscreen_position(vertex_index: u32) -> vec4<f32> {
    let x = f32(i32(vertex_index) / 2) * 4.0 - 1.0;
    let y = f32(i32(vertex_index) & 1) * 4.0 - 1.0;
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn fullscreen_uv(vertex_index: u32) -> vec2<f32> {
    let x = f32(i32(vertex_index) / 2) * 2.0;
    let y = f32(i32(vertex_index) & 1) * 2.0;
    return vec2<f32>(x, 1.0 - y);
}
