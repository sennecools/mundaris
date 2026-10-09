//! CPU f64 world-map composition (W2, biome catalog and heightmap variants):
//!
//! height(dir) = macro(dir) + Σ_b w_b · Σ_s w_s · detail_{b,s}(dir)
//!
//! - `macro` and the biome weights `w_b` are bilinear samples of the world map.
//! - `w_s` are the catalog's sub-biome weights over world-map fields.
//! - `detail_{b,s}` samples library tiles on a lattice of `cell_m` cells laid
//!   on each cube face's equi-angular chart (x = R·atan u, y = R·atan v). Each
//!   cell picks a variant, one of 8 dihedral orientations and an offset from
//!   an integer hash of (seed, biome, sub-biome, face, cell).
//! - Cells cross-fade over `cell_blend_m`, and cube faces cross-fade over the
//!   same width across their edges, so there are no seams.
//! - Every blend (cells, faces, sub-biomes, biomes) is variance-preserving:
//!   Σ wᵢ dᵢ / √Σ wᵢ², so detail keeps its amplitude where regions overlap
//!   instead of flattening (tiles are zero-mean after the high-pass).
//!
//! Library tiles are real-scale; the world map is at game scale. Composition
//! uses game metres on the body's game radius, which is the world-scale
//! contract (`docs/GAME_AND_SYSTEM_DIRECTION.md`): ground detail stays real.

use super::catalog::{BiomeCatalog, WorldField};
use super::cube::{CubeMap, neighbours, texel_directions};
use super::detail::DetailTile;
use glam::DVec3;
use astrum_math::noise::gradient_noise_value;
use astrum_math::surface::CubeFace;

/// Biome weights fade out smoothly below about twice this.
const MIN_WEIGHT: f64 = 1.0 / 64.0;

pub struct ComposedSurface {
    radius_m: f64,
    elevation: CubeMap<f32>,
    slope: CubeMap<f32>,
    /// One weight map per catalog biome (in catalog order).
    biome_weights: Vec<CubeMap<f32>>,
    catalog: BiomeCatalog,
    tiles: Vec<DetailTile>,
    /// [biome][sub-biome][variant] → index into `tiles`.
    variant_tiles: Vec<Vec<Vec<usize>>>,
}

/// One lattice cell touching a sample point.
#[derive(Debug, Clone, Copy)]
struct CellSample {
    face: usize,
    cell: (i64, i64),
    /// Position relative to the cell origin (m).
    local: (f64, f64),
    weight: f64,
}

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn cell_hash(seed: u64, keys: [u64; 5]) -> u64 {
    keys.iter()
        .enumerate()
        .fold(mix(seed ^ 0x6269_6F6D_6573_7631), |h, (k, v)| {
            mix(h ^ v
                .rotate_left(13 * k as u32)
                .wrapping_add(0x9E37_79B9_7F4A_7C15))
        })
}

fn unit(h: u64) -> f64 {
    (h >> 11) as f64 / (1u64 << 53) as f64
}

/// Variance-preserving combination of weighted values.
fn vp_blend(items: impl Iterator<Item = (f64, f64)>) -> f64 {
    let (mut sum, mut sq) = (0.0, 0.0);
    for (w, v) in items {
        sum += w * v;
        sq += w * w;
    }
    if sq > 0.0 { sum / sq.sqrt() } else { 0.0 }
}

/// 1-D cell weights at fractional position `f` within cell `c`: the own
/// cell, plus a neighbour inside the blend band (half-width `h`, cell units).
fn axis_weights(c: i64, f: f64, h: f64) -> [(i64, f64); 2] {
    if f < h {
        let own = super::catalog::smoothstep(-h, h, f);
        [(c, own), (c - 1, 1.0 - own)]
    } else if f > 1.0 - h {
        let own = 1.0 - super::catalog::smoothstep(1.0 - h, 1.0 + h, f);
        [(c, own), (c + 1, 1.0 - own)]
    } else {
        [(c, 1.0), (c, 0.0)]
    }
}

