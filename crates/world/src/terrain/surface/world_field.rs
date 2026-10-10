//! WorldV1 surface: the Tier A world map (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6)
//! plus band-limited detail, M1 (World map and planet editor).
//!
//! Under ADR 0023 the GPU bake and producer are the authority. This module is
//! the CPU test oracle: it bakes Tier A on the CPU only when a sample is first
//! requested (tests), or takes fields read back from the GPU. Building a
//! generator or a producer recipe never bakes.
use super::TerrainError;
use crate::terrain::{
    archetype::{MaterialAsset, PlanetArchetype, PlanetParams},
    biome_lut::BiomeLut,
    tier_a::{TierAFields, TierAInputs, bake},
    world_map::CubeMap,
};
use glam::DVec3;
use std::sync::{Arc, OnceLock};

/// Colour inputs of a world-map body (§10), resolved from the archetype's
/// file references: biome LUT, snow rule with its material, flat water.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldLook {
    pub lut: BiomeLut,
    pub snow_temperature_c: f64,
    pub snow_blend_c: f64,
    /// Slope angles (radians) over which snow fades out.
    pub snow_slope_rad: (f64, f64),
    pub snow_albedo: [f64; 3],
    pub water_shallow: [f64; 3],
    pub water_deep: [f64; 3],
    pub water_depth_scale_m: f64,
}

fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl WorldLook {
    pub fn new(
        archetype: &PlanetArchetype,
        lut: BiomeLut,
        snow: &MaterialAsset,
    ) -> Result<Self, TerrainError> {
        archetype.validate()?;
        snow.validate()?;
        let colour = |c: (f32, f32, f32)| [c.0, c.1, c.2].map(f64::from);
        let s = &archetype.snow;
        let m = snow.mean_albedo_linear;
        Ok(Self {
            lut,
            snow_temperature_c: s.temperature_c,
            snow_blend_c: s.blend_c,
            snow_slope_rad: (s.slope_deg.0.to_radians(), s.slope_deg.1.to_radians()),
            snow_albedo: [m.0, m.1, m.2],
            water_shallow: colour(archetype.water.shallow_linear),
            water_deep: colour(archetype.water.deep_linear),
            water_depth_scale_m: archetype.water.depth_scale_m,
        })
    }

    pub fn identity(&self) -> u64 {
        [
            self.snow_temperature_c,
            self.snow_blend_c,
            self.snow_slope_rad.0,
            self.snow_slope_rad.1,
            self.water_depth_scale_m,
        ]
        .into_iter()
        .chain(self.snow_albedo)
        .chain(self.water_shallow)
        .chain(self.water_deep)
        .fold(self.lut.identity(), |hash, v| {
            (hash ^ v.to_bits()).wrapping_mul(0x100_0000_01b3)
        })
    }

    /// Linear albedo of the water surface over ground at `height_m < 0`.
    pub fn water(&self, height_m: f64) -> [f64; 3] {
        let deep = 1.0 - (height_m / self.water_depth_scale_m).exp();
        std::array::from_fn(|k| {
            self.water_shallow[k] + (self.water_deep[k] - self.water_shallow[k]) * deep
        })
    }

    /// Linear albedo of land: LUT tint by climate, then snow by temperature
    /// and slope (angle between `normal` and the radial direction `d`).
    pub fn land(&self, temperature_c: f64, moisture: f64, d: DVec3, normal: DVec3) -> [f64; 3] {
        let tint = self.lut.sample(temperature_c, moisture);
        let slope = normal.dot(d).clamp(-1.0, 1.0).acos();
        let snow =
            (1.0 - smoothstep(
                self.snow_temperature_c - self.snow_blend_c,
                self.snow_temperature_c + self.snow_blend_c,
                temperature_c,
            )) * (1.0 - smoothstep(self.snow_slope_rad.0, self.snow_slope_rad.1, slope));
        std::array::from_fn(|k| tint[k] + (self.snow_albedo[k] - tint[k]) * snow)
    }
}

