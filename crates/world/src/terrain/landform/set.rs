//! A compiled landform set: weight bytecode plus one [`Program`] per landform.
use super::LandformError;
use super::eval::LandformParams;
use super::expr::{Rule, RuleFields, encode_set, evaluate_set};
use super::ir::{CompileOptions, Program, fold_hash};
use super::schema::{LandformSetFile, RecipeFile};
use crate::terrain::archetype::stage_seed;

/// Set schema version.
pub const SET_SCHEMA: u32 = 1;
/// Landform limit: one packed Tier A run holds four unorm8 weights.
pub const MAX_LANDFORMS: usize = 4;
/// Largest authored amplitude.
pub const MAX_AMPLITUDE_M: f64 = 10_000.0;
/// Stage tag of landform draws (§5.1 `hash(body_seed, stage)`).
pub const STAGE_LANDFORMS: u64 = 0x4c41_4e44;

/// One compiled landform.
#[derive(Debug, Clone, PartialEq)]
pub struct Landform {
    pub name: String,
    pub rule: Rule,
    pub program: Program,
    pub amplitude_m: (f64, f64),
}

/// Validated landform set.
#[derive(Debug, Clone, PartialEq)]
pub struct LandformSet {
    name: String,
    normalize: bool,
    fallback: usize,
    relief_band_edge_m: f64,
    landforms: Vec<Landform>,
    bytecode: Vec<u32>,
}

