//! Physically ordered scene lighting and sun shadow cascades
//! (docs/RENDER_PIPELINE_HDR.md §3).
//!
//! The application stages one [`FrameLighting`] per frame in camera-relative
//! view metres. The renderer derives per-pixel sun direction and illuminance
//! from the star position, so every body is lit consistently from orbit to the
//! ground. Cascade fitting runs in f64 in the shadowed body's fixed frame and
//! snaps to the shadow texel grid there, so cascades do not shimmer when the
//! camera moves or turns.

use crate::{CelestialProjection, ShadowSettings};
use glam::{DMat3, DMat4, DVec3, DVec4, Mat4};

/// Reflectance model of a surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Brdf {
    #[default]
    Lambert,
    /// McEwen lunar-Lambert: Lommel-Seeliger regolith blended with Lambert by
    /// phase angle. Flat full-phase disks and bright limbs, as observed.
    LunarLambert,
}

impl Brdf {
    pub fn name(self) -> &'static str {
        match self {
            Self::Lambert => "lambert",
            Self::LunarLambert => "lunar_lambert",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        [Self::Lambert, Self::LunarLambert]
            .into_iter()
            .find(|brdf| brdf.name() == name)
    }
    pub(crate) fn shader_index(self) -> f32 {
        match self {
            Self::Lambert => 0.0,
            Self::LunarLambert => 1.0,
        }
    }
}

/// Grey-world surface description; colour/materials beyond this are Phase M.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceMaterial {
    /// Linear normal albedo per channel, 0..1.
    pub albedo: [f32; 3],
    pub brdf: Brdf,
}

impl Default for SurfaceMaterial {
    fn default() -> Self {
        Self {
            albedo: [0.18; 3],
            brdf: Brdf::Lambert,
        }
    }
}

impl SurfaceMaterial {
    pub fn validate(&self) -> bool {
        self.albedo
            .iter()
            .all(|a| a.is_finite() && (0.0..=1.0).contains(a))
    }
}

/// Body whose terrain receives cascaded shadows: the drawn surface nearest the
/// camera. All values are f64 and body-fixed; nothing here is narrowed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowView {
    /// Columns are the body axes expressed in view axes.
    pub body_to_view: DMat3,
    pub observer_body_m: DVec3,
    pub reference_radius_m: f64,
    /// Upper bound of terrain relief above the reference radius.
    pub relief_m: f64,
}

/// One frame of authored, physically ordered light, in view metres.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameLighting {
    pub sun_center_view_m: DVec3,
    pub sun_radius_m: f64,
    /// Linear sun chromaticity (max component 1).
    pub sun_color: [f32; 3],
    /// Illuminance on a surface facing the sun at `reference_distance_m`.
    pub illuminance_lux: f64,
    pub reference_distance_m: f64,
    /// Isotropic ambient illuminance (starlight, planetshine stand-in).
    pub ambient_lux: f64,
    pub ambient_color: [f32; 3],
    /// Albedo of the sunlit ground that bounces light into shadows (a
    /// single-bounce flat-ground model until GI).
    pub bounce_fraction: f64,
    /// Sky light: horizontal sky illuminance as a fraction of the sun's
    /// normal illuminance at high sun (fades through twilight), applied over
    /// the upper hemisphere by n.up. 0 = airless.
    pub sky_fraction: f64,
    /// Linear sky light chromaticity (max component 1).
    pub sky_color: [f32; 3],
    /// Bodies that can block the sun: view-space centres and radii.
    pub occluders: Vec<(DVec3, f64)>,
}

pub const MAX_OCCLUDERS: usize = 8;

impl FrameLighting {
    pub fn validate(&self) -> Result<(), String> {
        let finite = self.sun_center_view_m.is_finite()
            && self.sun_radius_m.is_finite()
            && self.sun_radius_m > 0.0
            && self.illuminance_lux.is_finite()
            && self.illuminance_lux >= 0.0
            && self.reference_distance_m.is_finite()
            && self.reference_distance_m > 0.0
            && self.ambient_lux.is_finite()
            && self.ambient_lux >= 0.0
            && self.bounce_fraction.is_finite()
            && (0.0..=1.0).contains(&self.bounce_fraction)
            && self.sky_fraction.is_finite()
            && (0.0..=1.0).contains(&self.sky_fraction)
            && self
                .sun_color
                .iter()
                .chain(&self.ambient_color)
                .chain(&self.sky_color)
                .all(|c| c.is_finite() && *c >= 0.0)
            && self
                .occluders
                .iter()
                .all(|(c, r)| c.is_finite() && r.is_finite() && *r > 0.0);
        if finite {
            Ok(())
        } else {
            Err("frame lighting contains invalid values".into())
        }
    }

