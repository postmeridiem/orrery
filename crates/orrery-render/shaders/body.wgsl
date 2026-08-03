// Sun, planets, moons and ring systems.
//
// Every surface is procedural: there are no texture assets anywhere in this
// project. Each body gets a `kind` that selects how its albedo is synthesised,
// and a seed so two rocky worlds do not come out identical.

const KIND_SUN: f32 = 0.0;
const KIND_ROCKY: f32 = 1.0;
const KIND_EARTHLIKE: f32 = 2.0;
const KIND_GAS_GIANT: f32 = 3.0;
const KIND_ICE_GIANT: f32 = 4.0;
const KIND_RING: f32 = 5.0;

struct BodyInstance {
    model: mat4x4<f32>,
    // Inverse transpose of `model`, needed because flattening makes the scale
    // non-uniform and would otherwise skew the normals.
    normal_matrix: mat4x4<f32>,
    // rgb = base albedo, a = atmosphere strength.
    colour: vec4<f32>,
    // seed, drawn radius, kind, opacity.
    params: vec4<f32>,
    // xyz = parent body centre (rings only), w = parent radius.
    parent: vec4<f32>,
    // Rings only: inner radius, outer radius, both in scene units.
    ring: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> instances: array<BodyInstance>;

struct BodyVertex {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) local_position: vec3<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) @interpolate(flat) instance_index: u32,
};

@vertex
fn vertex_main(
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @builtin(instance_index) instance_index: u32,
) -> BodyVertex {
    let instance = instances[instance_index];

    // Ring meshes arrive on the unit circle; uv.x says which edge this vertex
    // belongs to, so one mesh can serve every ring system.
    var local = position;
    if (instance.params.z == KIND_RING) {
        local = position * mix(instance.ring.x, instance.ring.y, uv.x);
    }
    let world = instance.model * vec4<f32>(local, 1.0);

    var out: BodyVertex;
    out.clip_position = globals.view_projection * world;
    out.world_position = world.xyz;
    out.world_normal = normalize((instance.normal_matrix * vec4<f32>(normal, 0.0)).xyz);
    out.local_position = position;
    out.uv = uv;
    out.instance_index = instance_index;
    return out;
}

// --- surface synthesis ------------------------------------------------------

fn rocky_albedo(base: vec3<f32>, p: vec3<f32>, seed: f32) -> vec3<f32> {
    let broad = fbm(p * 2.6 + seed, 5);
    let fine = fbm(p * 11.0 + seed * 2.3, 4);
    // Two-tone mottling around the base colour, plus finer speckle.
    let shade = mix(0.72, 1.26, broad) * mix(0.92, 1.08, fine);
    var colour = base * shade;
    // Polar frost, following the body's own axis.
    let polar = smoothstep(0.86, 0.99, abs(p.y));
    colour = mix(colour, vec3<f32>(0.95, 0.96, 0.98), polar * 0.55 * smoothstep(0.4, 0.7, broad));
    return colour;
}

fn earthlike_albedo(p: vec3<f32>, seed: f32, time: f32) -> vec3<f32> {
    // Continents from thresholded fbm, with a warped domain so the coastlines
    // are not obviously noise-shaped.
    let warp = vec3<f32>(
        fbm(p * 1.4 + seed, 3),
        fbm(p * 1.4 + seed + 11.0, 3),
        fbm(p * 1.4 + seed + 23.0, 3),
    );
    let land_field = fbm(p * 2.2 + warp * 0.7 + seed, 6);
    let land = smoothstep(0.50, 0.57, land_field);

    let ocean = mix(vec3<f32>(0.03, 0.10, 0.28), vec3<f32>(0.06, 0.24, 0.45), fbm(p * 6.0, 3));
    let vegetation = mix(vec3<f32>(0.13, 0.32, 0.12), vec3<f32>(0.42, 0.36, 0.18), fbm(p * 7.5 + 3.1, 4));
    var colour = mix(ocean, vegetation, land);

    // Ice caps.
    let polar = smoothstep(0.72, 0.95, abs(p.y));
    colour = mix(colour, vec3<f32>(0.92, 0.94, 0.97), polar);

    // A slowly drifting cloud deck.
    let cloud_field = fbm(p * 3.1 + vec3<f32>(time * 0.004, 0.0, 0.0) + seed * 5.0, 5);
    let clouds = smoothstep(0.52, 0.72, cloud_field);
    colour = mix(colour, vec3<f32>(0.94, 0.95, 0.97), clouds * 0.72);
    return colour;
}