/// Authored world-map surface of one body.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldDefinition {
    pub archetype: PlanetArchetype,
    /// Sampled from the archetype with the body seed, then overridden.
    pub params: PlanetParams,
    /// Body-fixed rotation axis.
    pub pole: DVec3,
    pub look: Arc<WorldLook>,
}

impl WorldDefinition {
    /// Sample `archetype` with `body_seed` and apply named overrides.
    pub fn new(
        archetype: PlanetArchetype,
        look: WorldLook,
        body_seed: u64,
        overrides: &[(String, f64)],
        pole: DVec3,
    ) -> Result<Self, TerrainError> {
        archetype.validate()?;
        if !pole.is_finite() || (pole.length() - 1.0).abs() > 1e-9 {
            return Err(TerrainError::InvalidConfig);
        }
        let mut params = archetype.sample(body_seed);
        for (name, value) in overrides {
            params.set(name, *value)?;
        }
        Ok(Self {
            archetype,
            params,
            pole,
            look: Arc::new(look),
        })
    }

    pub fn inputs(&self, radius_m: f64) -> TierAInputs {
        TierAInputs {
            params: self.params,
            stages: self.archetype.stages.clone(),
            radius_m,
            pole: self.pole,
            face_cells: self.archetype.face_cells(radius_m) as usize,
        }
    }

    /// Largest absolute macro elevation.
    pub fn height_bound_m(&self) -> f64 {
        self.params.land_height_m.max(self.params.ocean_depth_m)
    }

    pub fn identity(&self) -> u64 {
        let stages = self.archetype.stages.iter().fold(0u64, |hash, stage| {
            hash.rotate_left(7) ^ (*stage as u64 + 1)
        });
        let resolution = self.archetype.resolution.iter().fold(0u64, |hash, band| {
            hash.rotate_left(11) ^ band.max_radius_km.to_bits() ^ u64::from(band.face_cells)
        });
        self.params.identity()
            ^ stages.rotate_left(17)
            ^ resolution.rotate_left(29)
            ^ self.pole.x.to_bits().rotate_left(3)
            ^ self.pole.y.to_bits().rotate_left(23)
            ^ self.pole.z.to_bits().rotate_left(43)
            ^ self.look.identity().rotate_left(53)
    }
}

/// Baked fields with the mip chains used for band-limited sampling.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldMaps {
    pub fields: TierAFields,
    /// Level 0 is the baked field; each level halves the face cells by 2×2
    /// box averaging within a face, down to 4 cells.
    pub elevation_mips: Vec<CubeMap<f32>>,
    pub temperature_mips: Vec<CubeMap<f32>>,
    pub moisture_mips: Vec<CubeMap<f32>>,
}

/// Mip levels of a field, level 0 the field itself (2×2 averages per face).
pub fn field_mips(field: &CubeMap<f32>) -> Vec<CubeMap<f32>> {
    let mut levels = vec![field.clone()];
    while levels.last().expect("level zero").n() / 2 >= 4 {
        let fine = levels.last().expect("level exists");
        let n = fine.n() / 2;
        let mut coarse = CubeMap::new(n, 0.0f32);
        for face in 0..6 {
            for j in 0..n {
                for i in 0..n {
                    let at = |x: usize, y: usize| f64::from(fine.data()[fine.index(face, x, y)]);
                    let sum = at(2 * i, 2 * j)
                        + at(2 * i + 1, 2 * j)
                        + at(2 * i, 2 * j + 1)
                        + at(2 * i + 1, 2 * j + 1);
                    let k = coarse.index(face, i, j);
                    coarse.data_mut()[k] = (0.25 * sum) as f32;
                }
            }
        }
        levels.push(coarse);
    }
    levels
}

impl WorldMaps {
    pub fn new(fields: TierAFields) -> Self {
        Self {
            elevation_mips: field_mips(&fields.elevation),
            temperature_mips: field_mips(&fields.temperature),
            moisture_mips: field_mips(&fields.moisture),
            fields,
        }
    }

