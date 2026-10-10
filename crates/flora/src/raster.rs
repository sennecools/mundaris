//! Tiny orthographic software rasteriser for contact sheets and silhouette
//! metrics (no GPU, deterministic). Not a renderer for the game.

use glam::{Vec2, Vec3};

use crate::mesh::Mesh;

pub struct Canvas {
    /// Output size in pixels.
    pub width: usize,
    pub height: usize,
    /// Samples per pixel along each axis (2 = 4x supersampling).
    pub samples: usize,
    /// Linear RGB at sample resolution (`width·samples` wide).
    pub color: Vec<Vec3>,
    pub depth: Vec<f32>,
    pub coverage: Vec<bool>,
}

impl Canvas {
    /// A canvas with 4x supersampling (2×2 samples per pixel), the default
    /// for anything people look at (contact sheets, browser thumbnails).
    pub fn new(width: usize, height: usize, background: Vec3) -> Self {
        Self::with_samples(width, height, background, 2)
    }

    /// `samples` per axis; 1 = no anti-aliasing (silhouette metrics).
    pub fn with_samples(width: usize, height: usize, background: Vec3, samples: usize) -> Self {
        let s = samples.max(1);
        let n = width * s * height * s;
        Self { width, height, samples: s, color: vec![background; n], depth: vec![f32::MAX; n], coverage: vec![false; n] }
    }

    /// Width of the sample grid.
    pub fn sample_width(&self) -> usize {
        self.width * self.samples
    }

    /// Output pixels (box-filtered samples), linear RGB.
    pub fn resolve(&self) -> Vec<Vec3> {
        let (s, sw) = (self.samples, self.sample_width());
        let mut out = Vec::with_capacity(self.width * self.height);
        for y in 0..self.height {
            for x in 0..self.width {
                let mut acc = Vec3::ZERO;
                for dy in 0..s {
                    for dx in 0..s {
                        acc += self.color[(y * s + dy) * sw + x * s + dx];
                    }
                }
                out.push(acc / (s * s) as f32);
            }
        }
        out
    }

    /// sRGB 8-bit bytes (RGB) of the resolved pixels.
    pub fn to_srgb8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.width * self.height * 3);
        for c in &self.resolve() {
            for v in [c.x, c.y, c.z] {
                let v = v.clamp(0.0, 1.0);
                let s = if v <= 0.003_130_8 {
                    12.92 * v
                } else {
                    1.055 * v.powf(1.0 / 2.4) - 0.055
                };
                out.push((s * 255.0).round() as u8);
            }
        }
        out
    }

    pub fn fill_rect(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, c: Vec3) {
        let (s, sw) = (self.samples, self.sample_width());
        for y in y0 * s..(y1.min(self.height)) * s {
            for x in x0 * s..(x1.min(self.width)) * s {
                self.color[y * sw + x] = c;
            }
        }
    }
}

/// Orthographic camera looking horizontally-ish at a plant.
#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Camera yaw around +z, radians (0 looks along +y).
    pub azimuth: f32,
    /// Pitch down, radians.
    pub elevation: f32,
    /// World point drawn at `anchor_px`.
    pub target: Vec3,
    pub anchor_px: Vec2,
    pub px_per_m: f32,
    /// Clip rectangle in pixels (x0, y0, x1, y1).
    pub clip: [usize; 4],
}

impl Camera {
    fn axes(&self) -> (Vec3, Vec3, Vec3) {
        let (sa, ca) = self.azimuth.sin_cos();
        let (se, ce) = self.elevation.sin_cos();
        let fwd = Vec3::new(sa * ce, ca * ce, -se);
        let right = Vec3::new(ca, -sa, 0.0);
        let up = right.cross(fwd);
        (right, up, fwd)
    }
}

