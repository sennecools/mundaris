//! Band-limited production recipes for terrain atlas tiles.
//!
//! Under ADR 0023 the GPU producer is the terrain authority: rendering,
//! collision and height queries all use its output (collision through
//! asynchronous readback). The CPU evaluation here and
//! `SurfaceGenerator::evaluate_point` are the test oracle the GPU is checked
//! against within documented tolerances. A recipe evaluates the surface for an
//! explicit texel footprint: profile image layers sample a pre-filtered mip
//! level, procedural crater bands and fBm detail octaves fade out below the
//! footprint's sampling limit (pipeline §9.3).
use super::{
    GeologicalField, MoonFieldDefinition, SurfaceGenerator, TerrainError,
    moon_fields::{self, MoonFieldsV1},
    moon_profile::{
        NoopProfileObserver, PROFILE_BANDS, ProfileGrid, ProfileKernel, sample_triplanar,
        triplanar_weights,
    },
    noise::DetailNoise,
    world_field::WorldField,
};
use glam::{DMat3, DVec3};
use std::sync::Arc;

/// Version of the footprint filtering semantics below. Derived caches must
/// include it in their identity.
pub const BAND_LIMIT_VERSION: u32 = 2;
/// A crater band contributes fully while `texel <= edge * BAND_FADE_START` and
/// nothing once `texel >= edge * BAND_FADE_END` (smoothstep in between).
pub const BAND_FADE_START: f64 = 0.125;
pub const BAND_FADE_END: f64 = 0.25;
/// Pyramid levels stop before either side drops below four samples, the
/// support of the periodic cubic kernel.
const MIN_PYRAMID_SIDE: u32 = 4;

/// Footprint weight of a procedural band with characteristic edge `edge_m`.
pub fn band_weight(texel_m: f64, edge_m: f64) -> f64 {
    let t =
        ((texel_m / edge_m - BAND_FADE_START) / (BAND_FADE_END - BAND_FADE_START)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

/// Nominal texel spacing of a cube-chart tile with `cells` intervals, measured
/// at the face centre. Spacing on the sphere is smaller towards face corners,
/// so this choice errs towards slightly stronger filtering there.
pub fn tile_texel_m(radius_m: f64, level: u8, cells: u32) -> f64 {
    radius_m * 2.0 / ((1u64 << level) as f64 * f64::from(cells))
}

/// One pre-filtered periodic level of a height profile.
#[derive(Debug)]
pub struct ProfilePyramidLevel {
    width: u32,
    height: u32,
    values: Arc<[u16]>,
}
impl ProfilePyramidLevel {
    pub fn width(&self) -> u32 {
        self.width
    }
    pub fn height(&self) -> u32 {
        self.height
    }
    pub fn values(&self) -> &[u16] {
        &self.values
    }
}

/// Derived 2x2 box-filtered mip chain of a periodic u16 profile. Level 0 shares
/// the immutable source samples. All levels use the source's sample range, so
/// normalized values of a coarse level are averages of the finer level.
#[derive(Debug)]
pub struct ProfilePyramid {
    levels: Vec<ProfilePyramidLevel>,
    sample_range: [u16; 2],
}
impl ProfilePyramid {
    fn build(values: &Arc<[u16]>, width: u32, height: u32, sample_range: [u16; 2]) -> Self {
        let mut levels = vec![ProfilePyramidLevel {
            width,
            height,
            values: Arc::clone(values),
        }];
        loop {
            let last = levels.last().expect("level zero exists");
            if last.width % 2 != 0
                || last.height % 2 != 0
                || last.width / 2 < MIN_PYRAMID_SIDE
                || last.height / 2 < MIN_PYRAMID_SIDE
            {
                break;
            }
            let (w, h) = (last.width / 2, last.height / 2);
            let source = &last.values;
            let stride = last.width as usize;
            let mut next = Vec::with_capacity((w * h) as usize);
            for y in 0..h as usize {
                for x in 0..w as usize {
                    let a = u32::from(source[2 * y * stride + 2 * x]);
                    let b = u32::from(source[2 * y * stride + 2 * x + 1]);
                    let c = u32::from(source[(2 * y + 1) * stride + 2 * x]);
                    let d = u32::from(source[(2 * y + 1) * stride + 2 * x + 1]);
                    next.push(((a + b + c + d + 2) / 4) as u16);
                }
            }
            levels.push(ProfilePyramidLevel {
                width: w,
                height: h,
                values: next.into(),
            });
        }
        Self {
            levels,
            sample_range,
        }
    }

    pub fn levels(&self) -> &[ProfilePyramidLevel] {
        &self.levels
    }

    pub fn sample_range(&self) -> [u16; 2] {
        self.sample_range
    }

    /// Mip level whose texel spacing first covers `texel_m`, given the level-0
    /// spacing `base_texel_m`. Shared by the CPU reference and GPU producers.
    pub fn mip_for(&self, texel_m: f64, base_texel_m: f64) -> u32 {
        if texel_m.partial_cmp(&base_texel_m) != Some(std::cmp::Ordering::Greater)
            || !base_texel_m.is_finite()
        {
            return 0;
        }
        let level = (texel_m / base_texel_m).log2().floor().max(0.0) as u32;
        level.min(self.levels.len() as u32 - 1)
    }

    /// Bytes owned by derived levels, excluding the shared source level.
    pub fn derived_bytes(&self) -> usize {
        self.levels[1..]
            .iter()
            .map(|level| level.values.len() * std::mem::size_of::<u16>())
            .sum()
    }

    fn grid(&self, level: u32) -> ProfileGrid<'_> {
        let level = &self.levels[level as usize];
        ProfileGrid {
            width: level.width,
            height: level.height,
            values: &level.values,
            sample_range: self.sample_range,
        }
    }
}

/// One triplanar profile layer: chart frequency, centred height scale and the
/// metres covered by one level-0 sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProfileLayer {
    pub frequency: f64,
    pub amplitude_m: f64,
    pub base_texel_m: f64,
}

