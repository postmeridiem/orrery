// Final composite: scene + bloom, exposed, tone mapped and dithered.

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var bloom: texture_2d<f32>;
@group(0) @binding(2) var linear_sampler: sampler;

struct Settings {
    // exposure, bloom intensity, unused, unused.
    values: vec4<f32>,
};
@group(0) @binding(3) var<uniform> settings: Settings;

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

// Narkowicz's ACES approximation: cheap, and it rolls highlights off without
// the magenta shift a naive Reinhard gives on a bright white Sun.
fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Interleaved gradient noise, used to dither away the banding that otherwise
// shows in the huge smooth gradients of a starfield at 8 bits per channel.
fn dither(position: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(position, vec2<f32>(0.06711056, 0.00583715))));
}

@fragment
fn fragment_main(in: BlitVertex) -> @location(0) vec4<f32> {
    let scene_colour = textureSampleLevel(scene, linear_sampler, in.uv, 0.0).rgb;
    let bloom_colour = textureSampleLevel(bloom, linear_sampler, in.uv, 0.0).rgb;

    var colour = scene_colour + bloom_colour * settings.values.y;
    colour = colour * settings.values.x;
    colour = aces(colour);

    // The swapchain is sRGB, so the hardware handles the transfer function;
    // dither in linear space just below one 8-bit step.
    colour = colour + (dither(in.clip_position.xy) - 0.5) / 255.0;
    return vec4<f32>(colour, 1.0);
}
