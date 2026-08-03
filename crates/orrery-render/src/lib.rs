//! The wgpu renderer.
//!
//! One frame is five stages:
//!
//! 1. **Scene** — sky, bodies, ring systems and orbit ribbons, drawn into a
//!    multisampled `Rgba16Float` target and resolved to a single-sample one.
//!    Rendering in HDR is what lets the Sun be genuinely hundreds of times
//!    brighter than Neptune instead of clipping to white.
//! 2. **Bloom downsample** — a mip chain built with a 13-tap filter.
//! 3. **Bloom upsample** — tent-filtered and additively blended back up.
//! 4. **Tone map** — exposure, ACES and a dither, into the swapchain.
//!
//! Depth is reversed (`Greater` compare, cleared to zero) because an orrery
//! spans an enormous near-to-far ratio and reversed Z is what keeps float depth
//! precision usable across it.

use std::borrow::Cow;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};
use orrery_core::bodies::Rings;
use orrery_core::config::Config;
use orrery_core::scene::{BodyInstance, Scene};
use orrery_core::sky::{Catalog, DeepSkyKind};
use wgpu::util::DeviceExt;

pub mod geometry;

/// The HDR format used for every intermediate target. Guaranteed by the wgpu
/// spec to be filterable, blendable, renderable and multisample-resolvable, so
/// none of this needs optional features.
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// Deepest the bloom chain will go. Six mips is already a very wide glow.
const MAX_BLOOM_MIPS: u32 = 6;

/// Radius of a star's core, in pixels. Small enough to stay point-like, large
/// enough that it does not alias into a flickering single pixel.
const STAR_CORE_RADIUS_PIXELS: f32 = 0.9;

/// How a body's surface is synthesised. Must match the `KIND_*` constants in
/// `body.wgsl`.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SurfaceKind {
    Sun = 0,
    Rocky = 1,
    Earthlike = 2,
    GasGiant = 3,
    IceGiant = 4,
    Ring = 5,
}

