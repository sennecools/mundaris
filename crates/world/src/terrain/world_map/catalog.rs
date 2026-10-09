//! Biome catalog (W2, biome catalog and heightmap variants): binds each world-map
//! biome channel to sub-biomes and their library detail variants.
//!
//! - A **biome** binds exactly one world-map biome channel (for the Moon:
//!   highland, mare, crater floor, crater rim/ejecta). Biome weights come from
//!   the world map, so blended biomes are overlapping weights.
//! - A **sub-biome** is selected inside its biome by soft ranges over world-map
//!   fields (macro elevation, macro slope); sub-biome weights are normalised
//!   within the biome.
//! - A **variant** is a library bundle, identified by id and the sha256 of its
//!   height channel, with a selection weight. Each lattice cell picks one
//!   variant per sub-biome by an integer hash (`compose.rs`).

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const CATALOG_FORMAT: &str = "mundaris.biome-catalog.v1";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BiomeCatalog {
    pub format: String,
    pub id: String,
    /// Seed of the placement hashes (variant, orientation, offset per cell).
    pub seed: u64,
    /// The world-map biome channels this catalog binds, in world-map order.
    pub channels: Vec<String>,
    /// Placement lattice cell size (real metres; library tiles are real-scale).
    pub cell_m: f64,
    /// Cross-fade width across cell and cube-face borders (m).
    pub cell_blend_m: f64,
    /// Amplitude of a smooth warp of the lattice coordinates (m), so cell
    /// borders meander instead of running as straight lines.
    pub cell_warp_m: f64,
    /// Detail above this wavelength is removed from every variant (m).
    pub band_limit_m: f64,
    pub biomes: Vec<CatalogBiome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CatalogBiome {
    pub id: String,
    /// World-map biome channel name.
    pub channel: String,
    pub sub_biomes: Vec<SubBiome>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SubBiome {
    pub id: String,
    /// Soft selection over a world-map field; absent = always selected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub select: Option<FieldRange>,
    /// Multiplier on the variant detail heights.
    pub amplitude: f64,
    pub variants: Vec<Variant>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorldField {
    /// Macro elevation above the reference radius (game metres).
    ElevationM,
    /// Macro slope, rise over run (dimensionless).
    Slope,
}

/// Weight = smoothstep up through `above` × smoothstep down through `below`,
/// each over ± `softness`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FieldRange {
    pub field: WorldField,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub above: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub below: Option<f64>,
    pub softness: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    pub bundle: String,
    /// Expected sha256 of the bundle's `height.r16` (hex).
    pub height_sha256: String,
    pub weight: f64,
}

pub(crate) fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl FieldRange {
    pub fn weight(&self, value: f64) -> f64 {
        let s = self.softness;
        let up = self.above.map_or(1.0, |a| smoothstep(a - s, a + s, value));
        let down = self
            .below
            .map_or(1.0, |b| 1.0 - smoothstep(b - s, b + s, value));
        up * down
    }
}

impl BiomeCatalog {
    /// Check the catalog against the world map's biome channels.
    pub fn validate(&self, world_map_channels: &[&str]) -> Result<(), String> {
        let fail = |what: String| Err(format!("biome catalog {}: {what}", self.id));
        let ok = |v: f64, lo: f64, hi: f64| v.is_finite() && (lo..=hi).contains(&v);
        if self.format != CATALOG_FORMAT {
            return fail(format!("format must be {CATALOG_FORMAT}"));
        }
        if self
            .channels
            .iter()
            .map(String::as_str)
            .ne(world_map_channels.iter().copied())
        {
            return fail(format!("channels must be {world_map_channels:?}"));
        }
        if !ok(self.cell_m, 100.0, 1.0e5)
            || !ok(self.cell_blend_m, 1.0, 0.5 * self.cell_m)
            || !ok(self.cell_warp_m, 0.0, 0.5 * self.cell_m)
            || !ok(self.band_limit_m, 10.0, 1.0e5)
        {
            return fail("cell_m, cell_blend_m or band_limit_m out of range".into());
        }
        let mut ids = std::collections::BTreeSet::new();
        for channel in &self.channels {
            let bound = self.biomes.iter().filter(|b| &b.channel == channel).count();
            if bound != 1 {
                return fail(format!(
                    "channel {channel} is bound {bound} times, must be once"
                ));
            }
        }
        for b in &self.biomes {
            if !self.channels.contains(&b.channel) {
                return fail(format!(
                    "biome {} binds unknown channel {}",
                    b.id, b.channel
                ));
            }
            if b.sub_biomes.is_empty() {
                return fail(format!("biome {} has no sub-biomes", b.id));
            }
            for id in std::iter::once(&b.id).chain(b.sub_biomes.iter().map(|s| &s.id)) {
                if id.is_empty() || !ids.insert(id.clone()) {
                    return fail(format!("id {id:?} is empty or not unique"));
                }
            }
            for s in &b.sub_biomes {
                if !ok(s.amplitude, 0.0, 4.0) || s.variants.is_empty() {
                    return fail(format!("sub-biome {}: amplitude or variants", s.id));
                }
                if let Some(r) = &s.select {
                    let finite = |v: Option<f64>| v.is_none_or(f64::is_finite);
                    if (r.above.is_none() && r.below.is_none())
                        || !finite(r.above)
                        || !finite(r.below)
                        || !(r.softness > 0.0 && r.softness.is_finite())
                    {
                        return fail(format!("sub-biome {}: select range", s.id));
                    }
                }
                for v in &s.variants {
                    let hex = v.height_sha256.len() == 64
                        && v.height_sha256.bytes().all(|c| c.is_ascii_hexdigit());
                    if v.bundle.is_empty() || !hex || !ok(v.weight, 1e-9, 1e9) {
                        return fail(format!("sub-biome {}: variant {}", s.id, v.bundle));
                    }
                }
            }
        }
        Ok(())
    }

    /// Content identity: sha256 of the canonical JSON, which includes every
    /// variant's height hash.
    pub fn identity_sha256(&self) -> String {
        let bytes = serde_json::to_vec(self).expect("catalog serialises");
        format!("{:x}", Sha256::digest(bytes))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const CHANNELS: [&str; 2] = ["rough", "smooth"];

    pub(crate) fn catalog() -> BiomeCatalog {
        let variant = |bundle: &str| Variant {
            bundle: bundle.into(),
            height_sha256: "0".repeat(64),
            weight: 1.0,
        };
        BiomeCatalog {
            format: CATALOG_FORMAT.into(),
            id: "test".into(),
            seed: 3,
            channels: CHANNELS.map(String::from).to_vec(),
            cell_m: 4000.0,
            cell_blend_m: 800.0,
            cell_warp_m: 600.0,
            band_limit_m: 1500.0,
            biomes: vec![
                CatalogBiome {
                    id: "rough".into(),
                    channel: "rough".into(),
                    sub_biomes: vec![
                        SubBiome {
                            id: "rough_low".into(),
                            select: Some(FieldRange {
                                field: WorldField::ElevationM,
                                above: None,
                                below: Some(0.0),
                                softness: 50.0,
                            }),
                            amplitude: 1.0,
                            variants: vec![variant("a"), variant("b")],
                        },
                        SubBiome {
                            id: "rough_high".into(),
                            select: Some(FieldRange {
                                field: WorldField::ElevationM,
                                above: Some(0.0),
                                below: None,
                                softness: 50.0,
                            }),
                            amplitude: 1.5,
                            variants: vec![variant("a")],
                        },
                    ],
                },
                CatalogBiome {
                    id: "smooth".into(),
                    channel: "smooth".into(),
                    sub_biomes: vec![SubBiome {
                        id: "smooth_all".into(),
                        select: None,
                        amplitude: 0.5,
                        variants: vec![variant("c")],
                    }],
                },
            ],
        }
    }

    #[test]
    fn a_valid_catalog_passes_and_round_trips() {
        let c = catalog();
        c.validate(&CHANNELS).unwrap();
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(serde_json::from_str::<BiomeCatalog>(&json).unwrap(), c);
        assert_eq!(c.identity_sha256().len(), 64);
    }

    #[test]
    fn invalid_catalogs_are_rejected() {
        let mut unbound = catalog();
        unbound.biomes.pop();
        assert!(unbound.validate(&CHANNELS).is_err());

        let mut wrong_channels = catalog();
        wrong_channels.channels.reverse();
        assert!(wrong_channels.validate(&CHANNELS).is_err());

        let mut bad_weight = catalog();
        bad_weight.biomes[0].sub_biomes[0].variants[0].weight = 0.0;
        assert!(bad_weight.validate(&CHANNELS).is_err());

        let mut bad_hash = catalog();
        bad_hash.biomes[1].sub_biomes[0].variants[0].height_sha256 = "xyz".into();
        assert!(bad_hash.validate(&CHANNELS).is_err());

        let mut duplicate = catalog();
        duplicate.biomes[1].sub_biomes[0].id = "rough_low".into();
        assert!(duplicate.validate(&CHANNELS).is_err());

        let mut json = serde_json::to_value(catalog()).unwrap();
        json["surprise"] = serde_json::json!(1);
        assert!(serde_json::from_value::<BiomeCatalog>(json).is_err());

        // Identity changes with any variant hash.
        let mut other = catalog();
        other.biomes[0].sub_biomes[0].variants[1].height_sha256 = "1".repeat(64);
        assert_ne!(other.identity_sha256(), catalog().identity_sha256());
    }

    #[test]
    fn field_ranges_are_soft() {
        let r = FieldRange {
            field: WorldField::Slope,
            above: Some(0.1),
            below: None,
            softness: 0.02,
        };
        assert_eq!(r.weight(0.0), 0.0);
        assert!((r.weight(0.1) - 0.5).abs() < 1e-12);
        assert_eq!(r.weight(0.2), 1.0);
    }
}
