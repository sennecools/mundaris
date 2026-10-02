//! Celestial reverse-Z, f64 clipping/projection and the checked pixel boundary.
use crate::RenderPreparationError;
use glam::{DVec3, Mat4, Vec4};

#[derive(Debug, Clone, Copy)]
pub struct CelestialProjection {
    origin: [u32; 2],
    width: u32,
    height: u32,
    near_m: f64,
    tan_half: f64,
    matrix: Mat4,
}
impl CelestialProjection {
    pub fn try_new(
        width: u32,
        height: u32,
        vertical_fov_rad: f64,
        near_m: f64,
    ) -> Result<Self, RenderPreparationError> {
        if width == 0
            || height == 0
            || !vertical_fov_rad.is_finite()
            || !(0.0..std::f64::consts::PI).contains(&vertical_fov_rad)
            || vertical_fov_rad == 0.0
            || !near_m.is_finite()
            || near_m <= 0.0
        {
            return Err(RenderPreparationError::InvalidProjection);
        }
        let tan_half = (vertical_fov_rad * 0.5).tan();
        let matrix = Mat4::perspective_infinite_reverse_rh(
            vertical_fov_rad as f32,
            width as f32 / height as f32,
            near_m as f32,
        );
        if !matrix.is_finite() || near_m as f32 <= 0.0 || !tan_half.is_finite() || tan_half <= 0.0 {
            return Err(RenderPreparationError::InvalidProjection);
        }
        Ok(Self {
            origin: [0, 0],
            width,
            height,
            near_m,
            tan_half,
            matrix,
        })
    }
    pub fn near_m(self) -> f64 {
        self.near_m
    }
    pub fn viewport(self) -> [u32; 2] {
        [self.width, self.height]
    }
    /// Physical content rectangle shared by fitting, overlays, unprojection and GPU viewport.
    pub fn with_origin(mut self, origin: [u32; 2]) -> Result<Self, RenderPreparationError> {
        if origin[0].checked_add(self.width).is_none()
            || origin[1].checked_add(self.height).is_none()
        {
            return Err(RenderPreparationError::InvalidProjection);
        }
        self.origin = origin;
        Ok(self)
    }
    pub fn origin(self) -> [u32; 2] {
        self.origin
    }
    pub fn vertical_fov_rad(self) -> f64 {
        2.0 * self.tan_half.atan()
    }
    pub fn unproject_ray(self, pointer: [f64; 2]) -> Result<DVec3, RenderPreparationError> {
        let ray = DVec3::new(
            (pointer[0] - self.origin[0] as f64 - self.width as f64 * 0.5) / self.focal_pixels(),
            -(pointer[1] - self.origin[1] as f64 - self.height as f64 * 0.5) / self.focal_pixels(),
            -1.0,
        );
        if !ray.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok(ray.normalize())
    }
    pub fn focal_pixels(self) -> f64 {
        self.height as f64 / (2.0 * self.tan_half)
    }
    pub fn gpu_bytes(self) -> [u8; 64] {
        let mut bytes = [0; 64];
        for (value, output) in self
            .matrix
            .to_cols_array()
            .iter()
            .zip(bytes.as_chunks_mut::<4>().0)
        {
            output.copy_from_slice(&value.to_le_bytes());
        }
        bytes
    }
    pub fn gpu_clip(self, view: [f32; 3]) -> [f32; 4] {
        (self.matrix * Vec4::new(view[0], view[1], view[2], 1.0)).to_array()
    }
    fn screen(self, p: DVec3) -> Result<[f64; 2], RenderPreparationError> {
        if !p.is_finite() || -p.z < self.near_m {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let focal = self.focal_pixels();
        let screen = [
            self.origin[0] as f64 + self.width as f64 * 0.5 + focal * (p.x / -p.z),
            self.origin[1] as f64 + self.height as f64 * 0.5 - focal * (p.y / -p.z),
        ];
        if screen.iter().all(|x| x.is_finite()) {
            Ok(screen)
        } else {
            Err(RenderPreparationError::InvalidDebugGeometry)
        }
    }
    /// Only bounded physical-pixel screen coordinates cross into UI f32.
    pub fn project_marker(self, p: DVec3) -> Result<Option<[f32; 2]>, RenderPreparationError> {
        if !p.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        if -p.z < self.near_m {
            return Ok(None);
        }
        let screen = self.screen(p)?;
        if screen[0] < self.origin[0] as f64
            || screen[1] < self.origin[1] as f64
            || screen[0] > (self.origin[0] + self.width) as f64
            || screen[1] > (self.origin[1] + self.height) as f64
        {
            return Ok(None);
        }
        let narrowed = screen.map(|x| x as f32);
        if screen
            .iter()
            .zip(narrowed)
            .any(|(a, b)| (*a - b as f64).abs() > 0.05)
        {
            return Err(RenderPreparationError::PrecisionBudgetExceeded {
                error_m: 0.05,
                limit_m: 0.05,
            });
        }
        Ok(Some(narrowed))
    }
    /// Unbounded f64 screen query for derived tessellation/simplification. Behind-near
    /// points are unavailable; only `project_marker` narrows bounded UI coordinates.
    pub fn project_pixels(self, p: DVec3) -> Result<Option<[f64; 2]>, RenderPreparationError> {
        if !p.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        if -p.z < self.near_m {
            Ok(None)
        } else {
            self.screen(p).map(Some)
        }
    }
    /// Compare actual vertex round trip in metres and physical pixels. Near-plane
    /// crossing vertices use the conservative near depth rather than divide by z.
    pub(crate) fn narrow(
        self,
        p: DVec3,
        radius_budget_m: f64,
        range_m: f64,
    ) -> Result<([f32; 3], f64, f64), RenderPreparationError> {
        if !p.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let distance = p.x.hypot(p.y).hypot(p.z);
        if distance > range_m {
            return Err(RenderPreparationError::OutsideRenderRange {
                distance_m: distance,
                limit_m: range_m,
            });
        }
        let gpu = p.as_vec3();
        if !gpu.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let round = gpu.as_dvec3();
        let error = (round - p).length();
        let depth = (-p.z).max(self.near_m);
        let lever = 1.0 + (p.x / depth).abs() + (p.y / depth).abs();
        let permitted = (0.05 * depth / (self.focal_pixels() * lever)).min(radius_budget_m);
        if error > permitted {
            return Err(RenderPreparationError::PrecisionBudgetExceeded {
                error_m: error,
                limit_m: permitted,
            });
        }
        let pixels = if -p.z >= self.near_m && -round.z >= self.near_m {
            let a = self.screen(p)?;
            let b = self.screen(round)?;
            (a[0] - b[0]).hypot(a[1] - b[1])
        } else {
            error * self.focal_pixels() * lever / self.near_m
        };
        if !pixels.is_finite() || pixels > 0.05 {
            return Err(RenderPreparationError::PrecisionBudgetExceeded {
                error_m: pixels,
                limit_m: 0.05,
            });
        }
        Ok((gpu.to_array(), error, pixels))
    }
    /// Convex f64 line clipping before GPU narrowing; no far physical plane.
    pub fn clip_segment(
        self,
        endpoints: [DVec3; 2],
    ) -> Result<Option<[DVec3; 2]>, RenderPreparationError> {
        let [a, b] = endpoints;
        let delta = b - a;
        if !a.is_finite() || !b.is_finite() || !delta.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let horizontal = self.tan_half * self.width as f64 / self.height as f64;
        let planes = [
            (DVec3::new(0.0, 0.0, -1.0), -self.near_m),
            (DVec3::new(1.0, 0.0, -horizontal), 0.0),
            (DVec3::new(-1.0, 0.0, -horizontal), 0.0),
            (DVec3::new(0.0, 1.0, -self.tan_half), 0.0),
            (DVec3::new(0.0, -1.0, -self.tan_half), 0.0),
        ];
        let mut low: f64 = 0.0;
        let mut high: f64 = 1.0;
        for (normal, offset) in planes {
            let at = normal.dot(a) + offset;
            let slope = normal.dot(delta);
            if !at.is_finite() || !slope.is_finite() {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            if slope == 0.0 {
                if at < 0.0 {
                    return Ok(None);
                }
            } else {
                let crossing = -at / slope;
                if slope > 0.0 {
                    low = low.max(crossing);
                } else {
                    high = high.min(crossing);
                }
            }
            if low > high {
                return Ok(None);
            }
        }
        Ok(Some([a + delta * low, a + delta * high]))
    }
}

/// CPU ray versus physical reference sphere; debug occlusion has no collision meaning.
pub fn reference_sphere_occludes(target: DVec3, center: DVec3, radius_m: f64) -> bool {
    let distance = target.length();
    if distance == 0.0 {
        return false;
    }
    let ray = target / distance;
    let along = center.dot(ray);
    let perpendicular = (center - ray * along).length();
    if along <= 0.0 || perpendicular >= radius_m {
        return false;
    }
    let ratio = perpendicular / radius_m;
    along - radius_m * (1.0 - ratio * ratio).sqrt() < distance
}
