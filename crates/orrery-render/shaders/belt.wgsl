// Debris belts: the main asteroid belt and the Kuiper belt.
//
// Drawn as individual camera-facing particles rather than a solid annulus.
// A torus of points reads as depth -- you can see the far side through the
// near side -- in a way a flat translucent band never does.

struct BeltParticle {
    // xyz = world position, w = drawn radius in scene units.
    position_size: vec4<f32>,
    // rgb = tint, a = brightness.
    colour: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> particles: array<BeltParticle>;

struct BeltVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) offset: vec2<f32>,
    @location(1) colour: vec3<f32>,
    @location(2) brightness: f32,
};

@vertex
fn vertex_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> BeltVertex {
    let particle = particles[instance_index];
    let centre = globals.view_projection * vec4<f32>(particle.position_size.xyz, 1.0);

    let corner = vec2<f32>(f32(vertex_index & 1u), f32(vertex_index >> 1u)) * 2.0 - 1.0;

    // Size in world units, but never allowed below a pixel and a half, or the
    // belt dissolves into aliasing shimmer as the camera drifts.
    let pixel_to_ndc = vec2<f32>(globals.viewport.z, globals.viewport.w) * 2.0;
    let world_ndc = particle.position_size.w / max(centre.w, 1e-6);
    let minimum = 1.5 * pixel_to_ndc.y;
    let radius = max(world_ndc, minimum);

    var out: BeltVertex;
    out.clip_position = vec4<f32>(
        centre.xy + corner * radius * vec2<f32>(1.0 / globals.viewport.x * globals.viewport.y, 1.0) * centre.w,
        centre.zw,
    );
    out.offset = corner;
    out.colour = particle.colour.rgb;
    out.brightness = particle.colour.a;
    return out;
}

@fragment
fn fragment_main(in: BeltVertex) -> @location(0) vec4<f32> {
    let falloff = exp(-dot(in.offset, in.offset) * 3.0);
    return vec4<f32>(in.colour * falloff * in.brightness, 1.0);
}
