//! Scene viewport: the engine's scene texture, the body-label overlay, the HUD,
//! and translation of egui input into toolkit-free [`ViewportEvent`]s in
//! viewport logical pixels.

use astrum_app::{
    studio::view::{LabelState, Shortcut, StatItem, StudioView},
    viewport::{FlightKey, PointerButton, ViewportEvent},
};
use egui::{
    Align2, Color32, CornerRadius, CursorIcon, Event, EventFilter, FontId, Key, MouseWheelUnit,
    Pos2, Rect, Sense, Stroke, StrokeKind, TextureId, Ui,
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
pub fn paint(ui: &Ui, rect: Rect, scene: Option<TextureId>, view: &StudioView, hud: &[StatItem]) {
    let painter = ui.painter_at(rect);
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
        let hud_rect = Rect::from_min_size(
            rect.min + egui::vec2(SPACE_3, SPACE_3),
            egui::vec2(190.0, row * hud.len() as f32 + 2.0 * SPACE_2),
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
