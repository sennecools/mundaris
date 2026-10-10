//! Adapts world band-limited recipes to the renderer's GPU producer inputs.
//! All tile-relative values are prepared in f64 and narrowed once.
use super::select::NodeChart;
use anyhow::{Result, ensure};
use astrum_math::surface::{CubeFace, CubePatchAddress};
use astrum_renderer::{
    AtlasChart, AtlasFieldsConstants, AtlasImageLevel, AtlasLadderLayer, AtlasOctaves,
    AtlasProfileLayer, AtlasSource, AtlasTileKind, AtlasWorldSource,
};
use astrum_world::terrain::noise::DetailNoise;
use astrum_world::terrain::producer::{
    FieldsRecipe, MOON_FIELD_JITTER, MOON_FIELD_LAYOUT_SHIFTS, MOON_FIELD_SHELL,
    MOON_FIELD_SUPPORT, ProducerRecipe, ProfileLayer, ProfilePyramid, ProfileRecipe,
};
use glam::{DVec2, DVec3};
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
        ProducerRecipe::World(world) => AtlasSource::World(Box::new(AtlasWorldSource {
            bake: super::tier_a::bake_inputs(world.field.inputs()),
            surface: super::tier_a::surface(&world.field)?,
            // PROTOTYPE (M3 Water): bodies with macro erosion get rivers.
            hydrology: world
                .field
                .inputs()
                .has(astrum_world::terrain::archetype::TierAStage::Erosion),
        })),
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

/// Flat-water look of a world map with a sea level (drawn over ground below
/// it); `None` for bodies without an ocean.
pub fn water_look(recipe: &ProducerRecipe) -> Option<astrum_renderer::AtlasWater> {
    let ProducerRecipe::World(world) = recipe else {
        return None;
    };
    let sea = astrum_world::terrain::archetype::TierAStage::SeaLevel;
    if !world.field.inputs().stages.contains(&sea) {
        return None;
    }
    let look = world.field.look();
    Some(astrum_renderer::AtlasWater {
        shallow: look.water_shallow.map(|c| c as f32),
        deep: look.water_deep.map(|c| c as f32),
        depth_scale_m: look.water_depth_scale_m as f32,
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
        ProducerRecipe::World(world) => {
            let cells = world.field.inputs().face_cells as u32;
            let layout = astrum_renderer::tier_a::field_mip_layout(cells);
            let base_texel = 2.0 * world.radius_m / f64::from(cells);
            let level =
                astrum_world::terrain::world_field::mip_for(texel_m, base_texel, layout.len());
            let (mip_offset, mip_cells) = layout[level];
            let map = world_texel_map(address, chart, mip_cells);
            let whole = map.origin.floor();
            ensure!(
                whole.abs().max_element() < f64::from(i32::MAX / 2),
                "world-map texel origin exceeds the GPU producer's integer range"
            );
            // Coast coverage: relief this tile leaves out, per landform lane
            // (0..3) and of the detail noise (4).
            let mut coast_unresolved_m = [0.0f32; 5];
            if let Some(landforms) = world.field.landforms() {
                for (lane, (landform, params)) in landforms
                    .set
                    .landforms()
                    .iter()
                    .zip(&landforms.params)
                    .take(4)
                    .enumerate()
                {
                    coast_unresolved_m[lane] =
                        landform.program.unresolved_bound_m(params, texel_m) as f32;
                }
            }
            coast_unresolved_m[4] = world
                .detail_noise
                .as_ref()
                .map_or(0.0, |n| n.unresolved_bound_m(texel_m) as f32);
            Ok(AtlasTileKind::World {
                mip_offset,
                mip_cells,
                base_cells: cells,
                face: map.face as u32,
                texel_origin: [whole.x as i32, whole.y as i32],
                texel_fraction: (map.origin - whole).as_vec2().to_array(),
                texel_jacobian: map.jacobian.map(|column| column.as_vec2().to_array()),
                coast_unresolved_m,
            })
        }
    }
}

/// Affine map from a tile's chart coordinate st to texel coordinates on one
/// face of a world-map mip (`world_map::CubeMap` convention: face number in
/// `CubeFace::ALL` order, `x = (u + 1)/2 · n − 0.5`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldTexelMap {
    pub face: usize,
    /// Texel coordinate at st = (0, 0).
    pub origin: DVec2,
    /// Columns d texel / d s and d texel / d t.
    pub jacobian: [DVec2; 2],
}

impl WorldTexelMap {
    pub fn at(&self, st: [f64; 2]) -> DVec2 {
        self.origin + self.jacobian[0] * st[0] + self.jacobian[1] * st[1]
    }
}