#[derive(Debug, Clone)]
pub struct ProfileDetailRecipe {
    pub layer: ProfileLayer,
    pub cubic_bspline: bool,
    pub pyramid: Arc<ProfilePyramid>,
}

/// MoonProfileV1 production recipe. The first macro layer is the coarse
/// geology signal that modulates detail layers by `0.35 + 0.65 * coarse`.
#[derive(Debug, Clone)]
pub struct ProfileRecipe {
    pub radius_m: f64,
    pub cubic_bspline: bool,
    pub macro_pyramid: Arc<ProfilePyramid>,
    pub macro_layers: Vec<ProfileLayer>,
    pub detail: Vec<ProfileDetailRecipe>,
    /// Band-limited fBm detail added after the profile (pipeline §9.3).
    pub detail_noise: Option<DetailNoise>,
}

/// Lattice shift of crater layout 0 and 1, in cell units.
pub const MOON_FIELD_LAYOUT_SHIFTS: [DVec3; 2] = [DVec3::ZERO, DVec3::new(0.31, -0.27, 0.19)];
pub const MOON_FIELD_JITTER: f64 = moon_fields::JITTER;
pub const MOON_FIELD_SHELL: f64 = moon_fields::SHELL;
pub const MOON_FIELD_SUPPORT: f64 = moon_fields::SUPPORT;

/// MoonFieldsV1 production recipe with the exact seeded constants a GPU mirror
/// needs. Crater recipes are rediscovered per sample from the hashed lattice.
#[derive(Debug, Clone)]
pub struct FieldsRecipe {
    field: MoonFieldsV1,
    detail_noise: Option<DetailNoise>,
}
impl FieldsRecipe {
    pub fn radius_m(&self) -> f64 {
        self.field.radius_m
    }
    pub fn axes(&self) -> [DVec3; 4] {
        self.field.axes
    }
    pub fn basins(&self) -> [DVec3; moon_fields::BASIN_COUNT] {
        self.field.basins
    }
    /// Per-basin `[scale, bowl depth]`.
    pub fn basin_parameters(&self) -> [[f64; 2]; moon_fields::BASIN_COUNT] {
        self.field.basin_parameters()
    }
    /// Body-to-field rotation is `rotation().transpose()`.
    pub fn rotation(&self) -> DMat3 {
        self.field.rotation
    }
    pub fn relief_fraction(&self) -> f64 {
        self.field.parameters.relief_fraction
    }
    pub fn definition(&self) -> MoonFieldDefinition {
        self.field.definition
    }
    pub fn lattice_salt(&self, band: usize, layout: usize) -> u64 {
        self.field.lattice_salt(band, layout)
    }
    pub fn band_weights(&self, texel_m: f64) -> [f64; 3] {
        let bands = self.field.definition.bands;
        std::array::from_fn(|band| band_weight(texel_m, bands[band].edge_m))
    }
}