    /// Bilinear temperature (°C) and moisture of `level` at direction `d`.
    pub fn climate(&self, level: usize, d: DVec3) -> (f64, f64) {
        (
            self.temperature_mips[level].bilinear(d),
            self.moisture_mips[level].bilinear(d),
        )
    }

    /// Face-centre texel spacing of elevation level `level`.
    pub fn texel_m(&self, level: usize, radius_m: f64) -> f64 {
        2.0 * radius_m / self.elevation_mips[level].n() as f64
    }

    /// Coarsest level whose texels are no wider than `texel_m` allows: the
    /// same rule as `ProfilePyramid::mip_for` (shared with the GPU producer).
    pub fn mip_for(&self, texel_m: f64, radius_m: f64) -> usize {
        mip_for(
            texel_m,
            self.texel_m(0, radius_m),
            self.elevation_mips.len(),
        )
    }

    /// Bicubic elevation of `level` at unit direction `d` and its tangent
    /// gradient in metres per unit direction (central differences over a
    /// quarter texel).
    pub fn sample(&self, level: usize, d: DVec3) -> (f64, DVec3) {
        let map = &self.elevation_mips[level];
        let h = map.bicubic(d);
        // A quarter texel (texels span 2/n): small enough for an accurate
        // slope, large enough for f32 on the GPU.
        let delta = 0.5 / map.n() as f64;
        let e1 = d.any_orthonormal_vector();
        let e2 = d.cross(e1);
        let at = |v: DVec3| map.bicubic((d + v * delta).normalize());
        let gradient =
            e1 * ((at(e1) - at(-e1)) / (2.0 * delta)) + e2 * ((at(e2) - at(-e2)) / (2.0 * delta));
        (h, gradient)
    }
}

/// Mip level for a footprint `texel_m` over a base spacing `base_texel_m`.
pub fn mip_for(texel_m: f64, base_texel_m: f64, levels: usize) -> usize {
    if texel_m.partial_cmp(&base_texel_m) != Some(std::cmp::Ordering::Greater)
        || !base_texel_m.is_finite()
    {
        return 0;
    }
    ((texel_m / base_texel_m).log2().floor().max(0.0) as usize).min(levels - 1)
}

/// Highest Tier A noise frequency, in cycles per unit direction, that f32
/// lattice coordinates on the GPU still resolve finely (ulp ~2.4e-4 at 4096).
pub const MAX_TIER_A_NOISE_FREQUENCY: f64 = 4096.0;

/// WorldV1 geology of a compiled generator: lazily baked CPU oracle maps.
#[derive(Debug, Clone)]
pub struct WorldField {
    inputs: TierAInputs,
    look: Arc<WorldLook>,
    maps: Arc<OnceLock<Arc<WorldMaps>>>,
}

impl WorldField {
    pub fn new(definition: &WorldDefinition, radius_m: f64) -> Result<Self, TerrainError> {
        if !radius_m.is_finite() || radius_m <= 0.0 {
            return Err(TerrainError::InvalidRadius);
        }
        // The GPU bake evaluates noise at absolute f32 lattice coordinates
        // (cycles per unit direction); beyond a few thousand f32 loses the
        // sub-cell precision the noise needs.
        let p = &definition.params;
        let highest = [
            radius_m / p.continent_wavelength_m
                * p.continent_lacunarity
                    .powi(p.continent_octaves.saturating_sub(1) as i32),
            radius_m / p.warp_wavelength_m,
            radius_m / p.temperature_noise_wavelength_m,
        ];
        if highest
            .iter()
            .any(|f| !f.is_finite() || *f > MAX_TIER_A_NOISE_FREQUENCY)
        {
            return Err(TerrainError::InvalidConfig);
        }
        Ok(Self {
            inputs: definition.inputs(radius_m),
            look: definition.look.clone(),
            maps: Arc::new(OnceLock::new()),
        })
    }

    pub fn inputs(&self) -> &TierAInputs {
        &self.inputs
    }

    pub fn look(&self) -> &WorldLook {
        &self.look
    }

