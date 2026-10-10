//! Presentation adapter shared by the application preparation and borrowing
//! boundary. The scene renders into an offscreen texture; the UI toolkit
//! presents it, so the host also carries the UI scale factor.
use super::*;
pub(super) struct FrameHost<'a> {
    pub renderer: &'a mut Renderer,
    pub pixels_per_point: f32,
}
impl FrameHost<'_> {
    pub fn take_terrain_atlas_bounds(&mut self) -> Vec<astrum_renderer::AtlasBounds> {
        self.renderer.take_terrain_atlas_bounds()
    }
    pub fn take_terrain_ready_sources(&mut self) -> Vec<(u64, Option<String>)> {
        self.renderer.take_terrain_ready_sources()
    }
    pub fn take_terrain_world_fields(&mut self) -> Vec<astrum_renderer::AtlasWorldFields> {
        self.renderer.take_terrain_world_fields()
    }
    pub fn set_terrain_world_rivers(&mut self, source: u64, words: Vec<u32>) {
        self.renderer.set_terrain_world_rivers(source, words);
    }
    pub fn take_terrain_collision_pages(&mut self) -> Vec<astrum_renderer::AtlasCollisionPage> {
        self.renderer.take_terrain_collision_pages()
    }
    pub fn terrain_atlas_report(&self) -> astrum_renderer::TerrainAtlasReport {
        self.renderer.terrain_atlas_report()
    }
    pub fn terrain_atlas_layer_limit(&self) -> u32 {
        self.renderer.terrain_atlas_layer_limit()
    }
    pub fn native_timestamp_sampling(
        &self,
    ) -> Option<crate::developer_snapshot::TimestampSamplingSnapshot> {
        Some(self.renderer.timestamp_profiling_metrics().into())
    }
    pub fn native_render_timings(&self) -> astrum_renderer::NativeRenderTimings {
        self.renderer.native_render_timings()
    }
    pub fn native_submission_id(&self) -> Option<u64> {
        match self.renderer.last_render_outcome() {
            astrum_renderer::RenderOutcome::Submitted { submission_id, .. } => Some(submission_id),
            _ => None,
        }
    }
    pub fn presentation_mode_label(&self) -> String {
        "ui-toolkit".into()
    }
    pub fn request_profile_timing(&mut self) {
        #[cfg(feature = "developer-tools")]
        self.renderer.request_developer_gpu_timing();
    }
    pub fn pixels_per_point(&self) -> f32 {
        self.pixels_per_point
    }
    #[cfg(feature = "developer-tools")]
    pub fn deterministic(&self) -> bool {
        false
    }
    pub fn latest_gpu_profile(&self) -> astrum_renderer::GpuProfile {
        self.renderer.latest_gpu_profile()
    }
    pub fn timestamp_availability(&self) -> astrum_renderer::TimestampAvailability {
        self.renderer.timestamp_availability()
    }
    pub fn gpu_source_frame(&self) -> Option<u64> {
        self.renderer.latest_gpu_profile_submission()
    }
    pub fn set_render_settings(&mut self, settings: astrum_renderer::RenderSettings) -> Result<()> {
        self.renderer
            .set_render_settings(settings)
            .map_err(anyhow::Error::msg)
    }
    pub fn shadow_report(&self) -> astrum_renderer::ShadowReport {
        self.renderer.shadow_report()
    }
    pub fn render_empty(&mut self) -> Result<()> {
        Ok(self.renderer.render_empty()?)
    }
    pub fn render_celestial(&mut self, frame: &CelestialFrame<'_, '_, '_>) -> Result<()> {
        Ok(self.renderer.render_celestial(frame)?)
    }
}
