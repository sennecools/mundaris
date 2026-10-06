//! Typed presentation operations shared by UI and development clients.
use super::*;
#[derive(Clone, Copy)]
pub(super) enum Layer {
    Terrain,
    Ocean,
    Clouds,
    Atmosphere,
    #[cfg(feature = "developer-tools")]
    Sky,
    Markers,
    Labels,
    Trails,
    Guides,
    Borders,
    LodColors,
}
impl Layer {
    #[cfg(feature = "developer-tools")]
    pub fn named(name: &str) -> Result<Self> {
        Ok(match name {
            "terrain" => Self::Terrain,
            "ocean" => Self::Ocean,
            "clouds" => Self::Clouds,
            "atmosphere" => Self::Atmosphere,
            "sky" => Self::Sky,
            "markers" => Self::Markers,
            "labels" => Self::Labels,
            "trails" => Self::Trails,
            "guides" => Self::Guides,
            "patch_borders" => Self::Borders,
            "lod_colors" => Self::LodColors,
            _ => anyhow::bail!("unsupported layer"),
        })
    }
    fn flag(self, controls: &mut Controls) -> &mut bool {
        match self {
            Self::Terrain => &mut controls.terrain_preview,
            Self::Ocean => &mut controls.planetary_ocean,
            Self::Clouds => &mut controls.planetary_clouds,
            Self::Atmosphere => &mut controls.planetary_atmosphere,
            #[cfg(feature = "developer-tools")]
            Self::Sky => &mut controls.sky.enabled,
            Self::Markers => &mut controls.markers,
            Self::Labels => &mut controls.labels,
            Self::Trails => &mut controls.trails,
            Self::Guides => &mut controls.guide_visible,
            Self::Borders => &mut controls.surface_style.borders,
            Self::LodColors => &mut controls.surface_style.lod_colors,
        }
    }
}
pub(super) enum VisualCommand {
    RenderMode(TerrainRenderMode),
    Layer(Layer, bool),
    Sky(mundaris_renderer::sky::SkySettings),
}
impl VisualCommand {
    pub fn apply(self, controls: &mut Controls) -> Result<()> {
        match self {
            Self::RenderMode(mode) => {
                controls.terrain_lighting = controls.terrain_lighting.with_mode(mode)
            }
            Self::Layer(layer, value) => *layer.flag(controls) = value,
            Self::Sky(settings) => controls.sky = settings.try_validate()?,
        };
        Ok(())
    }
}
pub(super) fn checkbox(
    ui: &mut egui::Ui,
    controls: &mut Controls,
    layer: Layer,
    label: &str,
) -> egui::Response {
    let mut value = *layer.flag(controls);
    let response = ui.checkbox(&mut value, label);
    if response.changed() {
        controls
            .pending
            .push_back(Command::Visual(VisualCommand::Layer(layer, value)));
    }
    response
}