    /// Sun disk radiance (cd/m²) that reproduces `illuminance_lux` at the
    /// reference distance: E = L·π·sin²α for a uniform disk.
    pub fn sun_disk_radiance(&self) -> f64 {
        let sin_alpha = (self.sun_radius_m / self.reference_distance_m).min(1.0);
        self.illuminance_lux / (std::f64::consts::PI * sin_alpha * sin_alpha)
    }
}

/// Fitted cascades of one frame.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Cascades {
    pub count: u32,
    /// View metres → light clip space ([-1, 1]² × [0, 1]).
    pub view_to_clip: [Mat4; 4],
    /// Far view depth of each cascade.
    pub splits: [f32; 4],
    /// World size of one shadow texel.
    pub texel_m: [f32; 4],
    /// Light-space depth covered by [0, 1].
    pub depth_range_m: [f32; 4],
    /// Unnarrowed view → light-clip transforms for caster culling.
    light: [DMat4; 4],
}

/// Minimal sphere around a symmetric frustum slice between view depths `a` and
/// `b`; returns (centre depth, radius). Depends only on the slice, so the
/// radius is constant while the camera turns.
pub fn slice_sphere(a: f64, b: f64, tan_x: f64, tan_y: f64) -> (f64, f64) {
    let k2 = tan_x * tan_x + tan_y * tan_y;
    let z = ((1.0 + k2) * (a + b) * 0.5).min(b);
    let corner = |depth: f64| ((depth * depth * k2) + (z - depth) * (z - depth)).sqrt();
    (z, corner(a).max(corner(b)))
}

/// Shadowed distance range: from the nearest visible ground to the horizon of
/// terrain up to `relief_m` tall, capped by settings.
pub fn shadow_range(view: &ShadowView, near_m: f64, settings: &ShadowSettings) -> (f64, f64) {
    let r = view.reference_radius_m;
    let altitude = (view.observer_body_m.length() - r).max(0.0);
    let relief = view.relief_m.max(500.0);
    let to_horizon =
        ((r + altitude).powi(2) - r * r).max(0.0).sqrt() + ((r + relief).powi(2) - r * r).sqrt();
    let start = near_m.max(0.7 * altitude);
    let far = (f64::from(settings.max_distance_m).min(to_horizon)).max(start + 10.0);
    (start, far)
}

