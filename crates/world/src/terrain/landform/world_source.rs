//! Tier A fields of a baked world map as the landform [`FieldSource`], the
//! baked landform weights, and the CPU composition `Σ wᵢ·landformᵢ` added on
//! top of the macro elevation (M2 Shape). The GPU producer
//! (`landform_eval.wgsl`) mirrors it.
//!
//! - Fields: level-0 maps, bicubic, with analytic gradients
//!   (`CubeMap::bicubic_gradient`).
//! - Weights: the Tier A weight run (rules evaluated per texel in the bake,
//!   `tier_a::landform_rule_fields`), sampled with a cubic B-spline, which
//!   never overshoots and keeps normalised weights summing to one.
//! - The weight gradient is left out of the composed gradient (weights vary
//!   over Tier A texels, ~1 km; relief gradients dominate the normal).
use super::eval::{Dual, FieldSource, LandformParams};
use super::schema::RecipeField;
use super::set::LandformSet;
use crate::terrain::surface::world_field::WorldMaps;
use crate::terrain::world_map::CubeMap;
use glam::DVec3;

/// Level-0 Tier A fields sampled bicubically at body positions, and the
/// four landform weight lanes.
#[derive(Debug)]
pub struct MapsFields {
    radius_m: f64,
    elevation: CubeMap<f32>,
    temperature: CubeMap<f32>,
    moisture: CubeMap<f32>,
    uplift: CubeMap<f32>,
    hardness: CubeMap<f32>,
    sediment: CubeMap<f32>,
    flow: CubeMap<f32>,
    volcanic: CubeMap<f32>,
    /// `boundary_coord` in metres.
    boundary_m: CubeMap<f32>,
    /// Landform weights (unorm8 lanes of the weight run) as 0..1.
    weights: [CubeMap<f32>; 4],
}

impl MapsFields {
    /// `None` without the M2 (Shape) maps.
    pub fn new(maps: &WorldMaps, radius_m: f64) -> Option<Self> {
        let shape = maps.shape.as_ref()?;
        let convert = |source: &CubeMap<i32>| {
            let mut out = CubeMap::new(source.n(), 0.0f32);
            for (o, v) in out.data_mut().iter_mut().zip(source.data()) {
                *o = (f64::from(*v) / crate::terrain::tier_a::BOUNDARY_UNITS_PER_M) as f32;
            }
            out
        };
        let byte = |source: &CubeMap<u32>, lane: u32| {
            let mut out = CubeMap::new(source.n(), 0.0f32);
            for (o, w) in out.data_mut().iter_mut().zip(source.data()) {
                *o = ((w >> (8 * lane)) & 0xff) as f32 / 255.0;
            }
            out
        };
        let landform = &shape.landform_mips[0];
        Some(Self {
            radius_m,
            elevation: maps.elevation_mips[0].clone(),
            temperature: maps.temperature_mips[0].clone(),
            moisture: maps.moisture_mips[0].clone(),
            uplift: shape.uplift_mips[0].clone(),
            hardness: shape.hardness_mips[0].clone(),
            sediment: shape.sediment_mips[0].clone(),
            flow: shape.flow_mips[0].clone(),
            volcanic: byte(&shape.aux1_mips[0], 2),
            boundary_m: convert(&shape.boundary_coord_mips[0]),
            weights: std::array::from_fn(|lane| byte(landform, lane as u32)),
        })
    }

    fn map(&self, f: RecipeField) -> &CubeMap<f32> {
        match f {
            RecipeField::Uplift => &self.uplift,
            RecipeField::Sediment => &self.sediment,
            RecipeField::Flow => &self.flow,
            RecipeField::Hardness => &self.hardness,
            RecipeField::Moisture => &self.moisture,
            RecipeField::Temperature => &self.temperature,
            RecipeField::Volcanic => &self.volcanic,
            RecipeField::Elevation => &self.elevation,
            RecipeField::BoundaryCoord => &self.boundary_m,
        }
    }

    /// Value and body-space gradient (per metre) of `map` at direction `d`.
    fn sample(&self, map: &CubeMap<f32>, d: DVec3) -> Dual {
        let (value, gradient) = map.bicubic_gradient(d);
        Dual::new(value, gradient / self.radius_m)
    }

    /// The four landform weights at direction `d` (B-spline of the run).
    pub fn weights(&self, d: DVec3) -> [f64; 4] {
        std::array::from_fn(|lane| self.weights[lane].bspline(d))
    }
}

impl FieldSource for MapsFields {
    fn field(&self, f: RecipeField, p_m: DVec3) -> Dual {
        self.sample(self.map(f), p_m.normalize())
    }
}

/// Landform relief `Σ wᵢ·landformᵢ` (metres) at unit direction `d`,
/// band-limited for texels of `texel_m`.
pub fn compose(
    set: &LandformSet,
    params: &[LandformParams],
    fields: &MapsFields,
    d: DVec3,
    texel_m: f64,
) -> Dual {
    let weights = fields.weights(d);
    let p = d * fields.radius_m;
    let mut sum = Dual::default();
    for ((landform, param), w) in set.landforms().iter().zip(params).zip(weights) {
        if w <= 0.0 {
            continue;
        }
        let h = landform.program.evaluate(p, param, Some(texel_m), fields);
        sum = Dual::new(sum.value + w * h.value, sum.gradient + h.gradient * w);
    }
    sum
}