impl ComposedSurface {
    /// `elevation` and `biomes` are a world map (game metres, weights in
    /// catalog channel order) for a body of game radius `radius_m`. Every
    /// catalog variant must be among `tiles` with the expected hash.
    pub fn new(
        radius_m: f64,
        elevation: CubeMap<f32>,
        biomes: &CubeMap<[f32; 4]>,
        catalog: BiomeCatalog,
        tiles: Vec<DetailTile>,
    ) -> Result<Self, String> {
        let names: Vec<&str> = catalog.channels.iter().map(String::as_str).collect();
        if names.len() != 4 {
            return Err("composition currently binds exactly 4 world-map channels".into());
        }
        catalog.validate(&names)?;
        if elevation.n() != biomes.n() {
            return Err("elevation and biome maps differ in resolution".into());
        }
        let biome_weights = catalog
            .biomes
            .iter()
            .map(|b| {
                let ch = catalog
                    .channels
                    .iter()
                    .position(|c| c == &b.channel)
                    .unwrap_or(0);
                let mut m = CubeMap::new(biomes.n(), 0.0f32);
                for (d, s) in m.data_mut().iter_mut().zip(biomes.data()) {
                    *d = s[ch];
                }
                m
            })
            .collect();
        let mut variant_tiles = Vec::new();
        for b in &catalog.biomes {
            let mut subs = Vec::new();
            for s in &b.sub_biomes {
                let mut vars = Vec::new();
                for v in &s.variants {
                    let k = tiles
                        .iter()
                        .position(|t| t.bundle_id == v.bundle && t.height_sha256 == v.height_sha256)
                        .ok_or_else(|| {
                            format!("variant {} ({}) not loaded", v.bundle, v.height_sha256)
                        })?;
                    vars.push(k);
                }
                subs.push(vars);
            }
            variant_tiles.push(subs);
        }
        let slope = slope_map(&elevation, radius_m);
        Ok(Self {
            radius_m,
            elevation,
            slope,
            biome_weights,
            catalog,
            tiles,
            variant_tiles,
        })
    }

    pub fn catalog(&self) -> &BiomeCatalog {
        &self.catalog
    }

    /// World-map elevation, Catmull-Rom sampled so slopes are continuous.
    pub fn macro_height(&self, dir: DVec3) -> f64 {
        self.elevation.bicubic(dir)
    }

    /// Normalised catalog-biome weights. Small weights fade out smoothly
    /// (w · smoothstep(0, 2·MIN_WEIGHT, w)) rather than being cut, so the
    /// composed height stays continuous where a biome ends.
    pub fn biome_weights(&self, dir: DVec3) -> Vec<f64> {
        let mut w: Vec<f64> = self
            .biome_weights
            .iter()
            .map(|m| m.bilinear(dir).max(0.0))
            .collect();
        let total: f64 = w.iter().sum();
        w.iter_mut().for_each(|v| *v /= total.max(1e-12));
        w.iter_mut()
            .for_each(|v| *v *= super::catalog::smoothstep(0.0, 2.0 * MIN_WEIGHT, *v));
        let total: f64 = w.iter().sum();
        w.iter_mut().for_each(|v| *v /= total.max(1e-12));
        w
    }

    /// Normalised sub-biome weights of biome `b`.
    pub fn sub_biome_weights(&self, b: usize, dir: DVec3) -> Vec<f64> {
        let elevation = self.macro_height(dir);
        let slope = self.slope.bilinear(dir);
        let mut w: Vec<f64> = self.catalog.biomes[b]
            .sub_biomes
            .iter()
            .map(|s| {
                s.select.as_ref().map_or(1.0, |r| {
                    r.weight(match r.field {
                        WorldField::ElevationM => elevation,
                        WorldField::Slope => slope,
                    })
                }) + 1e-9
            })
            .collect();
        let total: f64 = w.iter().sum();
        w.iter_mut().for_each(|v| *v /= total);
        w
    }