impl LandformSet {
    /// Validate `file` and compile its rules and recipes. `load_recipe`
    /// returns the parsed recipe for a path relative to the set file's
    /// directory.
    pub fn compile(
        file: &LandformSetFile,
        mut load_recipe: impl FnMut(&str) -> Result<RecipeFile, LandformError>,
    ) -> Result<Self, LandformError> {
        let set_error = |m: String| Err(LandformError::Set(m));
        if file.schema != SET_SCHEMA {
            return set_error(format!("schema {} is not {SET_SCHEMA}", file.schema));
        }
        if file.name.is_empty() || !(1..=MAX_LANDFORMS).contains(&file.landforms.len()) {
            return set_error(format!("needs a name and 1..={MAX_LANDFORMS} landforms"));
        }
        if !(file.relief_band_edge_m.is_finite() && file.relief_band_edge_m > 0.0) {
            return set_error("relief_band_edge_m must be positive".into());
        }
        let options = CompileOptions {
            relief_band_edge_m: file.relief_band_edge_m,
        };
        let mut landforms: Vec<Landform> = Vec::with_capacity(file.landforms.len());
        for entry in &file.landforms {
            let within = |e: LandformError| LandformError::InLandform {
                landform: entry.name.clone(),
                source: Box::new(e),
            };
            if entry.name.is_empty() || landforms.iter().any(|l| l.name == entry.name) {
                return set_error(format!(
                    "landform name '{}' is empty or repeated",
                    entry.name
                ));
            }
            let (lo, hi) = entry.amplitude_m;
            if !(lo.is_finite() && hi.is_finite() && 0.0 <= lo && lo <= hi && hi <= MAX_AMPLITUDE_M)
            {
                return Err(within(LandformError::Set(format!(
                    "amplitude_m must satisfy 0 ≤ min ≤ max ≤ {MAX_AMPLITUDE_M}"
                ))));
            }
            let rule = Rule::compile(&entry.weight).map_err(within)?;
            let recipe = load_recipe(&entry.recipe).map_err(within)?;
            let program = Program::compile(&entry.name, &recipe, &options).map_err(within)?;
            landforms.push(Landform {
                name: entry.name.clone(),
                rule,
                program,
                amplitude_m: entry.amplitude_m,
            });
        }
        let fallback = landforms
            .iter()
            .position(|l| l.name == file.fallback)
            .ok_or_else(|| {
                LandformError::Set(format!("fallback '{}' is not a landform", file.fallback))
            })?;
        let rules: Vec<Rule> = landforms.iter().map(|l| l.rule.clone()).collect();
        let bytecode = encode_set(&rules, file.normalize, fallback)?;
        Ok(Self {
            name: file.name.clone(),
            normalize: file.normalize,
            fallback,
            relief_band_edge_m: file.relief_band_edge_m,
            landforms,
            bytecode,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn normalize(&self) -> bool {
        self.normalize
    }

    pub fn fallback(&self) -> usize {
        self.fallback
    }

    pub fn relief_band_edge_m(&self) -> f64 {
        self.relief_band_edge_m
    }

    pub fn landforms(&self) -> &[Landform] {
        &self.landforms
    }

    /// Set buffer for the Tier A weights interpreter (`expr` module docs).
    pub fn bytecode(&self) -> &[u32] {
        &self.bytecode
    }

    /// CPU reference weights of one texel.
    pub fn weights(&self, fields: &RuleFields) -> Vec<f64> {
        evaluate_set(&self.bytecode, fields).unwrap_or_else(|_| {
            let mut w = vec![0.0; self.landforms.len()];
            w[self.fallback] = 1.0;
            w
        })
    }

    /// Per-body parameters: amplitudes sampled from their ranges and the
    /// landform noise seed, drawn from the landform stage seed.
    pub fn sample_params(&self, body_seed: u64) -> Vec<LandformParams> {
        let stage = stage_seed(body_seed, STAGE_LANDFORMS);
        let seed = (stage ^ (stage >> 32)) as u32;
        self.landforms
            .iter()
            .enumerate()
            .map(|(i, l)| {
                let draw = stage_seed(stage, i as u64 + 1);
                let unit = (draw >> 11) as f64 / (1u64 << 53) as f64;
                LandformParams {
                    amplitude_m: l.amplitude_m.0 + (l.amplitude_m.1 - l.amplitude_m.0) * unit,
                    seed,
                }
            })
            .collect()
    }

    /// Largest |Σ w_i·h_i| in metres: the largest landform bound when the
    /// weights are normalised (a convex combination), else their sum
    /// (weights are at most 1).
    pub fn bound_m(&self, params: &[LandformParams]) -> f64 {
        self.combine(
            params
                .iter()
                .zip(&self.landforms)
                .map(|(p, l)| l.program.bound_m(p)),
        )
    }

    /// Unresolved bound of the weighted sum at `texel_m` (same combination).
    pub fn unresolved_bound_m(&self, params: &[LandformParams], texel_m: f64) -> f64 {
        self.combine(
            params
                .iter()
                .zip(&self.landforms)
                .map(|(p, l)| l.program.unresolved_bound_m(p, texel_m)),
        )
    }

    fn combine(&self, bounds: impl Iterator<Item = f64>) -> f64 {
        if self.normalize {
            // Weights are stored as unorm8 per lane: rounding can lift a
            // normalised sum by half a step per lane (the B-spline is a convex
            // combination, so sampling adds nothing).
            const QUANTISED_SUM: f64 = 1.0 + 4.0 * 0.5 / 255.0;
            bounds.fold(0.0, f64::max) * QUANTISED_SUM
        } else {
            bounds.sum()
        }
    }

    /// Hash of the program structures: equal sets share generated code.
    pub fn structure_identity(&self) -> u64 {
        fold_hash(
            [0x4c46_5345_5453_0001, self.landforms.len() as u64]
                .into_iter()
                .chain(
                    self.landforms
                        .iter()
                        .map(|l| l.program.structure_identity()),
                ),
        )
    }

    /// Complete identity: structures, constants, rules and amplitude ranges.
    pub fn identity(&self) -> u64 {
        fold_hash(
            [self.structure_identity(), self.relief_band_edge_m.to_bits()]
                .into_iter()
                .chain(self.bytecode.iter().map(|w| u64::from(*w)))
                .chain(self.landforms.iter().flat_map(|l| {
                    [
                        l.program.constants_identity(),
                        l.amplitude_m.0.to_bits(),
                        l.amplitude_m.1.to_bits(),
                    ]
                })),
        )
    }
}
