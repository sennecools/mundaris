//! PROTOTYPE (M2 Shape Step 6a): Tier A fields of a baked world map as the
//! landform [`FieldSource`] and weight-rule inputs, and the CPU composition
//! `Σ wᵢ·landformᵢ` added on top of the macro elevation.
//!
//! Prototype shortcuts (hardened in Step 6b): weights are evaluated per
//! sample from level-0 fields instead of the Tier A weight run and its
//! B-spline lookup; field gradients are central differences over a quarter
//! texel of the bicubic level-0 maps; the weight gradient is left out of the
//! composed gradient; the landform seed comes from the continent seed.
use super::eval::{Dual, FieldSource, LandformParams};
use super::expr::{RuleFields, field};
use super::schema::RecipeField;
use super::set::LandformSet;
use crate::terrain::surface::world_field::WorldMaps;
use crate::terrain::world_map::CubeMap;
use glam::DVec3;

/// Level-0 Tier A fields sampled bicubically at body positions.
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
        let aux1 = &shape.aux1_mips[0];
        let mut volcanic = CubeMap::new(aux1.n(), 0.0f32);
        for (o, w) in volcanic.data_mut().iter_mut().zip(aux1.data()) {
            *o = ((w >> 16) & 0xff) as f32 / 255.0;
        }
        Some(Self {
            radius_m,
            elevation: maps.elevation_mips[0].clone(),
            temperature: maps.temperature_mips[0].clone(),
            moisture: maps.moisture_mips[0].clone(),
            uplift: shape.uplift_mips[0].clone(),
            hardness: shape.hardness_mips[0].clone(),
            sediment: shape.sediment_mips[0].clone(),
            flow: shape.flow_mips[0].clone(),
            volcanic,
            boundary_m: convert(&shape.boundary_coord_mips[0]),
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
        let delta = 0.5 / map.n() as f64;
        let e1 = d.any_orthonormal_vector();
        let e2 = d.cross(e1);
        let at = |v: DVec3| map.bicubic((d + v * delta).normalize());
        let g =
            e1 * ((at(e1) - at(-e1)) / (2.0 * delta)) + e2 * ((at(e2) - at(-e2)) / (2.0 * delta));
        Dual::new(map.bicubic(d), g / self.radius_m)
    }

    /// Weight-rule inputs at direction `d`.
    pub fn rule_fields(&self, d: DVec3) -> RuleFields {
        let mut f = [0.0; field::COUNT];
        let elevation = self.sample(&self.elevation, d);
        let t = self.temperature.bicubic(d);
        let m = self.moisture.bicubic(d).clamp(0.0, 1.0);
        let cold = {
            let x = ((t + 15.0) / 20.0).clamp(0.0, 1.0);
            1.0 - x * x * (3.0 - 2.0 * x)
        };
        f[field::UPLIFT as usize] = self.uplift.bicubic(d).clamp(0.0, 1.0);
        f[field::SEDIMENT as usize] = self.sediment.bicubic(d).clamp(0.0, 1.0);
        f[field::MOISTURE as usize] = m;
        f[field::ARID as usize] = 1.0 - m;
        f[field::TEMPERATURE as usize] = t;
        f[field::COLD as usize] = cold;
        f[field::HARDNESS as usize] = self.hardness.bicubic(d).clamp(0.0, 1.0);
        f[field::SLOPE_MACRO as usize] = elevation.gradient.length();
        f[field::ELEVATION as usize] = elevation.value;
        f[field::BOUNDARY_DISTANCE as usize] = self.boundary_m.bicubic(d).abs();
        f[field::VOLCANIC as usize] = self.volcanic.bicubic(d).clamp(0.0, 1.0);
        f[field::OCEAN as usize] = if elevation.value < 0.0 { 1.0 } else { 0.0 };
        f
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
    let weights = set.weights(&fields.rule_fields(d));
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