/// Texel map of `chart`'s points on its own cube face of a world-map mip with
/// `mip_cells` face cells, in f64 (pipeline §4.4). The world map's faces and
/// bases are the tile charts' (`CubeFace::basis`), and both charts are
/// gnomonic: q(st) = q0 + U (s − ½) w + V (t − ½) w has q·N = 1 on the face, so
/// u = q·U is affine in st and the map is exact wherever the direction lies on
/// that face (the GPU blends the neighbouring face within half a texel of its
/// edges).
pub fn world_texel_map(
    address: CubePatchAddress,
    chart: &NodeChart,
    mip_cells: u32,
) -> WorldTexelMap {
    let face = address.face();
    let index = CubeFace::ALL
        .iter()
        .position(|&f| f == face)
        .expect("CubeFace::ALL lists every face");
    let [normal, u_axis, v_axis] = face.basis();
    let q0 = chart.n0 * chart.q0_length;
    // Exact zeros: the chart steps along this face's own axes.
    debug_assert!(chart.face_u.dot(normal) == 0.0 && chart.face_v.dot(normal) == 0.0);
    let half_n = 0.5 * f64::from(mip_cells);
    let scale = half_n * chart.width / q0.dot(normal);
    let column = |axis: DVec3| DVec2::new(axis.dot(u_axis), axis.dot(v_axis)) * scale;
    let jacobian = [column(chart.face_u), column(chart.face_v)];
    let centre = DVec2::new(q0.dot(u_axis), q0.dot(v_axis)) / q0.dot(normal);
    let origin =
        (centre + DVec2::ONE) * half_n - DVec2::splat(0.5) - (jacobian[0] + jacobian[1]) * 0.5;
    WorldTexelMap {
        face: index,
        origin,
        jacobian,
    }
}

/// Noise octaves of one node (M2 design §3): fixed-point ladder anchors of
/// the exact chart centre `n0 * R`, computed in f64. The GPU derives every
/// octave's lattice cell and fraction from them and adds `diff * R`, its
/// chart-relative offset from that same centre, so f32 never holds an
/// absolute lattice coordinate. Band-limit weights at the node's texel are
/// computed on the GPU.
pub fn detail_octaves(
    recipe: &ProducerRecipe,
    address: CubePatchAddress,
    chart: &NodeChart,
    cells: u32,
) -> Result<AtlasOctaves> {
    let texel_m =
        astrum_world::terrain::producer::tile_texel_m(recipe.radius_m(), address.level(), cells);
    let climate = match recipe {
        ProducerRecipe::World(world) => world.field.look().climate.as_ref().map(|c| &c.noise),
        _ => None,
    };
    let mut octaves = AtlasOctaves {
        texel_m: texel_m as f32,
        detail: ladder_layer(recipe.detail_noise()),
        climate: ladder_layer(climate),
        ..Default::default()
    };
    if octaves.detail.octaves + octaves.climate.octaves > 0 {
        let anchors = astrum_world::terrain::ladder::tile_anchors(chart.n0 * recipe.radius_m())
            .map_err(|_| anyhow::anyhow!("noise anchors exceed the GPU integer range"))?;
        octaves.anchor_cells = anchors.cells;
        octaves.anchor_residuals = anchors.residual;
    }
    Ok(octaves)
}