/// WorldV1 production recipe: Tier A macro elevation from the mip matching the
/// footprint (bicubic, slopes by central differences), plus detail noise.
/// Holds the lazily baked oracle field; building it never bakes.
#[derive(Debug, Clone)]
pub struct WorldRecipe {
    pub field: WorldField,
    pub radius_m: f64,
    pub detail_noise: Option<DetailNoise>,
}

#[derive(Debug, Clone)]
pub enum ProducerRecipe {
    Profile(ProfileRecipe),
    Fields(Box<FieldsRecipe>),
    World(Box<WorldRecipe>),
}

/// Band-limited derived sample. Height is the radial offset from the reference
/// radius; the gradient is tangent, in metres per unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BandLimitedSample {
    pub height_m: f64,
    pub gradient_m: DVec3,
    pub normal: DVec3,
}

impl ProducerRecipe {
    pub fn radius_m(&self) -> f64 {
        match self {
            Self::Profile(recipe) => recipe.radius_m,
            Self::Fields(recipe) => recipe.radius_m(),
            Self::World(recipe) => recipe.radius_m,
        }
    }

    /// Band-limited fBm detail layer shared by every recipe kind.
    pub fn detail_noise(&self) -> Option<&DetailNoise> {
        match self {
            Self::Profile(recipe) => recipe.detail_noise.as_ref(),
            Self::Fields(recipe) => recipe.detail_noise.as_ref(),
            Self::World(recipe) => recipe.detail_noise.as_ref(),
        }
    }

    /// CPU reference for the derived band-limited surface at `texel_m`.
    pub fn evaluate(
        &self,
        direction: DVec3,
        texel_m: f64,
    ) -> Result<BandLimitedSample, TerrainError> {
        if !direction.is_finite()
            || direction.length_squared() < 1.0e-24
            || texel_m.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
        {
            return Err(TerrainError::NonFiniteResult);
        }
        let n = direction.normalize();
        let (height_m, gradient) = match self {
            Self::Profile(recipe) => recipe.evaluate(n, texel_m)?,
            Self::Fields(recipe) => {
                let (height, gradient, _, _) =
                    recipe
                        .field
                        .evaluate_weighted(n, None, Some(recipe.band_weights(texel_m)))?;
                (height, gradient)
            }
            Self::World(recipe) => {
                let maps = recipe.field.maps()?;
                maps.sample(maps.mip_for(texel_m, recipe.radius_m), n)
            }
        };
        let (height_m, gradient) = match self.detail_noise() {
            Some(detail) => {
                let radius = self.radius_m();
                let (h, g) = detail.evaluate(n * radius, Some(texel_m));
                (height_m + h, gradient + g * radius)
            }
            None => (height_m, gradient),
        };
        let gradient = gradient - n * n.dot(gradient);
        let normal = (n - gradient / (self.radius_m() + height_m)).normalize();
        if !height_m.is_finite() || !normal.is_finite() {
            return Err(TerrainError::NonFiniteResult);
        }
        Ok(BandLimitedSample {
            height_m,
            gradient_m: gradient,
            normal,
        })
    }

    /// CPU reference for the albedo page (linear colour) at `texel_m`, for
    /// recipes that own their colour (world maps); `None` otherwise. Water
    /// below sea level (height < 0), else the biome tint with snow, from
    /// the climate mips matching the footprint.
    pub fn albedo(&self, direction: DVec3, texel_m: f64) -> Result<Option<[f64; 3]>, TerrainError> {
        let Self::World(recipe) = self else {
            return Ok(None);
        };
        let sample = self.evaluate(direction, texel_m)?;
        if sample.height_m < 0.0 {
            return Ok(Some(recipe.field.look().water(sample.height_m)));
        }
        self.page_albedo(direction, texel_m)
    }

