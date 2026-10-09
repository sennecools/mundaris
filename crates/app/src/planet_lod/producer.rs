//! Adapts world band-limited recipes to the renderer's GPU producer inputs.
//! All tile-relative values are prepared in f64 and narrowed once.
use super::select::NodeChart;
use anyhow::{Result, ensure};
use astrum_math::surface::CubePatchAddress;
use astrum_renderer::{
    AtlasChart, AtlasFieldsConstants, AtlasImageLevel, AtlasOctave, AtlasProfileLayer, AtlasSource,
    AtlasTileKind, MAX_ATLAS_OCTAVES,
};
use astrum_world::terrain::producer::{
    FieldsRecipe, MOON_FIELD_JITTER, MOON_FIELD_LAYOUT_SHIFTS, MOON_FIELD_SHELL,
    MOON_FIELD_SUPPORT, ProducerRecipe, ProfileLayer, ProfilePyramid, ProfileRecipe,
};
use glam::DVec3;
use std::sync::Arc;

fn image(pyramid: &ProfilePyramid) -> Vec<AtlasImageLevel> {
    pyramid
        .levels()
        .iter()
        .map(|level| AtlasImageLevel {
            width: level.width(),
            height: level.height(),
            values: Arc::from(level.values()),
        })
        .collect()
}

/// Immutable GPU source for one bound body recipe.
pub fn atlas_source(recipe: &ProducerRecipe) -> Result<AtlasSource> {
    Ok(match recipe {
        ProducerRecipe::Profile(profile) => {
            let first = &profile.macro_pyramid.levels()[0];
            ensure!(
                first.width() == first.height(),
                "GPU profile producer requires a square profile"
            );
            ensure!(
                profile.macro_layers.len() + profile.detail.len() <= 5,
                "GPU profile producer supports at most five layers"
            );
            let mut detail_levels = None;
            for detail in &profile.detail {
                if !Arc::ptr_eq(&detail.pyramid, &profile.macro_pyramid) {
                    ensure!(
                        detail_levels.is_none(),
                        "GPU profile producer supports one distinct detail image"
                    );
                    ensure!(
                        detail.pyramid.sample_range() == profile.macro_pyramid.sample_range(),
                        "GPU profile producer requires a shared detail sample range"
                    );
                    detail_levels = Some(image(&detail.pyramid));
                }
            }
            AtlasSource::Profile {
                macro_levels: image(&profile.macro_pyramid),
                detail_levels,
                sample_range: profile.macro_pyramid.sample_range(),
            }
        }
        ProducerRecipe::Fields(fields) => AtlasSource::Fields(Box::new(fields_constants(fields))),
    })
}

fn v3(v: DVec3) -> [f32; 3] {
    v.as_vec3().to_array()
}

fn fields_constants(recipe: &FieldsRecipe) -> AtlasFieldsConstants {
    let definition = recipe.definition();
    let radius = recipe.radius_m();
    let relief = radius * recipe.relief_fraction();
    let rotation = recipe.rotation();
    let profile = definition.crater_profile;
    AtlasFieldsConstants {
        axes: recipe.axes().map(v3),
        basins: recipe.basins().map(v3),
        basin_params: recipe
            .basin_parameters()
            .map(|[scale, depth]| [scale as f32, depth as f32]),
        rotation_columns: [
            v3(rotation.x_axis),
            v3(rotation.y_axis),
            v3(rotation.z_axis),
        ],
        structure_weights: definition.structure_weights.map(|w| w as f32),
        plains: [
            definition.plains_offset as f32,
            definition.plains_low as f32,
            definition.plains_high as f32,
            definition.basin_rim_strength as f32,
        ],
        relief: [
            (relief * definition.global_relief_weights[0]) as f32,
            (relief * definition.global_relief_weights[1]) as f32,
            (relief * definition.global_relief_weights[2]) as f32,
            radius as f32,
        ],
        bands: definition.bands.map(|band| {
            [
                band.edge_m as f32,
                band.height_budget_m as f32,
                (band.edge_m * MOON_FIELD_SUPPORT) as f32,
            ]
        }),
        salts: std::array::from_fn(|band| {
            [recipe.lattice_salt(band, 0), recipe.lattice_salt(band, 1)]
        }),
        regional_strength: [
            definition.crater_regional_strength.start as f32,
            definition.crater_regional_strength.span as f32,
        ],
        crater: [
            profile.bowl_base,
            profile.bowl_freshness,
            profile.rim_base,
            profile.rim_freshness,
            profile.ejecta_base,
            profile.ejecta_freshness,
            profile.degradation_offset,
            profile.degradation_low,
            profile.degradation_high,
            profile.degradation_strength,
            MOON_FIELD_JITTER,
            MOON_FIELD_SHELL,
        ]
        .map(|value| value as f32),
    }
}

pub fn atlas_chart(chart: &NodeChart) -> AtlasChart {
    AtlasChart {
        n0: v3(chart.n0),
        q0_length: chart.q0_length as f32,
        face_u: v3(chart.face_u),
        width: chart.width as f32,
        face_v: v3(chart.face_v),
    }
}

