//! Impostors: a grown plant baked into views on a hemi-octahedral grid
//! (Ryan Brucks' octahedral impostors, upper hemisphere only), so the far
//! tier looks like the near tree. Each frame stores unlit albedo + coverage
//! and the plant-frame normal, so the renderer lights impostors with the
//! same shading as the meshes.
//!
//! Frame layout: `GRID × GRID` frames of `FRAME × FRAME` texels in one
//! square atlas. Frame (i, j) looks at the plant from the direction
//! `frame_dir(i, j)`; its image plane is spanned by `frame_axes(dir)` and
//! covers `±radius` around `centre` (all in the plant frame, z up).

use glam::{DVec3, Vec2, Vec3};

use crate::mesh::Mesh;

pub const GRID: usize = 4;
pub const FRAME: usize = 64;
pub const ATLAS: usize = GRID * FRAME;
const SS: usize = 2;

/// Hemi-octahedral decode: grid uv in [0, 1]² → unit direction with z ≥ 0.
pub fn hemi_oct_decode(u: f64, v: f64) -> DVec3 {
    let x = 2.0 * u - 1.0;
    let y = 2.0 * v - 1.0;
    // Rotated 45°: square → diamond.
    let px = 0.5 * (x + y);
    let py = 0.5 * (x - y);
    let z = 1.0 - px.abs() - py.abs();
    DVec3::new(px, py, z.max(0.0)).normalize_or(DVec3::Z)
}

/// Inverse of [`hemi_oct_decode`] (z clamped to the upper hemisphere).
pub fn hemi_oct_encode(d: DVec3) -> (f64, f64) {
    let d = DVec3::new(d.x, d.y, d.z.max(0.0));
    let s = d.x.abs() + d.y.abs() + d.z;
    let (px, py) = if s > 0.0 { (d.x / s, d.y / s) } else { (0.0, 0.0) };
    let x = px + py;
    let y = px - py;
    (0.5 * (x + 1.0), 0.5 * (y + 1.0))
}

/// View direction (toward the camera) of frame (i, j).
pub fn frame_dir(i: usize, j: usize) -> DVec3 {
    hemi_oct_decode((i as f64 + 0.5) / GRID as f64, (j as f64 + 0.5) / GRID as f64)
}

/// Image-plane axes (right, up) for a frame looking from `dir`.
pub fn frame_axes(dir: DVec3) -> (DVec3, DVec3) {
    let right = if dir.z > 0.999 { DVec3::X } else { DVec3::Z.cross(dir).normalize() };
    let up = dir.cross(right);
    (right, up)
}

/// Baked impostor of one plant variant.
pub struct Impostor {
    /// sRGB-encoded albedo, alpha = coverage (RGBA8, `ATLAS²`).
    pub albedo: Vec<u8>,
    /// Plant-frame normal × 0.5 + 0.5 (RGBA8), alpha = coverage.
    pub normal: Vec<u8>,
    /// Bounding-sphere centre (plant frame) and radius, metres.
    pub centre: Vec3,
    pub radius: f32,
}

fn srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { 12.92 * v } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

