//! Exact CPU visibility queries against the reference mesh's displaced triangles.
//!
//! This deliberately belongs to the temporary reference example. It has no
//! renderer/GPU state and makes no production shadowing claim.

use anyhow::{Result, bail};
use glam::DVec3;

const LEAF_TRIANGLES: usize = 16;
const STACK_CAPACITY: usize = 64;
const ORIGIN_OFFSET_M: f64 = 0.01;
const RAY_EPSILON_M: f64 = 1.0e-5;
const PARALLEL_RELATIVE_EPSILON: f64 = 1.0e-14;

/// Static triangle BVH for shadow rays against one reference mesh.
pub(super) struct TriangleShadows {
    vertices: Vec<DVec3>,
    triangles: Vec<[usize; 3]>,
    leaf_ids: Vec<usize>,
    nodes: Vec<Node>,
    bounds: (DVec3, DVec3),
}

#[derive(Clone, Copy, Debug)]
struct Aabb {
    min: DVec3,
    max: DVec3,
}

#[derive(Clone, Copy, Debug)]
struct Node {
    bounds: Aabb,
    left: usize,
    right: usize,
    first: usize,
    count: usize,
}

impl TriangleShadows {
    /// Copy and index the mesh, ignoring zero-area triangles.
    pub(super) fn new(vertices: &[DVec3], triangles: &[[usize; 3]]) -> Result<Self> {
        for (index, vertex) in vertices.iter().enumerate() {
            if !vertex.is_finite() {
                bail!("shadow mesh vertex {index} is non-finite");
            }
        }

        let mut valid_triangles = Vec::with_capacity(triangles.len());
        for (triangle_index, triangle) in triangles.iter().copied().enumerate() {
            if triangle.iter().any(|&index| index >= vertices.len()) {
                bail!("shadow triangle {triangle_index} has an out-of-range vertex index");
            }
            let [a, b, c] = triangle.map(|index| vertices[index]);
            let edge_scale = (b - a).length() * (c - a).length();
            if !edge_scale.is_finite() {
                bail!("shadow triangle {triangle_index} exceeds f64 geometric range");
            }
            let area_normal = (b - a).cross(c - a);
            if !area_normal.is_finite() {
                bail!("shadow triangle {triangle_index} exceeds f64 geometric range");
            }
            if area_normal == DVec3::ZERO {
                continue;
            }
            valid_triangles.push(triangle);
        }

        if valid_triangles.is_empty() {
            return Ok(Self {
                vertices: vertices.to_vec(),
                triangles: valid_triangles,
                leaf_ids: Vec::new(),
                nodes: Vec::new(),
                bounds: (DVec3::ZERO, DVec3::ZERO),
            });
        }

        let mut bounds = Aabb::empty();
        for triangle in &valid_triangles {
            for &index in triangle {
                bounds.include(vertices[index]);
            }
        }

        let mut leaf_ids: Vec<_> = (0..valid_triangles.len()).collect();
        let mut nodes = Vec::with_capacity(valid_triangles.len().div_ceil(LEAF_TRIANGLES) * 2);
        build_node(vertices, &valid_triangles, &mut leaf_ids, 0, &mut nodes);

        Ok(Self {
            vertices: vertices.to_vec(),
            triangles: valid_triangles,
            leaf_ids,
            nodes,
            bounds: (bounds.min, bounds.max),
        })
    }

    /// Return 0 when a triangle blocks the direction toward the light, else 1.
    /// Invalid query vectors are treated as clear to keep this infallible API safe.
    pub(super) fn visibility(&self, position: DVec3, toward_light: DVec3) -> f64 {
        if self.nodes.is_empty() || !position.is_finite() || !toward_light.is_finite() {
            return 1.0;
        }
        let scale = toward_light.abs().max_element();
        if !scale.is_finite() || scale == 0.0 {
            return 1.0;
        }
        let scaled = toward_light / scale;
        let length = scaled.length();
        if !length.is_finite() || length == 0.0 {
            return 1.0;
        }
        let direction = scaled / length;
        let origin = position + direction * ORIGIN_OFFSET_M;
        let mut stack = [0usize; STACK_CAPACITY];
        let mut stack_len = 1usize;
        stack[0] = 0;

        while stack_len > 0 {
            stack_len -= 1;
            let node_index = stack[stack_len];
            let node = self.nodes[node_index];
            if !node.bounds.intersects_ray(origin, direction) {
                continue;
            }
            if node.count != 0 {
                for &triangle_index in &self.leaf_ids[node.first..node.first + node.count] {
                    let triangle = self.triangles[triangle_index];
                    if ray_triangle(
                        origin,
                        direction,
                        self.vertices[triangle[0]],
                        self.vertices[triangle[1]],
                        self.vertices[triangle[2]],
                    ) {
                        return 0.0;
                    }
                }
            } else {
                // The tree is median-balanced. A 64-slot stack exceeds its
                // maximum depth for the supported usize-addressable triangle set.
                if stack_len + 2 > stack.len() {
                    return 1.0;
                }
                stack[stack_len] = node.right;
                stack[stack_len + 1] = node.left;
                stack_len += 2;
            }
        }
        1.0
    }

