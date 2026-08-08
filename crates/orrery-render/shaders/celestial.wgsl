// The real sky: catalogued stars and constellation figures.
//
// Both are billboards on a sphere at effectively infinite distance, drawn
// after the procedural background and before the planets, with depth writes off
// so the solar system always occludes them.

/// How far out the celestial sphere is placed, in scene units. Large enough
/// that camera motion cannot parallax it, small enough to keep f32 precise.
const SKY_DISTANCE: f32 = 1.0e5;

/// Magnitude that renders at unit intensity. Every step of 5 magnitudes is a
/// factor of 100 in brightness, which HDR and bloom handle from here.
const REFERENCE_MAGNITUDE: f32 = 5.2;

struct Star {
    // xyz = unit direction, w = visual magnitude.
    direction_magnitude: vec4<f32>,
    // rgb = linear colour, a unused.
    colour: vec4<f32>,
};

struct Segment {
    endpoint_a: vec4<f32>,
    endpoint_b: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> stars: array<Star>;
@group(1) @binding(1) var<storage, read> segments: array<Segment>;

/// The four corners of a unit quad, as a triangle strip.
fn quad_corner(vertex_index: u32) -> vec2<f32> {
    return vec2<f32>(f32(vertex_index & 1u), f32(vertex_index >> 1u)) * 2.0 - 1.0;
}

/// Place a direction on the celestial sphere and project it.
fn project_sky(direction: vec3<f32>) -> vec4<f32> {
    let world = globals.camera.xyz + direction * SKY_DISTANCE;
    return globals.view_projection * vec4<f32>(world, 1.0);
}

/// Rotate a catalogue direction by the configured sky rotation.
fn oriented(direction: vec3<f32>) -> vec3<f32> {
    return rotate_about_y(direction, globals.sky_b.y, globals.sky_b.z);
}

// --- stars ------------------------------------------------------------------

struct StarVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) offset: vec2<f32>,
    @location(1) colour: vec3<f32>,
    @location(2) intensity: f32,
};

@vertex
fn star_vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> StarVertex {
    let star = stars[instance_index];
    let magnitude = star.direction_magnitude.w;

    let centre = project_sky(oriented(star.direction_magnitude.xyz));

    // Brightness follows the real magnitude scale, so Sirius genuinely is
    // hundreds of times brighter than a sixth-magnitude star. The disc barely
    // grows -- bloom is what makes bright stars read as bright.
    let intensity = pow(10.0, -0.4 * (magnitude - REFERENCE_MAGNITUDE));
    let radius_pixels = 0.9 + 0.42 * max(0.0, 3.2 - magnitude);

    let corner = quad_corner(vertex_index);
    let pixel_to_ndc = vec2<f32>(globals.viewport.z, globals.viewport.w) * 2.0;
    let offset = corner * radius_pixels * pixel_to_ndc * centre.w;

    var out: StarVertex;
    out.clip_position = vec4<f32>(centre.xy + offset, centre.zw);
    out.offset = corner;
    out.colour = star.colour.rgb;
    out.intensity = intensity;
    return out;
}

@fragment
fn star_fragment(in: StarVertex) -> @location(0) vec4<f32> {
    // A tight Gaussian; anything harder-edged aliases into a flickering square.
    let falloff = exp(-dot(in.offset, in.offset) * 3.6);
    let brightness = in.intensity * falloff * globals.sky_a.y;
    return vec4<f32>(in.colour * brightness, 1.0);
}

// --- constellation figures --------------------------------------------------

struct LineVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) across: f32,
};

@vertex
fn constellation_vertex(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> LineVertex {
    let segment = segments[instance_index];
    let a = project_sky(oriented(segment.endpoint_a.xyz));
    let b = project_sky(oriented(segment.endpoint_b.xyz));

    var out: LineVertex;

    // Cull any segment with an endpoint at or behind the camera.
    //
    // The celestial sphere surrounds the viewer, so about half of it is behind
    // the camera at any moment and plenty of segments straddle the plane.
    // Clamping a negative w to a small positive one -- which is what this used
    // to do -- projects that endpoint to a garbage coordinate, and the quad
    // then runs from a valid on-screen vertex to a nonsensical one. The
    // near-plane clip stretches the result clean across the frame, which is
    // where the stray lines came from.
    //
    // Nothing is lost by dropping them: an endpoint behind the camera is more
    // than 90 degrees off-axis, and the field of view is 38.
    if (a.w <= 1e-4 || b.w <= 1e-4) {
        // Outside the depth range, so it is clipped rather than drawn.
        out.clip_position = vec4<f32>(0.0, 0.0, -1.0, 1.0);
        out.across = 0.0;
        return out;
    }

    let viewport = vec2<f32>(globals.viewport.x, globals.viewport.y);
    let pixels_a = a.xy / a.w * viewport;
    let pixels_b = b.xy / b.w * viewport;

    var along = pixels_b - pixels_a;
    let length_squared = dot(along, along);
    if (length_squared < 1e-12) {
        along = vec2<f32>(1.0, 0.0);
    } else {
        along = along * inverseSqrt(length_squared);
    }
    let across = vec2<f32>(-along.y, along.x);

    // Corner x selects the endpoint, corner y which side of the ribbon.
    let corner = quad_corner(vertex_index);
    let endpoint = select(pixels_a, pixels_b, corner.x > 0.0);
    let clip = select(a, b, corner.x > 0.0);

    let half_width = 0.75;
    let position = endpoint + across * corner.y * half_width;

    out.clip_position = vec4<f32>(position / viewport * clip.w, clip.z, clip.w);
    out.across = corner.y;
    return out;
}

@fragment
fn constellation_fragment(in: LineVertex) -> @location(0) vec4<f32> {
    // Feather the edge, then apply the (deliberately tiny) opacity.
    let coverage = 1.0 - smoothstep(0.35, 1.0, abs(in.across));
    let brightness = coverage * globals.sky_c.z;
    // A cool grey-blue, so the figures read as drawn lines rather than as
    // anything astronomical.
    return vec4<f32>(vec3<f32>(0.52, 0.64, 0.85) * brightness, 1.0);
}