/// Draw with Lambert lighting, two-sided normals and vertex AO.
pub fn draw(canvas: &mut Canvas, mesh: &Mesh, cam: &Camera) {
    // Work at sample resolution: scale the camera by the samples per axis.
    let ss = canvas.samples as f32;
    let cam = &Camera {
        anchor_px: cam.anchor_px * ss,
        px_per_m: cam.px_per_m * ss,
        clip: cam.clip.map(|v| v * canvas.samples),
        ..*cam
    };
    let sw = canvas.sample_width();
    let (right, up, fwd) = cam.axes();
    let light = Vec3::new(-0.45, -0.55, 0.7).normalize();
    let proj: Vec<(Vec2, f32)> = mesh
        .vertices
        .iter()
        .map(|v| {
            let p = Vec3::from(v.position) - cam.target;
            let s = Vec2::new(p.dot(right), -p.dot(up)) * cam.px_per_m + cam.anchor_px;
            (s, p.dot(fwd))
        })
        .collect();
    let [cx0, cy0, cx1, cy1] = cam.clip;
    for t in mesh.indices.as_chunks::<3>().0 {
        let (a, b, c) = (t[0] as usize, t[1] as usize, t[2] as usize);
        let (pa, pb, pc) = (proj[a].0, proj[b].0, proj[c].0);
        let area = (pb - pa).perp_dot(pc - pa);
        if area.abs() < 1e-9 {
            continue;
        }
        let x0 = pa.x.min(pb.x).min(pc.x).floor().max(cx0 as f32) as i64;
        let x1 = pa.x.max(pb.x).max(pc.x).ceil().min(cx1 as f32 - 1.0) as i64;
        let y0 = pa.y.min(pb.y).min(pc.y).floor().max(cy0 as f32) as i64;
        let y1 = pa.y.max(pb.y).max(pc.y).ceil().min(cy1 as f32 - 1.0) as i64;
        if x0 > x1 || y0 > y1 {
            continue;
        }
        let va = &mesh.vertices[a];
        let vb = &mesh.vertices[b];
        let vc = &mesh.vertices[c];
        let n = |v: &crate::mesh::FloraVertex| {
            Vec3::new(v.normal[0] as f32, v.normal[1] as f32, v.normal[2] as f32) / 127.0
        };
        let col = |v: &crate::mesh::FloraVertex| {
            let c = Vec3::new(v.color[0] as f32, v.color[1] as f32, v.color[2] as f32) / 255.0;
            c * c
        };
        let (na, nb, nc) = (n(va), n(vb), n(vc));
        let (ca, cb, cc) = (col(va), col(vb), col(vc));
        let (aa, ab, ac) = (
            va.color[3] as f32 / 255.0,
            vb.color[3] as f32 / 255.0,
            vc.color[3] as f32 / 255.0,
        );
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
                let i = y as usize * sw + x as usize;
                if z >= canvas.depth[i] {
                    continue;
                }
                canvas.depth[i] = z;
                canvas.coverage[i] = true;
                let mut nn = (na * w0 + nb * w1 + nc * w2).normalize_or_zero();
                if nn.dot(fwd) > 0.0 {
                    nn = -nn;
                }
                let lit = 0.3 + 0.7 * nn.dot(light).max(0.0) + 0.15 * nn.z.max(0.0);
                let ao = aa * w0 + ab * w1 + ac * w2;
                canvas.color[i] = (ca * w0 + cb * w1 + cc * w2) * lit * ao;
            }
        }
    }
}

/// 3×5 bitmap font for labels (digits, upper-case letters, a few symbols).
pub fn text(canvas: &mut Canvas, x: usize, y: usize, s: &str, scale: usize, c: Vec3) {
    let mut cx = x;
    for ch in s.chars() {
        let g = glyph(ch.to_ascii_uppercase());
        for (row, bits) in g.iter().enumerate() {
            for col in 0..3 {
                if bits & (0b100 >> col) != 0 {
                    for dy in 0..scale {
                        for dx in 0..scale {
                            let px = cx + col * scale + dx;
                            let py = y + row * scale + dy;
                            if px < canvas.width && py < canvas.height {
                                canvas.fill_rect(px, py, px + 1, py + 1, c);
                            }
                        }
                    }
                }
            }
        }
        cx += 4 * scale;
    }
}

fn glyph(c: char) -> [u8; 5] {
    match c {
        '0' => [7, 5, 5, 5, 7],
        '1' => [2, 6, 2, 2, 7],
        '2' => [7, 1, 7, 4, 7],
        '3' => [7, 1, 3, 1, 7],
        '4' => [5, 5, 7, 1, 1],
        '5' => [7, 4, 7, 1, 7],
        '6' => [7, 4, 7, 5, 7],
        '7' => [7, 1, 2, 2, 2],
        '8' => [7, 5, 7, 5, 7],
        '9' => [7, 5, 7, 1, 7],
        'A' => [2, 5, 7, 5, 5],
        'B' => [6, 5, 6, 5, 6],
        'C' => [3, 4, 4, 4, 3],
        'D' => [6, 5, 5, 5, 6],
        'E' => [7, 4, 6, 4, 7],
        'F' => [7, 4, 6, 4, 4],
        'G' => [3, 4, 5, 5, 3],
        'H' => [5, 5, 7, 5, 5],
        'I' => [7, 2, 2, 2, 7],
        'J' => [1, 1, 1, 5, 2],
        'K' => [5, 5, 6, 5, 5],
        'L' => [4, 4, 4, 4, 7],
        'M' => [5, 7, 7, 5, 5],
        'N' => [6, 5, 5, 5, 5],
        'O' => [2, 5, 5, 5, 2],
        'P' => [6, 5, 6, 4, 4],
        'Q' => [2, 5, 5, 6, 3],
        'R' => [6, 5, 6, 5, 5],
        'S' => [3, 4, 2, 1, 6],
        'T' => [7, 2, 2, 2, 2],
        'U' => [5, 5, 5, 5, 7],
        'V' => [5, 5, 5, 5, 2],
        'W' => [5, 5, 7, 7, 5],
        'X' => [5, 5, 2, 5, 5],
        'Y' => [5, 5, 2, 2, 2],
        'Z' => [7, 1, 2, 4, 7],
        '.' => [0, 0, 0, 0, 2],
        ',' => [0, 0, 0, 2, 4],
        ':' => [0, 2, 0, 2, 0],
        '-' => [0, 0, 7, 0, 0],
        '_' => [0, 0, 0, 0, 7],
        '/' => [1, 1, 2, 4, 4],
        '%' => [5, 1, 2, 4, 5],
        '=' => [0, 7, 0, 7, 0],
        '(' => [1, 2, 2, 2, 1],
        ')' => [4, 2, 2, 2, 4],
        '<' => [1, 2, 4, 2, 1],
        '>' => [4, 2, 1, 2, 4],
        '!' => [2, 2, 2, 0, 2],
        _ => [0, 0, 0, 0, 0],
    }
}
