//! Shared, deterministic terrain authoring graphs and published bundles.
#![forbid(unsafe_code)]

use std::fmt;

pub mod graph;
pub mod graph_bundle;

pub const ALGORITHM_VERSION: &str = "smooth-gradient-noise-1";

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Io(std::io::Error),
    Json(serde_json::Error),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(s) => write!(f, "invalid terrain graph or bundle: {s}"),
            Self::Io(e) => e.fmt(f),
            Self::Json(e) => e.fmt(f),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
impl From<serde_json::Error> for Error {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

/// Canonical cube mapping shared by graph sampling and bundle validation.
pub fn cube_direction(face: usize, u: f64, v: f64) -> Result<[f64; 3], Error> {
    if face >= 6
        || !u.is_finite()
        || !v.is_finite()
        || !(-1.0..=1.0).contains(&u)
        || !(-1.0..=1.0).contains(&v)
    {
        return Err(Error::Invalid(
            "cube face or coordinates out of range".into(),
        ));
    }
    let p = match face {
        0 => [1.0, v, -u],
        1 => [-1.0, v, u],
        2 => [u, 1.0, -v],
        3 => [u, -1.0, v],
        4 => [u, v, 1.0],
        _ => [-u, v, -1.0],
    };
    let length = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
    Ok([p[0] / length, p[1] / length, p[2] / length])
}
