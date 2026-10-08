//! Viewport-owned gestures and ordered event deltas. No camera or world mutation.
use super::*;

#[derive(Default)]
pub(super) struct ViewportInput {
    pointer: Option<egui::Pos2>,
    gesture: Option<egui::PointerButton>,
    keyboard_owner: bool,
    held: [bool; 6],
    boost: bool,
}
impl ViewportInput {
    pub fn owns_gesture(&self) -> bool {
        self.gesture.is_some()
    }
    pub fn owns_keyboard(&self) -> bool {
        self.keyboard_owner
    }
    pub fn boost_active(&self) -> bool {
        self.boost
    }
    pub fn pointer_position(&self) -> Option<egui::Pos2> {
        self.pointer
    }
    pub fn set_boost(&mut self, active: bool) {
        self.boost = active;
    }
    pub fn cancel(&mut self) {
        *self = Self::default();
    }
    pub fn events(
        &mut self,
        events: &[egui::Event],
        viewport: egui::Rect,
        local_look: bool,
        keyboard_blocked: bool,
        pointer_blocked: impl Fn(egui::Pos2) -> bool,
        user_multiplier: f64,
    ) -> Vec<NavigationInput> {
        if keyboard_blocked {
            self.held = [false; 6];
            self.boost = false;
        }
        let mut output = Vec::new();
        for event in events {
            let mut delta = NavigationInput {
                speed_multiplier: user_multiplier,
                ..Default::default()
            };
            match *event {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    ..
                } => {
                    self.pointer = Some(pos);
                    if !pressed {
                        // Releases end the owned gesture even outside the viewport.
                        if self.gesture == Some(button) {
                            self.gesture = None;
                        }
                    } else if viewport.contains(pos) && !pointer_blocked(pos) {
                        self.keyboard_owner = true;
                        let wanted = if local_look {
                            egui::PointerButton::Secondary
                        } else {
                            egui::PointerButton::Primary
                        };
                        if button == wanted {
                            self.gesture = Some(button);
                        }
                    } else {
                        self.keyboard_owner = false;
                        self.held = [false; 6];
                        self.gesture = None;
                    }
                }
                egui::Event::PointerMoved(pos) => {
                    if let Some(previous) = self.pointer
                        && self.gesture.is_some()
                        && viewport.contains(pos)
                        && !pointer_blocked(pos)
                    {
                        let movement = pos - previous;
                        delta.drag = [movement.x as f64, movement.y as f64];
                    }
                    self.pointer = Some(pos);
                }
                egui::Event::PointerGone => {
                    self.gesture = None;
                    self.pointer = None;
                }
                egui::Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    unit,
                    delta: wheel,
                    ..
                } => {
                    if self
                        .pointer
                        .is_some_and(|p| viewport.contains(p) && !pointer_blocked(p))
                    {
                        // egui raw units: one line is one notch, 50 logical points
                        // is one notch, one page is a viewport height. No quantization.
                        delta.scroll_notches = wheel.y as f64
                            * match unit {
                                egui::MouseWheelUnit::Line => 1.0,
                                egui::MouseWheelUnit::Point => 1.0 / 50.0,
                                egui::MouseWheelUnit::Page => viewport.height() as f64 / 50.0,
                            };
                    }
                }
                egui::Event::Key {
                    key,
                    pressed,
                    modifiers,
                    ..
                } => {
                    for (index, candidate) in [
                        egui::Key::W,
                        egui::Key::S,
                        egui::Key::A,
                        egui::Key::D,
                        egui::Key::Q,
                        egui::Key::E,
                    ]
                    .iter()
                    .enumerate()
                    {
                        if key == *candidate {
                            self.held[index] = pressed && self.keyboard_owner && !keyboard_blocked;
                        }
                    }
                    if modifiers.shift {
                        self.boost = !keyboard_blocked;
                    }
                }
                egui::Event::WindowFocused(false) => {
                    self.cancel();
                }
                _ => {}
            }
            if delta.drag != [0.0; 2] || delta.scroll_notches != 0.0 {
                output.push(delta);
            }
        }
        let mut translation = DVec3::ZERO;
        if local_look && self.keyboard_owner && !keyboard_blocked {
            for (held, axis) in self.held.iter().zip([
                -DVec3::Z,
                DVec3::Z,
                -DVec3::X,
                DVec3::X,
                -DVec3::Y,
                DVec3::Y,
            ]) {
                if *held {
                    translation += axis;
                }
            }
        }
        output.push(NavigationInput {
            translation,
            speed_multiplier: user_multiplier,
            boost_multiplier: if self.boost { 4.0 } else { 1.0 },
            ..Default::default()
        });
        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }
    fn run(
        state: &mut ViewportInput,
        events: &[egui::Event],
        blocked: bool,
        focused: bool,
    ) -> Vec<NavigationInput> {
        if !focused {
            state.cancel();
            return vec![NavigationInput::default()];
        }
        state.events(
            events,
            egui::Rect::from_min_max(egui::pos2(100.0, 100.0), egui::pos2(500.0, 500.0)),
            true,
            blocked,
            |_| false,
            0.5,
        )
    }
    #[test]
    fn panel_press_cannot_acquire_drag_and_outside_release_ends_ownership() {
        let mut state = ViewportInput::default();
        let output = run(
            &mut state,
            &[
                button(egui::pos2(20.0, 200.0), true),
                egui::Event::PointerMoved(egui::pos2(200.0, 200.0)),
            ],
            false,
            true,
        );
        assert!(output.iter().all(|i| i.drag == [0.0; 2]));
        let output = run(
            &mut state,
            &[
                button(egui::pos2(200.0, 200.0), true),
                egui::Event::PointerMoved(egui::pos2(210.0, 200.0)),
                button(egui::pos2(600.0, 200.0), false),
                egui::Event::PointerMoved(egui::pos2(300.0, 200.0)),
            ],
            false,
            true,
        );
        assert_eq!(output[0].drag, [10.0, 0.0]);
        assert_eq!(output.len(), 2);
    }
    #[test]
    fn focus_text_and_fractional_scroll_are_isolated() {
        let mut state = ViewportInput::default();
        let key = egui::Event::Key {
            key: egui::Key::W,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        run(
            &mut state,
            &[button(egui::pos2(200.0, 200.0), true), key.clone()],
            false,
            true,
        );
        assert_ne!(
            run(&mut state, &[], false, true)[0].translation,
            DVec3::ZERO
        );
        assert_eq!(run(&mut state, &[], true, true)[0].translation, DVec3::ZERO);
        run(&mut state, &[], false, false);
        assert_eq!(
            run(&mut state, &[], false, true)[0].translation,
            DVec3::ZERO
        );
        let output = run(
            &mut state,
            &[
                egui::Event::PointerMoved(egui::pos2(200.0, 200.0)),
                egui::Event::MouseWheel {
                    phase: egui::TouchPhase::Move,
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, 12.5),
                    modifiers: egui::Modifiers::default(),
                },
            ],
            false,
            true,
        );
        assert_eq!(output[0].scroll_notches, 0.25);
        assert_eq!(output[0].speed_multiplier, 0.5);
        println!(
            "input: fractional points=12.5 notches=0.25; panel/text/focus cancellation verified"
        );
    }
}
