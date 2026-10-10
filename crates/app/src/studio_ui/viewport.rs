//! Scene viewport: the engine's scene texture, the body-label overlay, the HUD,
//! and translation of egui input into toolkit-free [`ViewportEvent`]s in
//! viewport logical pixels.

use astrum_app::{
    studio::view::{CAMERA_MODES, LabelState, Shortcut, StatItem, StudioAction, StudioView},
    viewport::{FlightKey, PointerButton, ViewportEvent},
};
use astrum_renderer::TerrainViewMode;
use egui::{
    Align2, Color32, CornerRadius, CursorIcon, Event, EventFilter, FontId, Key, MouseWheelUnit,
    Pos2, Rect, RichText, Sense, Stroke, StrokeKind, TextureId, Ui,
};

use super::theme::{self, *};

/// Logical points per wheel notch for pixel-precise scroll devices.
const POINTS_PER_NOTCH: f32 = 50.0;

/// Viewport input state kept across frames.
#[derive(Default)]
pub struct ViewportInput {
    focused: bool,
    /// Buttons whose press started inside the viewport.
    gesture: [bool; 2],
    /// Label index under a primary press; clicks there select, never navigate.
    label_press: Option<usize>,
    hovered: bool,
}

/// One interaction the frame owner applies to the demo.
pub enum Interaction {
    Event(ViewportEvent),
    Shortcut(Shortcut),
    Click([f32; 2], bool),
    LabelClick(usize, bool),
    /// Alt+click on the globe: fly the camera to that surface point.
    FlyTo([f32; 2]),
    /// Human input that interrupts an automation lease.
    Human(&'static str),
}

fn button(button: egui::PointerButton) -> Option<(usize, PointerButton)> {
    match button {
        egui::PointerButton::Primary => Some((0, PointerButton::Primary)),
        egui::PointerButton::Secondary => Some((1, PointerButton::Secondary)),
        _ => None,
    }
}

fn label_at(view: &StudioView, local: Pos2) -> Option<usize> {
    view.labels.iter().position(|label| {
        let [x, y, w, h] = label.rect;
        Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h)).contains(local)
    })
}

fn shortcut(key: Key, shift: bool) -> Option<Shortcut> {
    Some(match key {
        Key::F => Shortcut::Focus,
        Key::I => Shortcut::SurfaceNavigation,
        Key::H => Shortcut::Horizon,
        Key::Home => Shortcut::Overview,
        Key::Escape => Shortcut::FreeFlight,
        Key::Tab if shift => Shortcut::PreviousBody,
        Key::Tab => Shortcut::NextBody,
        _ => return None,
    })
}

