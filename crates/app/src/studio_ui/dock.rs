//! Drag-to-dock panels (docs/STUDIO_UI.md amendment 2026-10-10c), built on
//! `egui_dock`. Each workspace owns one dock tree; every panel is a tab that
//! can be dragged to another edge, stacked with other tabs, or torn out into a
//! floating window. The viewport is a tab too, but it cannot be closed or
//! floated (the engine renders into its rect).

use astrum_app::studio::view::{StudioAction, StudioView};
use egui::{Color32, CornerRadius, Id, Margin, Painter, Rect, Stroke, Ui, WidgetText};
use egui_dock::{DockState, NodeIndex, Style, SurfaceIndex, TabStyle, TabViewer};
use serde::{Deserialize, Serialize};

use super::profiler_panel::{self, ProfilerInput, TimelineState};
use super::theme::*;
use super::viewport::{Interaction, ViewportInput};
use super::{InspectorTab, contained, panels};
use crate::profiler_ui::ProfilerData;

/// A dockable Studio panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Tab {
    Viewport,
    /// Scene outliner.
    Scene,
    Body,
    Planet,
    Render,
    Log,
    Profiler,
}

impl Tab {
    /// Every tab, in the order the Panels menu lists them.
    pub const ALL: [Self; 7] = [
        Self::Viewport,
        Self::Scene,
        Self::Body,
        Self::Planet,
        Self::Render,
        Self::Log,
        Self::Profiler,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Self::Viewport => "Viewport",
            Self::Scene => "Scene",
            Self::Body => "Body",
            Self::Planet => "Planet",
            Self::Render => "Render",
            Self::Log => "Log",
            Self::Profiler => "Profiler",
        }
    }

    fn inspector(self) -> Option<InspectorTab> {
        match self {
            Self::Body => Some(InspectorTab::Body),
            Self::Planet => Some(InspectorTab::Planet),
            Self::Render => Some(InspectorTab::Render),
            _ => None,
        }
    }
}

/// Default Editor layout: scene list left, viewport centre, inspector pages
/// right, and the log under the viewport when `log_open`. Widths are the
/// starting sizes in points for a 1600 × 900 window (they become fractions).
pub fn editor_layout(
    outliner_width: f32,
    inspector_width: f32,
    log_height: f32,
    log_open: bool,
    inspector: InspectorTab,
) -> DockState<Tab> {
    let (window_width, window_height) = (1600.0_f32, 830.0_f32);
    let mut state = DockState::new(vec![Tab::Viewport]);
    let tree = state.main_surface_mut();
    let left = (outliner_width / window_width).clamp(0.08, 0.4);
    // egui_dock fractions are the share of the left (or top) child.
    let [centre, _] = tree.split_left(NodeIndex::root(), left, vec![Tab::Scene]);
    let right = (inspector_width / (window_width - outliner_width)).clamp(0.12, 0.5);
    let [centre, inspector_node] = tree.split_right(
        centre,
        1.0 - right,
        vec![Tab::Body, Tab::Planet, Tab::Render],
    );
    let _ = tree.set_active_tab(inspector_node, inspector.index());
    if log_open {
        let below = (log_height / window_height).clamp(0.1, 0.6);
        tree.split_below(centre, 1.0 - below, vec![Tab::Log]);
    }
    state
}

/// Default Performance layout: viewport over a full-width profiler.
/// `profiler_share` is the profiler's share of the height (0 picks 60 %).
pub fn performance_layout(profiler_share: f32) -> DockState<Tab> {
    let share = if profiler_share > 0.0 {
        profiler_share.clamp(0.2, 0.85)
    } else {
        0.6
    };
    let mut state = DockState::new(vec![Tab::Viewport]);
    state
        .main_surface_mut()
        .split_below(NodeIndex::root(), 1.0 - share, vec![Tab::Profiler]);
    state
}

/// Whether `tab` is open anywhere in `state` (docked or floating).
pub fn is_open(state: &DockState<Tab>, tab: Tab) -> bool {
    state.find_tab(&tab).is_some()
}

