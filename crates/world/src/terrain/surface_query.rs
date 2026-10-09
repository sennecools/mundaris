//! Asynchronous surface queries (`ASTRUM_TERRAIN_PIPELINE.md` §15.1–15.2,
//! ADR 0023).
//!
//! The GPU generator is the terrain authority, so height queries resolve from
//! read-back collider pages and may be `Pending` for a few frames after they
//! are first asked. Callers must handle `Pending`; the far-field bounds are a
//! conservative stand-in that is always available once the body's base tiles
//! have been measured.
use crate::BodyId;
use glam::DVec3;

/// Result of an asynchronous query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum QueryResult<T> {
    Ready(T),
    /// Scheduled; ask again on a later frame.
    Pending,
}

impl<T> QueryResult<T> {
    pub fn ready(self) -> Option<T> {
        match self {
            Self::Ready(value) => Some(value),
            Self::Pending => None,
        }
    }
}

/// Surface at one direction: radial offset from the body's reference radius
/// and the unit surface normal, both in body-fixed axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceHeight {
    pub height_m: f64,
    pub normal: DVec3,
}

/// Height queries against a body's authoritative surface (§15.2). Material
/// and raycast queries are added when they have callers.
pub trait SurfaceQuery {
    /// Surface under body-fixed `direction`; schedules work when not resident.
    fn height_at(&mut self, body: BodyId, direction: DVec3) -> QueryResult<SurfaceHeight>;

    /// Conservative `[lowest, highest]` radial offsets of the surface around
    /// `direction`, from coarse measured data (the far-field collider, §15.1).
    /// `None` when nothing is known about the body yet.
    fn far_field_bounds_m(&self, body: BodyId, direction: DVec3) -> Option<[f64; 2]>;
}
