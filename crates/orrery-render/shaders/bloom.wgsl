// Bloom, as a mip chain: progressive downsample, then progressive upsample
// with additive blending back up the chain.
//
// This is the filter from Jimenez's "Next Generation Post Processing in Call of
// Duty: Advanced Warfare" -- a 13-tap downsample that suppresses the flickering
// fireflies a naive box filter produces on small bright things, which for this
// scene means every single star.

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var source_sampler: sampler;

struct BlitVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) vertex_index: u32) -> BlitVertex {
    let x = f32(i32(vertex_index) / 2) * 4.0 - 1.0;
    let y = f32(i32(vertex_index) & 1) * 4.0 - 1.0;
    var out: BlitVertex;
    out.clip_position = vec4<f32>(x, y, 0.0, 1.0);
    out.uv = vec2<f32>((x + 1.0) * 0.5, 1.0 - (y + 1.0) * 0.5);
    return out;
}

fn tap(uv: vec2<f32>, offset: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(source, source_sampler, uv + offset, 0.0).rgb;
}

@fragment
fn downsample_main(in: BlitVertex) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(source, 0));
    let dx = texel.x;
    let dy = texel.y;

    // Inner square, weighted most heavily.
    let inner = tap(in.uv, vec2<f32>(-dx, -dy))
        + tap(in.uv, vec2<f32>(dx, -dy))
        + tap(in.uv, vec2<f32>(-dx, dy))
        + tap(in.uv, vec2<f32>(dx, dy));

    let centre = tap(in.uv, vec2<f32>(0.0, 0.0));
    let edges = tap(in.uv, vec2<f32>(-2.0 * dx, 0.0))
        + tap(in.uv, vec2<f32>(2.0 * dx, 0.0))
        + tap(in.uv, vec2<f32>(0.0, -2.0 * dy))
        + tap(in.uv, vec2<f32>(0.0, 2.0 * dy));
    let corners = tap(in.uv, vec2<f32>(-2.0 * dx, -2.0 * dy))
        + tap(in.uv, vec2<f32>(2.0 * dx, -2.0 * dy))
        + tap(in.uv, vec2<f32>(-2.0 * dx, 2.0 * dy))
        + tap(in.uv, vec2<f32>(2.0 * dx, 2.0 * dy));

    let colour = inner * 0.125
        + centre * 0.125
        + edges * 0.0625
        + corners * 0.03125;
    return vec4<f32>(colour, 1.0);
}

@fragment
fn upsample_main(in: BlitVertex) -> @location(0) vec4<f32> {
    // A 3x3 tent filter, spread by the configured radius.
    let texel = 1.0 / vec2<f32>(textureDimensions(source, 0));
    let radius = texel * 2.0;

    var colour = tap(in.uv, vec2<f32>(-radius.x, -radius.y)) * 1.0;
    colour = colour + tap(in.uv, vec2<f32>(0.0, -radius.y)) * 2.0;
    colour = colour + tap(in.uv, vec2<f32>(radius.x, -radius.y)) * 1.0;
    colour = colour + tap(in.uv, vec2<f32>(-radius.x, 0.0)) * 2.0;
    colour = colour + tap(in.uv, vec2<f32>(0.0, 0.0)) * 4.0;
    colour = colour + tap(in.uv, vec2<f32>(radius.x, 0.0)) * 2.0;
    colour = colour + tap(in.uv, vec2<f32>(-radius.x, radius.y)) * 1.0;
    colour = colour + tap(in.uv, vec2<f32>(0.0, radius.y)) * 2.0;
    colour = colour + tap(in.uv, vec2<f32>(radius.x, radius.y)) * 1.0;

    return vec4<f32>(colour / 16.0, 1.0);
}
