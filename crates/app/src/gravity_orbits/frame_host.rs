//! Presentation adapters share the application preparation and borrowing boundary.
use super::*;
pub(super) enum FrameHost<'a> {
    Native(&'a mut Renderer),
}
impl FrameHost<'_> {
    pub fn take_terrain_atlas_bounds(&mut self) -> Vec<mundaris_renderer::AtlasBounds> {
        match self {
            Self::Native(r) => r.take_terrain_atlas_bounds(),
        }
    }
    pub fn terrain_atlas_report(&self) -> mundaris_renderer::TerrainAtlasReport {
        match self {
            Self::Native(r) => r.terrain_atlas_report(),
        }
    }
    pub fn terrain_atlas_layer_limit(&self) -> u32 {
        match self {
            Self::Native(r) => r.terrain_atlas_layer_limit(),
        }
    }
    pub fn set_cluster_settings(&mut self, settings: mundaris_renderer::ClusterSettings) {
        match self {
            Self::Native(r) => r.set_cluster_settings(settings),
        }
    }
    pub fn cluster_report(&self) -> mundaris_renderer::ClusterReport {
        match self {
            Self::Native(r) => r.cluster_report(),
        }
    }
    pub fn native_timestamp_sampling(
        &self,
    ) -> Option<crate::developer_snapshot::TimestampSamplingSnapshot> {
        match self {
            Self::Native(renderer) => Some(renderer.timestamp_profiling_metrics().into()),
        }
    }
    pub fn native_render_timings(&self) -> mundaris_renderer::NativeRenderTimings {
        match self {
            Self::Native(r) => r.native_render_timings(),
        }
    }
    pub fn native_submission_id(&self) -> Option<u64> {
        match self {
            Self::Native(r) => match r.last_render_outcome() {
                mundaris_renderer::RenderOutcome::Submitted { submission_id, .. } => {
                    Some(submission_id)
                }
                _ => None,
            },
        }
    }
    pub fn presentation_mode_label(&self) -> String {
        match self {
            Self::Native(r) => format!("{:?}", r.presentation_mode()),
        }
    }
    pub fn request_profile_timing(&mut self) {
        #[cfg(feature = "developer-tools")]
        match self {
            Self::Native(renderer) => {
                renderer.request_developer_gpu_timing();
            }
        }
    }
    pub fn pixels_per_point(&self) -> f32 {
        match self {
            Self::Native(r) => r.pixels_per_point(),
        }
    }
    pub fn deterministic(&self) -> bool {
        false
    }
    pub fn latest_gpu_profile(&self) -> mundaris_renderer::GpuProfile {
        match self {
            Self::Native(r) => r.latest_gpu_profile(),
        }
    }
    pub fn timestamp_availability(&self) -> mundaris_renderer::TimestampAvailability {
        match self {
            Self::Native(r) => r.timestamp_availability(),
        }
    }
    pub fn last_resident_regional_report(&self) -> mundaris_renderer::RegionalResidentReport {
        match self {
            Self::Native(r) => r.last_resident_regional_report(),
        }
    }
    pub fn gpu_source_frame(&self) -> Option<u64> {
        match self {
            Self::Native(r) => r.latest_gpu_profile_submission(),
        }
    }
    #[cfg(feature = "surface-profile")]
    pub fn last_terrain_upload_profile(&self) -> mundaris_renderer::CpuUploadProfile {
        match self {
            Self::Native(r) => r.last_terrain_upload_profile(),
        }
    }
    pub fn render(&mut self, ui: impl FnMut(&egui::Context)) -> Result<()> {
        match self {
            Self::Native(r) => Ok(r.render(ui)?),
        }
    }
    pub fn render_celestial(
        &mut self,
        frame: &CelestialFrame<'_, '_, '_>,
        ui: impl FnMut(&egui::Context),
    ) -> Result<()> {
        match self {
            Self::Native(r) => Ok(r.render_celestial(frame, ui)?),
        }
    }
}
