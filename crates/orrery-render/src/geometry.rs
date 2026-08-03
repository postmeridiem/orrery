//! Static meshes, built once at start-up.

use bytemuck::{Pod, Zeroable};

/// A position/normal vertex. Sphere normals are the unit position, but rings
/// are flat, so the two are stored separately.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Longitude/latitude in `0..1`, used to place procedural surface detail.
    pub uv: [f32; 2],
}

impl Vertex {
    pub const LAYOUT: wgpu::VertexBufferLayout<'static> = wgpu::VertexBufferLayout {
        array_stride: size_of::<Vertex>() as wgpu::BufferAddress,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
    };
}

pub struct Mesh {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
}

/// A UV sphere of unit radius.
///
/// UV spheres have denser vertices at the poles than an icosphere would, but
/// they give a clean longitude/latitude parameterisation, which is what makes
/// banded gas giants and axial rotation read correctly.
pub fn sphere(segments: u32, rings: u32) -> Mesh {
    let segments = segments.max(3);
    let rings = rings.max(2);

    let mut vertices = Vec::with_capacity(((segments + 1) * (rings + 1)) as usize);
    for ring in 0..=rings {
        // v runs pole to pole; theta is the polar angle.
        let v = ring as f32 / rings as f32;
        let theta = v * std::f32::consts::PI;
        let (sin_theta, cos_theta) = theta.sin_cos();
        for segment in 0..=segments {
            let u = segment as f32 / segments as f32;
            let phi = u * std::f32::consts::TAU;
            let (sin_phi, cos_phi) = phi.sin_cos();
            // +Y is the pole, matching the scene's Y-up convention.
            let position = [sin_theta * cos_phi, cos_theta, sin_theta * sin_phi];
            vertices.push(Vertex {
                position,
                normal: position,
                uv: [u, v],
            });
        }
    }

    let mut indices = Vec::with_capacity((segments * rings * 6) as usize);
    let stride = segments + 1;
    for ring in 0..rings {
        for segment in 0..segments {
            let a = ring * stride + segment;
            let b = a + stride;
            // Counter-clockwise when seen from outside.
            indices.extend_from_slice(&[a, b, a + 1, a + 1, b, b + 1]);
        }
    }

    Mesh { vertices, indices }
}

/// A flat annulus in the XZ plane, for ring systems.
///
/// Positions are on the *unit* circle and `uv.x` is 0 on the inner edge and 1
/// on the outer edge. The vertex shader scales each edge to that instance's
/// actual inner and outer radius, so one mesh serves every ring system —
/// Saturn's broad bright bands and Uranus's narrow dark ones — and `uv.x`
/// doubles as the coordinate the radial banding is sampled against.
pub fn ring(segments: u32) -> Mesh {
    let segments = segments.max(3);
    let mut vertices = Vec::with_capacity(((segments + 1) * 2) as usize);
    for segment in 0..=segments {
        let along = segment as f32 / segments as f32;
        let (sin, cos) = (along * std::f32::consts::TAU).sin_cos();
        for edge in 0..2 {
            vertices.push(Vertex {
                position: [cos, 0.0, sin],
                normal: [0.0, 1.0, 0.0],
                uv: [edge as f32, along],
            });
        }
    }

    let mut indices = Vec::with_capacity((segments * 6) as usize);
    for segment in 0..segments {
        let a = segment * 2;
        indices.extend_from_slice(&[a, a + 1, a + 2, a + 2, a + 1, a + 3]);
    }

    Mesh { vertices, indices }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_vertices_are_on_the_unit_sphere() {
        let mesh = sphere(32, 16);
        for v in &mesh.vertices {
            let r = glam::Vec3::from(v.position).length();
            assert!((r - 1.0).abs() < 1e-5, "vertex at radius {r}");
        }
    }

    #[test]
    fn sphere_indices_are_in_range_and_form_triangles() {
        let mesh = sphere(24, 12);
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(
            mesh.indices.iter().all(|i| (*i as usize) < mesh.vertices.len()),
            "index out of range"
        );
    }

    /// Winding must be counter-clockwise seen from outside, or back-face
    /// culling turns every planet inside out.
    #[test]
    fn sphere_triangles_face_outward() {
        let mesh = sphere(16, 8);
        let mut checked = 0;
        for triangle in mesh.indices.chunks_exact(3) {
            let p: Vec<glam::Vec3> = triangle
                .iter()
                .map(|i| glam::Vec3::from(mesh.vertices[*i as usize].position))
                .collect();
            let normal = (p[1] - p[0]).cross(p[2] - p[0]);
            // Degenerate triangles occur at the poles; skip them.
            if normal.length() < 1e-9 {
                continue;
            }
            let centroid = (p[0] + p[1] + p[2]) / 3.0;
            assert!(
                normal.dot(centroid) > 0.0,
                "triangle {triangle:?} faces inward"
            );
            checked += 1;
        }
        assert!(checked > 100);
    }

    #[test]
    fn ring_is_a_flat_unit_circle_with_edge_marked_uvs() {
        let mesh = ring(64);
        for v in &mesh.vertices {
            let r = (v.position[0].powi(2) + v.position[2].powi(2)).sqrt();
            assert!((r - 1.0).abs() < 1e-5, "radius {r} is not unit");
            assert_eq!(v.position[1], 0.0, "a ring must be flat");
            assert!(v.uv[0] == 0.0 || v.uv[0] == 1.0, "uv.x marks which edge");
        }
        assert_eq!(mesh.indices.len() % 3, 0);
        assert!(mesh.indices.iter().all(|i| (*i as usize) < mesh.vertices.len()));
        // Both edges must be present, or the shader has nothing to interpolate.
        assert!(mesh.vertices.iter().any(|v| v.uv[0] == 0.0));
        assert!(mesh.vertices.iter().any(|v| v.uv[0] == 1.0));
    }
}
