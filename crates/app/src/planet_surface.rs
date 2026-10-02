//! App-owned capability, one opaque owner and inspection anchors. No world mutation.
use anyhow::Result;
use glam::DVec3;
use mundaris_math::{surface::*, *};
use mundaris_renderer::CelestialProjection;
use mundaris_renderer::planet_surface::*;
use mundaris_world::BodyId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceRepresentationState {
    Far,
    Prewarm,
    Surface,
}
pub struct PlanetSurfaceSession {
    body: BodyId,
    lod: SurfaceLodSession,
    state: SurfaceRepresentationState,
    pub report: LodReport,
    pub far_error_pixels: f64,
}
impl PlanetSurfaceSession {
    /// Analytic picking identifies the logical covering region, including a culled
    /// guard leaf; it is not a claim that a GPU triangle exists under the pointer.
    pub fn hovered_patch(
        &self,
        pair: &mundaris_world::CoherentCelestialView<'_>,
        observer: FramePose,
        projection: CelestialProjection,
        pixels: [f64; 2],
    ) -> Result<Option<CubePatchAddress>> {
        let frame = pair.projection().frames_for(self.body)?.body_fixed;
        let center = pair
            .evaluation()
            .convert_position(observer.position(), frame)?
            .local()
            .metres();
        let ray = observer
            .orientation()
            .rotate_direction(Direction3::try_new(projection.unproject_ray(pixels)?)?)?;
        let ray = pair
            .evaluation()
            .convert_direction(FrameDirection::new(observer.position().frame(), ray), frame)?
            .local()
            .unit();
        let radius = pair
            .system()
            .body(self.body)?
            .properties()
            .reference_radius_m();
        let along = -center.dot(ray);
        let perpendicular = (center + ray * along).length();
        if perpendicular > radius {
            return Ok(None);
        }
        let half = radius * (1.0 - (perpendicular / radius).powi(2)).max(0.0).sqrt();
        let distance = if along - half > 0.0 {
            along - half
        } else {
            along + half
        };
        if distance <= 0.0 {
            return Ok(None);
        }
        let location = SurfaceLocation::new(Direction3::try_new(center + ray * distance)?);
        let (face, uv) = location.face_uv();
        Ok(self
            .lod
            .covering_leaves()
            .find(|p| p.face() == face && p.patch_local(uv).is_ok()))
    }
    pub fn new(body: BodyId, cache_quota: usize) -> Result<Self> {
        let lod = SurfaceLodSession::new(cache_quota)?;
        let (cache_records, cache_bytes) = lod.cache_usage();
        Ok(Self {
            body,
            lod,
            state: SurfaceRepresentationState::Far,
            report: LodReport {
                cache_records,
                cache_bytes,
                active_patches: 6,
                balanced_patches: 6,
                ..LodReport::default()
            },
            far_error_pixels: 0.0,
        })
    }
    pub fn body(&self) -> BodyId {
        self.body
    }
    pub fn state(&self) -> SurfaceRepresentationState {
        self.state
    }
    pub fn lod(&self) -> &SurfaceLodSession {
        &self.lod
    }
    pub fn update(
        &mut self,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
    ) -> Result<()> {
        let center = input
            .view
            .prepare_source(input.body_fixed_frame)?
            .view_displacement(FramePosition::new(
                input.body_fixed_frame,
                LocalPosition::origin(),
            ))?
            .metres();
        self.far_error_pixels =
            input
                .projection
                .sphere_error_pixels(center, input.reference_radius_m, 0.005)?;
        if self.state == SurfaceRepresentationState::Far && self.far_error_pixels < 0.05 {
            return Ok(());
        }
        self.report = self.lod.update(input, settings)?;
        if self.state == SurfaceRepresentationState::Far {
            self.state = SurfaceRepresentationState::Prewarm;
        }
        // Six pinned ready roots establish complete fallback coverage from setup.
        // An ordinary overlap switch waits for subpixel surface quality. Rapid
        // approach beyond the overlap uses this complete ready coarse fallback.
        if self.state == SurfaceRepresentationState::Prewarm
            && (self.report.max_error_pixels <= 0.125 || self.far_error_pixels > 0.125)
        {
            self.state = SurfaceRepresentationState::Surface;
        }
        Ok(())
    }
    /// Return is committed only after the far sphere's own preparation is ready.
    pub fn return_to_far_if_ready(&mut self, far_ready: bool) {
        if self.far_error_pixels < 0.05 && far_ready {
            self.state = SurfaceRepresentationState::Far;
        }
    }
}

/// Explicit f64 regional pose relative to fixed body axes. Never a tree node or patch ID.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceInspectionAnchor {
    pub body: BodyId,
    pub location: SurfaceLocation,
    pub body_from_regional: RigidTransform,
    pub observer_in_regional: LocalPosition,
    pub tangent: SurfaceTangentBasis,
}
impl SurfaceInspectionAnchor {
    pub fn new(
        body: BodyId,
        p: DVec3,
        radius: f64,
        previous: Option<SurfaceTangentBasis>,
    ) -> Result<Self> {
        let up = Direction3::try_new(p)?;
        let location = SurfaceLocation::new(up);
        let tangent = previous.map_or_else(|| SurfaceTangentBasis::new(up), |t| t.transported(up));
        let basis =
            glam::DMat3::from_cols(tangent.east().unit(), up.unit(), -tangent.north().unit());
        let rotation = UnitRotation::try_from_quaternion(glam::DQuat::from_mat3(&basis))?;
        let body_from_regional =
            RigidTransform::new(Displacement3::try_metres(up.unit() * radius)?, rotation);
        let observer_in_regional = body_from_regional
            .inverse()?
            .transform_position(LocalPosition::try_metres(p)?)?;
        Ok(Self {
            body,
            location,
            body_from_regional,
            observer_in_regional,
            tangent,
        })
    }
    pub fn position(self) -> Result<LocalPosition> {
        Ok(self
            .body_from_regional
            .transform_position(self.observer_in_regional)?)
    }
}