    /// CPU reference for the albedo page itself: the land colour everywhere,
    /// under the sea too (the draw lays flat water over it through the coast
    /// contour stored in the normal page).
    pub fn page_albedo(
        &self,
        direction: DVec3,
        texel_m: f64,
    ) -> Result<Option<[f64; 3]>, TerrainError> {
        let Self::World(recipe) = self else {
            return Ok(None);
        };
        let sample = self.evaluate(direction, texel_m)?;
        let maps = recipe.field.maps()?;
        let n = direction.normalize();
        let level = maps.mip_for(texel_m, recipe.radius_m);
        let (temperature, moisture) = maps.climate(level, n);
        let look = recipe.field.look();
        let (dt, dm) = look.climate_offsets(n * recipe.radius_m, texel_m);
        let (t, m) = (temperature + dt, moisture + dm);
        // PROTOTYPE (M4 Surface): ground materials from slope, height and the
        // Tier A hardness, sediment and flow under the world look's snow rule
        // (`material_rules::surface_colour`; GPU `world_albedo`).
        let tint = look.lut.sample(t, m);
        let slope = sample.normal.dot(n).clamp(-1.0, 1.0).acos();
        let step = |e0: f64, e1: f64, x: f64| {
            let s = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
            s * s * (3.0 - 2.0 * s)
        };
        let snow = (1.0
            - step(
                look.snow_temperature_c - look.snow_blend_c,
                look.snow_temperature_c + look.snow_blend_c,
                t,
            ))
            * (1.0 - step(look.snow_slope_rad.0, look.snow_slope_rad.1, slope));
        let (hardness, sediment, flow) = maps.shape.as_ref().map_or((0.0, 0.0, 0.0), |s| {
            (
                s.hardness_mips[level].bilinear(n),
                s.sediment_mips[level].bilinear(n),
                s.flow_mips[level].bilinear(n),
            )
        });
        let warp = look
            .climate
            .as_ref()
            .filter(|c| c.temperature_c > 0.0)
            .map_or(0.0, |c| dt / c.temperature_c);
        let ground = crate::terrain::material_rules::surface_colour(
            sample.height_m as f32,
            slope as f32,
            m as f32,
            hardness as f32,
            sediment as f32,
            flow as f32,
            tint.map(|c| c as f32),
            warp as f32,
        );
        Ok(Some(std::array::from_fn(|k| {
            let g = f64::from(ground[k]);
            g + (look.snow_albedo[k] - g) * snow
        })))
    }
}

impl ProfileRecipe {
    fn evaluate(&self, n: DVec3, texel_m: f64) -> Result<(f64, DVec3), TerrainError> {
        let (weights, weight_gradients) = triplanar_weights(n)?;
        let macro_kernel = kernel(self.cubic_bspline);
        let mut height = 0.0;
        let mut gradient = DVec3::ZERO;
        let mut coarse = 0.0;
        let mut coarse_gradient = DVec3::ZERO;
        for (index, layer) in self.macro_layers.iter().enumerate() {
            let mip = self.macro_pyramid.mip_for(texel_m, layer.base_texel_m);
            let (sampled, sampled_gradient) = sample_triplanar(
                self.macro_pyramid.grid(mip),
                macro_kernel,
                n,
                weights,
                weight_gradients,
                layer.frequency,
                &mut NoopProfileObserver,
            );
            height += (sampled - 0.5) * layer.amplitude_m;
            gradient += sampled_gradient * layer.amplitude_m;
            if index == 0 {
                coarse = sampled;
                coarse_gradient = sampled_gradient;
            }
        }
        for detail in &self.detail {
            let mip = detail.pyramid.mip_for(texel_m, detail.layer.base_texel_m);
            let (sampled, sampled_gradient) = sample_triplanar(
                detail.pyramid.grid(mip),
                kernel(detail.cubic_bspline),
                n,
                weights,
                weight_gradients,
                detail.layer.frequency,
                &mut NoopProfileObserver,
            );
            let centered = sampled - 0.5;
            let geology = 0.35 + 0.65 * coarse;
            height += centered * detail.layer.amplitude_m * geology;
            gradient += (sampled_gradient * geology + coarse_gradient * (centered * 0.65))
                * detail.layer.amplitude_m;
        }
        Ok((height, gradient))
    }
}

fn kernel(cubic_bspline: bool) -> ProfileKernel {
    if cubic_bspline {
        ProfileKernel::CubicBSpline
    } else {
        ProfileKernel::Smoothstep
    }
}