/// Fits camera-relative cascades. `sun_view` is the unit direction from the
/// shadowed body towards the sun, in view axes.
pub fn fit_cascades(
    view: &ShadowView,
    sun_view: DVec3,
    projection: CelestialProjection,
    settings: &ShadowSettings,
) -> Option<Cascades> {
    if !settings.enabled || !sun_view.is_finite() || sun_view.length_squared() < 0.5 {
        return None;
    }
    let count = settings.cascades.clamp(1, 4) as usize;
    let near = projection.near_m();
    let (start, far) = shadow_range(view, near, settings);
    let first = (start + f64::from(settings.first_cascade_m).max(0.25 * start)).min(far);
    let mut splits = [far; 4];
    for (i, split) in splits.iter_mut().enumerate().take(count) {
        *split = if count == 1 {
            far
        } else {
            first * (far / first).powf(i as f64 / (count - 1) as f64)
        };
    }
    let [w, h] = projection.viewport();
    let tan_y = (projection.vertical_fov_rad() * 0.5).tan();
    let tan_x = tan_y * f64::from(w) / f64::from(h.max(1));

    let view_to_body = view.body_to_view.transpose();
    let sun_body = (view_to_body * sun_view).normalize();
    let helper = if sun_body.y.abs() < 0.9 {
        DVec3::Y
    } else {
        DVec3::X
    };
    let x_l = helper.cross(sun_body).normalize();
    let y_l = sun_body.cross(x_l);
    // Rows of the body → light rotation.
    let light_rot = DMat3::from_cols(x_l, y_l, sun_body).transpose();
    let resolution = f64::from(settings.resolution.max(1));

    let mut out = Cascades {
        count: count as u32,
        ..Default::default()
    };
    let mut previous = near;
    for (c, &split) in splits.iter().enumerate().take(count) {
        let (depth, radius) = slice_sphere(previous, split, tan_x, tan_y);
        // Quantise the radius so texel size changes in discrete steps.
        let radius = 2f64.powf((radius.max(1.0).log2() * 16.0).ceil() / 16.0);
        let texel = 2.0 * radius / resolution;
        let centre_body = view.observer_body_m + view_to_body * DVec3::new(0.0, 0.0, -depth);
        let mut centre_light = light_rot * centre_body;
        centre_light.x = (centre_light.x / texel).round() * texel;
        centre_light.y = (centre_light.y / texel).round() * texel;
        let centre_body = light_rot.transpose() * centre_light;
        let extent = radius + far;
        let range = 2.0 * radius + extent;
        // l = R·(observer + V2B·p − centre) for view point p.
        let rotation = light_rot * view_to_body;
        let translation = light_rot * (view.observer_body_m - centre_body);
        let scale = DMat4::from_cols(
            DVec4::new(1.0 / radius, 0.0, 0.0, 0.0),
            DVec4::new(0.0, 1.0 / radius, 0.0, 0.0),
            DVec4::new(0.0, 0.0, -1.0 / range, 0.0),
            DVec4::new(0.0, 0.0, (radius + extent) / range, 1.0),
        );
        let light_from_view = DMat4::from_cols(
            rotation.x_axis.extend(0.0),
            rotation.y_axis.extend(0.0),
            rotation.z_axis.extend(0.0),
            translation.extend(1.0),
        );
        let clip = scale * light_from_view;
        out.light[c] = clip;
        out.view_to_clip[c] = clip.as_mat4();
        out.splits[c] = split as f32;
        out.texel_m[c] = texel as f32;
        out.depth_range_m[c] = range as f32;
        previous = split;
    }
    Some(out)
}

impl Cascades {
    /// True when a view-space bounding sphere can cast into cascade `c`.
    pub fn may_cast(&self, c: usize, centre_view: DVec3, radius_m: f64) -> bool {
        if c >= self.count as usize {
            return false;
        }
        let m = self.light[c];
        let p = m * centre_view.extend(1.0);
        // A sphere of radius r spans r times each output row's norm (the
        // rotation rows scaled by 1/radius and 1/range).
        let row = |i: usize| DVec3::new(m.x_axis[i], m.y_axis[i], m.z_axis[i]).length();
        let (rx, ry, rz) = (radius_m * row(0), radius_m * row(1), radius_m * row(2));
        p.x.abs() <= 1.0 + rx && p.y.abs() <= 1.0 + ry && p.z >= -rz && p.z <= 1.0 + rz
    }
}

/// Analytic visible fraction of a sun disk of angular radius `rs` behind one
/// occluding disk of angular radius `ro` at angular separation `d` (radians).
/// CPU reference for the shader's eclipse term.
pub fn disk_visible_fraction(rs: f64, ro: f64, d: f64) -> f64 {
    use std::f64::consts::PI;
    if d >= rs + ro {
        return 1.0;
    }
    if d <= (rs - ro).abs() {
        let covered = ro.min(rs);
        return 1.0 - (covered * covered) / (rs * rs);
    }
    let a = ((d * d + rs * rs - ro * ro) / (2.0 * d * rs))
        .clamp(-1.0, 1.0)
        .acos();
    let b = ((d * d + ro * ro - rs * rs) / (2.0 * d * ro))
        .clamp(-1.0, 1.0)
        .acos();
    let lens = rs * rs * a + ro * ro * b
        - 0.5
            * ((-d + rs + ro) * (d + rs - ro) * (d - rs + ro) * (d + rs + ro))
                .max(0.0)
                .sqrt();
    (1.0 - lens / (PI * rs * rs)).clamp(0.0, 1.0)
}

/// Byte size of the shader `Lighting` uniform.
pub(crate) const LIGHTING_BYTES: usize = 528;

