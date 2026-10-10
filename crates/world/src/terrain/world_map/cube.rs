//! Cube-map storage for world-map fields, on the normalized radial charts of
//! `astrum_math::surface::CubeFace` (direction = normalize(N + u·U + v·V),
//! u, v ∈ [−1, 1]).
//!
//! Layout: faces in `CubeFace::ALL` order, each `n × n` row-major with rows
//! along increasing v and columns along increasing u. Texel (i, j) covers
//! u ∈ [−1 + 2i/n, −1 + 2(i+1)/n] and its value is taken at the centre.

use astrum_math::surface::CubeFace;
use glam::DVec3;

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
        self.data[self.nearest_index(direction)]
    }

    fn nearest_index(&self, direction: DVec3) -> usize {
        let (face, u, v) = locate(direction);
        let to_texel = |c: f64| (((c + 1.0) * 0.5 * self.n as f64) as usize).min(self.n - 1);
        self.index(face, to_texel(u), to_texel(v))
    }
}

impl CubeMap<f32> {
    /// Texel (i, j) of `face`; outside the face, the adjacent face is sampled
    /// bilinearly (clamped within that face) at the extended chart position,
    /// so values agree across the edge to second order.
    fn value_across(&self, face: usize, i: i64, j: i64) -> f64 {
        let range = 0..self.n as i64;
        if range.contains(&i) && range.contains(&j) {
            return f64::from(self.data[self.index(face, i as usize, j as usize)]);
        }
        let [normal, u_axis, v_axis] = CubeFace::ALL[face].basis();
        let n = self.n as f64;
        let u = -1.0 + (2.0 * i as f64 + 1.0) / n;
        let v = -1.0 + (2.0 * j as f64 + 1.0) / n;
        let (f2, u2, v2) = locate((normal + u * u_axis + v * v_axis).normalize());
        let fx = ((u2 + 1.0) * 0.5 * n - 0.5).clamp(0.0, n - 1.0);
        let fy = ((v2 + 1.0) * 0.5 * n - 0.5).clamp(0.0, n - 1.0);
        let (x0, y0) = (fx.floor() as usize, fy.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.n - 1), (y0 + 1).min(self.n - 1));
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let at = |x: usize, y: usize| f64::from(self.data[self.index(f2, x, y)]);
        (at(x0, y0) * (1.0 - tx) + at(x1, y0) * tx) * (1.0 - ty)
            + (at(x0, y1) * (1.0 - tx) + at(x1, y1) * tx) * ty
    }

    /// Blend `on_face(face, u, v)` over every face whose (extended) chart
    /// lies within one texel of `direction`, weighted by the distance inside
    /// that face's square. Each face's interpolant is continuous on its own
    /// extended chart, so the blend is exactly continuous across edges.
    fn across_faces(&self, direction: DVec3, on_face: impl Fn(usize, f64, f64) -> f64) -> f64 {
        let band = 1.0 / self.n as f64;
        let (mut sum, mut total) = (0.0, 0.0);
        for (face, f) in CubeFace::ALL.iter().enumerate() {
            let [normal, u_axis, v_axis] = f.basis();
            let w = direction.dot(normal);
            if w <= 0.0 {
                continue;
            }
            let (u, v) = (direction.dot(u_axis) / w, direction.dot(v_axis) / w);
            let t = (1.0 - u.abs().max(v.abs()) + band) / (2.0 * band);
            let weight = if t >= 1.0 {
                1.0
            } else if t <= 0.0 {
                continue;
            } else {
                t * t * (3.0 - 2.0 * t)
            };
            if weight >= 1.0 {
                return on_face(face, u, v);
            }
            sum += weight * on_face(face, u, v);
            total += weight;
        }
        sum / total.max(1e-300)
    }

    fn bilinear_on(&self, face: usize, u: f64, v: f64) -> f64 {
        let n = self.n as f64;
        let fx = (u + 1.0) * 0.5 * n - 0.5;
        let fy = (v + 1.0) * 0.5 * n - 0.5;
        let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let at = |x: i64, y: i64| self.value_across(face, x, y);
        let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
        let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
        top * (1.0 - ty) + bottom * ty
    }

    fn bicubic_on(&self, face: usize, u: f64, v: f64) -> f64 {
        let n = self.n as f64;
        let fx = (u + 1.0) * 0.5 * n - 0.5;
        let fy = (v + 1.0) * 0.5 * n - 0.5;
        let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let weights = |t: f64| {
            let (t2, t3) = (t * t, t * t * t);
            [
                0.5 * (-t3 + 2.0 * t2 - t),
                0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
                0.5 * (-3.0 * t3 + 4.0 * t2 + t),
                0.5 * (t3 - t2),
            ]
        };
        let (wx, wy) = (weights(tx), weights(ty));
        let mut sum = 0.0;
        for (j, wj) in wy.iter().enumerate() {
            for (i, wi) in wx.iter().enumerate() {
                sum += wi * wj * self.value_across(face, x0 - 1 + i as i64, y0 - 1 + j as i64);
            }
        }
        sum
    }

    /// Catmull-Rom value of `face` at chart (u, v) and its derivatives with
    /// respect to the texel coordinates (fx, fy).
    fn bicubic_d_on(&self, face: usize, u: f64, v: f64) -> (f64, f64, f64) {
        let n = self.n as f64;
        let fx = (u + 1.0) * 0.5 * n - 0.5;
        let fy = (v + 1.0) * 0.5 * n - 0.5;
        let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
        let (tx, ty) = (fx - x0 as f64, fy - y0 as f64);
        let weights = |t: f64| {
            let (t2, t3) = (t * t, t * t * t);
            [
                0.5 * (-t3 + 2.0 * t2 - t),
                0.5 * (3.0 * t3 - 5.0 * t2 + 2.0),
                0.5 * (-3.0 * t3 + 4.0 * t2 + t),
                0.5 * (t3 - t2),
            ]
        };
        let derivatives = |t: f64| {
            let t2 = t * t;
            [
                0.5 * (-3.0 * t2 + 4.0 * t - 1.0),
                0.5 * (9.0 * t2 - 10.0 * t),
                0.5 * (-9.0 * t2 + 8.0 * t + 1.0),
                0.5 * (3.0 * t2 - 2.0 * t),
            ]
        };
        let (wx, wy, dx, dy) = (weights(tx), weights(ty), derivatives(tx), derivatives(ty));
        let (mut sum, mut sx, mut sy) = (0.0, 0.0, 0.0);
        for j in 0..4 {
            for i in 0..4 {
                let value = self.value_across(face, x0 - 1 + i as i64, y0 - 1 + j as i64);
                sum += wx[i] * wy[j] * value;
                sx += dx[i] * wy[j] * value;
                sy += wx[i] * dy[j] * value;
            }
        }
        (sum, sx, sy)
    }

    /// [`Self::bicubic`] and its tangent gradient per unit direction, from
    /// the analytic Catmull-Rom derivative: on face (normal b0, axes b1, b2),
    /// `∇u = (b1 − u·b0)/(d·b0)` and `∂fx/∂u = n/2`. In the one-texel blend
    /// band at face edges the blend `Σ wf/W` differentiates as
    /// `(Σ w∇f + Σ f∇w)/W − (Σ wf)(Σ ∇w)/W²`, with `w = s(t)` and
    /// `t = (1 − max(|u|, |v|) + band)/(2·band)`.
    pub fn bicubic_gradient(&self, direction: DVec3) -> (f64, DVec3) {
        let band = 1.0 / self.n as f64;
        let half_n = 0.5 * self.n as f64;
        let (mut sum, mut gradient, mut total) = (0.0, DVec3::ZERO, 0.0);
        let (mut value_dw, mut total_dw) = (DVec3::ZERO, DVec3::ZERO);
        for (face, f) in CubeFace::ALL.iter().enumerate() {
            let [normal, u_axis, v_axis] = f.basis();
            let w = direction.dot(normal);
            if w <= 0.0 {
                continue;
            }
            let (u, v) = (direction.dot(u_axis) / w, direction.dot(v_axis) / w);
            let t = (1.0 - u.abs().max(v.abs()) + band) / (2.0 * band);
            let weight = if t >= 1.0 {
                1.0
            } else if t <= 0.0 {
                continue;
            } else {
                t * t * (3.0 - 2.0 * t)
            };
            let (value, dfx, dfy) = self.bicubic_d_on(face, u, v);
            let grad_u = (u_axis - normal * u) / w;
            let grad_v = (v_axis - normal * v) / w;
            let g = (grad_u * dfx + grad_v * dfy) * half_n;
            if weight >= 1.0 {
                return (value, g - direction * direction.dot(g));
            }
            // ∇w = s'(t)·∇t, ∇t = −∇max(|u|, |v|)/(2·band).
            let grad_m = if u.abs() >= v.abs() {
                grad_u * u.signum()
            } else {
                grad_v * v.signum()
            };
            let dw = grad_m * (-6.0 * t * (1.0 - t) / (2.0 * band));
            sum += weight * value;
            gradient += g * weight;
            total += weight;
            value_dw += dw * value;
            total_dw += dw;
        }
        let total = total.max(1e-300);
        let g = (gradient + value_dw) / total - total_dw * (sum / (total * total));
        (sum / total, g - direction * direction.dot(g))
    }

    /// Uniform cubic B-spline of `face` at chart (u, v): smooth (C²) and
    /// non-overshooting (convex weights), so interpolated landform weights
    /// stay in [0, 1] and keep their sum.
    fn bspline_on(&self, face: usize, u: f64, v: f64) -> f64 {
        let n = self.n as f64;
        let fx = (u + 1.0) * 0.5 * n - 0.5;
        let fy = (v + 1.0) * 0.5 * n - 0.5;
        let (x0, y0) = (fx.floor() as i64, fy.floor() as i64);
        let (wx, wy) = (
            bspline_weights(fx - x0 as f64),
            bspline_weights(fy - y0 as f64),
        );
        let mut sum = 0.0;
        for (j, wj) in wy.iter().enumerate() {
            for (i, wi) in wx.iter().enumerate() {
                sum += wi * wj * self.value_across(face, x0 - 1 + i as i64, y0 - 1 + j as i64);
            }
        }
        sum
    }

    /// Cubic B-spline sample at `direction`, continuous across face edges.
    pub fn bspline(&self, direction: DVec3) -> f64 {
        self.across_faces(direction, |f, u, v| self.bspline_on(f, u, v))
    }

    /// Bilinear sample at `direction`, continuous across face edges.
    pub fn bilinear(&self, direction: DVec3) -> f64 {
        self.across_faces(direction, |f, u, v| self.bilinear_on(f, u, v))
    }

    /// Catmull-Rom (C¹ within faces) sample at `direction`, continuous across
    /// face edges. Use it where slopes matter: bilinear gradients are
    /// piecewise constant and shade as facets.
    pub fn bicubic(&self, direction: DVec3) -> f64 {
        self.across_faces(direction, |f, u, v| self.bicubic_on(f, u, v))
    }
}