    /// Lattice cells (over every cube face whose cross-fade band reaches
    /// `dir`) with normalised weights.
    fn cells(&self, dir: DVec3) -> Vec<CellSample> {
        let cell = self.catalog.cell_m;
        let band = 0.5 * self.catalog.cell_blend_m / self.radius_m; // radians
        let h = 0.5 * self.catalog.cell_blend_m / cell;
        let quarter = std::f64::consts::FRAC_PI_4;
        let mut out = Vec::with_capacity(8);
        for (face, f) in CubeFace::ALL.iter().enumerate() {
            let [normal, u_axis, v_axis] = f.basis();
            let w = dir.dot(normal);
            if w <= 0.1 {
                continue;
            }
            // Warp fields are noise of the direction itself, so each is one
            // continuous field over the sphere.
            let p = dir * (self.radius_m / cell);
            let warp = |salt: u64| {
                gradient_noise_value(self.catalog.seed ^ salt, p).unwrap_or(0.0)
                    * self.catalog.cell_warp_m
                    / cell
            };
            let (a, b) = ((dir.dot(u_axis) / w).atan(), (dir.dot(v_axis) / w).atan());
            // A per-face warp of the margin makes face borders meander: the
            // border between faces A and B moves by (m_B − m_A) / 2.
            let face_warp = warp(0x6661_6365_0000 + face as u64) * cell / self.radius_m;
            let margin = quarter - a.abs().max(b.abs()) + face_warp;
            let face_w = super::catalog::smoothstep(-band, band, margin);
            if face_w <= 0.0 {
                continue;
            }
            // Unwarped chart position (cell units) places the tile texture;
            // the warped one only decides cell membership and weights, so
            // tiles are never stretched.
            let (x, y) = (a * self.radius_m / cell, b * self.radius_m / cell);
            let (wx_pos, wy_pos) = (x + warp(0x7761_7270_5F78), y + warp(0x7761_7270_5F79));
            let (cx, cy) = (wx_pos.floor() as i64, wy_pos.floor() as i64);
            for (ix, wx) in axis_weights(cx, wx_pos - cx as f64, h) {
                for (iy, wy) in axis_weights(cy, wy_pos - cy as f64, h) {
                    let weight = face_w * wx * wy;
                    if weight > 0.0 {
                        out.push(CellSample {
                            face,
                            cell: (ix, iy),
                            local: ((x - ix as f64) * cell, (y - iy as f64) * cell),
                            weight,
                        });
                    }
                }
            }
        }
        let total: f64 = out.iter().map(|c| c.weight).sum();
        out.iter_mut().for_each(|c| c.weight /= total.max(1e-12));
        out
    }

    /// Detail of one sub-biome in one lattice cell.
    fn cell_detail(&self, b: usize, s: usize, c: &CellSample) -> f64 {
        let sub = &self.catalog.biomes[b].sub_biomes[s];
        let key = [
            b as u64,
            s as u64,
            c.face as u64,
            c.cell.0 as u64,
            c.cell.1 as u64,
        ];
        let h = cell_hash(self.catalog.seed, key);
        // Variant by weight.
        let total: f64 = sub.variants.iter().map(|v| v.weight).sum();
        let mut pick = unit(h) * total;
        let mut k = sub.variants.len() - 1;
        for (i, v) in sub.variants.iter().enumerate() {
            if pick < v.weight {
                k = i;
                break;
            }
            pick -= v.weight;
        }
        let tile = &self.tiles[self.variant_tiles[b][s][k]];
        let h2 = mix(h ^ 0xD1B5_4A32_D192_ED03);
        let (mut x, mut y) = c.local;
        let dihedral = h2 & 7;
        if dihedral & 4 != 0 {
            std::mem::swap(&mut x, &mut y);
        }
        if dihedral & 1 != 0 {
            x = -x;
        }
        if dihedral & 2 != 0 {
            y = -y;
        }
        let h3 = mix(h2);
        let ox = unit(h3) * tile.footprint_m;
        let oy = unit(mix(h3)) * tile.footprint_m;
        sub.amplitude * tile.sample(x + ox, y + oy)
    }