/// Opens or closes `tab`. A reopened tab docks below the viewport (the log)
/// or stacks with the inspector pages, else joins the viewport's node.
pub fn set_open(state: &mut DockState<Tab>, tab: Tab, open: bool) {
    match (state.find_tab(&tab), open) {
        (Some(path), false) if tab != Tab::Viewport => {
            state.remove_tab(path);
        }
        (None, true) => {
            let sibling = [Tab::Body, Tab::Planet, Tab::Render]
                .into_iter()
                .filter(|other| tab.inspector().is_some() && *other != tab)
                .find_map(|other| state.find_tab(&other));
            if let Some(path) = sibling {
                if let Ok(leaf) = state.leaf_mut(path.node_path()) {
                    leaf.append_tab(tab);
                }
                return;
            }
            let Some(viewport) = state.find_tab(&Tab::Viewport) else {
                state.push_to_first_leaf(tab);
                return;
            };
            let below = if tab == Tab::Profiler { 0.4 } else { 0.78 };
            state[viewport.surface].split_below(viewport.node, below, vec![tab]);
        }
        _ => {}
    }
}

/// Dock colours from the theme tokens: tabs sit on the darkest surface, the
/// active tab joins its panel's surface, and splitters are thin borders.
pub fn style(egui_style: &egui::Style) -> Style {
    let mut style = Style::from_egui(egui_style);
    style.dock_area_padding = None;
    style.main_surface_border_stroke = Stroke::NONE;
    style.separator.width = 1.0;
    style.separator.extra_interact_width = 4.0;
    style.separator.color_idle = SURFACE_0;
    style.separator.color_hovered = BORDER_STRONG;
    style.separator.color_dragged = ACCENT;
    style.tab_bar.bg_fill = SURFACE_0;
    style.tab_bar.height = 28.0;
    style.tab_bar.hline_color = SURFACE_0;
    style.tab_bar.corner_radius = CornerRadius::ZERO;
    style.tab_bar.inner_margin = Margin::ZERO;
    style.tab.spacing = 2.0;
    style.tab.hline_below_active_tab_name = false;
    let radius = CornerRadius {
        nw: RADIUS_SMALL,
        ne: RADIUS_SMALL,
        sw: 0,
        se: 0,
    };
    for (state, fill, text) in [
        (&mut style.tab.active, SURFACE_1, TEXT_PRIMARY),
        (&mut style.tab.focused, SURFACE_1, TEXT_PRIMARY),
        (&mut style.tab.active_with_kb_focus, SURFACE_1, TEXT_PRIMARY),
        (
            &mut style.tab.focused_with_kb_focus,
            SURFACE_1,
            TEXT_PRIMARY,
        ),
        (&mut style.tab.inactive, SURFACE_0, TEXT_SECONDARY),
        (
            &mut style.tab.inactive_with_kb_focus,
            SURFACE_0,
            TEXT_SECONDARY,
        ),
        (&mut style.tab.hovered, SURFACE_2, TEXT_PRIMARY),
    ] {
        state.bg_fill = fill;
        state.text_color = text;
        state.outline_color = Color32::TRANSPARENT;
        state.corner_radius = radius;
    }
    style.tab.tab_body.bg_fill = SURFACE_1;
    style.tab.tab_body.stroke = Stroke::NONE;
    style.tab.tab_body.corner_radius = CornerRadius::ZERO;
    style.tab.tab_body.inner_margin = Margin::same(SPACE_3 as i8);
    // Hidden until the tab is hovered (`Tabs::on_tab_button` draws it then);
    // egui_dock still draws the bright × while the button itself is hovered.
    style.buttons.close_tab_color = Color32::TRANSPARENT;
    style.buttons.close_tab_active_color = TEXT_PRIMARY;
    style.buttons.close_tab_bg_fill = SURFACE_3;
    style.overlay.selection_color = ACCENT.linear_multiply(0.25);
    style.overlay.button_color = TEXT_SECONDARY;
    style.overlay.button_border_stroke = Stroke::new(1.0, BORDER_STRONG);
    style
}

/// egui_dock's close button and × sizes (crate-private there).
const CLOSE_BUTTON: f32 = 24.0;
const CLOSE_X: f32 = 9.0;

/// Where the viewport tab sits this frame; the engine renders after the dock
/// pass and paints into it with the tab's painter.
pub struct ViewportSlot {
    pub rect: Rect,
    pub painter: Painter,
}

/// Draws the tabs. It only reads engine state; interactions are collected
/// and applied after the dock pass.
pub struct Tabs<'a> {
    pub view: &'a StudioView,
    pub panel: &'a StudioView,
    pub profiler: &'a ProfilerData,
    pub timeline: &'a mut TimelineState,
    pub viewport_input: &'a mut ViewportInput,
    pub actions: Vec<StudioAction>,
    pub profiler_inputs: Vec<ProfilerInput>,
    pub interactions: Vec<Interaction>,
    pub viewport: Option<ViewportSlot>,
    pub profiler_shown: bool,
}