    pub(super) fn bounds(&self) -> (DVec3, DVec3) {
        self.bounds
    }

    pub(super) fn triangle_count(&self) -> usize {
        self.triangles.len()
    }

    pub(super) fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

impl Aabb {
    fn empty() -> Self {
        Self {
            min: DVec3::splat(f64::INFINITY),
            max: DVec3::splat(f64::NEG_INFINITY),
        }
    }

    fn include(&mut self, point: DVec3) {
        self.min = self.min.min(point);
        self.max = self.max.max(point);
    }

    fn largest_axis(self) -> usize {
        let extent = self.max - self.min;
        if extent.x >= extent.y && extent.x >= extent.z {
            0
        } else if extent.y >= extent.z {
            1
        } else {
            2
        }
    }

    fn intersects_ray(self, origin: DVec3, direction: DVec3) -> bool {
        let mut near = RAY_EPSILON_M;
        let mut far = f64::INFINITY;
        for axis in 0..3 {
            let o = origin[axis];
            let d = direction[axis];
            if d == 0.0 {
                if o < self.min[axis] || o > self.max[axis] {
                    return false;
                }
                continue;
            }
            let inverse = 1.0 / d;
            let mut t0 = (self.min[axis] - o) * inverse;
            let mut t1 = (self.max[axis] - o) * inverse;
            if t0 > t1 {
                std::mem::swap(&mut t0, &mut t1);
            }
            near = near.max(t0);
            far = far.min(t1);
            if far < near {
                return false;
            }
        }
        far >= near
    }
}

fn build_node(
    vertices: &[DVec3],
    triangles: &[[usize; 3]],
    leaf_ids: &mut [usize],
    first: usize,
    nodes: &mut Vec<Node>,
) -> usize {
    let mut bounds = Aabb::empty();
    for &triangle_id in leaf_ids.iter() {
        for &vertex_index in &triangles[triangle_id] {
            bounds.include(vertices[vertex_index]);
        }
    }
    let node_index = nodes.len();
    nodes.push(Node {
        bounds,
        left: 0,
        right: 0,
        first,
        count: leaf_ids.len(),
    });
    if leaf_ids.len() <= LEAF_TRIANGLES {
        return node_index;
    }

    let axis = bounds.largest_axis();
    let split = leaf_ids.len() / 2;
    leaf_ids.select_nth_unstable_by(split, |&a, &b| {
        let centroid = |triangle_id: usize| {
            let triangle = triangles[triangle_id];
            vertices[triangle[0]][axis] / 3.0
                + vertices[triangle[1]][axis] / 3.0
                + vertices[triangle[2]][axis] / 3.0
        };
        centroid(a).total_cmp(&centroid(b)).then_with(|| a.cmp(&b))
    });
    let (left_ids, right_ids) = leaf_ids.split_at_mut(split);
    let left = build_node(vertices, triangles, left_ids, first, nodes);
    let right = build_node(vertices, triangles, right_ids, first + split, nodes);
    nodes[node_index].left = left;
    nodes[node_index].right = right;
    nodes[node_index].count = 0;
    node_index
}

fn ray_triangle(origin: DVec3, direction: DVec3, a: DVec3, b: DVec3, c: DVec3) -> bool {
    let edge1 = b - a;
    let edge2 = c - a;
    let edge_scale = edge1.length() * edge2.length();
    if !edge_scale.is_finite() || edge_scale == 0.0 {
        return false;
    }
    let p = direction.cross(edge2);
    let determinant = edge1.dot(p);
    if !determinant.is_finite() || determinant.abs() <= PARALLEL_RELATIVE_EPSILON * edge_scale {
        return false;
    }
    let inverse = 1.0 / determinant;
    let from_a = origin - a;
    let u = from_a.dot(p) * inverse;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = from_a.cross(edge1);
    let v = direction.dot(q) * inverse;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let distance = edge2.dot(q) * inverse;
    distance > RAY_EPSILON_M
}

#[cfg(test)]
mod tests {
    use super::*;

    fn triangle_shadows(triangles: &[[DVec3; 3]]) -> TriangleShadows {
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        for triangle in triangles {
            let base = vertices.len();
            vertices.extend_from_slice(triangle);
            indices.push([base, base + 1, base + 2]);
        }
        TriangleShadows::new(&vertices, &indices).unwrap()
    }