/// Bake `mesh` (decoded vertex colour × AO as albedo).
pub fn bake(mesh: &Mesh) -> Impostor {
    let (lo, hi) = mesh.bounds();
    let centre = (lo + hi) * 0.5;
    let radius = mesh
        .vertices
        .iter()
        .map(|v| (Vec3::from(v.position) - centre).length())
        .fold(0.05f32, f32::max);
    let n = FRAME * SS;
    let mut albedo = vec![0u8; ATLAS * ATLAS * 4];
    let mut normal = vec![0u8; ATLAS * ATLAS * 4];
    for j in 0..GRID {
        for i in 0..GRID {
            let dir = frame_dir(i, j).as_vec3();
            let (r, u) = frame_axes(frame_dir(i, j));
            let (r, u) = (r.as_vec3(), u.as_vec3());
            // Sample buffers.
            let mut col = vec![Vec3::ZERO; n * n];
            let mut nrm = vec![Vec3::ZERO; n * n];
            let mut depth = vec![f32::MAX; n * n];
            let mut cov = vec![false; n * n];
            let scale = n as f32 / (2.0 * radius);
            let proj: Vec<(Vec2, f32)> = mesh
                .vertices
                .iter()
                .map(|v| {
                    let p = Vec3::from(v.position) - centre;
                    (Vec2::new((p.dot(r) + radius) * scale, (radius - p.dot(u)) * scale), -p.dot(dir))
                })
                .collect();
            for t in mesh.indices.as_chunks::<3>().0 {
                let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
                let (pa, pb, pc) = (proj[a].0, proj[b].0, proj[c].0);
                let area = (pb - pa).perp_dot(pc - pa);
                if area.abs() < 1e-12 {
                    continue;
                }
                let x0 = pa.x.min(pb.x).min(pc.x).floor().max(0.0) as usize;
                let x1 = (pa.x.max(pb.x).max(pc.x).ceil() as usize).min(n - 1);
                let y0 = pa.y.min(pb.y).min(pc.y).floor().max(0.0) as usize;
                let y1 = (pa.y.max(pb.y).max(pc.y).ceil() as usize).min(n - 1);
                let vtx = |k: usize| &mesh.vertices[k];
                let decode = |k: usize| {
                    let v = vtx(k);
                    let c = Vec3::new(v.color[0] as f32, v.color[1] as f32, v.color[2] as f32) / 255.0;
                    (c * c * (v.color[3] as f32 / 255.0), Vec3::new(v.normal[0] as f32, v.normal[1] as f32, v.normal[2] as f32) / 127.0)
                };
                let ((ca, na), (cb, nb), (cc, nc)) = (decode(a), decode(b), decode(c));
                for y in y0..=y1 {
                    for x in x0..=x1 {
                        let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                        let w0 = (pb - p).perp_dot(pc - p) / area;
                        let w1 = (pc - p).perp_dot(pa - p) / area;
                        let w2 = 1.0 - w0 - w1;
                        if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                            continue;
                        }
                        let z = w0 * proj[a].1 + w1 * proj[b].1 + w2 * proj[c].1;
                        let k = y * n + x;
                        if z >= depth[k] {
                            continue;
                        }
                        depth[k] = z;
                        cov[k] = true;
                        col[k] = ca * w0 + cb * w1 + cc * w2;
                        let mut nn = (na * w0 + nb * w1 + nc * w2).normalize_or_zero();
                        // Two-sided cards: the side that faces the viewer.
                        if nn.dot(dir) < 0.0 {
                            nn = -nn;
                        }
                        nrm[k] = nn;
                    }
                }
            }
            // Resolve 2×2 samples; albedo un-premultiplied, then dilate
            // into empty texels so bilinear filtering has no dark halo.
            let mut texels: Vec<(Vec3, Vec3, f32)> = vec![(Vec3::ZERO, Vec3::Z, 0.0); FRAME * FRAME];
            for ty in 0..FRAME {
                for tx in 0..FRAME {
                    let (mut c, mut nm, mut w) = (Vec3::ZERO, Vec3::ZERO, 0.0f32);
                    for sy in 0..SS {
                        for sx in 0..SS {
                            let k = (ty * SS + sy) * n + tx * SS + sx;
                            if cov[k] {
                                c += col[k];
                                nm += nrm[k];
                                w += 1.0;
                            }
                        }
                    }
                    let a = w / (SS * SS) as f32;
                    texels[ty * FRAME + tx] = if w > 0.0 { (c / w, nm.normalize_or(Vec3::Z), a) } else { (Vec3::ZERO, Vec3::Z, 0.0) };
                }
            }
            for _ in 0..4 {
                let prev = texels.clone();
                for ty in 0..FRAME {
                    for tx in 0..FRAME {
                        if prev[ty * FRAME + tx].2 > 0.0 {
                            continue;
                        }
                        let (mut c, mut nm, mut w) = (Vec3::ZERO, Vec3::ZERO, 0.0);
                        for (dx, dy) in [(-1i32, 0i32), (1, 0), (0, -1), (0, 1)] {
                            let (x, y) = (tx as i32 + dx, ty as i32 + dy);
                            if x < 0 || y < 0 || x >= FRAME as i32 || y >= FRAME as i32 {
                                continue;
                            }
                            let t = prev[y as usize * FRAME + x as usize];
                            if t.2 > 0.0 || t.0 != Vec3::ZERO {
                                c += t.0;
                                nm += t.1;
                                w += 1.0;
                            }
                        }
                        if w > 0.0 {
                            texels[ty * FRAME + tx] = (c / w, (nm / w).normalize_or(Vec3::Z), 0.0);
                        }
                    }
                }
            }
            for ty in 0..FRAME {
                for tx in 0..FRAME {
                    let (c, nm, a) = texels[ty * FRAME + tx];
                    let o = ((j * FRAME + ty) * ATLAS + i * FRAME + tx) * 4;
                    let alpha = (a * 255.0).round() as u8;
                    albedo[o..o + 4].copy_from_slice(&[srgb(c.x), srgb(c.y), srgb(c.z), alpha]);
                    let e = |v: f32| ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8;
                    normal[o..o + 4].copy_from_slice(&[e(nm.x), e(nm.y), e(nm.z), alpha]);
                }
            }
        }
    }
    Impostor { albedo, normal, centre, radius }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hemi_oct_round_trips() {
        for j in 0..GRID {
            for i in 0..GRID {
                let d = frame_dir(i, j);
                assert!(d.z >= 0.0);
                let (u, v) = hemi_oct_encode(d);
                assert!((u - (i as f64 + 0.5) / GRID as f64).abs() < 1e-9, "{i} {j}");
                assert!((v - (j as f64 + 0.5) / GRID as f64).abs() < 1e-9, "{i} {j}");
            }
        }
    }
}
