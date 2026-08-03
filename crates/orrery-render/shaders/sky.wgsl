// The stellar background: a procedural starfield, the galactic band and
// nebulosity, evaluated per pixel from the view ray. No textures, no assets.

struct SkyVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vertex_main(@builtin(vertex_index) vertex_index: u32) -> SkyVertex {
    var out: SkyVertex;
    out.clip_position = fullscreen_position(vertex_index);
    out.ndc = out.clip_position.xy;
    return out;
}

// Recover the world-space view direction for a pixel by unprojecting it.
fn view_ray(ndc: vec2<f32>) -> vec3<f32> {
    // Reversed-Z: the far plane is at depth 0.
    let near = globals.inverse_view_projection * vec4<f32>(ndc, 1.0, 1.0);
    let far = globals.inverse_view_projection * vec4<f32>(ndc, 0.0001, 1.0);
    return normalize(far.xyz / far.w - near.xyz / near.w);
}

// One scale of stars. Cells are sampled on a shell of radius `scale`, so the
// 27 neighbours around the view ray are exactly the candidates whose star could
// land near this pixel.
fn star_layer(
    dir: vec3<f32>,
    scale: f32,
    seed: u32,
    threshold: f32,
    core_radius: f32,
) -> vec3<f32> {
    let base = vec3<i32>(floor(dir * scale));
    var accumulated = vec3<f32>(0.0);

    for (var i = -1; i <= 1; i = i + 1) {
        for (var j = -1; j <= 1; j = j + 1) {
            for (var k = -1; k <= 1; k = k + 1) {
                let cell = base + vec3<i32>(i, j, k);
                let random = hash_cell3(cell, seed);

                // Most cells hold no star; `threshold` is what density controls.
                if (random.x > threshold) {
                    continue;
                }

                // A second draw places the star inside its cell, so position is
                // independent of whether the cell is occupied at all.
                let offset = hash_cell3(cell, seed ^ 0x51ED270Bu);
                let star_direction = normalize(vec3<f32>(cell) + offset);

                // Angular separation in radians. Sizing the star against an
                // absolute angle rather than against the cell is what keeps
                // every layer the same apparent size -- defining it per cell
                // made the fine layers sub-pixel and the sky turned to noise.
                let separation = length(dir - star_direction) / core_radius;

                // A tight core plus a faint halo; bloom widens it from there.
                let core = exp(-separation * separation);
                // The halo must stay tight. A wide one turns every star into a
                // visible blob and the field reads as a globular cluster
                // rather than a sky.
                let halo = exp(-separation * separation * 0.30) * 0.07;

                // Skew the magnitude distribution hard, so the sky is mostly
                // faint stars with a scattering of bright ones, as the real one is.
                let magnitude = 0.025 + 0.975 * pow(offset.y, 3.0);
                accumulated = accumulated + star_tint(offset.z) * (core + halo) * magnitude;
            }
        }
    }
    return accumulated;
}

// Distance from the galactic plane, in radians of galactic latitude.
fn galactic_latitude(dir: vec3<f32>) -> f32 {
    return asin(clamp(dot(dir, GALACTIC_POLE), -1.0, 1.0));
}

fn milky_way(dir: vec3<f32>, seed: f32) -> vec3<f32> {
    let latitude = galactic_latitude(dir);

    // The band itself: a Gaussian in galactic latitude, brightest toward the
    // galactic centre where the bulge is.
    let toward_centre = clamp(dot(dir, GALACTIC_CENTRE), -1.0, 1.0);
    let bulge = exp(-pow((1.0 - toward_centre) * 1.9, 2.0));
    let width = 0.20 + 0.10 * (1.0 - bulge);
    var band = exp(-pow(latitude / width, 2.0));

    // Clumpy star clouds along the band.
    // Higher frequency and lower contrast than a standalone band would want:
    // at close zoom the old low-frequency version read as cumulus cloud.
    let clouds = fbm(dir * 16.0 + seed, 5);
    band = band * (0.72 + 0.48 * clouds) * (0.55 + 0.9 * bulge);

    // Dark dust lanes cutting through the plane. Ridged noise gives them the
    // filamentary look the real thing has.
    let dust = ridged(dir * 22.0 + seed * 1.7, 4);
    let lane = smoothstep(0.45, 0.90, dust) * exp(-pow(latitude / (width * 0.75), 2.0));
    band = max(band - lane * 0.55, 0.0);

    // Slightly warm in the bulge, cooler out along the arms.
    let colour = mix(vec3<f32>(0.62, 0.68, 0.92), vec3<f32>(1.0, 0.90, 0.72), bulge * 0.8);
    return colour * band;
}

