//! Landforms: data-driven weight rules and recipe graphs
//! (`docs/ASTRUM_TERRAIN_PIPELINE.md` §8.1–§8.3, §9.6, App. C.3; M2 Shape
//! design §2–§4).
//!
//! - [`expr`]: weight rules → stack bytecode, interpreted per Tier A texel.
//! - [`schema`]: RON file types of landform sets and recipes.
//! - [`ir`]: validated typed recipe DAG ([`Program`]) and its identities.
//! - [`eval`]: f64 CPU oracle with dual numbers ([`FieldSource`] supplies the
//!   Tier A fields).
//! - [`bounds`]: interval bounds and the unresolved bound of coarse texels.
//! - [`set`]: a compiled set ([`LandformSet`]).
pub mod bounds;
pub mod eval;
pub mod expr;
pub mod gpu;
pub mod ir;
pub mod schema;
pub mod set;
pub mod world_source;

pub use bounds::Interval;
pub use eval::{Dual, FieldSource, LandformParams, node_seed};
pub use expr::Rule;
pub use ir::{CompileOptions, Op, Program, StackOp};
pub use schema::{LandformSetFile, RecipeField, RecipeFile};
pub use set::{Landform, LandformSet};

/// Authoring and validation errors of landform data.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LandformError {
    #[error("weight rule: {0}")]
    Rule(String),
    #[error("bytecode: {0}")]
    Bytecode(String),
    #[error("recipe: {0}")]
    Recipe(String),
    #[error("recipe node '{node}': {message}")]
    Node { node: String, message: String },
    #[error("landform set: {0}")]
    Set(String),
    #[error("cannot load '{path}': {message}")]
    Load { path: String, message: String },
    #[error("landform '{landform}': {source}")]
    InLandform {
        landform: String,
        source: Box<LandformError>,
    },
}

#[cfg(test)]
mod tests;