/// GPU description of a compiled ladder fBm layer (empty without one).
fn ladder_layer(noise: Option<&DetailNoise>) -> AtlasLadderLayer {
    let Some(noise) = noise else {
        return AtlasLadderLayer::default();
    };
    let Some(first) = noise.octaves().first() else {
        return AtlasLadderLayer::default();
    };
    AtlasLadderLayer {
        salt: noise.salt(),
        ladder: u32::from(first.octave.ladder),
        first_level: first.octave.level,
        octaves: noise.octaves().len() as u32,
        amplitude_m: noise.amplitude_m() as f32,
        gain: noise.gain() as f32,
    }
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
        ProducerRecipe::Fields(_) | ProducerRecipe::World(_) => generator_bound_m,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planet_lod::select::chart;
    use astrum_world::terrain::world_map::locate;

    /// Tiles on every face at several levels, including face corners.
    fn tiles() -> Vec<CubePatchAddress> {
        let mut out = Vec::new();
        let mut seed = 0x7e1_5eed_u64;
        let mut next = |count: u32| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % u64::from(count)) as u32
        };
        for face in CubeFace::ALL {
            for level in [0u8, 1, 3, 7, 12, 17, 20] {
                let count = 1u32 << level;
                for (x, y) in [(0, 0), (count - 1, count - 1), (next(count), next(count))] {
                    out.push(CubePatchAddress::try_new(face, level, x, y).unwrap());
                }
            }
        }
        out
    }

    #[test]
    fn world_texel_map_reproduces_cube_map_locate() {
        let mut checked = 0;
        for address in tiles() {
            let chart = chart(address);
            for mip_cells in [4u32, 512, 4096] {
                let map = world_texel_map(address, &chart, mip_cells);
                let n = f64::from(mip_cells);
                for j in 0..=8 {
                    for i in 0..=8 {
                        let st = [f64::from(i) / 8.0, f64::from(j) / 8.0];
                        let (face, u, v) = locate(chart.direction(st));
                        if face != map.face {
                            // Only on the face edge itself (ties to the earlier face).
                            assert!(i == 0 || i == 8 || j == 0 || j == 8);
                            continue;
                        }
                        let texel = DVec2::new(u + 1.0, v + 1.0) * (0.5 * n) - DVec2::splat(0.5);
                        let error = (map.at(st) - texel).abs().max_element();
                        assert!(
                            error <= 1e-9,
                            "{address:?} n {mip_cells} st {st:?}: {error} texel"
                        );
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 6 * 7 * 3 * 3 * 49);
    }

    /// f32 replica of the GPU lookups: the absolute path (chart_point,
    /// normalize, cube_locate) against the CPU split (integer origin + f32
    /// fraction + f32 Jacobian · st), both measured against the f64 map.
    #[test]
    fn split_texel_lookup_keeps_f32_precision_at_deep_levels() {
        use glam::{Vec2, Vec3};
        let cells = 64u32;
        let sizes = [512u32, 4096, 16384];
        let mut worst = [(0.0f64, 0.0f64); 3];
        for address in tiles().into_iter().filter(|a| a.level() >= 12) {
            let chart = chart(address);
            let gpu = atlas_chart(&chart);
            let n0 = Vec3::from_array(gpu.n0);
            let face_u = Vec3::from_array(gpu.face_u);
            let face_v = Vec3::from_array(gpu.face_v);
            let [normal, u_axis, v_axis] = address.face().basis().map(|a| a.as_vec3());
            for (slot, mip_cells) in sizes.into_iter().enumerate() {
                let map = world_texel_map(address, &chart, mip_cells);
                let whole = map.origin.floor();
                let fraction = (map.origin - whole).as_vec2();
                let jacobian = map.jacobian.map(|c| c.as_vec2());
                let n = mip_cells as f32;
                for j in 0..=cells + 2 {
                    for i in (0..=cells + 2).step_by(5) {
                        let st = (Vec2::new(i as f32, j as f32) - Vec2::ONE) / cells as f32;
                        let truth = map.at([f64::from(st.x), f64::from(st.y)]);
                        // terrain_atlas_produce.wgsl chart_point + cube_locate.
                        let tangent = (face_u * ((st.x - 0.5) * gpu.width)
                            + face_v * ((st.y - 0.5) * gpu.width))
                            / gpu.q0_length;
                        let s = 2.0 * n0.dot(tangent) + tangent.dot(tangent);
                        let root = (1.0 + s).sqrt();
                        let k = 1.0 / root;
                        let diff = tangent * k - n0 * (s * k / (1.0 + root));
                        let d = (n0 + diff).normalize();
                        let w = d.dot(normal);
                        let uv = Vec2::new(d.dot(u_axis) / w, d.dot(v_axis) / w);
                        let absolute = (uv + Vec2::ONE) * 0.5 * n - Vec2::splat(0.5);
                        let absolute_error = (absolute.as_dvec2() - truth).abs().max_element();
                        // cube_bicubic_split's coordinate: cell + (fraction + J st).
                        let local = fraction + jacobian[0] * st.x + jacobian[1] * st.y;
                        let split_error = (whole + local.as_dvec2() - truth).abs().max_element();
                        worst[slot].0 = worst[slot].0.max(absolute_error);
                        worst[slot].1 = worst[slot].1.max(split_error);
                    }
                }
            }
        }
        for (mip_cells, (absolute, split)) in sizes.iter().zip(worst) {
            println!("n {mip_cells}: absolute {absolute:.3e} texel, split {split:.3e} texel");
            assert!(split < 1e-6, "split {split} texel at n {mip_cells}");
        }
        // The absolute path's error grows with n; the split's does not.
        assert!(worst[2].0 > 1e-4, "absolute {} texel", worst[2].0);
        assert!(worst[2].0 > 100.0 * worst[2].1);
    }
}
