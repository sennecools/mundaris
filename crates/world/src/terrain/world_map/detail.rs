//! Library detail tiles for world-map composition (W2, biome catalog and
//! heightmap variants): the height channel of a `mundaris.terrain-bundle.v1`
//! bundle, high-passed so that it carries only detail below the band limit.
//! Macro relief comes only from the world map (`docs/PLANET_DATA_PIPELINE.md`
//! §5). Tiles are periodic, real-scale and sampled in metres.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::Path;

pub const BUNDLE_FORMAT: &str = "mundaris.terrain-bundle.v1";

/// The fields of `bundle.json` that composition needs; the rest is ignored.
#[derive(Debug, Deserialize)]
struct BundleHeader {
    format: String,
    bundle_id: String,
    provenance: String,
    footprint_m: f64,
    resolution: usize,
    wrap: String,
    height: HeightEncoding,
    channels: Vec<ChannelHeader>,
}

#[derive(Debug, Deserialize)]
struct HeightEncoding {
    offset_m: f64,
    range_m: f64,
}

#[derive(Debug, Deserialize)]
struct ChannelHeader {
    file: String,
    width: usize,
    height: usize,
    sample_type: String,
    byte_order: String,
    sha256: String,
}

/// A periodic, zero-mean, high-passed height grid.
#[derive(Debug, Clone)]
pub struct DetailTile {
    pub bundle_id: String,
    pub provenance: String,
    /// sha256 of the source `height.r16` (hex).
    pub height_sha256: String,
    pub footprint_m: f64,
    n: usize,
    heights: Vec<f32>,
    /// RMS of the high-passed heights (m).
    pub rms_m: f64,
}