    /// Use fields baked elsewhere (the GPU read back in tests) instead of a
    /// CPU bake. Returns false if maps were already present.
    pub fn provide(&self, fields: TierAFields) -> bool {
        self.maps.set(Arc::new(WorldMaps::new(fields))).is_ok()
    }

    /// Heap bytes of maps present now (a CPU bake or provided fields).
    pub fn resident_heap_bytes(&self) -> usize {
        self.maps.get().map_or(0, |maps| {
            let field_bytes = maps.fields.elevation.data().len() * 4 * 5;
            let mip_bytes: usize = [
                &maps.elevation_mips,
                &maps.temperature_mips,
                &maps.moisture_mips,
            ]
            .iter()
            .flat_map(|chain| chain[1..].iter())
            .map(|m| m.data().len() * 4)
            .sum();
            field_bytes + mip_bytes
        })
    }

    /// The maps, baking on the CPU on first use (test oracle only).
    pub fn maps(&self) -> Result<&Arc<WorldMaps>, TerrainError> {
        if let Some(maps) = self.maps.get() {
            return Ok(maps);
        }
        let fields = bake(&self.inputs)?;
        Ok(self.maps.get_or_init(|| Arc::new(WorldMaps::new(fields))))
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::terrain::archetype::tests::terra;

    pub(crate) fn terra_look() -> WorldLook {
        let root = crate::terrain::biome_lut::tests::content();
        let snow: MaterialAsset = ron::from_str(
            &std::fs::read_to_string(root.join("materials/snow/material.ron")).unwrap(),
        )
        .unwrap();
        WorldLook::new(
            &terra(),
            crate::terrain::biome_lut::tests::terra_lut(),
            &snow,
        )
        .unwrap()
    }

    #[test]
    fn mips_average_and_bicubic_gradient_matches_differences() {
        let definition = WorldDefinition::new(terra(), terra_look(), 7, &[], DVec3::Y).unwrap();
        let mut inputs = definition.inputs(338_950.0);
        inputs.face_cells = 32;
        let maps = WorldMaps::new(bake(&inputs).unwrap());
        assert_eq!(maps.elevation_mips.len(), 4); // 32, 16, 8, 4
        let fine = &maps.elevation_mips[0];
        let coarse = &maps.elevation_mips[1];
        let mean = 0.25
            * [(0, 0), (1, 0), (0, 1), (1, 1)]
                .iter()
                .map(|&(x, y)| f64::from(fine.data()[fine.index(2, x, y)]))
                .sum::<f64>();
        assert!((f64::from(coarse.data()[coarse.index(2, 0, 0)]) - mean).abs() < 1e-3);
        let radius = 338_950.0;
        let base = maps.texel_m(0, radius);
        assert_eq!(maps.mip_for(base * 0.5, radius), 0);
        assert_eq!(maps.mip_for(base * 2.5, radius), 1);
        assert_eq!(maps.mip_for(base * 1e6, radius), 3);
        let d = DVec3::new(0.3, 0.8, -0.2).normalize();
        let (_, gradient) = maps.sample(0, d);
        assert!(gradient.dot(d).abs() < 1e-6 * (1.0 + gradient.length()));
        let e = d.any_orthonormal_vector();
        let h = 1e-4;
        let fd = (maps.sample(0, (d + e * h).normalize()).0
            - maps.sample(0, (d - e * h).normalize()).0)
            / (2.0 * h);
        assert!(
            (fd - gradient.dot(e)).abs() < 0.05 * (1.0 + fd.abs()),
            "{fd} vs {}",
            gradient.dot(e)
        );
        let mut overridden =
            WorldDefinition::new(terra(), terra_look(), 7, &[("rain".into(), 0.05)], DVec3::Y)
                .unwrap();
        assert_ne!(overridden.identity(), definition.identity());
        overridden.params = definition.params;
        assert_eq!(overridden.identity(), definition.identity());
        assert!(
            WorldDefinition::new(terra(), terra_look(), 7, &[("rain".into(), 9.0)], DVec3::Y)
                .is_err()
        );
    }
}