impl ViewportInput {
    /// Reads this frame's input for the viewport at `rect` and returns the
    /// interactions in order. Pointer input counts only where the viewport is
    /// the topmost layer; keyboard input only while the viewport has focus.
    pub fn interact(&mut self, ui: &mut Ui, rect: Rect, view: &StudioView) -> Vec<Interaction> {
        let id = ui.id().with("scene-viewport");
        let response = ui.interact(rect, id, Sense::click_and_drag());
        let layer = ui.layer_id();
        let ctx = ui.ctx().clone();
        let over_viewport =
            |pos: Pos2| rect.contains(pos) && ctx.layer_id_at(pos).is_none_or(|l| l == layer);
        let local = |pos: Pos2| {
            let p = pos - rect.min;
            [p.x, p.y]
        };
        let mut out = Vec::new();
        let events = ui.input(|input| input.events.clone());
        let double = ui.input(|input| {
            input
                .pointer
                .button_double_clicked(egui::PointerButton::Primary)
        });
        let mut label_clicked = false;
        for event in events {
            match event {
                Event::PointerMoved(pos) => {
                    let inside = over_viewport(pos);
                    if inside || self.gesture.iter().any(|g| *g) {
                        out.push(Interaction::Event(ViewportEvent::PointerMoved(local(pos))));
                    } else if self.hovered {
                        out.push(Interaction::Event(ViewportEvent::PointerLeft));
                    }
                    self.hovered = inside;
                }
                Event::PointerGone => {
                    if self.hovered {
                        out.push(Interaction::Event(ViewportEvent::PointerLeft));
                    }
                    self.hovered = false;
                }
                Event::PointerButton {
                    pos,
                    button: pressed_button,
                    pressed,
                    ..
                } => {
                    let Some((slot, button)) = button(pressed_button) else {
                        continue;
                    };
                    let p = local(pos);
                    if pressed {
                        if !over_viewport(pos) {
                            continue;
                        }
                        ui.memory_mut(|memory| memory.request_focus(id));
                        out.push(Interaction::Human("human_input_mouse_button"));
                        if slot == 0
                            && let Some(index) = label_at(view, egui::pos2(p[0], p[1]))
                        {
                            self.label_press = Some(index);
                            continue;
                        }
                        self.gesture[slot] = true;
                        out.push(Interaction::Event(ViewportEvent::PointerButton {
                            pos: p,
                            button,
                            pressed: true,
                        }));
                    } else if slot == 0
                        && let Some(index) = self.label_press.take()
                    {
                        if label_at(view, egui::pos2(p[0], p[1])) == Some(index) {
                            out.push(Interaction::LabelClick(index, false));
                            if double {
                                out.push(Interaction::LabelClick(index, true));
                            }
                        }
                        label_clicked = true;
                    } else if std::mem::take(&mut self.gesture[slot]) {
                        out.push(Interaction::Event(ViewportEvent::PointerButton {
                            pos: p,
                            button,
                            pressed: false,
                        }));
                    }
                }
                Event::MouseWheel { unit, delta, .. } => {
                    let pointer = ctx.pointer_latest_pos();
                    if pointer.is_some_and(over_viewport) && delta.y != 0.0 {
                        let notches = match unit {
                            MouseWheelUnit::Line => delta.y,
                            MouseWheelUnit::Point => delta.y / POINTS_PER_NOTCH,
                            MouseWheelUnit::Page => delta.y * 3.0,
                        };
                        out.push(Interaction::Human("human_input_mouse_wheel"));
                        out.push(Interaction::Event(ViewportEvent::Wheel(f64::from(notches))));
                    }
                }
                Event::Key {
                    key,
                    pressed,
                    repeat,
                    modifiers,
                    ..
                } if self.focused => {
                    if pressed {
                        out.push(Interaction::Human("human_input_keyboard_pressed"));
                    }
                    out.push(Interaction::Event(ViewportEvent::Boost(modifiers.shift)));
                    if let Some(key) = FlightKey::from_text(key.name()) {
                        out.push(Interaction::Event(ViewportEvent::Key { key, pressed }));
                    } else if pressed
                        && !repeat
                        && let Some(shortcut) = shortcut(key, modifiers.shift)
                    {
                        out.push(Interaction::Shortcut(shortcut));
                    }
                }
                Event::ModifiersChanged(modifiers) if self.focused => {
                    out.push(Interaction::Event(ViewportEvent::Boost(modifiers.shift)));
                }
                Event::WindowFocused(false) if self.focused => {
                    ui.memory_mut(|memory| memory.surrender_focus(id));
                }
                _ => {}
            }
        }
        if !label_clicked {
            let alt = ui.input(|input| input.modifiers.alt);
            if let Some(pos) = response
                .interact_pointer_pos()
                .filter(|_| response.clicked())
            {
                out.push(if alt {
                    Interaction::FlyTo(local(pos))
                } else {
                    Interaction::Click(local(pos), false)
                });
            }
            if !alt
                && let Some(pos) = response
                    .interact_pointer_pos()
                    .filter(|_| response.double_clicked())
            {
                out.push(Interaction::Click(local(pos), true));
            }
        }
        let focused = ui.memory(|memory| memory.has_focus(id));
        if focused {
            // Keep Tab, Escape and arrows for viewport shortcuts and flight.
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }
        if focused != self.focused {
            self.focused = focused;
            out.push(Interaction::Event(ViewportEvent::FocusChanged(focused)));
        }
        if self.gesture.iter().any(|g| *g) {
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        out
    }
}

fn label_color(state: LabelState) -> Color32 {
    match state {
        LabelState::Normal => TEXT_PRIMARY,
        LabelState::Selected => WARN,
        LabelState::Focused => OK,
    }
}

/// Paints the scene texture, labels and HUD into `rect`.
pub fn paint(
    painter: &egui::Painter,
    rect: Rect,
    scene: Option<TextureId>,
    view: &StudioView,
    hud: &[StatItem],
) {
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    painter.rect_filled(rect, CornerRadius::ZERO, SURFACE_0);
    if let Some(texture) = scene {
        painter.image(
            texture,
            rect,
            Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
    for label in &view.labels {
        let color = label_color(label.state);
        let marker = rect.min + egui::vec2(label.marker[0], label.marker[1]);
        if label.show_marker {
            let marker_color = match label.state {
                LabelState::Normal => TEXT_SECONDARY,
                state => label_color(state),
            };
            painter.circle_stroke(marker, 4.0, Stroke::new(1.3, marker_color));
        }
        if label.ring > 0.0 {
            let ring_color = if label.state == LabelState::Selected {
                WARN
            } else {
                OK
            };
            painter.circle_stroke(marker, label.ring, Stroke::new(1.5, ring_color));
        }
        let [x, y, w, h] = label.rect;
        let box_rect = Rect::from_min_size(rect.min + egui::vec2(x, y), egui::vec2(w, h));
        painter.rect_filled(
            box_rect,
            CornerRadius::same(3),
            Color32::from_rgba_premultiplied(0, 0, 0, 0xc0),
        );
        painter.text(
            box_rect.center(),
            Align2::CENTER_CENTER,
            &label.text,
            FontId::proportional(12.0),
            color,
        );
    }
    if !hud.is_empty() {
        let row = 18.0;
        let size = egui::vec2(170.0, row * hud.len() as f32 + 2.0 * SPACE_2);
        // Top right; the camera and view controls sit top left.
        let hud_rect = Rect::from_min_size(
            egui::pos2(rect.right() - SPACE_3 - size.x, rect.top() + SPACE_3),
            size,
        );
        painter.rect(
            hud_rect,
            CornerRadius::same(RADIUS),
            Color32::from_rgba_premultiplied(0x08, 0x0a, 0x0d, 0xb0),
            Stroke::NONE,
            StrokeKind::Inside,
        );
        for (index, stat) in hud.iter().enumerate() {
            let y = hud_rect.top() + SPACE_2 + row * (index as f32 + 0.5);
            painter.text(
                egui::pos2(hud_rect.left() + SPACE_2, y),
                Align2::LEFT_CENTER,
                &stat.label,
                FontId::proportional(FONT_UI),
                TEXT_SECONDARY,
            );
            painter.text(
                egui::pos2(hud_rect.right() - SPACE_2, y),
                Align2::RIGHT_CENTER,
                &stat.value,
                mono(),
                theme::tone_color(stat.tone),
            );
        }
    }
}

/// Movement hint per camera mode (`CAMERA_MODES` order).
const CAMERA_HINTS: [&str; 4] = [
    "Drag to orbit · wheel to zoom · double-click a body to focus",
    "Drag to orbit · wheel to zoom · F focus · Alt+click flies to a point",
    "Drag to look · WASD to move · Shift boosts · wheel sets speed · Esc free flight",
    "Drag to look · WASD to move · Shift boosts · wheel sets speed",
];

/// Controls drawn over the viewport in their own layer (so clicks on them
/// never reach the camera): camera mode and view mode top left, the movement
/// hint bottom left and a rebuild notice top centre.
pub fn overlay_controls(
    ctx: &egui::Context,
    rect: Rect,
    view: &StudioView,
    actions: &mut Vec<StudioAction>,
) {
    let chrome = egui::Frame::new()
        .fill(Color32::from_rgba_premultiplied(0x08, 0x0a, 0x0d, 0xd0))
        .corner_radius(CornerRadius::same(RADIUS))
        .inner_margin(egui::Margin::same(SPACE_1 as i8));
    egui::Area::new(egui::Id::new("viewport-controls"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min + egui::vec2(SPACE_3, SPACE_3))
        .show(ctx, |ui| {
            chrome.show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (index, mode) in CAMERA_MODES.iter().enumerate() {
                        if super::panels::tool_button(ui, mode, view.camera_index == index)
                            .clicked()
                        {
                            actions.push(StudioAction::SetCamera(index));
                        }
                    }
                    ui.add_space(SPACE_2);
                    let current = TerrainViewMode::ALL
                        .get(view.view_index)
                        .copied()
                        .map_or("—", super::panels::view_mode_name);
                    egui::ComboBox::from_id_salt("viewport-view-mode")
                        .width(140.0)
                        .selected_text(format!("View: {current}"))
                        .show_ui(ui, |ui| {
                            for (index, mode) in TerrainViewMode::ALL.iter().enumerate() {
                                if ui
                                    .selectable_label(
                                        index == view.view_index,
                                        super::panels::view_mode_name(*mode),
                                    )
                                    .clicked()
                                {
                                    actions.push(StudioAction::SetView(index));
                                }
                            }
                        });
                });
            });
        });
    if let Some(hint) = CAMERA_HINTS.get(view.camera_index) {
        // On a dark backdrop, so it stays readable over bright terrain.
        let painter = ctx
            .layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                egui::Id::new("viewport-hint"),
            ))
            .with_clip_rect(rect);
        let galley = painter.layout_no_wrap(
            (*hint).to_string(),
            FontId::proportional(FONT_SMALL),
            TEXT_SECONDARY,
        );
        let text_rect = Rect::from_min_size(
            egui::pos2(
                rect.left() + SPACE_3 + SPACE_2,
                rect.bottom() - SPACE_3 - SPACE_1 - galley.size().y,
            ),
            galley.size(),
        );
        painter.rect_filled(
            text_rect.expand2(egui::vec2(SPACE_2, SPACE_1)),
            CornerRadius::same(RADIUS),
            Color32::from_rgba_premultiplied(0x08, 0x0a, 0x0d, 0xd0),
        );
        painter.galley(text_rect.min, galley, TEXT_SECONDARY);
    }
    if view.planet.as_ref().is_some_and(|planet| planet.baking) {
        egui::Area::new(egui::Id::new("viewport-rebuild"))
            .order(egui::Order::Foreground)
            .pivot(Align2::CENTER_TOP)
            .fixed_pos(egui::pos2(rect.center().x, rect.top() + SPACE_3))
            .interactable(false)
            .show(ctx, |ui| {
                chrome.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.add(egui::Spinner::new().size(12.0).color(WARN));
                        ui.label(RichText::new("Rebuilding the world map…").color(WARN));
                    });
                });
            });
    }
}