impl DetailTile {
    /// Load `<dir>/bundle.json` and its height channel, checking the hash
    /// against the bundle and, if given, the catalog's expected hash.
    pub fn load(
        dir: &Path,
        expected_sha256: Option<&str>,
        band_limit_m: f64,
    ) -> Result<Self, String> {
        let at = |what: &str| format!("{}: {what}", dir.display());
        let header: BundleHeader = serde_json::from_slice(
            &std::fs::read(dir.join("bundle.json")).map_err(|e| at(&e.to_string()))?,
        )
        .map_err(|e| at(&e.to_string()))?;
        if header.format != BUNDLE_FORMAT || header.wrap != "periodic" {
            return Err(at("not a periodic mundaris.terrain-bundle.v1"));
        }
        let channel = header
            .channels
            .iter()
            .find(|c| c.file == "height.r16")
            .ok_or_else(|| at("no height.r16 channel"))?;
        let n = header.resolution;
        if channel.width != n || channel.height != n || channel.sample_type != "u16" {
            return Err(at("height.r16 is not an n×n u16 grid"));
        }
        let bytes = std::fs::read(dir.join(&channel.file)).map_err(|e| at(&e.to_string()))?;
        let sha = format!("{:x}", Sha256::digest(&bytes));
        if sha != channel.sha256 || expected_sha256.is_some_and(|e| e != sha) {
            return Err(at(&format!("height.r16 sha256 {sha} does not match")));
        }
        if bytes.len() != 2 * n * n {
            return Err(at("height.r16 has the wrong size"));
        }
        let big = match channel.byte_order.as_str() {
            "little-endian" => false,
            "big-endian" => true,
            other => return Err(at(&format!("byte order {other}"))),
        };
        let heights = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|&b| {
                let v = if big {
                    u16::from_be_bytes(b)
                } else {
                    u16::from_le_bytes(b)
                };
                header.height.offset_m + f64::from(v) / 65535.0 * header.height.range_m
            })
            .collect();
        let mut tile = Self::from_heights(
            &header.bundle_id,
            n,
            header.footprint_m,
            heights,
            band_limit_m,
        )?;
        tile.provenance = header.provenance;
        tile.height_sha256 = sha;
        Ok(tile)
    }

    /// Build from an in-memory periodic `n × n` grid (row-major, rows along
    /// +y) covering `footprint_m`; high-pass at `band_limit_m`.
    pub fn from_heights(
        id: &str,
        n: usize,
        footprint_m: f64,
        heights: Vec<f64>,
        band_limit_m: f64,
    ) -> Result<Self, String> {
        let positive = |v: f64| v.is_finite() && v > 0.0;
        if n < 4 || heights.len() != n * n || !positive(footprint_m) || !positive(band_limit_m) {
            return Err(format!("{id}: invalid detail grid"));
        }
        if heights.iter().any(|h| !h.is_finite()) {
            return Err(format!("{id}: non-finite height"));
        }
        let high = high_pass(&heights, n, footprint_m / band_limit_m);
        let rms_m = (high.iter().map(|v| v * v).sum::<f64>() / high.len() as f64).sqrt();
        Ok(Self {
            bundle_id: id.to_string(),
            provenance: String::new(),
            height_sha256: String::new(),
            footprint_m,
            n,
            heights: high.into_iter().map(|v| v as f32).collect(),
            rms_m,
        })
    }

    /// Periodic bilinear sample at (x, y) metres.
    pub fn sample(&self, x: f64, y: f64) -> f64 {
        let n = self.n as f64;
        let fx = (x / self.footprint_m * n - 0.5).rem_euclid(n);
        let fy = (y / self.footprint_m * n - 0.5).rem_euclid(n);
        let (x0, y0) = (fx.floor() as usize % self.n, fy.floor() as usize % self.n);
        let (x1, y1) = ((x0 + 1) % self.n, (y0 + 1) % self.n);
        let (tx, ty) = (fx - fx.floor(), fy - fy.floor());
        let at = |i: usize, j: usize| f64::from(self.heights[j * self.n + i]);
        let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
        let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

/// `heights` minus every periodic Fourier mode whose wavelength is at least
/// the cutoff: modes (kx, ky) with kx² + ky² ≤ (footprint / cutoff)², the
/// mean included. The tile is periodic, so this is exact; the few low modes
/// are found and rebuilt separably in O(n² · modes).
fn high_pass(heights: &[f64], n: usize, cycles: f64) -> Vec<f64> {
    let k_max = cycles.floor().max(0.0) as i64;
    let ks: Vec<i64> = (-k_max..=k_max).collect();
    let m = ks.len();
    let tau = std::f64::consts::TAU;
    // twiddle[(a, x)] = exp(-i·2π·ks[a]·x / n)
    let twiddle: Vec<(f64, f64)> = ks
        .iter()
        .flat_map(|&k| {
            (0..n).map(move |x| {
                let phase = -tau * (k * x as i64) as f64 / n as f64;
                (phase.cos(), phase.sin())
            })
        })
        .collect();
    let tw = |a: usize, x: usize| twiddle[a * n + x];
    // Row transforms: rows[y][a] = Σ_x h(x, y) · tw(a, x).
    let mut rows = vec![(0.0, 0.0); n * m];
    for y in 0..n {
        for a in 0..m {
            let (mut re, mut im) = (0.0, 0.0);
            for x in 0..n {
                let (c, s) = tw(a, x);
                re += heights[y * n + x] * c;
                im += heights[y * n + x] * s;
            }
            rows[y * m + a] = (re, im);
        }
    }
    // Mode coefficients C(a, b) = Σ_y rows[y][a] · tw(b, y), inside the disc.
    let inside = |a: usize, b: usize| (ks[a] * ks[a] + ks[b] * ks[b]) as f64 <= cycles * cycles;
    let mut coeff = vec![(0.0, 0.0); m * m];
    for a in 0..m {
        for b in 0..m {
            if !inside(a, b) {
                continue;
            }
            let (mut re, mut im) = (0.0, 0.0);
            for y in 0..n {
                let (r, i) = rows[y * m + a];
                let (c, s) = tw(b, y);
                re += r * c - i * s;
                im += r * s + i * c;
            }
            coeff[a * m + b] = (re, im);
        }
    }
    // Rebuild the low-pass: low(x, y) = Σ C(a, b) · conj(tw(a, x) tw(b, y)) / n².
    let norm = (n * n) as f64;
    let mut out = heights.to_vec();
    for y in 0..n {
        // col[a] = Σ_b C(a, b) · conj(tw(b, y))
        let col: Vec<(f64, f64)> = (0..m)
            .map(|a| {
                (0..m).fold((0.0, 0.0), |(re, im), b| {
                    let (cr, ci) = coeff[a * m + b];
                    let (c, s) = tw(b, y);
                    (re + cr * c + ci * s, im + ci * c - cr * s)
                })
            })
            .collect();
        for x in 0..n {
            let low: f64 = (0..m)
                .map(|a| {
                    let (cr, ci) = col[a];
                    let (c, s) = tw(a, x);
                    cr * c + ci * s
                })
                .sum();
            out[y * n + x] -= low / norm;
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A deterministic periodic test grid: one long wave plus short detail.
    pub(crate) fn synthetic(n: usize, seed: u64) -> Vec<f64> {
        let mut out = Vec::with_capacity(n * n);
        let tau = std::f64::consts::TAU;
        for j in 0..n {
            for i in 0..n {
                let (x, y) = (i as f64 / n as f64, j as f64 / n as f64);
                let long = 200.0 * (tau * x).sin();
                let s = seed as f64;
                let short = 3.0 * (tau * (16.0 * x + 3.0 * y) + s).sin()
                    + 2.0 * (tau * (5.0 * x - 21.0 * y) + 2.0 * s).cos();
                out.push(long + short);
            }
        }
        out
    }

    #[test]
    fn high_pass_removes_the_long_wave_and_keeps_detail() {
        let (n, footprint) = (128, 4000.0);
        let tile = DetailTile::from_heights("t", n, footprint, synthetic(n, 1), 1500.0).unwrap();
        // Short detail RMS is √((3² + 2²) / 2) ≈ 2.55 m; the 4 km wave of
        // 200 m must be gone exactly.
        assert!((tile.rms_m - 6.5f64.sqrt()).abs() < 1e-6, "{}", tile.rms_m);
        let mean = tile.heights.iter().map(|v| f64::from(*v)).sum::<f64>() / (n * n) as f64;
        assert!(mean.abs() < 1e-3);
    }

    #[test]
    fn sampling_is_periodic_and_hits_texel_centres() {
        let n = 32;
        let tile = DetailTile::from_heights("t", n, 4000.0, synthetic(n, 2), 1500.0).unwrap();
        let c = 4000.0 / n as f64;
        for (i, j) in [(0, 0), (5, 7), (31, 31)] {
            let x = (i as f64 + 0.5) * c;
            let y = (j as f64 + 0.5) * c;
            let v = f64::from(tile.heights[j * n + i]);
            assert!((tile.sample(x, y) - v).abs() < 1e-9);
            assert!((tile.sample(x + 4000.0, y - 8000.0) - v).abs() < 1e-6);
        }
    }
}