    /// Composed detail height (m) at `dir`.
    pub fn detail(&self, dir: DVec3) -> f64 {
        let cells = self.cells(dir);
        let biomes = self.biome_weights(dir);
        vp_blend(
            biomes
                .iter()
                .enumerate()
                .filter(|(_, w)| **w > 0.0)
                .map(|(b, &wb)| {
                    let subs = self.sub_biome_weights(b, dir);
                    let d = vp_blend(subs.iter().enumerate().filter(|(_, w)| **w > 1e-6).map(
                        |(s, &ws)| {
                            (
                                ws,
                                vp_blend(
                                    cells.iter().map(|c| (c.weight, self.cell_detail(b, s, c))),
                                ),
                            )
                        },
                    ));
                    (wb, d)
                }),
        )
    }

    /// Composed height above the reference radius (game m).
    pub fn height(&self, dir: DVec3) -> f64 {
        self.macro_height(dir) + self.detail(dir)
    }
}

/// Smoothing passes over the slope field (5-point average, crossing face
/// edges); σ grows by about 0.7 texel per pass, so 16 passes ≈ 2.8 texels.
const SLOPE_SMOOTHING_PASSES: usize = 16;

/// Regional macro slope (rise over run) per texel: central differences
/// across the four neighbours, then smoothed. Sub-biomes are regional, so
/// selection must not follow every crater wall: on the raw gradient
/// magnitude a soft threshold can flip within a few metres.
fn slope_map(elevation: &CubeMap<f32>, radius_m: f64) -> CubeMap<f32> {
    let n = elevation.n();
    let dirs = texel_directions(n);
    let h = elevation.data();
    let mut out = CubeMap::new(n, 0.0f32);
    for (k, v) in out.data_mut().iter_mut().enumerate() {
        let [e, w, s, nn] = neighbours(k, n);
        let dist = |a: usize, b: usize| dirs[a].dot(dirs[b]).clamp(-1.0, 1.0).acos() * radius_m;
        let gx = f64::from(h[e] - h[w]) / dist(e, w).max(1e-9);
        let gy = f64::from(h[s] - h[nn]) / dist(s, nn).max(1e-9);
        *v = (gx * gx + gy * gy).sqrt() as f32;
    }
    let adjacency: Vec<[usize; 4]> = (0..6 * n * n).map(|k| neighbours(k, n)).collect();
    let mut next = out.clone();
    for _ in 0..SLOPE_SMOOTHING_PASSES {
        for (k, v) in next.data_mut().iter_mut().enumerate() {
            let s = out.data();
            *v = 0.5 * s[k] + 0.125 * adjacency[k].iter().map(|&j| s[j]).sum::<f32>();
        }
        std::mem::swap(&mut out, &mut next);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::world_map::catalog::tests::catalog;
    use crate::terrain::world_map::detail::tests::synthetic;

    const R: f64 = 50_000.0;

    /// A tiny world: flat, with "rough" in the +X half and "smooth" in −X,
    /// blended across a band around x = 0; channels 3 and 4 unused.
    fn surface() -> ComposedSurface {
        let n = 32;
        let dirs = texel_directions(n);
        let mut elevation = CubeMap::new(n, 0.0f32);
        let mut biomes = CubeMap::new(n, [0.0f32; 4]);
        for (k, d) in dirs.iter().enumerate() {
            elevation.data_mut()[k] = (100.0 * d.y) as f32;
            let t = super::super::catalog::smoothstep(-0.2, 0.2, d.x) as f32;
            biomes.data_mut()[k] = [t, 1.0 - t, 0.0, 0.0];
        }
        let mut cat = catalog();
        cat.channels = ["rough", "smooth", "unused_a", "unused_b"]
            .map(String::from)
            .to_vec();
        // Bind the two spare channels so the catalog validates.
        for spare in ["unused_a", "unused_b"] {
            let mut b = cat.biomes[1].clone();
            b.id = spare.into();
            b.channel = spare.into();
            b.sub_biomes[0].id = format!("{spare}_all");
            cat.biomes.push(b);
        }
        let tile = |id: &str, seed: u64| {
            let mut t =
                DetailTile::from_heights(id, 64, 4000.0, synthetic(64, seed), 1500.0).unwrap();
            t.height_sha256 = "0".repeat(64);
            t
        };
        ComposedSurface::new(
            R,
            elevation,
            &biomes,
            cat,
            vec![tile("a", 1), tile("b", 2), tile("c", 3)],
        )
        .unwrap()
    }

    fn tangent_walk(start: DVec3, step_m: f64, count: usize) -> Vec<DVec3> {
        let e = start.any_orthonormal_vector();
        (0..count)
            .map(|k| (start + e * (k as f64 * step_m / R)).normalize())
            .collect()
    }

    #[test]
    fn composition_is_deterministic_and_finite() {
        let (a, b) = (surface(), surface());
        for d in tangent_walk(DVec3::new(0.3, 0.5, 0.8).normalize(), 37.0, 400) {
            let (x, y) = (a.height(d), b.height(d));
            assert!(x.is_finite());
            assert_eq!(x.to_bits(), y.to_bits());
        }
    }

    #[test]
    fn cell_and_face_weights_form_a_partition_of_unity() {
        let s = surface();
        let quarter = std::f64::consts::FRAC_PI_4.tan();
        // Points on a face interior, across a cell edge, on a face edge and
        // near a cube corner.
        let probes = [
            DVec3::new(1.0, 0.01, 0.02),
            DVec3::new(1.0, 4000.0 / R, 0.0),
            DVec3::new(1.0, quarter, 0.3),
            DVec3::new(1.0, 0.999, 0.998),
        ];
        for p in probes {
            let cells = s.cells(p.normalize());
            let total: f64 = cells.iter().map(|c| c.weight).sum();
            assert!((total - 1.0).abs() < 1e-12, "{p}: {total}");
        }
        assert!(s.cells(DVec3::new(1.0, quarter, 0.3).normalize()).len() >= 2);
    }

    #[test]
    fn detail_is_continuous_across_cell_and_face_edges() {
        let s = surface();
        // Walk across a cube-face edge (x = y) and many cell edges in 2 m
        // steps; the largest step must stay within the tile's own slopes.
        let start = DVec3::new(1.0, 0.95, 0.1).normalize();
        let end = DVec3::new(0.95, 1.0, 0.1).normalize();
        let steps = ((start.angle_between(end) * R) / 2.0) as usize;
        let mut prev = s.detail(start);
        let mut max_step: f64 = 0.0;
        for k in 1..=steps {
            let d = start.lerp(end, k as f64 / steps as f64).normalize();
            let v = s.detail(d);
            max_step = max_step.max((v - prev).abs());
            prev = v;
        }
        // The synthetic tiles change by at most about 1 m per 2 m step.
        assert!(max_step < 1.5, "max step {max_step} m");
    }

    #[test]
    fn blends_preserve_detail_amplitude() {
        let s = surface();
        let rms = |centre: DVec3| {
            let pts = tangent_walk(centre, 61.0, 2000);
            (pts.iter().map(|d| s.detail(*d).powi(2)).sum::<f64>() / pts.len() as f64).sqrt()
        };
        // Pure "smooth" (amplitude 0.5), pure "rough" low sub-biome (1.0) and
        // the 50/50 border between them.
        let smooth = rms(DVec3::new(-1.0, -0.2, 0.1).normalize());
        let rough = rms(DVec3::new(1.0, -0.2, 0.1).normalize());
        let border = rms(DVec3::new(0.0, -0.2, 1.0).normalize());
        let expected = (0.5 * (smooth * smooth + rough * rough)).sqrt();
        assert!(smooth > 0.0 && rough > smooth);
        assert!(
            (border / expected - 1.0).abs() < 0.25,
            "{smooth} {rough} {border}"
        );
        // A plain weighted average would sink below both ends at the border.
        assert!(border > 0.5 * (smooth + rough) * 0.9);
    }

    #[test]
    fn sub_biomes_follow_their_field_ranges() {
        let s = surface();
        // `rough_low` below 0 m elevation (y < 0), `rough_high` above.
        let low = s.sub_biome_weights(0, DVec3::new(1.0, -0.5, 0.0).normalize());
        let high = s.sub_biome_weights(0, DVec3::new(1.0, 0.5, 0.0).normalize());
        assert!(low[0] > 0.99 && high[1] > 0.99, "{low:?} {high:?}");
    }
}
