//! Toolkit-free Studio view model and actions (docs/STUDIO_UI.md §2).
//!
//! The frame owner publishes one [`StudioView`] per prepared frame; the UI
//! renders it and reports interactions as [`StudioAction`]s. Neither type
//! references the UI toolkit, so headless sessions and tests use them directly.

/// Display tone shared with the UI theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tone {
    #[default]
    Normal,
    Ok,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatItem {
    pub label: String,
    pub value: String,
    pub tone: Tone,
}

impl StatItem {
    pub fn new(label: &str, value: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            value: value.into(),
            tone: Tone::Normal,
        }
    }
    pub fn tone(mut self, tone: Tone) -> Self {
        self.tone = tone;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct BodyItem {
    pub name: String,
    pub detail: String,
    pub depth: u32,
    pub selected: bool,
    pub focused: bool,
}

/// Label state shown on the viewport overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LabelState {
    #[default]
    Normal,
    Selected,
    Focused,
}

/// A placed body label in viewport logical pixels.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LabelItem {
    pub rect: [f32; 4],
    pub marker: [f32; 2],
    pub text: String,
    pub state: LabelState,
    pub leader: bool,
    pub show_marker: bool,
    pub ring: f32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct LogItem {
    pub time: String,
    pub text: String,
    pub tone: Tone,
}

/// Everything the Studio UI shows for one frame.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StudioView {
    pub paused: bool,
    pub time_text: String,
    /// Index into [`RATE_PRESETS`], or `None` for a custom rate.
    pub rate_index: Option<usize>,
    pub camera_index: usize,
    pub view_index: usize,
    pub terrain_enabled: bool,
    pub labels_enabled: bool,
    /// Overlay toggles in [`Overlay`] order: markers, trails, guides.
    pub overlays: [bool; 3],
    pub bodies: Vec<BodyItem>,
    pub labels: Vec<LabelItem>,
    pub hud: Vec<StatItem>,
    pub inspector_title: String,
    pub inspector_subtitle: String,
    pub body_stats: Vec<StatItem>,
    pub camera_stats: Vec<StatItem>,
    pub terrain_stats: Vec<StatItem>,
    pub speed_exponent: f32,
    pub surface_available: bool,
    pub status: Vec<StatItem>,
    pub automation_owner: String,
    pub build_text: String,
    pub log: Vec<LogItem>,
    /// Latest completed GPU timings in ms: main scene pass, terrain, overlays.
    pub gpu_passes: [Option<f64>; 3],
    /// Render registry values in `render_settings::SPECS` order.
    pub render_settings: Vec<crate::render_settings::SettingValue>,
    /// Per-pass GPU times and shadow cascade state for the render inspector.
    pub render_stats: Vec<StatItem>,
    /// Planet editor of the selected body, when it is a world map.
    pub planet: Option<PlanetView>,
}

/// One editable planet parameter (`archetype::PARAM_FIELDS` order).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanetParamItem {
    pub name: String,
    pub value: f64,
    /// Slider range.
    pub range: [f64; 2],
    /// Overridden in the edit (else sampled from the archetype).
    pub overridden: bool,
}

/// Planet editor panel (pipeline §18.1, M1 Step 6).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PlanetView {
    pub archetype: String,
    pub terrain_file: String,
    pub seed: u64,
    /// Unsaved changes.
    pub dirty: bool,
    pub params: Vec<PlanetParamItem>,
    pub stats: Vec<StatItem>,
    pub can_undo: bool,
    pub can_redo: bool,
    /// A Tier A bake of the latest edit is still running.
    pub baking: bool,
}

/// Playback-rate presets shown in the toolbar.
pub const RATE_PRESETS: [f64; 6] = [1.0, 10.0, 100.0, 1_000.0, 10_000.0, 100_000.0];
/// Clearance presets in metres for the inspector's "go to altitude" row.
pub const ALTITUDE_PRESETS_M: [f64; 6] = [100_000.0, 10_000.0, 1_000.0, 100.0, 10.0, 2.0];
/// Camera modes in toolbar order.
pub const CAMERA_MODES: [&str; 4] = ["System", "Orbit", "Surface", "Free"];

/// One user interaction from the Studio UI. Each maps to existing app commands.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StudioAction {
    TogglePause,
    SetRate(usize),
    SetCamera(usize),
    SetView(usize),
    ToggleTerrain,
    ToggleLabels,
    ToggleOverlay(Overlay),
    SelectBody(usize),
    FocusBody(usize),
    SetSpeedExponent(f32),
    Approach(usize),
    SurfaceNavigation,
    FrameSelected,
    ProfilerEnabled(bool),
    ProfilerFrozen(bool),
    ProfilerExport,
    /// Starts or stops the bad-frame capture bundle writer.
    CaptureBadFrames,
    Capture,
    /// Sets render registry entry `index` (`render_settings::SPECS`).
    SetSetting(usize, crate::render_settings::SettingValue),
    ResetRenderSettings,
    /// Planet editor (selected world-map body).
    PlanetSeed(u64),
    PlanetRandomSeed,
    /// Override parameter `index` (`archetype::PARAM_FIELDS`).
    PlanetParam(usize, f64),
    PlanetResetParam(usize),
    PlanetRevert,
    PlanetSave,
    PlanetUndo,
    PlanetRedo,
}

/// Keyboard shortcuts handled by the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shortcut {
    Focus,
    SurfaceNavigation,
    Horizon,
    Overview,
    FreeFlight,
    NextBody,
    PreviousBody,
}

/// Index of `rate` in [`RATE_PRESETS`], if it is one of them.
pub fn rate_preset_index(rate: f64) -> Option<usize> {
    RATE_PRESETS
        .iter()
        .position(|preset| (preset - rate).abs() <= preset * 1e-9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_presets_round_trip_and_reject_custom_rates() {
        for (index, rate) in RATE_PRESETS.iter().enumerate() {
            assert_eq!(rate_preset_index(*rate), Some(index));
        }
        assert_eq!(rate_preset_index(3.0), None);
        assert_eq!(rate_preset_index(-1.0), None);
    }
}

/// Navigation overlays drawn over the scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlay {
    Markers,
    Trails,
    Guides,
}

impl Overlay {
    pub const ALL: [Self; 3] = [Self::Markers, Self::Trails, Self::Guides];
}