impl TabViewer for Tabs<'_> {
    type Tab = Tab;

    fn id(&mut self, tab: &mut Tab) -> Id {
        Id::new(("studio-tab", *tab))
    }

    fn title(&mut self, tab: &mut Tab) -> WidgetText {
        tab.title().into()
    }

    fn ui(&mut self, ui: &mut Ui, tab: &mut Tab) {
        match *tab {
            Tab::Viewport => {
                let rect = ui.max_rect();
                self.interactions = self.viewport_input.interact(ui, rect, self.view);
                self.viewport = Some(ViewportSlot {
                    rect,
                    painter: ui.painter().clone(),
                });
                ui.advance_cursor_after_rect(rect);
            }
            Tab::Scene => contained(ui, |ui| panels::outliner(ui, self.view, &mut self.actions)),
            Tab::Body | Tab::Planet | Tab::Render => {
                if let Some(page) = tab.inspector() {
                    contained(ui, |ui| {
                        panels::inspector(ui, self.view, self.panel, page, &mut self.actions)
                    });
                }
            }
            Tab::Log => contained(ui, |ui| panels::log(ui, self.panel)),
            Tab::Profiler => {
                self.profiler_shown = true;
                contained(ui, |ui| {
                    self.profiler_inputs.extend(profiler_panel::show(
                        ui,
                        self.profiler,
                        self.timeline,
                    ));
                });
            }
        }
    }

    /// Close-on-hover: a small × appears on a closeable tab while the pointer
    /// is over it (user direction 2026-10-10). egui_dock reserves the button
    /// area at the tab's right end and handles the click.
    fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
        if self.is_closeable(tab) {
            hover_close(response);
        }
    }

    fn is_closeable(&self, tab: &Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn allowed_in_windows(&self, tab: &mut Tab) -> bool {
        *tab != Tab::Viewport
    }

    fn scroll_bars(&self, _tab: &Tab) -> [bool; 2] {
        [false, false]
    }

    fn tab_style_override(&self, tab: &Tab, global: &TabStyle) -> Option<TabStyle> {
        let margin = match tab {
            Tab::Viewport => Margin::ZERO,
            Tab::Log | Tab::Profiler => Margin::same(SPACE_2 as i8),
            _ => return None,
        };
        let mut style = global.clone();
        style.tab_body.inner_margin = margin;
        Some(style)
    }
}

