//! Sixteen shared boundary-collapse index variants; no skirts or per-patch indices.
pub const GRID_CELLS: u32 = 16;
pub const GRID_SAMPLES: usize = 289;

pub struct SurfaceTopology {
    variants: [Vec<u16>; 16],
    triangles: Box<[[u16; 3]]>,
}
impl Default for SurfaceTopology {
    fn default() -> Self {
        Self::new()
    }
}
impl SurfaceTopology {
    pub fn new() -> Self {
        let variants = std::array::from_fn(|mask| {
            let remap = |index: u32| {
                let mut i = index % 17;
                let mut j = index / 17;
                if (i == 0 && mask & 1 != 0) || (i == 16 && mask & 2 != 0) {
                    j -= j % 2;
                }
                if (j == 0 && mask & 4 != 0) || (j == 16 && mask & 8 != 0) {
                    i -= i % 2;
                }
                (j * 17 + i) as u16
            };
            let mut indices = Vec::with_capacity(1536);
            for j in 0..16 {
                for i in 0..16 {
                    let a = j * 17 + i;
                    let b = a + 1;
                    let c = a + 17;
                    let d = c + 1;
                    for triangle in [[a, b, d], [a, d, c]] {
                        let [a, b, c] = triangle.map(remap);
                        if a != b && b != c && a != c {
                            indices.extend([a, b, c]);
                        }
                    }
                }
            }
            indices
        });
        // Most interior triangles are identical across masks. Enumerate their union
        // once for conservative metadata without sixteen redundant interior passes.
        let mut triangles: Vec<_> = variants
            .iter()
            .flat_map(|v| v.as_chunks::<3>().0.iter().copied())
            .collect();
        triangles.sort_unstable();
        triangles.dedup();
        Self {
            variants,
            triangles: triangles.into_boxed_slice(),
        }
    }
    pub fn indices(&self, mask: u8) -> &[u16] {
        &self.variants[usize::from(mask)]
    }
    pub fn allocated_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self
                .variants
                .iter()
                .map(|v| v.capacity() * 2)
                .sum::<usize>()
            + self.triangles.len() * 6
    }
    pub(crate) fn triangles(&self) -> &[[u16; 3]] {
        &self.triangles
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_math::surface::*;
    use std::collections::BTreeMap;
    fn grid(i: u16) -> [i32; 2] {
        [i32::from(i % 17), i32::from(i / 17)]
    }
    fn area(a: [i32; 2], b: [i32; 2], c: [i32; 2]) -> i32 {
        (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
    }
    #[test]
    fn every_mask_manifold_area_diameter_and_actual_spherical_winding() {
        let topology = SurfaceTopology::new();
        for mask in 0..16 {
            let mut edges = BTreeMap::<(u16, u16), (u32, i32)>::new();
            let mut total = 0;
            for &[a, b, c] in topology.indices(mask).as_chunks::<3>().0 {
                let domain = [a, b, c].map(grid);
                let signed = area(domain[0], domain[1], domain[2]);
                assert!(signed > 0);
                total += signed;
                for (x, y) in [(a, b), (b, c), (c, a)] {
                    let entry = edges.entry((x.min(y), x.max(y))).or_default();
                    entry.0 += 1;
                    entry.1 += if x < y { 1 } else { -1 };
                    let [p, q] = [grid(x), grid(y)];
                    assert!((q[0] - p[0]).pow(2) + (q[1] - p[1]).pow(2) <= 5);
                }
                for face in CubeFace::ALL {
                    for level in [0, 1, 5, 16, 30] {
                        let count = 1u32 << level;
                        let patch =
                            CubePatchAddress::try_new(face, level, count - 1, count - 1).unwrap();
                        let [a, b, c] = [a, b, c].map(|i| {
                            patch
                                .sample_direction(u32::from(i % 17), u32::from(i / 17), 16)
                                .unwrap()
                                .unit()
                        });
                        assert!((b - a).cross(c - a).dot(a) > 0.0);
                    }
                }
            }
            assert_eq!(total, 512);
            for ((a, b), (incidence, orientation)) in edges {
                assert!(incidence == 1 || incidence == 2);
                if incidence == 2 {
                    assert_eq!(orientation, 0);
                } else {
                    let [p, q] = [grid(a), grid(b)];
                    let edge = if p[0] == 0 && q[0] == 0 {
                        PatchEdge::UMin
                    } else if p[0] == 16 && q[0] == 16 {
                        PatchEdge::UMax
                    } else if p[1] == 0 && q[1] == 0 {
                        PatchEdge::VMin
                    } else {
                        assert!(p[1] == 16 && q[1] == 16);
                        PatchEdge::VMax
                    };
                    let varying = if matches!(edge, PatchEdge::UMin | PatchEdge::UMax) {
                        1
                    } else {
                        0
                    };
                    assert_eq!(
                        (p[varying] - q[varying]).abs(),
                        if mask & edge.bit() != 0 { 2 } else { 1 }
                    );
                }
            }
            // Independent dense points must belong to exactly one interior triangle.
            for j in 0..64 {
                for i in 0..64 {
                    let p = [i as f64 * 0.25 + 0.073, j as f64 * 0.25 + 0.119];
                    let count = topology
                        .indices(mask)
                        .as_chunks::<3>()
                        .0
                        .iter()
                        .filter(|tri| {
                            let v = tri.map(grid);
                            (0..3).all(|k| {
                                let a = v[k];
                                let b = v[(k + 1) % 3];
                                (b[0] - a[0]) as f64 * (p[1] - a[1] as f64)
                                    - (b[1] - a[1]) as f64 * (p[0] - a[0] as f64)
                                    > 0.0
                            })
                        })
                        .count();
                    assert_eq!(count, 1, "mask={mask} point={p:?}");
                }
            }
        }
        assert!(topology.allocated_bytes() < 80 * 1024);
    }
    #[test]
    fn cross_face_fine_coarse_drawn_boundary_keys_match() {
        let topology = SurfaceTopology::new();
        for face in CubeFace::ALL {
            for edge in PatchEdge::ALL {
                for level in [0, 1, 5, 16, 29] {
                    let count = 1u32 << level;
                    let [x, y] = edge.grid(count - 1, count - 1);
                    let coarse = CubePatchAddress::try_new(face, level, x, y).unwrap();
                    let neighbor = coarse.neighbor(edge);
                    for child in neighbor.address.children().unwrap() {
                        let [cx, cy] = child.coordinates();
                        let touches = match neighbor.edge {
                            PatchEdge::UMin => cx % 2 == 0,
                            PatchEdge::UMax => cx % 2 == 1,
                            PatchEdge::VMin => cy % 2 == 0,
                            PatchEdge::VMax => cy % 2 == 1,
                        };
                        if !touches {
                            continue;
                        }
                        let drawn = topology.indices(neighbor.edge.bit());
                        for along in (0..=16).step_by(2) {
                            let [i, j] = neighbor.edge.grid(along, 16);
                            assert!(drawn.contains(&((j * 17 + i) as u16)));
                            let key = child.sample_key(i, j, 16).unwrap();
                            assert!((0..=16).any(|k| {
                                let [a, b] = edge.grid(k, 16);
                                coarse.sample_key(a, b, 16).unwrap() == key
                            }));
                        }
                    }
                }
            }
        }
    }
}