/// Uniform cubic B-spline weights of the four taps around fraction `t`.
pub fn bspline_weights(t: f64) -> [f64; 4] {
    let (t2, t3) = (t * t, t * t * t);
    let s = 1.0 - t;
    [
        s * s * s / 6.0,
        (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0,
        (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0,
        t3 / 6.0,
    ]
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

/// The four edge neighbours of texel `k`, crossing cube-face edges.
pub fn neighbours(k: usize, n: usize) -> [usize; 4] {
    let face = k / (n * n);
    let (j, i) = ((k / n) % n, k % n);
    let step = |di: i64, dj: i64| -> usize {
        let (ii, jj) = (i as i64 + di, j as i64 + dj);
        let range = 0..n as i64;
        if range.contains(&ii) && range.contains(&jj) {
            return (face * n + jj as usize) * n + ii as usize;
        }
        let [normal, u_axis, v_axis] = CubeFace::ALL[face].basis();
        let u = -1.0 + (2.0 * ii as f64 + 1.0) / n as f64;
        let v = -1.0 + (2.0 * jj as f64 + 1.0) / n as f64;
        let (f2, u2, v2) = locate((normal + u * u_axis + v * v_axis).normalize());
        let texel = |c: f64| (((c + 1.0) * 0.5 * n as f64) as usize).min(n - 1);
        (f2 * n + texel(v2)) * n + texel(u2)
    };
    [step(1, 0), step(-1, 0), step(0, 1), step(0, -1)]
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

    #[test]
    fn bicubic_gradient_matches_the_value_and_central_differences() {
        let n = 64;
        let f = |d: DVec3| (3.0 * d.x).sin() + d.y * d.z + 0.5 * (2.0 * d.z).cos();
        let mut map = CubeMap::new(n, 0.0f32);
        for (k, d) in texel_directions(n).iter().enumerate() {
            map.data_mut()[k] = f(*d) as f32;
        }
        let delta = 0.5 / n as f64;
        let mut worst = (0.0f64, 0.0f64);
        let mut edge_errors = Vec::new();
        for k in 0..2000 {
            // Fibonacci directions: interiors, edges and corners.
            let y = 1.0 - 2.0 * (k as f64 + 0.5) / 2000.0;
            let r = (1.0 - y * y).sqrt();
            let a = 2.399_963_229_728_653 * k as f64;
            let d = DVec3::new(r * a.cos(), y, r * a.sin());
            let (value, gradient) = map.bicubic_gradient(d);
            assert_eq!(value, map.bicubic(d));
            let e1 = d.any_orthonormal_vector();
            let e2 = d.cross(e1);
            let at = |v: DVec3| map.bicubic((d + v * delta).normalize());
            let fd = e1 * ((at(e1) - at(-e1)) / (2.0 * delta))
                + e2 * ((at(e2) - at(-e2)) / (2.0 * delta));
            let error = (gradient - fd).length() / (1.0 + fd.length());
            let (u, v) = (locate(d).1, locate(d).2);
            let edge = u.abs().max(v.abs()) > 1.0 - 2.0 / n as f64;
            if edge {
                worst.1 = worst.1.max(error);
                edge_errors.push(error);
            } else {
                worst.0 = worst.0.max(error);
            }
        }
        edge_errors.sort_by(f64::total_cmp);
        let p95 = edge_errors[edge_errors.len() * 95 / 100];
        println!(
            "bicubic gradient vs central differences: interior {:.2e}, edge band p95 {p95:.2e}, max {:.2e}",
            worst.0, worst.1
        );
        // Central differences straddle the kinks of max(|u|, |v|) at face
        // corners and the clamped cross-face taps, so the edge band is
        // checked by quantile.
        assert!(worst.0 < 5e-3, "interior {}", worst.0);
        assert!(
            p95 < 1e-2 && worst.1 < 0.1,
            "edge band p95 {p95}, max {}",
            worst.1
        );
    }

    #[test]
    fn bilinear_is_continuous_across_face_edges() {
        let n = 32;
        let f = |d: DVec3| d.x + 2.0 * d.y + 3.0 * d.z;
        let mut map = CubeMap::new(n, 0.0f32);
        for (k, d) in texel_directions(n).iter().enumerate() {
            map.data_mut()[k] = f(*d) as f32;
        }
        // A texel spans about 0.05 rad, over which f changes by up to ~0.19;
        // clamping at the edge would jump by about half of that.
        // Points on an edge, and the difference of the two face normals.
        for (a, b) in [
            (DVec3::new(1.0, 1.0, 0.3), DVec3::new(1.0, -1.0, 0.0)),
            (DVec3::new(0.2, -1.0, -1.0), DVec3::new(0.0, -1.0, 1.0)),
            (DVec3::new(-1.0, 0.4, 1.0), DVec3::new(-1.0, 0.0, -1.0)),
        ] {
            let across = b * 1e-6;
            let (p, q) = ((a + across).normalize(), (a - across).normalize());
            assert_ne!(locate(p).0, locate(q).0, "probe must straddle an edge");
            let jump = (map.bilinear(p) - map.bilinear(q)).abs();
            assert!(jump < 1e-4, "jump {jump} at {a}");
            assert!((map.bilinear(p) - f(p)).abs() < 0.02);
            let cubic = (map.bicubic(p) - map.bicubic(q)).abs();
            assert!(cubic < 1e-4, "bicubic jump {cubic} at {a}");
            assert!((map.bicubic(p) - f(p)).abs() < 0.02);
        }
    }
}