fn gas_giant_albedo(base: vec3<f32>, p: vec3<f32>, uv: vec2<f32>, seed: f32) -> vec3<f32> {
    // Zonal bands are a function of latitude, warped along longitude so they
    // shear and curl the way real belts do.
    let turbulence = fbm(vec3<f32>(p.x * 3.0, p.y * 14.0, p.z * 3.0) + seed, 5);
    let latitude = uv.y + (turbulence - 0.5) * 0.055;

    let bands = sin(latitude * PI * 13.0) * 0.5 + 0.5;
    let sharpened = smoothstep(0.22, 0.78, bands);

    let light = base * 1.18;
    // Belts are darker and browner than the zones, but not by as much as a
    // hard contrast suggests -- Jupiter is a cream planet with tan bands.
    let dark = base * vec3<f32>(0.74, 0.66, 0.56);
    var colour = mix(dark, light, sharpened);

    // Fine streaks within each band.
    let streaks = fbm(vec3<f32>(p.x * 6.0, p.y * 40.0, p.z * 6.0) + seed * 3.0, 4);
    colour = colour * mix(0.90, 1.12, streaks);

    // One long-lived storm, placed by the seed.
    let storm_centre = vec2<f32>(fract(seed * 0.37), 0.63);
    var delta = uv - storm_centre;
    delta.x = delta.x - round(delta.x); // wrap in longitude
    let storm = smoothstep(0.11, 0.0, length(delta * vec2<f32>(1.0, 2.6)));
    colour = mix(colour, vec3<f32>(0.78, 0.34, 0.22), storm * 0.85);
    return colour;
}

fn ice_giant_albedo(base: vec3<f32>, p: vec3<f32>, uv: vec2<f32>, seed: f32) -> vec3<f32> {
    // Far smoother than the gas giants: faint banding and a little haze.
    let bands = sin(uv.y * PI * 7.0) * 0.5 + 0.5;
    let haze = fbm(p * 3.0 + seed, 4);
    return base * mix(0.90, 1.10, bands * 0.45 + haze * 0.55);
}

fn sun_emission(base: vec3<f32>, p: vec3<f32>, normal: vec3<f32>, view: vec3<f32>, time: f32) -> vec3<f32> {
    // Granulation, drifting slowly.
    let cells = fbm(p * 14.0 + vec3<f32>(0.0, time * 0.01, 0.0), 4);
    let fine = fbm(p * 34.0 - vec3<f32>(time * 0.02, 0.0, 0.0), 3);
    let surface = mix(0.82, 1.30, cells * 0.65 + fine * 0.35);

    // Limb darkening: the classic quadratic law makes the disc read as a
    // sphere rather than a flat token.
    let mu = clamp(dot(normal, view), 0.0, 1.0);
    let limb = 0.34 + 0.66 * mu + 0.12 * mu * mu;

    return base * surface * limb * 14.0;
}

// Is a ring point in the planet's shadow? The Sun sits at the origin, so the
// shadow is the cylinder behind the planet along its own sun-ward axis.
fn ring_shadow(world_position: vec3<f32>, parent_centre: vec3<f32>, parent_radius: f32) -> f32 {
    let to_point = length(world_position);
    if (to_point < 1e-6) {
        return 1.0;
    }
    let light = world_position / to_point;
    let along = dot(parent_centre, light);
    // Only points beyond the planet can be shadowed by it.
    if (along <= 0.0 || to_point < along) {
        return 1.0;
    }
    let offset = length(parent_centre - light * along);
    // A soft edge stands in for the Sun's angular size.
    return smoothstep(parent_radius * 0.92, parent_radius * 1.12, offset);
}

