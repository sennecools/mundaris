//! Shared parameter descriptors (docs/STUDIO_UI.md, amendment 2026-10-10b).
//!
//! A content type (planet parameters, a flora genome, later materials) lists
//! its editable values once as a static table of [`ParamDesc`]: a stable key,
//! a display label, an editor group, a unit, help text, the value kind with
//! its validation bounds, and typed accessors. Editors (Studio), file loaders
//! and the developer protocol all work from that table, so nothing else
//! enumerates the parameters and no content crate depends on a UI toolkit.
use std::borrow::Cow;
use std::fmt;

/// Kind and validation bounds of one parameter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    /// A real value in `min..=max`; `log` asks editors for a logarithmic slider.
    Float {
        min: f64,
        max: f64,
        log: bool,
    },
    /// An integer in `min..=max`.
    Int {
        min: i64,
        max: i64,
    },
    /// One of a fixed list of named options (the value is the option index).
    Choice {
        options: &'static [&'static str],
    },
    Bool,
}

/// A parameter value; the variant matches the descriptor's [`ParamKind`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamValue {
    Float(f64),
    Int(i64),
    Choice(usize),
    Bool(bool),
}

impl ParamValue {
    /// The value as a number (choices as their index, booleans as 0/1).
    pub fn as_f64(self) -> f64 {
        match self {
            Self::Float(x) => x,
            Self::Int(i) => i as f64,
            Self::Choice(i) => i as f64,
            Self::Bool(b) => f64::from(u8::from(b)),
        }
    }
}

/// Why a parameter edit was rejected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamError {
    UnknownKey,
    /// The value's variant does not match the parameter's kind.
    WrongKind,
    /// Outside the bounds, not finite, or not one of the options.
    OutOfRange,
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnknownKey => "unknown parameter",
            Self::WrongKind => "wrong value kind for parameter",
            Self::OutOfRange => "parameter value out of range",
        })
    }
}

impl std::error::Error for ParamError {}

impl ParamKind {
    /// The value if it has this kind and lies within its bounds.
    pub fn check(&self, value: ParamValue) -> Result<ParamValue, ParamError> {
        let ok = match (*self, value) {
            (Self::Float { min, max, .. }, ParamValue::Float(x)) => {
                x.is_finite() && (min..=max).contains(&x)
            }
            (Self::Int { min, max }, ParamValue::Int(i)) => (min..=max).contains(&i),
            (Self::Choice { options }, ParamValue::Choice(i)) => i < options.len(),
            (Self::Bool, ParamValue::Bool(_)) => true,
            _ => return Err(ParamError::WrongKind),
        };
        if ok {
            Ok(value)
        } else {
            Err(ParamError::OutOfRange)
        }
    }
}

/// One editable parameter of a `T`.
pub struct ParamDesc<T> {
    /// Stable key used in content files, edits and the developer protocol.
    pub key: &'static str,
    /// Display label; empty means "derive from the key" (see [`Self::display_label`]).
    pub label: &'static str,
    /// Editor section; editors show groups in table order.
    pub group: &'static str,
    /// Display unit (`m`, `°C`, `deg`, …), empty when unitless.
    pub unit: &'static str,
    /// One-line help shown on hover.
    pub help: &'static str,
    pub kind: ParamKind,
    pub get: fn(&T) -> ParamValue,
    pub set: fn(&mut T, ParamValue),
}

impl<T> ParamDesc<T> {
    /// The label, or one derived from the key: underscores become spaces, the
    /// first letter is capitalised, and a trailing unit word (`_m`, `_km`,
    /// `_c`, `_deg`) is dropped: `land_height_m` → "Land height".
    pub fn display_label(&self) -> Cow<'static, str> {
        if !self.label.is_empty() {
            return Cow::Borrowed(self.label);
        }
        Cow::Owned(label_from_key(self.key))
    }
}

/// Readable label of a snake_case key (see [`ParamDesc::display_label`]).
pub fn label_from_key(key: &str) -> String {
    let base = match key.rsplit_once('_') {
        Some((base, "m" | "km" | "c" | "deg")) => base,
        _ => key,
    };
    let mut label = base.replace('_', " ");
    if let Some(first) = label.get_mut(..1) {
        first.make_ascii_uppercase();
    }
    label
}

