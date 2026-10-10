//! Kit primitive format (genesis design §7), low-poly profile.
//!
//! The kit is authored once for all planets (`content/flora/kit.ron`). Each
//! shape is a tiny triangle mesh in a unit organ frame: x from attachment (0)
//! to tip (1), y across, z the face normal. The grower places, scales and
//! tints shapes; it never edits them.

use serde::{Deserialize, Serialize};

use crate::genome::{FruitKind, OrganKind};

pub const KIT_SCHEMA: u32 = 1;

/// The built-in kit (the canonical content file, embedded so the grower stays
/// pure and file-system free; hot reload passes a freshly parsed `Kit`).
pub const BUILTIN_KIT_RON: &str = include_str!("../../../content/flora/kit.ron");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KitShape {
    pub name: String,
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u16; 3]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KitFile {
    pub schema: u32,
    pub shapes: Vec<KitShape>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum KitError {
    #[error("kit parse: {0}")]
    Parse(String),
    #[error("kit schema {0} unsupported")]
    Schema(u32),
    #[error("kit shape `{0}` missing")]
    Missing(String),
    #[error("kit shape `{0}` has an index out of range")]
    Index(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Kit {
    pub shapes: Vec<KitShape>,
}

impl Kit {
    pub fn from_ron(text: &str) -> Result<Self, KitError> {
        let file: KitFile = ron::from_str(text).map_err(|e| KitError::Parse(e.to_string()))?;
        if file.schema != KIT_SCHEMA {
            return Err(KitError::Schema(file.schema));
        }
        for s in &file.shapes {
            if s.triangles
                .iter()
                .flatten()
                .any(|&i| i as usize >= s.positions.len())
            {
                return Err(KitError::Index(s.name.clone()));
            }
        }
        let kit = Kit {
            shapes: file.shapes,
        };
        for o in OrganKind::ALL.iter().filter(|o| **o != OrganKind::Leafless) {
            kit.shape_named(&format!("{o:?}"))?;
        }
        for f in FruitKind::ALL.iter().filter(|f| **f != FruitKind::Barren) {
            kit.shape_named(&format!("{f:?}"))?;
        }
        Ok(kit)
    }

    pub fn builtin() -> Self {
        Self::from_ron(BUILTIN_KIT_RON).expect("built-in kit is valid")
    }

    pub fn shape_named(&self, name: &str) -> Result<&KitShape, KitError> {
        self.shapes
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| KitError::Missing(name.into()))
    }

    pub fn organ(&self, kind: OrganKind) -> Option<&KitShape> {
        (kind != OrganKind::Leafless)
            .then(|| self.shape_named(&format!("{kind:?}")).ok())
            .flatten()
    }

    pub fn fruit(&self, kind: FruitKind) -> Option<&KitShape> {
        (kind != FruitKind::Barren)
            .then(|| self.shape_named(&format!("{kind:?}")).ok())
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_kit_loads() {
        let kit = Kit::builtin();
        assert!(kit.organ(OrganKind::Blade).is_some());
        assert!(kit.fruit(FruitKind::Bloom).is_some());
    }
}