fn nebulosity(dir: vec3<f32>, seed: f32) -> vec3<f32> {
    let latitude = galactic_latitude(dir);
    // Nebulae hug the galactic plane, but more loosely than the star clouds.
    let confinement = exp(-pow(latitude / 0.42, 2.0));

    // Domain warping turns bland noise into something with structure.
    let warp = vec3<f32>(
        fbm(dir * 2.1 + seed, 3),
        fbm(dir * 2.1 + seed + 19.7, 3),
        fbm(dir * 2.1 + seed + 41.3, 3),
    );
    let warped = dir * 3.4 + warp * 1.6;

    let hydrogen = pow(smoothstep(0.42, 0.95, fbm(warped, 5)), 2.0);
    let oxygen = pow(smoothstep(0.50, 0.98, fbm(warped * 1.7 + 7.3, 4)), 2.2);

    // Hydrogen-alpha red and doubly-ionised-oxygen teal, the two that dominate
    // real emission nebulae.
    let red = vec3<f32>(0.85, 0.18, 0.32) * hydrogen;
    let teal = vec3<f32>(0.16, 0.52, 0.72) * oxygen;
    return (red + teal) * confinement;
}

@fragment
fn fragment_main(in: SkyVertex) -> @location(0) vec4<f32> {
    let ray = view_ray(in.ndc);
    // The seed arrives as a small exact integer in an f32 slot. Star cells key
    // the integer hash directly; the continuum noises take a small float
    // offset, kept small so f32 lattice precision stays intact.
    let seed = u32(globals.sky_b.w);
    let noise_seed = f32(seed % 1024u) * 0.0625;
    let dir = rotate_about_y(ray, globals.sky_b.y, globals.sky_b.z);

    let density = globals.sky_a.x;
    let brightness = globals.sky_a.y;

    // Three scales of stars so the field has depth rather than one uniform
    // sprinkle.
    //
    // A layer at cell scale S with occupancy p yields about p*S^2 stars per
    // steradian, so the occupancies below are chosen to put roughly 4000
    // stars/sr on screen in total -- a few thousand across a typical viewport.
    // Left uncalibrated, the fine layers alone put a star on every pixel.
    let core_radius = globals.sky_c.x * globals.sky_c.y;
    var stars = star_layer(dir, 320.0, seed, 0.0045 * density, core_radius) * 0.55;
    stars = stars + star_layer(dir, 700.0, seed + 13u, 0.0011 * density, core_radius) * 0.38;
    stars = stars + star_layer(dir, 1300.0, seed + 71u, 0.00030 * density, core_radius) * 0.24;

    // The galactic band carries its own haze of stars too faint to resolve.
    // Confined tightly to the band, or it just raises the count everywhere.
    let band = milky_way(dir, noise_seed);
    let haze = star_layer(dir, 1500.0, seed + 137u, 0.0012 * density, core_radius * 0.75) * 0.35
        * smoothstep(0.06, 0.45, length(band));

    var colour = (stars + haze) * brightness * 1.7;
    colour = colour + band * globals.sky_a.z * 0.11;
    colour = colour + nebulosity(dir, noise_seed) * globals.sky_a.w * 0.05;
    colour = colour + vec3<f32>(0.010, 0.013, 0.026) * globals.sky_b.x * 4.0;

    return vec4<f32>(colour, 1.0);
}
