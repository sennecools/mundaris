//! Typed presentation operations shared by UI and development clients.
use super::*;
#[derive(Clone, Copy)]
pub(super) enum Layer {
    Terrain,
    Markers,
    Labels,
    Trails,
    Guides,
}
impl Layer {
    #[cfg(feature = "developer-tools")]
    pub fn named(name: &str) -> Result<Self> {
        Ok(match name {
            "terrain" => Self::Terrain,
            "markers" => Self::Markers,
            "labels" => Self::Labels,
            "trails" => Self::Trails,
            "guides" => Self::Guides,
            _ => anyhow::bail!("unsupported layer"),
        })
    }
    fn flag(self, controls: &mut Controls) -> &mut bool {
        match self {
            Self::Terrain => &mut controls.terrain_preview,
            Self::Markers => &mut controls.markers,
            Self::Labels => &mut controls.labels,
            Self::Trails => &mut controls.trails,
            Self::Guides => &mut controls.guide_visible,
        }
    }
}
pub(super) enum VisualCommand {
    RenderMode(TerrainViewMode),
    Layer(Layer, bool),
}
impl VisualCommand {
    pub fn apply(self, controls: &mut Controls) -> Result<()> {
        match self {
            Self::RenderMode(mode) => controls.terrain_view = mode,
            Self::Layer(layer, value) => *layer.flag(controls) = value,
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