fn profile_layer(
    layer: &ProfileLayer,
    pyramid: &ProfilePyramid,
    cubic: bool,
    n0: DVec3,
    texel_m: f64,
) -> AtlasProfileLayer {
    let mip = pyramid.mip_for(texel_m, layer.base_texel_m);
    let width = pyramid.levels()[mip as usize].width();
    let w = f64::from(width);
    let k = 0.5 * layer.frequency * w;
    let origin = n0
        .to_array()
        .map(|c| ((c * 0.5 + 0.5) * layer.frequency * w).rem_euclid(w) as f32);
    AtlasProfileLayer {
        origin,
        scale: k as f32,
        mip,
        width,
        amplitude_m: layer.amplitude_m as f32,
        cubic,
    }
}

fn profile_kind(recipe: &ProfileRecipe, n0: DVec3, texel_m: f64) -> AtlasTileKind {
    AtlasTileKind::Profile {
        macro_layers: recipe
            .macro_layers
            .iter()
            .map(|layer| {
                profile_layer(
                    layer,
                    &recipe.macro_pyramid,
                    recipe.cubic_bspline,
                    n0,
                    texel_m,
                )
            })
            .collect(),
        detail_layers: recipe
            .detail
            .iter()
            .map(|detail| {
                profile_layer(
                    &detail.layer,
                    &detail.pyramid,
                    detail.cubic_bspline,
                    n0,
                    texel_m,
                )
            })
            .collect(),
    }
}

fn fields_kind(recipe: &FieldsRecipe, n0: DVec3, texel_m: f64) -> Result<AtlasTileKind> {
    let origin = recipe.rotation().transpose() * n0 * recipe.radius_m();
    let definition = recipe.definition();
    let mut cells = [[0i32; 3]; 6];
    let mut fractions = [[0.0f32; 3]; 6];
    for band in 0..3 {
        let edge = definition.bands[band].edge_m;
        for (layout, shift) in MOON_FIELD_LAYOUT_SHIFTS.iter().enumerate() {
            let scaled = origin / edge - *shift;
            let base = scaled.floor();
            ensure!(
                base.abs().max_element() < f64::from(i32::MAX / 2),
                "crater lattice exceeds the GPU producer's integer range"
            );
            cells[band * 2 + layout] = base.to_array().map(|value| value as i32);
            fractions[band * 2 + layout] = v3(scaled - base);
        }
    }
    Ok(AtlasTileKind::Fields {
        cells,
        fractions,
        band_weights: recipe.band_weights(texel_m).map(|w| w as f32),
    })
}

/// Producer parameters of one node at its nominal texel footprint.
pub fn tile_kind(
    recipe: &ProducerRecipe,
    address: CubePatchAddress,
    chart: &NodeChart,
    cells: u32,
) -> Result<AtlasTileKind> {
    let texel_m =
        astrum_world::terrain::producer::tile_texel_m(recipe.radius_m(), address.level(), cells);
    match recipe {
        ProducerRecipe::Profile(profile) => Ok(profile_kind(profile, chart.n0, texel_m)),
        ProducerRecipe::Fields(fields) => fields_kind(fields, chart.n0, texel_m),
    }
}

/// Detail-noise octave origins of one node, split in f64 at the exact chart
/// centre `n0 * R` (pipeline §4.4). The GPU adds `diff * R`, its chart-relative
/// offset from that same centre, so f32 never holds an absolute lattice
/// coordinate. Octaves above the node's band limit are omitted.
pub fn detail_octaves(
    recipe: &ProducerRecipe,
    address: CubePatchAddress,
    chart: &NodeChart,
    cells: u32,
) -> Result<Vec<AtlasOctave>> {
    let Some(detail) = recipe.detail_noise() else {
        return Ok(Vec::new());
    };
    let texel_m =
        astrum_world::terrain::producer::tile_texel_m(recipe.radius_m(), address.level(), cells);
    let origins = detail
        .split(chart.n0 * recipe.radius_m(), texel_m)
        .map_err(|_| anyhow::anyhow!("detail noise lattice exceeds the GPU integer range"))?;
    ensure!(
        origins.len() <= MAX_ATLAS_OCTAVES,
        "detail noise has more octaves than the GPU producer supports"
    );
    Ok(origins
        .into_iter()
        .map(|o| AtlasOctave {
            cell: o.cell,
            seed: o.seed,
            fraction: o.fraction,
            frequency_per_m: o.frequency_per_m,
            amplitude_m: o.amplitude_m,
        })
        .collect())
}

/// Conservative absolute radial-offset bound of a recipe, used before any
/// produced bounds are known.
pub fn recipe_height_bound(recipe: &ProducerRecipe, generator_bound_m: f64) -> f64 {
    match recipe {
        ProducerRecipe::Profile(profile) => {
            let macro_bound: f64 = profile
                .macro_layers
                .iter()
                .map(|l| l.amplitude_m * 0.5)
                .sum();
            let detail_bound: f64 = profile
                .detail
                .iter()
                .map(|d| d.layer.amplitude_m * 0.5)
                .sum();
            let noise_bound = recipe
                .detail_noise()
                .map_or(0.0, astrum_world::terrain::noise::DetailNoise::bound_m);
            (macro_bound + detail_bound + noise_bound).min(generator_bound_m)
        }
        ProducerRecipe::Fields(_) => generator_bound_m,
    }
}