impl SurfaceGenerator {
    /// Build the derived band-limited production recipe. Supported for spherical
    /// MoonProfileV1 and MoonFieldsV1 bodies; other algorithms and body shapes
    /// return `InvalidConfig`. Building a profile pyramid costs one pass over
    /// the source samples.
    pub fn producer_recipe(&self) -> Result<ProducerRecipe, TerrainError> {
        if !self.definition.shape.is_sphere() {
            return Err(TerrainError::InvalidConfig);
        }
        let radius_m = self.radius_m;
        match &self.field {
            GeologicalField::MoonFields(field) => {
                Ok(ProducerRecipe::Fields(Box::new(FieldsRecipe {
                    field: field.clone(),
                    detail_noise: self.detail.clone(),
                })))
            }
            GeologicalField::MoonProfile(field) => {
                let profile = &field.profile;
                let pyramid = Arc::new(ProfilePyramid::build(
                    profile.shared_values(),
                    profile.width(),
                    profile.height(),
                    profile.sample_range(),
                ));
                let width = f64::from(profile.width());
                let macro_layers = match profile.terrain_scale() {
                    Some((footprint_m, amplitude_m)) => vec![ProfileLayer {
                        frequency: 2.0 * radius_m / footprint_m,
                        amplitude_m,
                        base_texel_m: footprint_m / width,
                    }],
                    None => PROFILE_BANDS
                        .iter()
                        .map(|&(frequency, amplitude)| ProfileLayer {
                            frequency,
                            amplitude_m: radius_m * amplitude,
                            base_texel_m: 2.0 * radius_m / (frequency * width),
                        })
                        .collect(),
                };
                let mut detail = Vec::with_capacity(profile.detail_layer_count());
                for index in 0..profile.detail_layer_count() {
                    let layer = profile
                        .detail_layer(index)
                        .ok_or(TerrainError::InvalidConfig)?;
                    let source = layer.profile();
                    let layer_pyramid =
                        if Arc::ptr_eq(source.shared_values(), profile.shared_values())
                            && source.sample_range() == profile.sample_range()
                            && source.width() == profile.width()
                            && source.height() == profile.height()
                        {
                            Arc::clone(&pyramid)
                        } else {
                            Arc::new(ProfilePyramid::build(
                                source.shared_values(),
                                source.width(),
                                source.height(),
                                source.sample_range(),
                            ))
                        };
                    detail.push(ProfileDetailRecipe {
                        layer: ProfileLayer {
                            frequency: 2.0 * radius_m / layer.footprint_m(),
                            amplitude_m: layer.amplitude_m(),
                            base_texel_m: layer.footprint_m() / f64::from(source.width()),
                        },
                        cubic_bspline: source.is_cubic_bspline(),
                        pyramid: layer_pyramid,
                    });
                }
                Ok(ProducerRecipe::Profile(ProfileRecipe {
                    radius_m,
                    cubic_bspline: profile.is_cubic_bspline(),
                    macro_pyramid: pyramid,
                    macro_layers,
                    detail,
                    detail_noise: self.detail.clone(),
                }))
            }
            GeologicalField::World(field) => Ok(ProducerRecipe::World(Box::new(WorldRecipe {
                field: field.clone(),
                radius_m,
                detail_noise: self.detail.clone(),
            }))),
            _ => Err(TerrainError::InvalidConfig),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{
        SurfaceAlgorithm, SurfaceDefinition, TerrainHeightProfile, TerrainIdentity, TerrainSeed,
    };
    use astrum_math::{Direction3, surface::SurfaceLocation};

    fn profile_generator() -> SurfaceGenerator {
        let side = 64_u32;
        let mut bytes = Vec::with_capacity((side * side * 2) as usize);
        for y in 0..side {
            for x in 0..side {
                let fx = std::f64::consts::TAU * f64::from(x) / f64::from(side);
                let fy = std::f64::consts::TAU * f64::from(y) / f64::from(side);
                let value =
                    (0.5 + 0.2 * (3.0 * fx).sin() * fy.cos() + 0.2 * (17.0 * fy).sin()) * 65535.0;
                bytes.extend_from_slice(&(value.round() as u16).to_le_bytes());
            }
        }
        let source = TerrainHeightProfile::from_u16_le(side, side, &bytes)
            .unwrap()
            .with_cubic_bspline();
        let profile = source
            .clone()
            .with_terrain_scale(4000.0, 600.0)
            .unwrap()
            .with_detail_layer(source, 64.0, 1.5)
            .unwrap();
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x1234),
            TerrainSeed(0x5eed),
            SurfaceAlgorithm::MoonProfileV1,
        )
        .with_height_profile(profile)
        .unwrap();
        SurfaceGenerator::new(&definition, 109_081.776_801_130_12).unwrap()
    }

