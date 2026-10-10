//! PROTOTYPE: local mirror of the shared parameter descriptors that the Studio
//! lane lands as `astrum_core::params` (agreed shape, ai/lanes/LANES.md
//! interface log 2026-10-10). When that module is on `main`, delete this file
//! and `pub use astrum_core::params::*` instead; the API is identical.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamKind {
    Float { min: f64, max: f64, log: bool },
    Int { min: i64, max: i64 },
    Choice { options: &'static [&'static str] },
    Bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamValue {
    Float(f64),
    Int(i64),
    Choice(usize),
    Bool(bool),
}

impl ParamValue {
    pub fn as_f64(self) -> f64 {
        match self {
            ParamValue::Float(v) => v,
            ParamValue::Int(v) => v as f64,
            ParamValue::Choice(v) => v as f64,
            ParamValue::Bool(v) => v as u8 as f64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ParamError {
    #[error("unknown parameter `{0}`")]
    UnknownKey(String),
    #[error("parameter `{0}` has a different kind")]
    WrongKind(String),
    #[error("parameter `{0}` out of range")]
    OutOfRange(String),
}

impl ParamKind {
    pub fn check(&self, value: ParamValue) -> Result<ParamValue, ParamError> {
        let bad = || ParamError::OutOfRange(String::new());
        match (*self, value) {
            (ParamKind::Float { min, max, .. }, ParamValue::Float(v)) => {
                if v.is_finite() && v >= min && v <= max {
                    Ok(value)
                } else {
                    Err(bad())
                }
            }
            (ParamKind::Int { min, max }, ParamValue::Int(v)) => {
                if v >= min && v <= max {
                    Ok(value)
                } else {
                    Err(bad())
                }
            }
            (ParamKind::Choice { options }, ParamValue::Choice(v)) => {
                if v < options.len() {
                    Ok(value)
                } else {
                    Err(bad())
                }
            }
            (ParamKind::Bool, ParamValue::Bool(_)) => Ok(value),
            _ => Err(ParamError::WrongKind(String::new())),
        }
    }
}

pub struct ParamDesc<T> {
    pub key: &'static str,
    /// Empty means "derive from key".
    pub label: &'static str,
    pub group: &'static str,
    pub unit: &'static str,
    pub help: &'static str,
    pub kind: ParamKind,
    pub get: fn(&T) -> ParamValue,
    pub set: fn(&mut T, ParamValue),
}

pub trait Params: Sized + 'static {
    fn descriptors() -> &'static [ParamDesc<Self>];

    fn param_index(key: &str) -> Option<usize> {
        Self::descriptors().iter().position(|d| d.key == key)
    }

    fn param(&self, key: &str) -> Option<ParamValue> {
        Self::param_index(key).map(|i| (Self::descriptors()[i].get)(self))
    }

    fn set_param(&mut self, key: &str, value: ParamValue) -> Result<(), ParamError> {
        let i = Self::param_index(key).ok_or_else(|| ParamError::UnknownKey(key.into()))?;
        let d = &Self::descriptors()[i];
        let v = d.kind.check(value).map_err(|e| match e {
            ParamError::WrongKind(_) => ParamError::WrongKind(key.into()),
            _ => ParamError::OutOfRange(key.into()),
        })?;
        (d.set)(self, v);
        Ok(())
    }
}