/// Which surface synthesis and how much atmospheric rim each body gets.
fn appearance(name: &str) -> (SurfaceKind, f32) {
    match name {
        "Sun" => (SurfaceKind::Sun, 0.0),
        "Earth" => (SurfaceKind::Earthlike, 0.85),
        // Venus's atmosphere is the thickest in the solar system and it shows.
        "Venus" => (SurfaceKind::Rocky, 1.15),
        "Mars" => (SurfaceKind::Rocky, 0.18),
        "Jupiter" | "Saturn" => (SurfaceKind::GasGiant, 0.45),
        "Uranus" | "Neptune" => (SurfaceKind::IceGiant, 0.60),
        // Mercury, the Moon and Pluto are airless.
        _ => (SurfaceKind::Rocky, 0.0),
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuGlobals {
    view_projection: [[f32; 4]; 4],
    inverse_view_projection: [[f32; 4]; 4],
    camera: [f32; 4],
    sun: [f32; 4],
    sky_a: [f32; 4],
    sky_b: [f32; 4],
    viewport: [f32; 4],
    post: [f32; 4],
    sky_c: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuBodyInstance {
    model: [[f32; 4]; 4],
    normal_matrix: [[f32; 4]; 4],
    colour: [f32; 4],
    params: [f32; 4],
    parent: [f32; 4],
    ring: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuOrbitVertex {
    position: [f32; 3],
    neighbour: [f32; 3],
    /// x = which side of the ribbon (-1 or +1), y = brightness.
    params: [f32; 2],
    colour: [f32; 3],
}

impl GpuOrbitVertex {
    const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<GpuOrbitVertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![
            0 => Float32x3, 1 => Float32x3, 2 => Float32x2, 3 => Float32x3
        ],
    };
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuStar {
    /// xyz = unit direction, w = visual magnitude.
    direction_magnitude: [f32; 4],
    colour: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuSegment {
    endpoint_a: [f32; 4],
    endpoint_b: [f32; 4],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuDeepSky {
    /// xyz = unit direction, w = angular radius in radians.
    direction_radius: [f32; 4],
    /// rgb = tint, a = prominence.
    colour: [f32; 4],
    /// kind, then padding.
    params: [f32; 4],
}

/// Tint and kind index for each class of deep-sky object. Must match the
/// `KIND_*` constants in `celestial.wgsl`.
fn deep_sky_appearance(kind: DeepSkyKind) -> (f32, [f32; 3]) {
    match kind {
        // Hydrogen-alpha red, shading pink where it is brightest.
        DeepSkyKind::EmissionNebula => (0.0, [0.90, 0.34, 0.38]),
        // Doubly-ionised oxygen.
        DeepSkyKind::PlanetaryNebula => (1.0, [0.36, 0.82, 0.74]),
        // Old, metal-poor, and distinctly yellow.
        DeepSkyKind::GlobularCluster => (2.0, [1.00, 0.88, 0.66]),
        // Young hot stars.
        DeepSkyKind::OpenCluster => (3.0, [0.72, 0.82, 1.00]),
        // Integrated starlight, faintly warm.
        DeepSkyKind::Galaxy => (4.0, [0.92, 0.88, 0.80]),
        DeepSkyKind::SupernovaRemnant => (5.0, [0.70, 0.62, 0.92]),
    }
}

/// Buffers and counts for everything drawn on the celestial sphere.
struct Celestial {
    stars: wgpu::Buffer,
    star_count: u32,
    segments: wgpu::Buffer,
    segment_count: u32,
    deep_sky: wgpu::Buffer,
    deep_sky_count: u32,
    bind_group: wgpu::BindGroup,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct GpuTonemapSettings {
    /// exposure, bloom intensity, unused, unused.
    values: [f32; 4],
}

struct GpuMesh {
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

impl GpuMesh {
    fn upload(device: &wgpu::Device, label: &str, mesh: &geometry::Mesh) -> Self {
        Self {
            vertices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label} vertices")),
                contents: bytemuck::cast_slice(&mesh.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            indices: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(&format!("{label} indices")),
                contents: bytemuck::cast_slice(&mesh.indices),
                usage: wgpu::BufferUsages::INDEX,
            }),
            index_count: mesh.indices.len() as u32,
        }
    }
}

/// Size-dependent GPU resources, rebuilt on resize.
struct Targets {
    width: u32,
    height: u32,
    multisampled_colour: wgpu::TextureView,
    hdr: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// One view per bloom mip.
    bloom_mips: Vec<wgpu::TextureView>,
    /// Bind group reading mip `i`, used as the source when writing a neighbour.
    bloom_sources: Vec<wgpu::BindGroup>,
    /// Bind group reading the resolved HDR scene.
    hdr_source: wgpu::BindGroup,
    tonemap_bind_group: wgpu::BindGroup,
}

pub struct Renderer {
    globals: wgpu::Buffer,
    globals_bind_group: wgpu::BindGroup,
    tonemap_settings: wgpu::Buffer,

    instances: wgpu::Buffer,
    instances_capacity: usize,
    instances_bind_group: wgpu::BindGroup,
    instances_layout: wgpu::BindGroupLayout,

    orbit_vertices: wgpu::Buffer,
    orbit_capacity: usize,
    /// One vertex range per orbit. Each is drawn separately: a single strip
    /// spanning every ring would stitch the end of one orbit to the start of
    /// the next.
    orbit_ranges: Vec<std::ops::Range<u32>>,

    sphere: GpuMesh,
    ring: GpuMesh,

    sky_pipeline: wgpu::RenderPipeline,
    star_pipeline: wgpu::RenderPipeline,
    constellation_pipeline: wgpu::RenderPipeline,
    deep_sky_pipeline: wgpu::RenderPipeline,
    body_pipeline: wgpu::RenderPipeline,
    ring_pipeline: wgpu::RenderPipeline,
    orbit_pipeline: wgpu::RenderPipeline,
    bloom_downsample: wgpu::RenderPipeline,
    bloom_upsample: wgpu::RenderPipeline,
    tonemap_pipeline: wgpu::RenderPipeline,

    blit_layout: wgpu::BindGroupLayout,
    tonemap_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,

    sample_count: u32,
    targets: Targets,
    celestial: Option<Celestial>,
    celestial_layout: wgpu::BindGroupLayout,
}

impl Renderer {
    pub fn new(
        device: &wgpu::Device,
        output_format: wgpu::TextureFormat,
        width: u32,
        height: u32,
        config: &Config,
    ) -> Self {
        let sample_count = config.render.msaa.max(1);

        let common = include_str!("../shaders/common.wgsl");
        let make_shader = |label: &str, source: &str, with_common: bool| {
            let text = if with_common {
                Cow::Owned(format!("{common}\n{source}"))
            } else {
                Cow::Borrowed(source)
            };
            device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(text),
            })
        };

        let sky_shader = make_shader("sky", include_str!("../shaders/sky.wgsl"), true);
        let body_shader = make_shader("body", include_str!("../shaders/body.wgsl"), true);
        let orbit_shader = make_shader("orbit", include_str!("../shaders/orbit.wgsl"), true);
        let celestial_shader =
            make_shader("celestial", include_str!("../shaders/celestial.wgsl"), true);
        let bloom_shader = make_shader("bloom", include_str!("../shaders/bloom.wgsl"), false);
        let tonemap_shader = make_shader("tonemap", include_str!("../shaders/tonemap.wgsl"), false);

        // --- bind group layouts ---
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let instances_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("instances"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        // Three read-only storage buffers: stars, constellation segments, and
        // deep-sky objects.
        let celestial_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("celestial"),
            entries: &std::array::from_fn::<_, 3, _>(|index| wgpu::BindGroupLayoutEntry {
                binding: index as u32,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Storage { read_only: true },
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }),
        });

        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let tonemap_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tonemap"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        // --- buffers ---
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: size_of::<GpuGlobals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: globals.as_entire_binding(),
            }],
        });

        let tonemap_settings = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tonemap settings"),
            size: size_of::<GpuTonemapSettings>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let instances_capacity = 32;
        let instances = new_instance_buffer(device, instances_capacity);
        let instances_bind_group =
            new_instance_bind_group(device, &instances_layout, &instances);

        let orbit_capacity = 4096;
        let orbit_vertices = new_orbit_buffer(device, orbit_capacity);

        let sphere = GpuMesh::upload(device, "sphere", &geometry::sphere(96, 48));
        let ring = GpuMesh::upload(device, "ring", &geometry::ring(256));

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear clamp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });

        // --- pipelines ---
        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[Some(&globals_layout), Some(&instances_layout)],
            immediate_size: 0,
        });
        let globals_only_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("globals only"),
                bind_group_layouts: &[Some(&globals_layout)],
                immediate_size: 0,
            });
        let blit_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("blit"),
                bind_group_layouts: &[Some(&blit_layout)],
                immediate_size: 0,
            });
        let tonemap_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("tonemap"),
                bind_group_layouts: &[Some(&tonemap_layout)],
                immediate_size: 0,
            });

        let multisample = wgpu::MultisampleState {
            count: sample_count,
            mask: !0,
            alpha_to_coverage_enabled: false,
        };

        // Reversed Z: nearer geometry has the greater depth value.
        let depth_test = |write: bool| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            })
        };

        let sky_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&globals_only_layout),
            vertex: wgpu::VertexState {
                module: &sky_shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            // The sky is behind everything and occludes nothing.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::Always),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &sky_shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &[Some(HDR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let body_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("bodies"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &body_shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[Some(geometry::Vertex::LAYOUT)],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: depth_test(true),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &body_shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &[Some(HDR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let ring_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("rings"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &body_shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[Some(geometry::Vertex::LAYOUT)],
            },
            primitive: wgpu::PrimitiveState {
                // Rings are visible from both faces.
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth_test(false),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &body_shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let orbit_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("orbits"),
            layout: Some(&globals_only_layout),
            vertex: wgpu::VertexState {
                module: &orbit_shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[Some(GpuOrbitVertex::LAYOUT)],
            },
            primitive: wgpu::PrimitiveState {
                // The ribbon vertices alternate sides along each ring, which is
                // a strip. Drawing them as a list makes every other triangle
                // vanish and the orbits come out dashed.
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                cull_mode: None,
                ..Default::default()
            },
            depth_stencil: depth_test(false),
            multisample,
            fragment: Some(wgpu::FragmentState {
                module: &orbit_shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    // The shader already multiplies colour by alpha.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let celestial_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("celestial"),
                bind_group_layouts: &[Some(&globals_layout), Some(&celestial_layout)],
                immediate_size: 0,
            });

        // Everything on the celestial sphere is additive: stars and nebulosity
        // emit light, they do not occlude one another.
        let celestial_pipeline = |label: &str, vertex: &str, fragment: &str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&celestial_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &celestial_shader,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    cull_mode: None,
                    ..Default::default()
                },
                // Behind everything, and occluding nothing.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::Always),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample,
                fragment: Some(wgpu::FragmentState {
                    module: &celestial_shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend: Some(wgpu::BlendState {
                            color: wgpu::BlendComponent {
                                src_factor: wgpu::BlendFactor::One,
                                dst_factor: wgpu::BlendFactor::One,
                                operation: wgpu::BlendOperation::Add,
                            },
                            alpha: wgpu::BlendComponent::REPLACE,
                        }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let star_pipeline = celestial_pipeline("stars", "star_vertex", "star_fragment");
        let constellation_pipeline = celestial_pipeline(
            "constellations",
            "constellation_vertex",
            "constellation_fragment",
        );
        let deep_sky_pipeline =
            celestial_pipeline("deep sky", "deep_sky_vertex", "deep_sky_fragment");

        let blit_pipeline = |label: &str, entry: &str, blend: Option<wgpu::BlendState>| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&blit_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &bloom_shader,
                    entry_point: Some("vertex_main"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &bloom_shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: HDR_FORMAT,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };

        let bloom_downsample = blit_pipeline("bloom downsample", "downsample_main", None);
        // Upsampling accumulates into the coarser mip already there.
        let bloom_upsample = blit_pipeline(
            "bloom upsample",
            "upsample_main",
            Some(wgpu::BlendState {
                color: wgpu::BlendComponent {
                    src_factor: wgpu::BlendFactor::One,
                    dst_factor: wgpu::BlendFactor::One,
                    operation: wgpu::BlendOperation::Add,
                },
                alpha: wgpu::BlendComponent::REPLACE,
            }),
        );

        let tonemap_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("tonemap"),
            layout: Some(&tonemap_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &tonemap_shader,
                entry_point: Some("vertex_main"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            fragment: Some(wgpu::FragmentState {
                module: &tonemap_shader,
                entry_point: Some("fragment_main"),
                compilation_options: Default::default(),
                targets: &[Some(output_format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });

        let targets = Targets::new(
            device,
            width,
            height,
            sample_count,
            &blit_layout,
            &tonemap_layout,
            &sampler,
            &tonemap_settings,
        );

        Self {
            globals,
            globals_bind_group,
            tonemap_settings,
            instances,
            instances_capacity,
            instances_bind_group,
            instances_layout,
            orbit_vertices,
            orbit_capacity,
            orbit_ranges: Vec::new(),
            sphere,
            ring,
            sky_pipeline,
            star_pipeline,
            constellation_pipeline,
            deep_sky_pipeline,
            body_pipeline,
            ring_pipeline,
            orbit_pipeline,
            bloom_downsample,
            bloom_upsample,
            tonemap_pipeline,
            blit_layout,
            tonemap_layout,
            sampler,
            sample_count,
            targets,
            celestial: None,
            celestial_layout,
        }
    }

    /// Upload the star catalogue, constellation figures and deep-sky objects.
    ///
    /// Done once: this data is fixed, so the buffers are built at start-up and
    /// only the magnitude cut-off is applied here.
    pub fn set_catalog(&mut self, device: &wgpu::Device, catalog: &Catalog, config: &Config) {
        let stars: Vec<GpuStar> = catalog
            .stars_to_magnitude(config.sky.magnitude_limit)
            .map(|star| GpuStar {
                direction_magnitude: [
                    star.direction.x,
                    star.direction.y,
                    star.direction.z,
                    star.magnitude,
                ],
                colour: [star.color[0], star.color[1], star.color[2], 1.0],
            })
            .collect();

        let segments: Vec<GpuSegment> = catalog
            .constellations
            .iter()
            .flat_map(|figure| figure.segments.iter())
            .map(|(a, b)| GpuSegment {
                endpoint_a: [a.x, a.y, a.z, 0.0],
                endpoint_b: [b.x, b.y, b.z, 0.0],
            })
            .collect();

        let deep_sky: Vec<GpuDeepSky> = catalog
            .deep_sky
            .iter()
            .map(|object| {
                let (kind, colour) = deep_sky_appearance(object.kind);
                GpuDeepSky {
                    direction_radius: [
                        object.direction.x,
                        object.direction.y,
                        object.direction.z,
                        object.angular_radius_deg.to_radians(),
                    ],
                    colour: [colour[0], colour[1], colour[2], object.prominence],
                    params: [kind, 0.0, 0.0, 0.0],
                }
            })
            .collect();

        log::info!(
            "sky: {} stars to magnitude {}, {} constellation segments, {} deep-sky objects",
            stars.len(),
            config.sky.magnitude_limit,
            segments.len(),
            deep_sky.len()
        );

        // A zero-length storage buffer is invalid, so empty sets still get one
        // element; the draw count is what actually suppresses them.
        let upload = |label: &str, bytes: &[u8], stride: usize| {
            let fallback = vec![0u8; stride];
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: if bytes.is_empty() { &fallback } else { bytes },
                usage: wgpu::BufferUsages::STORAGE,
            })
        };

        let star_buffer = upload("stars", bytemuck::cast_slice(&stars), size_of::<GpuStar>());
        let segment_buffer = upload(
            "constellation segments",
            bytemuck::cast_slice(&segments),
            size_of::<GpuSegment>(),
        );
        let deep_sky_buffer = upload(
            "deep sky",
            bytemuck::cast_slice(&deep_sky),
            size_of::<GpuDeepSky>(),
        );

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("celestial"),
            layout: &self.celestial_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: star_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: segment_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: deep_sky_buffer.as_entire_binding(),
                },
            ],
        });

        self.celestial = Some(Celestial {
            stars: star_buffer,
            star_count: stars.len() as u32,
            segments: segment_buffer,
            segment_count: segments.len() as u32,
            deep_sky: deep_sky_buffer,
            deep_sky_count: deep_sky.len() as u32,
            bind_group,
        });
    }

    pub fn resize(&mut self, device: &wgpu::Device, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if width == self.targets.width && height == self.targets.height {
            return;
        }
        self.targets = Targets::new(
            device,
            width,
            height,
            self.sample_count,
            &self.blit_layout,
            &self.tonemap_layout,
            &self.sampler,
            &self.tonemap_settings,
        );
    }

    pub fn size(&self) -> (u32, u32) {
        (self.targets.width, self.targets.height)
    }

    /// Draw one frame into `output`.
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        output: &wgpu::TextureView,
        scene: &Scene,
        config: &Config,
        elapsed_seconds: f32,
    ) {
        let (width, height) = (self.targets.width, self.targets.height);
        let aspect = width as f32 / height.max(1) as f32;

        self.upload_globals(queue, scene, config, aspect, elapsed_seconds);
        let (sphere_instances, ring_instances) = self.upload_instances(device, queue, scene);
        self.upload_orbits(device, queue, scene, config);

        queue.write_buffer(
            &self.tonemap_settings,
            0,
            bytemuck::bytes_of(&GpuTonemapSettings {
                values: [
                    2.0_f32.powf(config.render.exposure_stops),
                    config.render.bloom_intensity,
                    0.0,
                    0.0,
                ],
            }),
        );

        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });

        self.scene_pass(
            &mut encoder,
            sphere_instances,
            ring_instances,
            config.sky.real_stars,
            config.sky.constellations,
            config.sky.deep_sky,
        );
        self.bloom_pass(&mut encoder);
        self.tonemap_pass(&mut encoder, output);

        queue.submit([encoder.finish()]);
    }

    fn upload_globals(
        &self,
        queue: &wgpu::Queue,
        scene: &Scene,
        config: &Config,
        aspect: f32,
        elapsed_seconds: f32,
    ) {
        let view_projection = scene.camera.view_projection(aspect);
        let rotation = config.sky.rotation_deg.to_radians();
        let globals = GpuGlobals {
            view_projection: view_projection.to_cols_array_2d(),
            inverse_view_projection: view_projection.inverse().to_cols_array_2d(),
            camera: [
                scene.camera.eye.x,
                scene.camera.eye.y,
                scene.camera.eye.z,
                elapsed_seconds,
            ],
            sun: [0.0, 0.0, 0.0, scene.sun.radius],
            sky_a: [
                config.sky.star_density,
                config.sky.star_brightness,
                config.sky.milky_way,
                config.sky.nebula,
            ],
            sky_b: [
                config.sky.ambient,
                rotation.cos(),
                rotation.sin(),
                // Kept small and exactly representable; the shader casts it to u32.
                (config.sky.seed % 1_048_576) as f32,
            ],
            viewport: [
                self.targets.width as f32,
                self.targets.height as f32,
                1.0 / self.targets.width as f32,
                1.0 / self.targets.height as f32,
            ],
            post: [
                2.0_f32.powf(config.render.exposure_stops),
                config.render.bloom_intensity,
                config.render.bloom_radius,
                config.orbits.width_px,
            ],
            sky_c: [
                // One pixel's angular size, so stars can be sized in pixels
                // rather than in units of whatever lattice generated them.
                scene.camera.fov_y_radians / self.targets.height.max(1) as f32,
                STAR_CORE_RADIUS_PIXELS,
                config.sky.constellation_opacity,
                config.sky.deep_sky_opacity,
            ],
        };
        queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(&globals));
    }

    /// Pack every body and ring system into the instance buffer.
    ///
    /// Returns the instance ranges for the sphere draw and the ring draw. Rings
    /// live in the same buffer so both draws can share one bind group; the
    /// shader tells them apart by `params.z`.
    fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
    ) -> (std::ops::Range<u32>, std::ops::Range<u32>) {
        let mut instances = Vec::with_capacity(scene.bodies.len() + 3);

        instances.push(body_instance(&scene.sun));
        for body in &scene.bodies {
            instances.push(body_instance(body));
        }
        let sphere_count = instances.len() as u32;

        for body in &scene.bodies {
            if let Some(rings) = body.rings {
                instances.push(ring_instance(body, &rings));
            }
        }
        let total = instances.len() as u32;

        if instances.len() > self.instances_capacity {
            self.instances_capacity = instances.len().next_power_of_two();
            self.instances = new_instance_buffer(device, self.instances_capacity);
            self.instances_bind_group =
                new_instance_bind_group(device, &self.instances_layout, &self.instances);
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));

        (0..sphere_count, sphere_count..total)
    }

    fn upload_orbits(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene,
        config: &Config,
    ) {
        let mut vertices = Vec::new();
        self.orbit_ranges.clear();
        for ring in &scene.orbits {
            let count = ring.points.len();
            if count < 2 {
                continue;
            }
            let start = vertices.len() as u32;
            for index in 0..count {
                let position = ring.points[index];
                let neighbour = ring.points[(index + 1) % count];
                let along = index as f32 / count as f32;

                // The planet travels toward increasing `along`, so the trail is
                // the stretch just behind it.
                let behind = (ring.body_fraction - along).rem_euclid(1.0);
                let trail_length = config.orbits.trail_length.clamp(1e-4, 1.0);
                let falloff = (1.0 - behind / trail_length).max(0.0);
                let brightness = config.orbits.opacity
                    * (1.0 + config.orbits.trail_strength * falloff * falloff);

                for side in [-1.0_f32, 1.0] {
                    vertices.push(GpuOrbitVertex {
                        position: position.into(),
                        neighbour: neighbour.into(),
                        params: [side, brightness],
                        colour: ring.color,
                    });
                }
            }
            // Close the loop by repeating this ring's first pair.
            let first = vertices[start as usize];
            let second = vertices[start as usize + 1];
            vertices.push(first);
            vertices.push(second);
            self.orbit_ranges.push(start..vertices.len() as u32);
        }

        if vertices.is_empty() {
            return;
        }
        if vertices.len() > self.orbit_capacity {
            self.orbit_capacity = vertices.len().next_power_of_two();
            self.orbit_vertices = new_orbit_buffer(device, self.orbit_capacity);
        }
        queue.write_buffer(&self.orbit_vertices, 0, bytemuck::cast_slice(&vertices));
    }

    #[allow(clippy::too_many_arguments)]
    fn scene_pass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        sphere_instances: std::ops::Range<u32>,
        ring_instances: std::ops::Range<u32>,
        draw_stars: bool,
        draw_constellations: bool,
        draw_deep_sky: bool,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("scene"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.targets.multisampled_colour,
                depth_slice: None,
                resolve_target: Some(&self.targets.hdr),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    // Only the resolved single-sample image is ever read.
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.targets.depth,
                depth_ops: Some(wgpu::Operations {
                    // Reversed Z clears to the far plane, which is zero.
                    load: wgpu::LoadOp::Clear(0.0),
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });

        pass.set_bind_group(0, &self.globals_bind_group, &[]);

        pass.set_pipeline(&self.sky_pipeline);
        pass.draw(0..3, 0..1);

        // Real sky on top of the procedural haze, still behind the planets.
        if let Some(celestial) = &self.celestial {
            pass.set_bind_group(1, &celestial.bind_group, &[]);
            if celestial.deep_sky_count > 0 && draw_deep_sky {
                pass.set_pipeline(&self.deep_sky_pipeline);
                pass.draw(0..4, 0..celestial.deep_sky_count);
            }
            if celestial.segment_count > 0 && draw_constellations {
                pass.set_pipeline(&self.constellation_pipeline);
                pass.draw(0..4, 0..celestial.segment_count);
            }
            // Stars last, so they sit over the nebulosity they are embedded in.
            if celestial.star_count > 0 && draw_stars {
                pass.set_pipeline(&self.star_pipeline);
                pass.draw(0..4, 0..celestial.star_count);
            }
        }

        pass.set_bind_group(1, &self.instances_bind_group, &[]);
        pass.set_pipeline(&self.body_pipeline);
        pass.set_vertex_buffer(0, self.sphere.vertices.slice(..));
        pass.set_index_buffer(self.sphere.indices.slice(..), wgpu::IndexFormat::Uint32);
        pass.draw_indexed(0..self.sphere.index_count, 0, sphere_instances);

        if !ring_instances.is_empty() {
            pass.set_pipeline(&self.ring_pipeline);
            pass.set_vertex_buffer(0, self.ring.vertices.slice(..));
            pass.set_index_buffer(self.ring.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.ring.index_count, 0, ring_instances);
        }

        if !self.orbit_ranges.is_empty() {
            pass.set_pipeline(&self.orbit_pipeline);
            pass.set_vertex_buffer(0, self.orbit_vertices.slice(..));
            for range in &self.orbit_ranges {
                pass.draw(range.clone(), 0..1);
            }
        }
    }

    fn bloom_pass(&self, encoder: &mut wgpu::CommandEncoder) {
        let mips = self.targets.bloom_mips.len();
        if mips == 0 {
            return;
        }

        // Downsample: the scene into mip 0, then each mip into the next.
        for mip in 0..mips {
            let source = if mip == 0 {
                &self.targets.hdr_source
            } else {
                &self.targets.bloom_sources[mip - 1]
            };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom downsample"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.bloom_mips[mip],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.bloom_downsample);
            pass.set_bind_group(0, source, &[]);
            pass.draw(0..3, 0..1);
        }

        // Upsample: accumulate each mip additively into the one above it.
        for mip in (1..mips).rev() {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("bloom upsample"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets.bloom_mips[mip - 1],
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        // Load, because this blends onto what downsampling left.
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.bloom_upsample);
            pass.set_bind_group(0, &self.targets.bloom_sources[mip], &[]);
            pass.draw(0..3, 0..1);
        }
    }

    fn tonemap_pass(&self, encoder: &mut wgpu::CommandEncoder, output: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("tonemap"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.tonemap_pipeline);
        pass.set_bind_group(0, &self.targets.tonemap_bind_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

fn body_instance(body: &BodyInstance) -> GpuBodyInstance {
    let (kind, atmosphere) = appearance(body.name);
    // Flattening squashes the poles, so the scale is not uniform and the
    // normals need the inverse transpose rather than the model matrix.
    let scale = Vec3::new(
        body.radius,
        body.radius * (1.0 - body.flattening),
        body.radius,
    );
    let model = Mat4::from_scale_rotation_translation(scale, body.orientation, body.position);
    GpuBodyInstance {
        model: model.to_cols_array_2d(),
        normal_matrix: model.inverse().transpose().to_cols_array_2d(),
        colour: [body.color[0], body.color[1], body.color[2], atmosphere],
        params: [
            body.surface_seed as f32 * 7.31 + 1.0,
            body.radius,
            kind as i32 as f32,
            1.0,
        ],
        parent: [0.0, 0.0, 0.0, 0.0],
        ring: [0.0, 0.0, 0.0, 0.0],
    }
}

fn ring_instance(body: &BodyInstance, rings: &Rings) -> GpuBodyInstance {
    // Ring radii are catalogued in units of the parent's equatorial radius.
    let inner = rings.inner_radius * body.radius;
    let outer = rings.outer_radius * body.radius;
    // Rings lie in the planet's equatorial plane, so they inherit its tilt but
    // not its spin scale.
    let model = Mat4::from_rotation_translation(body.orientation, body.position);
    GpuBodyInstance {
        model: model.to_cols_array_2d(),
        normal_matrix: model.inverse().transpose().to_cols_array_2d(),
        colour: [rings.color[0], rings.color[1], rings.color[2], 0.0],
        params: [
            body.surface_seed as f32 * 3.17 + 2.0,
            outer,
            SurfaceKind::Ring as i32 as f32,
            rings.opacity,
        ],
        parent: [
            body.position.x,
            body.position.y,
            body.position.z,
            // The shadow test wants the planet's own radius.
            body.radius,
        ],
        ring: [inner, outer, 0.0, 0.0],
    }
}

fn new_instance_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("body instances"),
        size: (capacity * size_of::<GpuBodyInstance>()) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn new_instance_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("body instances"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: buffer.as_entire_binding(),
        }],
    })
}