    fn fields_generator() -> SurfaceGenerator {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(0x4321),
            TerrainSeed(7),
            SurfaceAlgorithm::MoonFieldsV1,
        );
        SurfaceGenerator::new(&definition, 140_000.0).unwrap()
    }

    fn with_detail(generator: SurfaceGenerator) -> SurfaceGenerator {
        let definition = generator
            .definition
            .clone()
            .with_detail_noise(crate::terrain::noise::tests::moon_like())
            .unwrap();
        SurfaceGenerator::new(&definition, generator.radius_m).unwrap()
    }

    fn generators() -> [SurfaceGenerator; 4] {
        [
            profile_generator(),
            fields_generator(),
            with_detail(profile_generator()),
            with_detail(fields_generator()),
        ]
    }

    fn directions() -> Vec<DVec3> {
        [
            DVec3::new(0.3, 0.2, 0.93),
            DVec3::new(-0.7, 0.1, 0.2),
            DVec3::new(0.577, -0.577, 0.577),
            DVec3::new(1.0, 1e-7, 0.0),
        ]
        .into_iter()
        .map(DVec3::normalize)
        .collect()
    }

    #[test]
    fn fine_footprint_reproduces_the_complete_authority() {
        for generator in generators() {
            let recipe = generator.producer_recipe().unwrap();
            for n in directions() {
                let reference = generator
                    .evaluate_point(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
                    .unwrap();
                // Below every band's fade start and the profile detail spacing.
                let derived = recipe.evaluate(n, 1.0e-4).unwrap();
                let height = reference.terrain().height_m();
                assert!(
                    (derived.height_m - height).abs() <= 1.0e-9 * (1.0 + height.abs()),
                    "{} vs {height}",
                    derived.height_m
                );
                assert!(derived.normal.dot(reference.normal()) > 1.0 - 1.0e-12);
            }
        }
    }

    #[test]
    fn coarse_footprints_remove_unresolvable_detail_and_stay_bounded() {
        for generator in generators() {
            let recipe = generator.producer_recipe().unwrap();
            let bound = generator.height_bound_m;
            for n in directions() {
                for texel_m in [0.01, 1.0, 64.0, 4096.0] {
                    let sample = recipe.evaluate(n, texel_m).unwrap();
                    assert!(sample.height_m.abs() <= bound);
                    assert!(sample.normal.is_normalized());
                }
            }
        }
    }

    #[test]
    fn band_weights_fade_monotonically_and_pyramids_average() {
        assert_eq!(band_weight(0.5, 8.0), 1.0);
        assert_eq!(band_weight(2.0, 8.0), 0.0);
        assert!(band_weight(1.5, 8.0) > 0.0 && band_weight(1.5, 8.0) < 1.0);
        let ProducerRecipe::Profile(recipe) = profile_generator().producer_recipe().unwrap() else {
            panic!("profile recipe expected");
        };
        let levels = recipe.macro_pyramid.levels();
        assert_eq!(levels.len(), 5); // 64, 32, 16, 8, 4
        assert!(Arc::ptr_eq(
            &recipe.macro_pyramid,
            &recipe.detail[0].pyramid
        ));
        let fine = &levels[0];
        let coarse = &levels[1];
        let a = u32::from(fine.values()[0])
            + u32::from(fine.values()[1])
            + u32::from(fine.values()[64])
            + u32::from(fine.values()[65]);
        assert_eq!(u32::from(coarse.values()[0]), (a + 2) / 4);
        let base = recipe.macro_layers[0].base_texel_m;
        assert_eq!(recipe.macro_pyramid.mip_for(base * 0.5, base), 0);
        assert_eq!(recipe.macro_pyramid.mip_for(base * 2.0, base), 1);
        assert_eq!(recipe.macro_pyramid.mip_for(base * 1.0e9, base), 4);
    }
}
