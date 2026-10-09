//! Cube-map storage for world-map fields, on the normalized radial charts of
//! `mundaris_math::surface::CubeFace` (direction = normalize(N + u·U + v·V),
//! u, v ∈ [−1, 1]).
//!
//! Layout: faces in `CubeFace::ALL` order, each `n × n` row-major with rows
//! along increasing v and columns along increasing u. Texel (i, j) covers
//! u ∈ [−1 + 2i/n, −1 + 2(i+1)/n] and its value is taken at the centre.

use glam::DVec3;
use mundaris_math::surface::CubeFace;

/// One value per texel of a six-face cube map.
#[derive(Debug, Clone, PartialEq)]
pub struct CubeMap<T> {
    n: usize,
    data: Vec<T>,
}

impl<T: Copy> CubeMap<T> {
    pub fn new(n: usize, fill: T) -> Self {
        Self {
            n,
            data: vec![fill; 6 * n * n],
        }
    }

    /// Texels per face edge.
    pub fn n(&self) -> usize {
        self.n
    }

    pub fn data(&self) -> &[T] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// Flat index of texel (i, j) on face number `face` (`CubeFace::ALL` order).
    pub fn index(&self, face: usize, i: usize, j: usize) -> usize {
        (face * self.n + j) * self.n + i
    }

    /// Value at the texel containing `direction` (nearest texel).
    pub fn nearest(&self, direction: DVec3) -> T {
        let (face, u, v) = locate(direction);
        let to_texel = |c: f64| (((c + 1.0) * 0.5 * self.n as f64) as usize).min(self.n - 1);
        self.data[self.index(face, to_texel(u), to_texel(v))]
    }
}

impl CubeMap<f32> {
    /// Bilinear sample within the face containing `direction`. Texel centres are
    /// clamped at face edges, so values across a face edge are continuous only to
    /// within one texel's change (adequate for previews; runtime sampling needs
    /// a cross-face gutter).
    pub fn bilinear(&self, direction: DVec3) -> f64 {
        let (face, u, v) = locate(direction);
        let n = self.n as f64;
        let fx = ((u + 1.0) * 0.5 * n - 0.5).clamp(0.0, n - 1.0);
        let fy = ((v + 1.0) * 0.5 * n - 0.5).clamp(0.0, n - 1.0);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.n - 1), (y0 + 1).min(self.n - 1));
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let at = |x: usize, y: usize| f64::from(self.data[self.index(face, x, y)]);
        let top = at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx;
        let bottom = at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx;
        top * (1.0 - ty) + bottom * ty
    }
}

/// Unit direction through the centre of texel (i, j) on face number `face`.
pub fn texel_direction(face: usize, i: usize, j: usize, n: usize) -> DVec3 {
    let [normal, u_axis, v_axis] = CubeFace::ALL[face].basis();
    let u = -1.0 + (2.0 * i as f64 + 1.0) / n as f64;
    let v = -1.0 + (2.0 * j as f64 + 1.0) / n as f64;
    (normal + u * u_axis + v * v_axis).normalize()
}

/// Face number and chart coordinates (u, v) ∈ [−1, 1] of a direction: the face
/// whose normal has the largest dot product (ties resolve to the earlier face).
pub fn locate(direction: DVec3) -> (usize, f64, f64) {
    let mut best = (0, f64::NEG_INFINITY);
    for (k, face) in CubeFace::ALL.iter().enumerate() {
        let d = direction.dot(face.basis()[0]);
        if d > best.1 {
            best = (k, d);
        }
    }
    let [normal, u_axis, v_axis] = CubeFace::ALL[best.0].basis();
    let w = direction.dot(normal);
    (
        best.0,
        (direction.dot(u_axis) / w).clamp(-1.0, 1.0),
        (direction.dot(v_axis) / w).clamp(-1.0, 1.0),
    )
}

/// Directions of every texel centre, in storage order.
pub fn texel_directions(n: usize) -> Vec<DVec3> {
    let mut out = Vec::with_capacity(6 * n * n);
    for face in 0..6 {
        for j in 0..n {
            for i in 0..n {
                out.push(texel_direction(face, i, j, n));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn texel_centres_locate_back_to_their_texel() {
        let n = 16;
        let mut map = CubeMap::new(n, 0u32);
        for (k, v) in map.data_mut().iter_mut().enumerate() {
            *v = k as u32;
        }
        for (k, d) in texel_directions(n).iter().enumerate() {
            assert_eq!(map.nearest(*d), k as u32);
        }
    }
}