/// A type whose editable values are described by a static descriptor table.
pub trait Params: Sized + 'static {
    fn descriptors() -> &'static [ParamDesc<Self>];

    /// Index of `key` in [`Self::descriptors`].
    fn param_index(key: &str) -> Option<usize> {
        Self::descriptors().iter().position(|d| d.key == key)
    }

    fn param(&self, key: &str) -> Option<ParamValue> {
        let desc = Self::descriptors().iter().find(|d| d.key == key)?;
        Some((desc.get)(self))
    }

    /// Set one parameter; rejects unknown keys and values that fail
    /// [`ParamKind::check`], leaving `self` unchanged.
    fn set_param(&mut self, key: &str, value: ParamValue) -> Result<(), ParamError> {
        let desc = Self::descriptors()
            .iter()
            .find(|d| d.key == key)
            .ok_or(ParamError::UnknownKey)?;
        let value = desc.kind.check(value)?;
        (desc.set)(self, value);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Toy {
        size_m: f64,
        count: i64,
        form: usize,
        on: bool,
    }

    const FORMS: &[&str] = &["tree", "shrub"];

    impl Params for Toy {
        fn descriptors() -> &'static [ParamDesc<Self>] {
            &[
                ParamDesc {
                    key: "size_m",
                    label: "",
                    group: "Shape",
                    unit: "m",
                    help: "",
                    kind: ParamKind::Float {
                        min: 0.0,
                        max: 10.0,
                        log: false,
                    },
                    get: |t| ParamValue::Float(t.size_m),
                    set: |t, v| t.size_m = v.as_f64(),
                },
                ParamDesc {
                    key: "count",
                    label: "Branches",
                    group: "Shape",
                    unit: "",
                    help: "",
                    kind: ParamKind::Int { min: 1, max: 3 },
                    get: |t| ParamValue::Int(t.count),
                    set: |t, v| {
                        if let ParamValue::Int(i) = v {
                            t.count = i;
                        }
                    },
                },
                ParamDesc {
                    key: "form",
                    label: "",
                    group: "Kind",
                    unit: "",
                    help: "",
                    kind: ParamKind::Choice { options: FORMS },
                    get: |t| ParamValue::Choice(t.form),
                    set: |t, v| {
                        if let ParamValue::Choice(i) = v {
                            t.form = i;
                        }
                    },
                },
                ParamDesc {
                    key: "on",
                    label: "",
                    group: "Kind",
                    unit: "",
                    help: "",
                    kind: ParamKind::Bool,
                    get: |t| ParamValue::Bool(t.on),
                    set: |t, v| {
                        if let ParamValue::Bool(b) = v {
                            t.on = b;
                        }
                    },
                },
            ]
        }
    }

    #[test]
    fn set_validates_kind_and_bounds() {
        let mut toy = Toy::default();
        toy.set_param("size_m", ParamValue::Float(2.5)).unwrap();
        toy.set_param("count", ParamValue::Int(3)).unwrap();
        toy.set_param("form", ParamValue::Choice(1)).unwrap();
        toy.set_param("on", ParamValue::Bool(true)).unwrap();
        assert_eq!(toy.param("size_m"), Some(ParamValue::Float(2.5)));
        assert_eq!((toy.count, toy.form, toy.on), (3, 1, true));
        assert_eq!(
            toy.set_param("size_m", ParamValue::Float(f64::NAN)),
            Err(ParamError::OutOfRange)
        );
        assert_eq!(
            toy.set_param("count", ParamValue::Int(4)),
            Err(ParamError::OutOfRange)
        );
        assert_eq!(
            toy.set_param("form", ParamValue::Choice(2)),
            Err(ParamError::OutOfRange)
        );
        assert_eq!(
            toy.set_param("count", ParamValue::Float(2.0)),
            Err(ParamError::WrongKind)
        );
        assert_eq!(
            toy.set_param("nope", ParamValue::Bool(true)),
            Err(ParamError::UnknownKey)
        );
        assert_eq!(toy.count, 3);
        assert_eq!(Toy::param_index("form"), Some(2));
    }

    #[test]
    fn labels_derive_from_keys() {
        let d = &Toy::descriptors();
        assert_eq!(d[0].display_label(), "Size");
        assert_eq!(d[1].display_label(), "Branches");
        assert_eq!(label_from_key("land_height_m"), "Land height");
        assert_eq!(label_from_key("ocean_coverage"), "Ocean coverage");
        assert_eq!(label_from_key("equator_c"), "Equator");
    }
}