fn new_orbit_buffer(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("orbit ribbons"),
        size: (capacity * size_of::<GpuOrbitVertex>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Targets {
    #[allow(clippy::too_many_arguments)]
    fn new(
        device: &wgpu::Device,
        width: u32,
        height: u32,
        sample_count: u32,
        blit_layout: &wgpu::BindGroupLayout,
        tonemap_layout: &wgpu::BindGroupLayout,
        sampler: &wgpu::Sampler,
        tonemap_settings: &wgpu::Buffer,
    ) -> Self {
        let width = width.max(1);
        let height = height.max(1);
        let extent = |w: u32, h: u32| wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };

        let multisampled_colour = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("scene msaa"),
                size: extent(width, height),
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());

        let hdr_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene hdr"),
            size: extent(width, height),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let hdr = hdr_texture.create_view(&Default::default());

        let depth = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: extent(width, height),
                mip_level_count: 1,
                sample_count,
                dimension: wgpu::TextureDimension::D2,
                format: DEPTH_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            })
            .create_view(&Default::default());

        // The bloom chain starts at half resolution. Stop before any mip would
        // collapse to nothing.
        let bloom_width = (width / 2).max(1);
        let bloom_height = (height / 2).max(1);
        let mip_count = (bloom_width.min(bloom_height).max(1).ilog2()).min(MAX_BLOOM_MIPS).max(1);

        let bloom_texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("bloom chain"),
            size: extent(bloom_width, bloom_height),
            mip_level_count: mip_count,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });

        let mut bloom_mips = Vec::with_capacity(mip_count as usize);
        let mut bloom_sources = Vec::with_capacity(mip_count as usize);
        for mip in 0..mip_count {
            let view = bloom_texture.create_view(&wgpu::TextureViewDescriptor {
                label: Some("bloom mip"),
                base_mip_level: mip,
                mip_level_count: Some(1),
                ..Default::default()
            });
            bloom_sources.push(device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("bloom mip source"),
                layout: blit_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(sampler),
                    },
                ],
            }));
            bloom_mips.push(view);
        }

        let hdr_source = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hdr source"),
            layout: blit_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
            ],
        });

        let tonemap_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tonemap"),
            layout: tonemap_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&hdr),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&bloom_mips[0]),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: tonemap_settings.as_entire_binding(),
                },
            ],
        });

        Self {
            width,
            height,
            multisampled_colour,
            hdr,
            depth,
            bloom_mips,
            bloom_sources,
            hdr_source,
            tonemap_bind_group,
        }
    }
}
