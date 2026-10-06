//! Presentation adapters share the application preparation and borrowing boundary.
use super::*;
pub(super) enum FrameHost<'a> {
    Native(&'a mut Renderer),
    #[cfg(feature = "developer-tools")]
    Offscreen(
        &'a mut mundaris_renderer::terrain_capture::TerrainCaptureRenderer,
        &'a mut Vec<u8>,
    ),
}
impl FrameHost<'_> {
    pub fn pixels_per_point(&self) -> f32 {
        match self {
            Self::Native(r) => r.pixels_per_point(),
            #[cfg(feature = "developer-tools")]
            Self::Offscreen(..) => 1.0,
        }
    }
    pub fn deterministic(&self) -> bool {
        match self {
            Self::Native(_) => false,
            #[cfg(feature = "developer-tools")]
            Self::Offscreen(..) => true,
        }
    }
    pub fn latest_gpu_profile(&self) -> mundaris_renderer::GpuProfile {
        match self {
            Self::Native(r) => r.latest_gpu_profile(),
            #[cfg(feature = "developer-tools")]
            Self::Offscreen(r, _) => r.last_gpu_profile(),
        }
    }
    #[cfg(feature = "developer-tools")]
    pub fn last_resident_tile_report(&self) -> mundaris_renderer::ResidentTileReport {
        match self {
            Self::Native(r) => r.last_resident_tile_report(),
            Self::Offscreen(r, _) => r.last_resident_tile_report(),
        }
    }
    #[cfg(feature = "developer-tools")]
    pub fn last_resident_hierarchy_report(&self) -> mundaris_renderer::ResidentHierarchyReport {
        match self {
            Self::Native(r) => r.last_resident_hierarchy_report(),
            Self::Offscreen(r, _) => r.last_resident_hierarchy_report(),
        }
    }
    #[cfg(feature = "developer-tools")]
    pub fn validate_resident_tile(
        &mut self,
        draw: &mundaris_renderer::TileDraw,
    ) -> Result<Vec<mundaris_renderer::ReconstructedTileVertex>> {
        match self {
            Self::Native(r) => Ok(r.validate_resident_tile(draw)?),
            Self::Offscreen(r, _) => Ok(r.validate_resident_tile(draw)?),
        }
    }
    #[cfg(feature = "developer-tools")]
    pub fn last_resident_regional_report(&self) -> mundaris_renderer::RegionalResidentReport {
        match self {
            Self::Native(r) => r.last_resident_regional_report(),
            Self::Offscreen(r, _) => r.last_resident_regional_report(),
        }
    }
    #[cfg(feature = "developer-tools")]
    pub fn validate_resident_hierarchy(
        &mut self,
        draw: &mundaris_renderer::ResidentHierarchyDraw,
        patch_index: usize,
    ) -> Result<Vec<mundaris_renderer::ReconstructedTileVertex>> {
        match self {
            Self::Native(r) => Ok(r.validate_resident_hierarchy(draw, patch_index)?),
            Self::Offscreen(r, _) => Ok(r.validate_resident_hierarchy(draw, patch_index)?),
        }
    }
    pub fn gpu_source_frame(&self) -> Option<u64> {
        match self {
            Self::Native(r) => r.latest_gpu_profile_submission(),
            #[cfg(feature = "developer-tools")]
            Self::Offscreen(..) => None,
        }
    }
    pub fn last_sky_resource_report(&self) -> Option<mundaris_renderer::sky::SkyResourceReport> {
        match self {
            Self::Native(r) => r.last_sky_resource_report(),
            #[cfg(feature = "developer-tools")]
            Self::Offscreen(r, _) => Some(r.last_sky_resource_report()),
        }
    }
    #[cfg(feature = "surface-profile")]
    pub fn last_terrain_upload_profile(&self) -> mundaris_renderer::CpuUploadProfile {
        match self {
            Self::Native(r) => r.last_terrain_upload_profile(),
            #[cfg(feature = "developer-tools")]
            Self::Offscreen(r, _) => r.last_terrain_upload_profile(),
        }
    }
    pub fn render(&mut self, ui: impl FnMut(&egui::Context)) -> Result<()> {
        match self {
            Self::Native(r) => Ok(r.render(ui)?),
            #[cfg(feature = "developer-tools")]
            Self::Offscreen(..) => {
                anyhow::bail!("production preparation unavailable; no offscreen image")
            }
        }
    }
    pub fn render_celestial(
        &mut self,
        frame: &CelestialFrame<'_, '_, '_>,
        ui: impl FnMut(&egui::Context),
    ) -> Result<()> {
        match self {
            Self::Native(r) => Ok(r.render_celestial(frame, ui)?),
            #[cfg(feature = "developer-tools")]
            Self::Offscreen(r, pixels) => {
                **pixels = r.render(frame)?;
                Ok(())
            }
        }
    }
}
