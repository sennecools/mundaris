//! Bounded decorative distant content; no celestial-domain identities or motion.
//!
//! Catalogue coordinates are galactic-local metres relative to an inertial anchor.
//! Source-relative observer/axes are prepared in f64, then expressed in 1e18 m GPU
//! units. Ordinary camera motion uploads only a small uniform, never the catalogue.
use crate::{CelestialProjection, PreparedView, RenderPreparationError};
use glam::{DQuat, DVec3, Mat3, Vec3};
use astrum_math::{Direction3, FrameId};
use std::{sync::Arc, time::Instant};

#[path = "sky_background.rs"]
mod background;
#[path = "sky_gpu.rs"]
mod gpu;
#[path = "sky_structure.rs"]
mod structure;
pub(crate) use gpu::SkyRenderer;
pub use structure::{SkyBranch, SkyCavity, SkyComplex, SkyDiskRegion, SkyMorphology};

pub const FINITE_DISTANCE_RANGE_M: [f64; 2] = [1.0e18, 2.0e19];
pub const OBSERVER_ENVELOPE_M: f64 = 1.0e14;
pub const GPU_UNIT_M: f64 = 1.0e18;
const STAR_LIMIT: usize = 131_072;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SkyIdentity {
    pub preset: &'static str,
    pub version: u32,
    pub seed: u64,
}
#[derive(Clone, Debug, PartialEq)]
pub struct SkyStar {
    pub position_m: DVec3,
    pub color: [f32; 3],
    pub flux: f32,
    /// Gaussian standard deviation in physical pixels, not a physical radius.
    pub radius_pixels: f32,
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyBackground {
    pub seed: u64,
    pub width: u32,
    pub height: u32,
    pub band_width_rad: f64,
    pub dust_strength: f64,
    pub brightness: f32,
}
impl Default for SkyBackground {
    fn default() -> Self {
        Self {
            seed: 0,
            width: 1024,
            height: 512,
            band_width_rad: 0.13,
            dust_strength: 0.85,
            brightness: 0.09,
        }
    }
}

/// Immutable validated inputs. Construction publishes nothing on any failure.
#[derive(Clone, Debug)]
pub struct SkyDefinition {
    identity: SkyIdentity,
    anchor_m: DVec3,
    galactic_to_system: DQuat,
    stars: Vec<SkyStar>,
    background: SkyBackground,
    morphology: Option<SkyMorphology>,
}
impl SkyDefinition {
    pub fn try_new(
        identity: SkyIdentity,
        anchor_m: DVec3,
        galactic_to_system: DQuat,
        stars: Vec<SkyStar>,
        background: SkyBackground,
    ) -> Result<Self, RenderPreparationError> {
        if identity.preset.is_empty()
            || identity.preset.len() > 128
            || identity.version == 0
            || !anchor_m.is_finite()
            || anchor_m.length() > OBSERVER_ENVELOPE_M
            || !galactic_to_system.is_finite()
            || (galactic_to_system.length() - 1.0).abs() > 1e-10
            || stars.len() > STAR_LIMIT
            || !background.width.is_power_of_two()
            || !background.height.is_power_of_two()
            || background.width > 4096
            || background.height > 2048
            || !background.band_width_rad.is_finite()
            || !(0.01..=1.5).contains(&background.band_width_rad)
            || !background.dust_strength.is_finite()
            || !(0.0..=1.0).contains(&background.dust_strength)
            || !background.brightness.is_finite()
            || !(0.0..=1.0).contains(&background.brightness)
            || stars.iter().any(|s| {
                !s.position_m.is_finite()
                    || !(FINITE_DISTANCE_RANGE_M[0]..=FINITE_DISTANCE_RANGE_M[1])
                        .contains(&s.position_m.length())
                    || s.color
                        .iter()
                        .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
                    || !s.flux.is_finite()
                    || !(0.0..=100.0).contains(&s.flux)
                    || !s.radius_pixels.is_finite()
                    || !(0.35..=4.0).contains(&s.radius_pixels)
            })
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        Ok(Self {
            identity,
            anchor_m,
            galactic_to_system,
            stars,
            background,
            morphology: None,
        })
    }
    /// Attaches a bounded immutable decorative composition. Invalid graphs cannot
    /// enter the cache; renderer evaluation never changes these authored inputs.
    pub fn with_morphology(
        mut self,
        morphology: SkyMorphology,
    ) -> Result<Self, RenderPreparationError> {
        if !morphology.valid() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        self.morphology = Some(morphology);
        Ok(self)
    }
    pub fn identity(&self) -> &SkyIdentity {
        &self.identity
    }
    pub fn anchor_m(&self) -> DVec3 {
        self.anchor_m
    }
    pub fn galactic_to_system(&self) -> DQuat {
        self.galactic_to_system
    }
    pub fn stars(&self) -> &[SkyStar] {
        &self.stars
    }
    pub fn background(&self) -> SkyBackground {
        self.background
    }
    pub fn morphology(&self) -> Option<&SkyMorphology> {
        self.morphology.as_ref()
    }
    pub fn cpu_capacity_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.stars.capacity() * std::mem::size_of::<SkyStar>()
            + self
                .morphology
                .as_ref()
                .map_or(0, SkyMorphology::capacity_bytes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkySettings {
    pub enabled: bool,
    pub intensity: f32,
    pub star_intensity: f32,
    pub background_intensity: f32,
    pub halo_strength: f32,
    pub galactic_yaw_rad: f64,
    pub galactic_roll_rad: f64,
}
impl Default for SkySettings {
    fn default() -> Self {
        Self {
            enabled: true,
            intensity: 1.0,
            star_intensity: 1.0,
            background_intensity: 1.0,
            halo_strength: 0.12,
            galactic_yaw_rad: 0.0,
            galactic_roll_rad: 0.0,
        }
    }
}
impl SkySettings {
    pub fn try_validate(self) -> Result<Self, RenderPreparationError> {
        if [
            self.intensity,
            self.star_intensity,
            self.background_intensity,
        ]
        .iter()
        .any(|x| !x.is_finite() || !(0.0..=4.0).contains(x))
            || !self.halo_strength.is_finite()
            || !(0.0..=0.5).contains(&self.halo_strength)
            || [self.galactic_yaw_rad, self.galactic_roll_rad]
                .iter()
                .any(|x| !x.is_finite() || x.abs() > std::f64::consts::TAU)
        {
            Err(RenderPreparationError::InvalidDebugGeometry)
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyPreparationReport {
    pub enabled: bool,
    pub stars_drawn: bool,
    pub background_drawn: bool,
    pub outside_envelope: bool,
    pub star_count: usize,
    /// Host preparation only; excludes definition generation and upload API work.
    pub cpu_preparation_ms: f64,
}
pub struct SkyPrepared {
    report: SkyPreparationReport,
    definition: Arc<SkyDefinition>,
    settings: SkySettings,
    axes: Mat3,
    observer_units: Vec3,
    projection: CelestialProjection,
    scales: [f32; 2],
}
impl SkyPrepared {
    pub fn new(
        view: &PreparedView<'_>,
        inertial_frame: FrameId,
        definition: Arc<SkyDefinition>,
        settings: SkySettings,
        projection: CelestialProjection,
    ) -> Result<Self, RenderPreparationError> {
        let started = Instant::now();
        let settings = settings.try_validate()?;
        let source = view.prepare_source(inertial_frame)?;
        let observer = source.observer_in_source().metres() - definition.anchor_m;
        if !observer.is_finite() {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let outside = observer.length() > OBSERVER_ENVELOPE_M;
        let orientation = definition.galactic_to_system
            * DQuat::from_rotation_y(settings.galactic_yaw_rad)
            * DQuat::from_rotation_z(settings.galactic_roll_rad);
        let mut axes = [Vec3::ZERO; 3];
        for (out, axis) in axes.iter_mut().zip([DVec3::X, DVec3::Y, DVec3::Z]) {
            *out = source
                .view_direction(Direction3::try_new(orientation * axis)?)?
                .unit()
                .as_vec3();
        }
        // No sky is submitted outside the envelope. Do not narrow an unsupported
        // observer (or invalidate otherwise valid foreground drawing) merely to
        // populate an unused uniform. This does not clamp the camera itself.
        let observer_units = if outside {
            Vec3::ZERO
        } else {
            (orientation.inverse() * observer / GPU_UNIT_M).as_vec3()
        };
        let [width, height] = projection.viewport();
        let scales = [
            (2.0 * projection.focal_pixels() / f64::from(width)) as f32,
            (2.0 * projection.focal_pixels() / f64::from(height)) as f32,
        ];
        if !observer_units.is_finite()
            || axes.iter().any(|a| !a.is_finite())
            || scales.iter().any(|x| !x.is_finite())
        {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let drawn = settings.enabled && !outside && settings.intensity > 0.0;
        let report = SkyPreparationReport {
            enabled: settings.enabled,
            stars_drawn: drawn && settings.star_intensity > 0.0 && !definition.stars.is_empty(),
            background_drawn: drawn
                && settings.background_intensity > 0.0
                && definition.background.brightness > 0.0,
            outside_envelope: outside,
            star_count: definition.stars.len(),
            cpu_preparation_ms: started.elapsed().as_secs_f64() * 1000.0,
        };
        Ok(Self {
            report,
            definition,
            settings,
            axes: Mat3::from_cols(axes[0], axes[1], axes[2]),
            observer_units,
            projection,
            scales,
        })
    }
    pub fn report(&self) -> SkyPreparationReport {
        self.report
    }
    pub fn definition(&self) -> &Arc<SkyDefinition> {
        &self.definition
    }
    pub fn settings(&self) -> SkySettings {
        self.settings
    }
    pub(crate) fn uniform_bytes(&self) -> [u8; 112] {
        let [w, h] = self.projection.viewport();
        let [x, y] = self.projection.origin();
        let mut values = Vec::with_capacity(28);
        for axis in [self.axes.x_axis, self.axes.y_axis, self.axes.z_axis] {
            values.extend_from_slice(&[axis.x, axis.y, axis.z, 0.0]);
        }
        values.extend_from_slice(&[
            self.observer_units.x,
            self.observer_units.y,
            self.observer_units.z,
            0.0,
        ]);
        values.extend_from_slice(&[self.scales[0], self.scales[1], w as f32, h as f32]);
        values.extend_from_slice(&[
            self.settings.intensity * self.settings.star_intensity,
            self.settings.intensity * self.settings.background_intensity,
            self.settings.halo_strength,
            1.0,
        ]);
        values.extend_from_slice(&[x as f32, y as f32, 0.0, 0.0]);
        let mut bytes = [0; 112];
        for (out, value) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(values) {
            *out = value.to_le_bytes();
        }
        bytes
    }
    /// CPU emulation of shader f32 centre arithmetic, for numerical fixtures only.
    /// This is not a GPU measurement or an observer-precision universe claim.
    pub fn gpu_star_center_pixels(&self, index: usize) -> Option<[f64; 2]> {
        if self.report.outside_envelope {
            return None;
        }
        let star = self.definition.stars.get(index)?;
        let camera = self.axes * ((star.position_m / GPU_UNIT_M).as_vec3() - self.observer_units);
        if camera.z >= 0.0 {
            return None;
        }
        let ndc = [
            camera.x * self.scales[0] / -camera.z,
            camera.y * self.scales[1] / -camera.z,
        ];
        let [w, h] = self.projection.viewport();
        let [x, y] = self.projection.origin();
        Some([
            f64::from(x as f32 + (ndc[0] + 1.0) * 0.5 * w as f32),
            f64::from(y as f32 + (1.0 - ndc[1]) * 0.5 * h as f32),
        ])
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SkyResourceReport {
    pub generation_ms: Option<f64>,
    pub upload_api_ms: f64,
    pub static_upload_bytes: u64,
    pub frame_upload_bytes: u64,
    pub gpu_capacity_bytes: u64,
    pub cpu_capacity_bytes: u64,
    pub resource_growth_events: u64,
    pub catalogue_upload_count: u64,
    pub background_upload_count: u64,
    /// Conservative owned transient generation-payload bound, excluding allocator,
    /// driver staging and process RSS. Only populated on definition replacement.
    pub transient_generation_payload_bound_bytes: Option<u64>,
}

/// Raw, unsharpened cached base/focal pixels for angular-detail diagnosis.
#[cfg(feature = "terrain-capture")]
pub struct SkyBackgroundImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Regenerates diagnostic pixels only; it neither uploads nor changes residency.
#[cfg(feature = "terrain-capture")]
pub fn inspect_background(definition: &SkyDefinition) -> Vec<SkyBackgroundImage> {
    inspect_background_mips(definition)
        .into_iter()
        .map(|mut levels| levels.remove(0))
        .collect()
}

/// Complete cached mip pixels, for filtered-image numerical regression oracles.
#[cfg(feature = "terrain-capture")]
pub fn inspect_background_mips(definition: &SkyDefinition) -> Vec<Vec<SkyBackgroundImage>> {
    let mut layers = vec![background::precompute(
        definition.background,
        definition.morphology(),
    )];
    if let Some(morphology) = definition.morphology() {
        layers.extend(background::precompute_details(
            definition.background,
            morphology,
        ));
    }
    layers
        .into_iter()
        .map(|levels| {
            levels
                .into_iter()
                .map(|mip| SkyBackgroundImage {
                    width: mip.width,
                    height: mip.height,
                    rgba: mip.rgba,
                })
                .collect()
        })
        .collect()
}

/// Linear generated-field sample, before texture quantization and GPU filtering.
#[cfg(feature = "terrain-capture")]
pub fn sample_background(
    definition: &SkyDefinition,
    direction: DVec3,
) -> Result<[f32; 3], RenderPreparationError> {
    if !direction.is_finite() || (direction.length() - 1.0).abs() > 1e-10 {
        return Err(RenderPreparationError::InvalidDebugGeometry);
    }
    Ok(background::sample_linear(
        definition.background,
        definition.morphology(),
        direction.to_array(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrum_math::{FramePose, FramePosition, FrameTree, LocalPosition, UnitRotation};
    use std::num::NonZeroU64;

    fn definition() -> Arc<SkyDefinition> {
        Arc::new(
            SkyDefinition::try_new(
                SkyIdentity {
                    preset: "fixture",
                    version: 1,
                    seed: 42,
                },
                DVec3::ZERO,
                DQuat::from_rotation_z(0.3),
                vec![SkyStar {
                    position_m: DVec3::new(0.15, 0.1, -1.0).normalize() * 1.0e18,
                    color: [1.0; 3],
                    flux: 2.0,
                    radius_pixels: 1.0,
                }],
                SkyBackground::default(),
            )
            .unwrap(),
        )
    }
    #[test]
    fn invalid_inputs_cannot_publish_definition_or_settings() {
        let d = definition();
        for flux in [f32::NAN, -1.0, f32::INFINITY] {
            let mut stars = d.stars.clone();
            stars[0].flux = flux;
            assert!(
                SkyDefinition::try_new(
                    d.identity.clone(),
                    d.anchor_m,
                    d.galactic_to_system,
                    stars,
                    d.background
                )
                .is_err()
            );
        }
        for value in [f32::NAN, -1.0, 5.0] {
            assert!(
                SkySettings {
                    intensity: value,
                    ..Default::default()
                }
                .try_validate()
                .is_err()
            );
        }
        assert!(
            SkySettings {
                galactic_roll_rad: f64::INFINITY,
                ..Default::default()
            }
            .try_validate()
            .is_err()
        );
    }
    #[test]
    fn source_relative_projection_matches_independent_f64_fixture() {
        let tree = FrameTree::new(NonZeroU64::new(51313).unwrap());
        let d = definition();
        for fov in [10.0_f64, 30.0, 60.0, 120.0] {
            for [w, h] in [[960, 640], [2560, 1440], [3840, 2160]] {
                for observer in [
                    DVec3::ZERO,
                    DVec3::new(1.495978707e11, 0.0, 0.0),
                    DVec3::new(9.0e13, 1.0e12, -1.0e12),
                ] {
                    let q = DQuat::from_rotation_y(0.04) * DQuat::from_rotation_x(-0.03);
                    let pose = FramePose::new(
                        FramePosition::new(
                            tree.root(),
                            LocalPosition::try_metres(observer).unwrap(),
                        ),
                        UnitRotation::try_from_quaternion(q).unwrap(),
                    );
                    let view = PreparedView::new(
                        &tree.evaluate(),
                        pose,
                        crate::RenderPrecisionBudget::near_debug(),
                    )
                    .unwrap();
                    let projection = CelestialProjection::try_new(w, h, fov.to_radians(), 0.1)
                        .unwrap()
                        .with_origin([71, 39])
                        .unwrap();
                    let prepared = SkyPrepared::new(
                        &view,
                        tree.root(),
                        Arc::clone(&d),
                        SkySettings::default(),
                        projection,
                    )
                    .unwrap();
                    let p = q.inverse() * (d.galactic_to_system * d.stars[0].position_m - observer);
                    let focal = f64::from(h) / (2.0 * (fov.to_radians() * 0.5).tan());
                    let oracle = [
                        71.0 + f64::from(w) * 0.5 + focal * p.x / -p.z,
                        39.0 + f64::from(h) * 0.5 - focal * p.y / -p.z,
                    ];
                    let actual = prepared.gpu_star_center_pixels(0).unwrap();
                    let error = (actual[0] - oracle[0]).hypot(actual[1] - oracle[1]);
                    assert!(error <= 0.05, "{w}x{h} FOV {fov}: {error} px");
                }
            }
        }
    }
    #[test]
    fn finite_parallax_is_small_and_outside_envelope_is_explicit() {
        let tree = FrameTree::new(NonZeroU64::new(51314).unwrap());
        let d = definition();
        let projection =
            CelestialProjection::try_new(2560, 1440, 60_f64.to_radians(), 0.1).unwrap();
        let prepare = |offset| {
            let view = PreparedView::new(
                &tree.evaluate(),
                FramePose::new(
                    FramePosition::new(
                        tree.root(),
                        LocalPosition::try_metres(DVec3::X * offset).unwrap(),
                    ),
                    UnitRotation::identity(),
                ),
                crate::RenderPrecisionBudget::near_debug(),
            )
            .unwrap();
            SkyPrepared::new(
                &view,
                tree.root(),
                Arc::clone(&d),
                SkySettings::default(),
                projection,
            )
            .unwrap()
        };
        let a = prepare(0.0).gpu_star_center_pixels(0).unwrap();
        let b = prepare(1.495978707e11).gpu_star_center_pixels(0).unwrap();
        let c = prepare(1.0e14).gpu_star_center_pixels(0).unwrap();
        let small = (a[0] - b[0]).abs();
        let large = (a[0] - c[0]).abs();
        assert!(small < 0.001 && small > 0.0, "realistic parallax {small}");
        assert!(large > 0.1 && large < 1.0, "enlarged baseline {large}");
        let outside = prepare(1.01e14);
        assert!(outside.report().outside_envelope);
        assert!(!outside.report().stars_drawn && !outside.report().background_drawn);
        assert_eq!(outside.gpu_star_center_pixels(0), None);
        let far_outside = prepare(1.0e80);
        assert!(far_outside.report().outside_envelope);
        assert!(!far_outside.report().stars_drawn && !far_outside.report().background_drawn);
    }
}