/// Pack the `Lighting` uniform shared by `lighting.wgsl` users.
#[allow(clippy::too_many_arguments)] // One flat GPU record.
pub(crate) fn pack_lighting(
    lighting: Option<&FrameLighting>,
    settings: &crate::RenderSettings,
    look: &crate::StylisedLook,
    cascades: Option<&Cascades>,
    near_m: f64,
    view_mode: u32,
) -> Vec<u8> {
    let mut floats = vec![0f32; LIGHTING_BYTES / 4];
    let mut put = |row: usize, values: [f32; 4]| {
        floats[row * 4..row * 4 + 4].copy_from_slice(&values);
    };
    let mut occluder_count = 0usize;
    if let Some(l) = lighting {
        let sun_scale = f64::from(settings.lighting.sun_scale);
        let ambient = l.ambient_lux * f64::from(settings.lighting.ambient_scale);
        put(
            0,
            [
                l.sun_center_view_m.x as f32,
                l.sun_center_view_m.y as f32,
                l.sun_center_view_m.z as f32,
                l.sun_radius_m as f32,
            ],
        );
        put(
            1,
            [
                l.sun_color[0] * (l.illuminance_lux * sun_scale) as f32,
                l.sun_color[1] * (l.illuminance_lux * sun_scale) as f32,
                l.sun_color[2] * (l.illuminance_lux * sun_scale) as f32,
                l.reference_distance_m as f32,
            ],
        );
        put(
            2,
            [
                l.ambient_color[0] * ambient as f32,
                l.ambient_color[1] * ambient as f32,
                l.ambient_color[2] * ambient as f32,
                (l.bounce_fraction * f64::from(settings.lighting.ambient_scale)) as f32,
            ],
        );
        if settings.shadows.eclipses {
            for (i, (centre, radius)) in l.occluders.iter().take(MAX_OCCLUDERS).enumerate() {
                put(
                    8 + i,
                    [
                        centre.x as f32,
                        centre.y as f32,
                        centre.z as f32,
                        *radius as f32,
                    ],
                );
                occluder_count += 1;
            }
        }
    }
    put(
        3,
        [
            near_m as f32,
            view_mode as f32,
            occluder_count as f32,
            if lighting.is_some() { 1.0 } else { 0.0 },
        ],
    );
    if let Some(c) = cascades {
        put(
            4,
            [
                c.count as f32,
                settings.shadows.normal_bias,
                settings.shadows.softness,
                0.0,
            ],
        );
        put(5, c.splits);
        put(6, c.texel_m);
        put(7, c.depth_range_m);
        for (i, matrix) in c.view_to_clip.iter().enumerate() {
            floats[64 + i * 16..64 + i * 16 + 16].copy_from_slice(&matrix.to_cols_array());
        }
    }
    if let Some(l) = lighting {
        // Row 32 (after the cascade matrices): sky light.
        // The stylised look scales the sky light by the planet's `shadow_sky`
        // (lifted, sky-coloured shadows) and turns on the soft rim (w = 1).
        let stylised = settings.look.preset == crate::LookPreset::Stylised;
        let sky = (l.sky_fraction
            * f64::from(settings.lighting.ambient_scale)
            * f64::from(settings.lighting.sky_scale)
            * if stylised {
                f64::from(look.shadow_sky)
            } else {
                1.0
            }) as f32;
        floats[128..132].copy_from_slice(&[
            l.sky_color[0] * sky,
            l.sky_color[1] * sky,
            l.sky_color[2] * sky,
            if stylised { 1.0 } else { 0.0 },
        ]);
    }
    floats.iter().flat_map(|f| f.to_le_bytes()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> ShadowSettings {
        crate::RenderSettings::default().shadows
    }

    #[test]
    fn slice_sphere_contains_every_corner() {
        for (a, b) in [(0.1, 40.0), (40.0, 350.0), (1000.0, 26_000.0)] {
            let (tx, ty) = (0.7, 0.45);
            let (z, r) = slice_sphere(a, b, tx, ty);
            for depth in [a, b] {
                for sx in [-1.0, 1.0] {
                    for sy in [-1.0, 1.0] {
                        let corner = DVec3::new(sx * depth * tx, sy * depth * ty, -depth);
                        assert!((corner - DVec3::new(0.0, 0.0, -z)).length() <= r * (1.0 + 1e-12));
                    }
                }
            }
        }
    }

    #[test]
    fn shadow_range_follows_altitude_and_horizon() {
        let radius = 109_081.0;
        let at = |altitude: f64| ShadowView {
            body_to_view: DMat3::IDENTITY,
            observer_body_m: DVec3::new(0.0, radius + altitude, 0.0),
            reference_radius_m: radius,
            relief_m: 3000.0,
        };
        let (near_start, near_far) = shadow_range(&at(2.0), 0.1, &settings());
        assert!(near_start < 2.0 && (20_000.0..40_000.0).contains(&near_far));
        let (orbit_start, orbit_far) = shadow_range(&at(100_000.0), 0.1, &settings());
        assert!(orbit_start > 60_000.0 && orbit_far > orbit_start);
    }

    #[test]
    fn cascades_cover_receivers_and_stay_stable_under_rotation() {
        let radius = 109_081.0;
        let shadow = |yaw: f64| ShadowView {
            body_to_view: DMat3::from_rotation_y(yaw),
            observer_body_m: DVec3::new(0.0, radius + 2.0, 0.0),
            reference_radius_m: radius,
            relief_m: 3000.0,
        };
        let projection = CelestialProjection::try_new(1920, 1080, 1.0, 0.1).unwrap();
        let sun_body = DVec3::new(0.3, 0.6, 0.2).normalize();
        let fit = |yaw: f64| {
            let s = shadow(yaw);
            fit_cascades(&s, s.body_to_view * sun_body, projection, &settings()).unwrap()
        };
        let a = fit(0.0);
        assert_eq!(a.count, 4);
        assert!(a.splits.windows(2).all(|w| w[0] < w[1]));
        // A receiver on the view axis inside cascade 0 projects inside its map.
        let p = a.view_to_clip[0] * glam::Vec4::new(0.0, -1.5, -10.0, 1.0);
        assert!(p.x.abs() < 1.0 && p.y.abs() < 1.0 && (0.0..=1.0).contains(&p.z));
        // Texel size depends only on the slice, not on the view direction.
        let b = fit(0.7);
        assert_eq!(a.texel_m, b.texel_m);
    }

    #[test]
    fn casters_near_the_camera_pass_for_a_sun_along_the_view_x_axis() {
        let radius = 109_081.0;
        let view = ShadowView {
            body_to_view: DMat3::IDENTITY,
            observer_body_m: DVec3::new(0.0, radius + 2.0, 0.0),
            reference_radius_m: radius,
            relief_m: 3000.0,
        };
        let projection = CelestialProjection::try_new(1092, 490, 1.0, 0.1).unwrap();
        // Sun low from the right: view +x, 15 degrees up.
        let sun = DVec3::new(15f64.to_radians().cos(), 15f64.to_radians().sin(), 0.0);
        let cascades = fit_cascades(&view, sun, projection, &settings()).unwrap();
        for c in 0..cascades.count as usize {
            // Ground under the camera, and a large ancestor node around it.
            assert!(
                cascades.may_cast(c, DVec3::new(0.0, -2.0, -5.0), 1.0),
                "cascade {c}"
            );
            assert!(cascades.may_cast(c, DVec3::new(0.0, -20_000.0, 0.0), 40_000.0));
        }
        // Far behind the sunward strip is culled.
        assert!(!cascades.may_cast(0, DVec3::new(-50_000.0, 0.0, 0.0), 10.0));
    }

    #[test]
    fn disk_overlap_limits() {
        let rs = 0.01;
        assert_eq!(disk_visible_fraction(rs, 0.005, 1.0), 1.0);
        assert!((disk_visible_fraction(rs, 0.5, 0.0) - 0.0).abs() < 1e-12);
        assert!((disk_visible_fraction(rs, 0.005, 0.0) - 0.75).abs() < 1e-12);
        // A straight-edged occluder through the disk centre hides half.
        let half = disk_visible_fraction(rs, 1.0, 1.0);
        assert!((half - 0.5).abs() < 0.01, "{half}");
    }

    #[test]
    fn sun_disk_radiance_reproduces_reference_illuminance() {
        let lighting = FrameLighting {
            sun_center_view_m: DVec3::ZERO,
            sun_radius_m: 800_000.0,
            sun_color: [1.0; 3],
            illuminance_lux: 120_000.0,
            reference_distance_m: 24_000_000.0,
            ambient_lux: 0.0,
            ambient_color: [1.0; 3],
            bounce_fraction: 0.0,
            sky_fraction: 0.0,
            sky_color: [1.0; 3],
            occluders: Vec::new(),
        };
        let l = lighting.sun_disk_radiance();
        let s = 800_000.0 / 24_000_000.0f64;
        assert!((l * std::f64::consts::PI * s * s - 120_000.0).abs() < 1e-6);
    }
}