/// Draws the hover × of a closeable tab button (see `Tabs::on_tab_button`),
/// except while dragging or while the button itself is hovered (egui_dock
/// then draws its highlighted ×).
fn hover_close(response: &egui::Response) {
    if response.ctx.dragged_id().is_some() {
        return;
    }
    let Some(pointer) = response.ctx.pointer_hover_pos() else {
        return;
    };
    let rect = response.rect;
    let button = Rect::from_center_size(
        egui::pos2(rect.right() - CLOSE_BUTTON / 2.0, rect.center().y),
        egui::Vec2::splat(CLOSE_BUTTON),
    );
    if !rect.contains(pointer) || button.contains(pointer) {
        return;
    }
    let x = Rect::from_center_size(button.center(), egui::Vec2::splat(CLOSE_X));
    let painter = response.ctx.layer_painter(response.layer_id);
    let stroke = Stroke::new(1.0, TEXT_SECONDARY);
    painter.line_segment([x.left_top(), x.right_bottom()], stroke);
    painter.line_segment([x.right_top(), x.left_bottom()], stroke);
}
/// Ensures the viewport exists on the main surface (a hand-edited or
/// corrupt layout file could lose it).
pub fn repair(state: &mut DockState<Tab>) {
    match state.find_tab(&Tab::Viewport) {
        Some(path) if path.surface == SurfaceIndex::main() => {}
        Some(path) => {
            state.remove_tab(path);
            state.push_to_first_leaf(Tab::Viewport);
        }
        None => state.push_to_first_leaf(Tab::Viewport),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_dock::DockArea;

    fn tabs(state: &DockState<Tab>) -> Vec<Tab> {
        let mut tabs: Vec<Tab> = state.iter_all_tabs().map(|(_, tab)| *tab).collect();
        tabs.sort_by_key(|tab| *tab as usize);
        tabs
    }

    #[test]
    fn default_layouts_hold_the_expected_panels() {
        let editor = editor_layout(240.0, 340.0, 170.0, false, InspectorTab::Planet);
        assert_eq!(
            tabs(&editor),
            [
                Tab::Viewport,
                Tab::Scene,
                Tab::Body,
                Tab::Planet,
                Tab::Render
            ]
        );
        let path = editor.find_tab(&Tab::Planet).unwrap();
        assert_eq!(editor.leaf(path.node_path()).unwrap().active.0, path.tab.0);
        let with_log = editor_layout(240.0, 340.0, 170.0, true, InspectorTab::Body);
        assert!(is_open(&with_log, Tab::Log));
        assert_eq!(
            tabs(&performance_layout(0.0)),
            [Tab::Viewport, Tab::Profiler]
        );
    }

    #[test]
    fn panels_close_reopen_and_the_viewport_stays() {
        let mut state = editor_layout(240.0, 340.0, 170.0, false, InspectorTab::Body);
        set_open(&mut state, Tab::Viewport, false);
        assert!(is_open(&state, Tab::Viewport));
        set_open(&mut state, Tab::Planet, false);
        assert!(!is_open(&state, Tab::Planet));
        set_open(&mut state, Tab::Planet, true);
        // Back with the other inspector pages.
        let planet = state.find_tab(&Tab::Planet).unwrap();
        let body = state.find_tab(&Tab::Body).unwrap();
        assert_eq!(planet.node_path(), body.node_path());
        set_open(&mut state, Tab::Log, true);
        set_open(&mut state, Tab::Profiler, true);
        assert!(is_open(&state, Tab::Log) && is_open(&state, Tab::Profiler));
    }

    /// Viewer with the real hover-close hook but no Studio state.
    struct HoverViewer {
        rects: Vec<(Tab, Rect)>,
    }

    impl TabViewer for HoverViewer {
        type Tab = Tab;
        fn id(&mut self, tab: &mut Tab) -> Id {
            Id::new(("test-tab", *tab))
        }
        fn title(&mut self, tab: &mut Tab) -> WidgetText {
            tab.title().into()
        }
        fn ui(&mut self, _ui: &mut Ui, _tab: &mut Tab) {}
        fn on_tab_button(&mut self, tab: &mut Tab, response: &egui::Response) {
            self.rects.push((*tab, response.rect));
            if *tab != Tab::Viewport {
                hover_close(response);
            }
        }
        fn is_closeable(&self, tab: &Tab) -> bool {
            *tab != Tab::Viewport
        }
    }

    /// Runs one headless frame with the pointer at `pointer`; returns the tab
    /// button rects and the number of hover-× strokes painted.
    fn hover_frame(
        ctx: &egui::Context,
        state: &mut DockState<Tab>,
        pointer: Option<egui::Pos2>,
    ) -> (Vec<(Tab, Rect)>, usize) {
        let mut viewer = HoverViewer { rects: Vec::new() };
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            events: pointer.map(egui::Event::PointerMoved).into_iter().collect(),
            ..Default::default()
        };
        let mut output = ctx.run_ui(input, |ui| {
            let style = style(ui.style());
            DockArea::new(state)
                .style(style)
                .show_close_buttons(true)
                .show_inside(ui, &mut viewer);
        });
        output.textures_delta.clear();
        let strokes = output
            .shapes
            .iter()
            .filter(|clipped| {
                matches!(&clipped.shape, egui::Shape::LineSegment { stroke, .. }
                    if stroke.color == TEXT_SECONDARY)
            })
            .count();
        (viewer.rects, strokes)
    }

    #[test]
    fn close_cross_shows_only_on_hovered_closeable_tabs() {
        let ctx = egui::Context::default();
        let mut state = editor_layout(240.0, 340.0, 170.0, false, InspectorTab::Body);
        let (rects, idle) = hover_frame(&ctx, &mut state, None);
        assert_eq!(idle, 0, "no × without hover");
        let rect_of = |tab| rects.iter().find(|(t, _)| *t == tab).unwrap().1;
        let planet = rect_of(Tab::Planet);
        let (_, hovered) = hover_frame(
            &ctx,
            &mut state,
            Some(planet.left_center() + egui::vec2(6.0, 0.0)),
        );
        assert_eq!(hovered, 2, "one × (two strokes) on the hovered tab");
        let viewport = rect_of(Tab::Viewport);
        let (_, on_viewport) = hover_frame(&ctx, &mut state, Some(viewport.center()));
        assert_eq!(on_viewport, 0, "the viewport never offers close");
    }
    #[test]
    fn repair_restores_a_lost_viewport() {
        let mut state = DockState::new(vec![Tab::Scene]);
        repair(&mut state);
        assert!(state.find_main_surface_tab(&Tab::Viewport).is_some());
        let mut floating = DockState::new(vec![Tab::Scene]);
        floating.add_window(vec![Tab::Viewport]);
        repair(&mut floating);
        let path = floating.find_tab(&Tab::Viewport).unwrap();
        assert_eq!(path.surface, SurfaceIndex::main());
    }
}