fn ring_albedo(base: vec3<f32>, radial: f32, seed: f32) -> vec4<f32> {
    // Concentric structure: broad divisions plus fine ringlets.
    let ringlets = fbm(vec3<f32>(radial * 90.0, seed, 0.0), 5);
    let divisions = smoothstep(0.30, 0.42, abs(radial - 0.62));
    let cassini = smoothstep(0.02, 0.06, abs(radial - 0.52));

    var density = mix(0.35, 1.0, ringlets) * divisions * cassini;
    // Fade both edges so the annulus has no hard rim.
    density = density * smoothstep(0.0, 0.06, radial) * smoothstep(1.0, 0.88, radial);

    let colour = base * mix(0.82, 1.15, ringlets);
    return vec4<f32>(colour, density);
}

// --- fragment ---------------------------------------------------------------

@fragment
fn fragment_main(in: BodyVertex) -> @location(0) vec4<f32> {
    let instance = instances[in.instance_index];
    let seed = instance.params.x;
    let kind = instance.params.z;
    let time = globals.camera.w;

    let view = normalize(globals.camera.xyz - in.world_position);
    var normal = normalize(in.world_normal);

    if (kind == KIND_SUN) {
        return vec4<f32>(sun_emission(instance.colour.rgb, in.local_position, normal, view, time), 1.0);
    }

    // Direction to the Sun, which sits at the origin.
    let to_sun_vector = -in.world_position;
    let distance_to_sun = max(length(to_sun_vector), 1e-6);
    let light = to_sun_vector / distance_to_sun;

    if (kind == KIND_RING) {
        let ring = ring_albedo(instance.colour.rgb, in.uv.x, seed);
        // Rings are thin sheets lit from either face.
        let facing = abs(dot(normal, light));
        let shadow = ring_shadow(in.world_position, instance.parent.xyz, instance.parent.w);
        // Forward scattering makes the far side of the rings glow.
        let forward = pow(clamp(dot(-view, light), 0.0, 1.0), 6.0) * 0.6;
        let lit = ring.rgb * (0.18 + 0.95 * facing + forward) * shadow;
        return vec4<f32>(lit, ring.a * instance.params.w);
    }

    var albedo: vec3<f32>;
    if (kind == KIND_EARTHLIKE) {
        albedo = earthlike_albedo(in.local_position, seed, time);
    } else if (kind == KIND_GAS_GIANT) {
        albedo = gas_giant_albedo(instance.colour.rgb, in.local_position, in.uv, seed);
    } else if (kind == KIND_ICE_GIANT) {
        albedo = ice_giant_albedo(instance.colour.rgb, in.local_position, in.uv, seed);
    } else {
        albedo = rocky_albedo(instance.colour.rgb, in.local_position, seed);
    }

    let incidence = dot(normal, light);
    // A narrow wrap softens the terminator by about the Sun's angular width
    // rather than smearing light right around the body.
    let wrap = 0.06;
    let diffuse = clamp((incidence + wrap) / (1.0 + wrap), 0.0, 1.0);

    var colour = albedo * diffuse;

    // Specular glint, mostly visible on Earth's oceans.
    if (kind == KIND_EARTHLIKE) {
        let half_vector = normalize(light + view);
        let water = 1.0 - smoothstep(0.20, 0.30, length(albedo - vec3<f32>(0.04, 0.16, 0.36)));
        colour = colour + vec3<f32>(0.9, 0.95, 1.0)
            * pow(clamp(dot(normal, half_vector), 0.0, 1.0), 90.0)
            * water * diffuse * 0.5;
    }

    // Atmospheric rim: brightest where the limb is still lit.
    let atmosphere = instance.colour.a;
    if (atmosphere > 0.0) {
        let rim = pow(1.0 - clamp(dot(normal, view), 0.0, 1.0), 3.5);
        let rim_light = pow(clamp(incidence + 0.25, 0.0, 1.0), 0.7);
        var tint = vec3<f32>(0.45, 0.62, 1.0);
        if (kind == KIND_GAS_GIANT) {
            tint = vec3<f32>(1.0, 0.86, 0.66);
        } else if (kind == KIND_ICE_GIANT) {
            tint = vec3<f32>(0.55, 0.86, 1.0);
        }
        colour = colour + tint * rim * rim_light * atmosphere;
    }

    // A trace of ambient so the night side is not pure black.
    colour = colour + albedo * globals.sky_b.x * 0.5;

    return vec4<f32>(colour, 1.0);
}
