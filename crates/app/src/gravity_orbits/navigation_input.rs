//! Viewport-owned gestures and ordered event deltas. No camera or world mutation.
//! Events arrive from the UI toolkit only while the pointer is over the viewport,
//! in viewport logical pixels.
use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerButton {
    Primary,
    Secondary,
}

/// Flight keys held for continuous local movement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlightKey {
    Forward,
    Back,
    Left,
    Right,
    Down,
    Up,
}

impl FlightKey {
    const ALL: [Self; 6] = [
        Self::Forward,
        Self::Back,
        Self::Left,
        Self::Right,
        Self::Down,
        Self::Up,
    ];
    /// Maps W/S/A/D/Q/E text (any case) to a flight key.
    pub fn from_text(text: &str) -> Option<Self> {
        match text {
            "w" | "W" => Some(Self::Forward),
            "s" | "S" => Some(Self::Back),
            "a" | "A" => Some(Self::Left),
            "d" | "D" => Some(Self::Right),
            "q" | "Q" => Some(Self::Down),
            "e" | "E" => Some(Self::Up),
            _ => None,
        }
    }
    fn axis(self) -> DVec3 {
        match self {
            Self::Forward => -DVec3::Z,
            Self::Back => DVec3::Z,
            Self::Left => -DVec3::X,
            Self::Right => DVec3::X,
            Self::Down => -DVec3::Y,
            Self::Up => DVec3::Y,
        }
    }
}

/// One toolkit-neutral viewport input event.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ViewportEvent {
    PointerMoved([f32; 2]),
    PointerButton {
        pos: [f32; 2],
        button: PointerButton,
        pressed: bool,
    },
    PointerLeft,
    /// Wheel movement in notches; positive moves forward / zooms in.
    Wheel(f64),
    Key {
        key: FlightKey,
        pressed: bool,
    },
    Boost(bool),
    FocusChanged(bool),
}

#[derive(Default)]
pub(super) struct ViewportInput {
    pointer: Option<[f32; 2]>,
    gesture: Option<PointerButton>,
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
    pub fn cancel(&mut self) {
        *self = Self::default();
    }
    pub fn events(
        &mut self,
        events: &[ViewportEvent],
        local_look: bool,
        keyboard_blocked: bool,
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
                ViewportEvent::PointerButton {
                    pos,
                    button,
                    pressed,
                } => {
                    self.pointer = Some(pos);
                    if !pressed {
                        if self.gesture == Some(button) {
                            self.gesture = None;
                        }
                    } else {
                        self.keyboard_owner = true;
                        let wanted = if local_look {
                            PointerButton::Secondary
                        } else {
                            PointerButton::Primary
                        };
                        if button == wanted {
                            self.gesture = Some(button);
                        }
                    }
                }
                ViewportEvent::PointerMoved(pos) => {
                    if let Some(previous) = self.pointer
                        && self.gesture.is_some()
                    {
                        delta.drag = [
                            f64::from(pos[0] - previous[0]),
                            f64::from(pos[1] - previous[1]),
                        ];
                    }
                    self.pointer = Some(pos);
                }
                ViewportEvent::PointerLeft => {
                    self.gesture = None;
                    self.pointer = None;
                }
                ViewportEvent::Wheel(notches) => {
                    if self.pointer.is_some() {
                        delta.scroll_notches = notches;
                    }
                }
                ViewportEvent::Key { key, pressed } => {
                    let index = FlightKey::ALL
                        .iter()
                        .position(|candidate| *candidate == key)
                        .expect("flight key listed");
                    self.held[index] = pressed && self.keyboard_owner && !keyboard_blocked;
                }
                ViewportEvent::Boost(active) => {
                    self.boost = active && !keyboard_blocked;
                }
                ViewportEvent::FocusChanged(false) => self.cancel(),
                ViewportEvent::FocusChanged(true) => {}
            }
            if delta.drag != [0.0; 2] || delta.scroll_notches != 0.0 {
                output.push(delta);
            }
        }
        let mut translation = DVec3::ZERO;
        if local_look && self.keyboard_owner && !keyboard_blocked {
            for (held, key) in self.held.iter().zip(FlightKey::ALL) {
                if *held {
                    translation += key.axis();
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
    fn button(pos: [f32; 2], pressed: bool) -> ViewportEvent {
        ViewportEvent::PointerButton {
            pos,
            button: PointerButton::Secondary,
            pressed,
        }
    }
    fn run(
        state: &mut ViewportInput,
        events: &[ViewportEvent],
        blocked: bool,
        focused: bool,
    ) -> Vec<NavigationInput> {
        if !focused {
            state.cancel();
            return vec![NavigationInput::default()];
        }
        state.events(events, true, blocked, 0.5)
    }
    #[test]
    fn drag_follows_the_owned_button_and_release_ends_ownership() {
        let mut state = ViewportInput::default();
        let output = run(
            &mut state,
            &[
                button([200.0, 200.0], true),
                ViewportEvent::PointerMoved([210.0, 200.0]),
                button([210.0, 200.0], false),
                ViewportEvent::PointerMoved([300.0, 200.0]),
            ],
            false,
            true,
        );
        assert_eq!(output[0].drag, [10.0, 0.0]);
        assert_eq!(output.len(), 2);
    }
    #[test]
    fn focus_blocking_and_fractional_scroll_are_isolated() {
        let mut state = ViewportInput::default();
        let key = ViewportEvent::Key {
            key: FlightKey::Forward,
            pressed: true,
        };
        run(
            &mut state,
            &[button([200.0, 200.0], true), key],
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
                ViewportEvent::PointerMoved([200.0, 200.0]),
                ViewportEvent::Wheel(0.25),
            ],
            false,
            true,
        );
        assert_eq!(output[0].scroll_notches, 0.25);
        assert_eq!(output[0].speed_multiplier, 0.5);
    }
}
