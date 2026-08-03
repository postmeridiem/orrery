// Orbit rings, drawn as screen-space-expanded polylines.
//
// Expanding in the vertex shader keeps every ring the same width in pixels no
// matter how far away it is, which is what makes Neptune's orbit as legible as
// Mercury's. Native line primitives cannot do sub-pixel widths or antialiasing,
// so each segment becomes a quad instead.

struct OrbitVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) colour: vec3<f32>,
    @location(1) brightness: f32,
    // Signed distance across the ribbon, -1..1, for the antialiased edge.
    @location(2) across: f32,
};

@vertex
fn vertex_main(
    @location(0) position: vec3<f32>,
    @location(1) neighbour: vec3<f32>,
    // x = which side of the line (-1 or +1), y = brightness.
    @location(2) params: vec2<f32>,
    @location(3) colour: vec3<f32>,
) -> OrbitVertex {
    let clip_here = globals.view_projection * vec4<f32>(position, 1.0);
    let clip_there = globals.view_projection * vec4<f32>(neighbour, 1.0);

    // Perspective divide, guarding against vertices at or behind the eye.
    let w_here = max(clip_here.w, 1e-6);
    let w_there = max(clip_there.w, 1e-6);
    let ndc_here = clip_here.xy / w_here;
    let ndc_there = clip_there.xy / w_there;

    // Work in pixels so the width is exact, then convert back.
    let aspect = vec2<f32>(globals.viewport.x, globals.viewport.y);
    let pixels_here = ndc_here * aspect;
    let pixels_there = ndc_there * aspect;

    var direction = pixels_there - pixels_here;
    let length_squared = dot(direction, direction);
    if (length_squared < 1e-12) {
        direction = vec2<f32>(1.0, 0.0);
    } else {
        direction = direction * inverseSqrt(length_squared);
    }
    let perpendicular = vec2<f32>(-direction.y, direction.x);

    // Half a pixel of extra width on each side gives the fragment shader room
    // to feather the edge.
    let half_width = max(globals.post.w, 0.5) * 0.5 + 0.5;
    let offset = perpendicular * params.x * half_width;

    var out: OrbitVertex;
    out.clip_position = vec4<f32>(
        (pixels_here + offset) / aspect * w_here,
        clip_here.z,
        w_here,
    );
    out.colour = colour;
    out.brightness = params.y;
    out.across = params.x;
    return out;
}

@fragment
fn fragment_main(in: OrbitVertex) -> @location(0) vec4<f32> {
    // Feather the ribbon edge. `across` spans the widened quad, so the solid
    // core sits inside and the outer half-pixel fades.
    let half_width = max(globals.post.w, 0.5) * 0.5 + 0.5;
    let distance_from_centre = abs(in.across) * half_width;
    let coverage = clamp(half_width - distance_from_centre, 0.0, 1.0);

    let alpha = coverage * in.brightness;
    return vec4<f32>(in.colour * alpha, alpha);
}