    #[test]
    fn raised_triangle_blocks_flat_receiver_but_exposed_receiver_stays_clear() {
        let shadows = triangle_shadows(&[
            [
                DVec3::new(-2.0, -2.0, 0.0),
                DVec3::new(2.0, -2.0, 0.0),
                DVec3::new(0.0, 2.0, 0.0),
            ],
            [
                DVec3::new(-0.5, -0.5, 10.0),
                DVec3::new(0.5, -0.5, 10.0),
                DVec3::new(0.0, 0.5, 10.0),
            ],
        ]);
        assert_eq!(shadows.visibility(DVec3::ZERO, DVec3::Z), 0.0);
        assert_eq!(shadows.visibility(DVec3::new(0.0, 1.5, 0.0), DVec3::Z), 1.0);
        assert_eq!(shadows.triangle_count(), 2);
        assert!(shadows.node_count() > 0);
    }

    #[test]
    fn sloped_receiver_does_not_shadow_itself_from_a_parallel_light_ray() {
        let shadows = triangle_shadows(&[[
            DVec3::new(-1.0, -1.0, -1.0),
            DVec3::new(1.0, -1.0, 1.0),
            DVec3::new(0.0, 1.0, 0.0),
        ]]);
        let point = DVec3::new(0.0, 0.0, 0.0);
        let toward_light = DVec3::new(1.0, 0.0, 1.0).normalize();
        assert_eq!(shadows.visibility(point, toward_light), 1.0);
    }

    #[test]
    fn slab_test_handles_zero_direction_axes() {
        let bounds = Aabb {
            min: DVec3::new(-1.0, -1.0, 2.0),
            max: DVec3::new(1.0, 1.0, 4.0),
        };
        assert!(bounds.intersects_ray(DVec3::ZERO, DVec3::Z));
        assert!(!bounds.intersects_ray(DVec3::new(2.0, 0.0, 0.0), DVec3::Z));
    }

    #[test]
    fn ray_triangle_handles_known_hit_miss_and_geometry_behind_origin() {
        let a = DVec3::new(-1.0, -1.0, 3.0);
        let b = DVec3::new(1.0, -1.0, 3.0);
        let c = DVec3::new(0.0, 1.0, 3.0);
        assert!(ray_triangle(DVec3::ZERO, DVec3::Z, a, b, c));
        assert!(!ray_triangle(DVec3::new(2.0, 0.0, 0.0), DVec3::Z, a, b, c));
        assert!(!ray_triangle(DVec3::new(0.0, 0.0, 4.0), DVec3::Z, a, b, c));
    }

    #[test]
    fn empty_and_degenerate_geometry_is_clear_with_zero_bounds() {
        let empty = TriangleShadows::new(&[], &[]).unwrap();
        assert_eq!(empty.visibility(DVec3::ZERO, DVec3::Z), 1.0);
        assert_eq!(empty.bounds(), (DVec3::ZERO, DVec3::ZERO));
        assert_eq!(empty.triangle_count(), 0);
        assert_eq!(empty.node_count(), 0);

        let vertices = [DVec3::ONE, DVec3::ONE, DVec3::ONE];
        let degenerate = TriangleShadows::new(&vertices, &[[0, 1, 2]]).unwrap();
        assert_eq!(degenerate.visibility(DVec3::ZERO, DVec3::Z), 1.0);
        assert_eq!(degenerate.bounds(), (DVec3::ZERO, DVec3::ZERO));
    }

    #[test]
    fn multi_node_traversal_agrees_with_exhaustive_intersections() {
        let triangles: Vec<_> = (0..80)
            .map(|i| {
                let x = (i % 10) as f64 * 3.0 - 13.5;
                let y = (i / 10) as f64 * 3.0 - 10.5;
                let z = 2.0 + (i % 7) as f64;
                [
                    DVec3::new(x - 0.9, y - 0.8, z),
                    DVec3::new(x + 0.9, y - 0.8, z),
                    DVec3::new(x, y + 0.9, z + 0.4),
                ]
            })
            .collect();
        let shadows = triangle_shadows(&triangles);
        assert!(shadows.node_count() > 3);
        for y in -13..=13 {
            for x in -17..=17 {
                let position = DVec3::new(x as f64, y as f64, 0.0);
                for direction in [DVec3::Z, DVec3::new(0.3, -0.2, 1.0).normalize()] {
                    let origin = position + direction * ORIGIN_OFFSET_M;
                    let blocked = triangles
                        .iter()
                        .any(|t| ray_triangle(origin, direction, t[0], t[1], t[2]));
                    assert_eq!(
                        shadows.visibility(position, direction),
                        if blocked { 0.0 } else { 1.0 }
                    );
                }
            }
        }
    }

    #[test]
    fn malformed_mesh_data_is_rejected() {
        assert!(TriangleShadows::new(&[DVec3::splat(f64::NAN)], &[]).is_err());
        assert!(TriangleShadows::new(&[DVec3::ZERO], &[[0, 1, 2]]).is_err());
    }
}
