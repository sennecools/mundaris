//! GPU terrain authority vs the CPU test oracle (ADR 0023; pipeline §19.3, §20
//! step 3). Runs by default on a hardware or software adapter.
//!
//! Tolerances:
//! - Height: the producer evaluates in f32 relative to each tile's chart
//!   centre. f32 storage contributes ~1e-7 |h|. The f32 chart arithmetic places
//!   samples with a second-order position error of about |diff| * 6e-8 * R,
//!   which grows with tile size: on coarse tiles (levels 2-3, ~400 m texels)
//!   it reaches a few mm on steep slopes, about 1e-5 of a texel. Allowed:
//!   max(1 mm, 1e-5 texel) + 1e-6 |h|.
//!   World-map (WorldV1) bodies look up macro elevation at a texel coordinate
//!   split on the CPU in f64 (integer origin + f32 offset, affine in st), so
//!   no term for absolute f32 direction rounding (formerly 1.2e-7 R · slope)
//!   is allowed. Within half a world-map texel of a cube-face edge the
//!   neighbouring face's blend contribution still uses the f32 direction;
//!   that residual (≤ slope × 1.2e-7 R × its blend weight) fits the budget
//!   above on the canonical bodies.
//! - Normals: stored as rgba8snorm (1/127 per component, up to ~0.45 deg of
//!   quantisation) plus f32 gradient error; 0.75 deg.
mod common;

use astrum_app::{planet_lod::select, shared_system::SharedTestSystem};
use astrum_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, SurfaceLocation},
};
use astrum_renderer::TerrainAtlasConfig;
use astrum_world::terrain::{SurfaceGenerator, noise, producer::tile_texel_m};
use glam::DVec3;

const HEIGHT_TOLERANCE_M: f64 = 1.0e-3;
const HEIGHT_TOLERANCE_RELATIVE: f64 = 1.0e-6;
const HEIGHT_TOLERANCE_TEXEL_FRACTION: f64 = 1.0e-5;
const NORMAL_TOLERANCE_DEG: f64 = 0.75;

fn node_on_path(direction: DVec3, level: u8) -> CubePatchAddress {
    let (face, uv) = SurfaceLocation::new(Direction3::try_new(direction).unwrap()).face_uv();
    let count = 1u32 << level;
    let index = |c: f64| (((c + 1.0) * 0.5 * f64::from(count)) as u32).min(count - 1);
    CubePatchAddress::try_new(face, level, index(uv[0]), index(uv[1])).unwrap()
}

/// About 64 nodes: roots, cube-face corners, the canonical camera path at every
/// fourth data level down to the finest, and seeded random nodes.
fn node_set(camera: DVec3, finest_data_level: u8) -> Vec<CubePatchAddress> {
    let mut nodes: Vec<_> = CubeFace::ALL
        .iter()
        .map(|&face| CubePatchAddress::root(face))
        .collect();
    for level in [2u8, 5, 9] {
        let last = (1u32 << level) - 1;
        for (face, x, y) in [
            (CubeFace::NegativeX, last, 0),
            (CubeFace::PositiveY, 0, last),
            (CubeFace::PositiveZ, last, last),
            (CubeFace::NegativeZ, 0, 0),
        ] {
            nodes.push(CubePatchAddress::try_new(face, level, x, y).unwrap());
        }
    }
    let mut level = 0;
    while level <= finest_data_level {
        nodes.push(node_on_path(camera, level));
        level += 2;
    }
    nodes.push(node_on_path(camera, finest_data_level));
    let mut rng = common::Rng(0x5eed_0123_4567_89ab);
    while nodes.len() < 64 {
        let level = rng.below(u32::from(finest_data_level) + 1) as u8;
        let count = 1u32 << level;
        let face = CubeFace::ALL[rng.below(6) as usize];
        nodes.push(
            CubePatchAddress::try_new(face, level, rng.below(count), rng.below(count)).unwrap(),
        );
    }
    nodes.sort_by_key(|n| (n.level(), n.face() as u8, n.coordinates()));
    nodes.dedup();
    nodes
}

/// Texel indices sampled along one axis: a stride plus both chart edges.
fn sampled(side: usize, cells: usize, stride: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..side).step_by(stride).collect();
    indices.extend([1, cells + 1]);
    indices.sort_unstable();
    indices.dedup();
    indices
}

#[test]
fn gpu_tiles_match_the_cpu_oracle_within_documented_tolerances() {
    let Some(context) = common::gpu() else {
        return;
    };
    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(5).unwrap()).unwrap();
    let policy = loaded.lod;
    // ASTRUM_ORACLE_DIRECTION="x,y,z" checks the node set around another
    // direction (e.g. a mountain range) instead of the canonical camera.
    let camera = std::env::var("ASTRUM_ORACLE_DIRECTION").ok().map_or_else(
        || DVec3::from_array(loaded.camera.position_body_m).normalize(),
        |v| {
            let c: Vec<f64> = v.split(',').map(|x| x.trim().parse().unwrap()).collect();
            DVec3::new(c[0], c[1], c[2]).normalize()
        },
    );
    let finest_data_level = policy.max_level - policy.data_level_offset();
    let config = TerrainAtlasConfig {
        cells: policy.tile_cells,
        draw_cells: policy.draw_cells,
        layers: 32,
        normal_scale: policy.normal_scale,
    };
    let nodes = node_set(camera, finest_data_level);
    let mut checked_bodies = 0;
    for (_, body) in loaded.system.bodies().filter(|(_, b)| b.has_surface()) {
        let definition = body.surface_definition().unwrap();
        assert!(
            definition.detail_noise().is_some(),
            "canonical content enables detail noise on {}",
            body.name()
        );
        let radius = body.properties().reference_radius_m();
        let recipe = SurfaceGenerator::new(definition, radius)
            .unwrap()
            .producer_recipe()
            .unwrap();
        common::provide_gpu_world(&context, &recipe);
        let tiles = common::produce(&context, config, &recipe, radius, &nodes);
        let (mut worst_height, mut worst_normal_1x, mut worst_normal_2x) = (0.0f64, 0.0f64, 0.0f64);
        // (ratio to tolerance, node, |dh|, slope at that texel)
        let mut height_report: Vec<(f64, CubePatchAddress, f64, f64)> = Vec::new();
        let mut failures = Vec::new();
        for (node, tile) in nodes.iter().zip(&tiles) {
            let chart = select::chart(*node);
            let texel = tile_texel_m(radius, node.level(), config.cells);
            let cells = config.cells as usize;
            let side = config.height_side() as usize;
            let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
            let mut node_height = 0.0f64;
            let mut node_ratio = (0.0f64, 0.0f64);
            for &j in &sampled(side, cells, 5) {
                for &i in &sampled(side, cells, 5) {
                    let st = [
                        (i as f64 - 1.0) / cells as f64,
                        (j as f64 - 1.0) / cells as f64,
                    ];
                    let reference = recipe.evaluate(chart.direction(st), texel).unwrap();
                    let gpu = f64::from(tile.heights[j * side + i]);
                    let error = (gpu - reference.height_m).abs();
                    let slope = reference.gradient_m.length() / radius;
                    let allowed = HEIGHT_TOLERANCE_M.max(HEIGHT_TOLERANCE_TEXEL_FRACTION * texel)
                        + HEIGHT_TOLERANCE_RELATIVE * reference.height_m.abs();
                    if error > allowed {
                        failures.push(format!(
                            "{node:?} texel ({i},{j}): |dh| {error} m > {allowed} m"
                        ));
                    }
                    if error / allowed > node_ratio.0 {
                        node_ratio = (error / allowed, slope);
                    }
                    node_height = node_height.max(error);
                    low = low.min(gpu);
                    high = high.max(gpu);
                }
            }
            worst_height = worst_height.max(node_height);
            height_report.push((node_ratio.0, *node, node_height, node_ratio.1));
            let nside = config.normal_side() as usize;
            let ncells = (config.cells * config.normal_scale) as usize;
            for &j in &sampled(nside, ncells, 7) {
                for &i in &sampled(nside, ncells, 7) {
                    let st = [
                        (i as f64 - 1.0) / ncells as f64,
                        (j as f64 - 1.0) / ncells as f64,
                    ];
                    let reference = recipe.evaluate(chart.direction(st), texel).unwrap();
                    let n = tile.normals[j * nside + i];
                    let n = DVec3::new(n[0].into(), n[1].into(), n[2].into()).normalize();
                    let degrees = n.dot(reference.normal).clamp(-1.0, 1.0).acos().to_degrees();
                    if degrees > NORMAL_TOLERANCE_DEG {
                        failures.push(format!("{node:?} normal texel ({i},{j}): {degrees} deg"));
                    }
                    // With the one-texel apron, odd 2x indices fall on the geometry
                    // grid; even ones lie between vertices (2x normal map only).
                    if i % 2 == 1 && j % 2 == 1 {
                        worst_normal_1x = worst_normal_1x.max(degrees);
                    } else {
                        worst_normal_2x = worst_normal_2x.max(degrees);
                    }
                }
            }
            let (min_m, max_m) = tile.bounds.expect("bounds readback");
            assert!(
                f64::from(min_m) <= low + 1e-3 && f64::from(max_m) >= high - 1e-3,
                "{} {node:?}: bounds [{min_m}, {max_m}] miss sampled [{low}, {high}]",
                body.name()
            );
        }
        println!(
            "{}: {} nodes, levels 0..={finest_data_level}; max |dh| {worst_height:.6} m, \
             max normal {:.3} deg (on-grid texels {worst_normal_1x:.3}, 2x-only texels {worst_normal_2x:.3})",
            body.name(),
            nodes.len(),
            worst_normal_1x.max(worst_normal_2x)
        );
        height_report.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (ratio, node, dh, slope) in height_report.iter().take(5) {
            println!(
                "  worst: {node:?} |dh| {dh:.6} m ({ratio:.2} of tolerance), slope {slope:.3}"
            );
        }
        assert!(
            failures.is_empty(),
            "{}: {} out of tolerance, first {:?}",
            body.name(),
            failures.len(),
            &failures[..failures.len().min(5)]
        );
        checked_bodies += 1;
    }
    assert_eq!(checked_bodies, 2);
}

#[test]
fn read_back_colliders_match_the_cpu_oracle_within_a_few_frames() {
    use astrum_app::planet_lod::collision::{ColliderPage, page_at};
    let Some(context) = common::gpu() else {
        return;
    };
    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(6).unwrap()).unwrap();
    let policy = loaded.lod;
    let collision = loaded.collision;
    let config = TerrainAtlasConfig {
        cells: policy.tile_cells,
        draw_cells: policy.draw_cells,
        layers: 32,
        normal_scale: policy.normal_scale,
    };
    let camera = DVec3::from_array(loaded.camera.position_body_m).normalize();
    let mut rng = common::Rng(0xc011_1de5);
    for (_, body) in loaded.system.bodies().filter(|(_, b)| b.has_surface()) {
        let radius = body.properties().reference_radius_m();
        let recipe = SurfaceGenerator::new(body.surface_definition().unwrap(), radius)
            .unwrap()
            .producer_recipe()
            .unwrap();
        common::provide_gpu_world(&context, &recipe);
        let level = collision.physics_level(radius);
        let texel = tile_texel_m(radius, level, collision.page_cells);
        // Pages under the canonical camera direction and a few random ones.
        let mut directions = vec![camera];
        for _ in 0..5 {
            let unit = |r: &mut common::Rng| (r.next() >> 11) as f64 / (1u64 << 53) as f64 - 0.5;
            directions.push(DVec3::new(unit(&mut rng), unit(&mut rng), unit(&mut rng)).normalize());
        }
        let pages: Vec<_> = directions
            .iter()
            .map(|d| page_at(*d, level).unwrap().0)
            .collect();
        let jobs = common::jobs(&recipe, radius, &pages, collision.page_cells);
        let source =
            std::sync::Arc::new(astrum_app::planet_lod::producer::atlas_source(&recipe).unwrap());
        let (read_back, submissions) = astrum_renderer::collision_for_validation(
            &context.device,
            &context.queue,
            config,
            &[(1, source)],
            &jobs,
            collision.page_cells,
        )
        .unwrap();
        // Frames of pipeline latency (each validation frame waits for its
        // GPU work, so other adapter users do not stretch it).
        assert!(
            submissions <= 4,
            "pages arrived after {submissions} submissions"
        );
        let mut worst = (0.0f64, 0.0f64);
        for (k, direction) in directions.iter().enumerate() {
            let readback = read_back.iter().find(|p| p.token == jobs[k].token).unwrap();
            let page = ColliderPage::new(
                readback.cells,
                readback.heights.clone(),
                readback.normals.clone(),
            )
            .unwrap();
            // Grid samples against the oracle at the page's texel footprint.
            let chart = select::chart(pages[k]);
            let cells = f64::from(collision.page_cells);
            for j in 0..=collision.page_cells {
                for i in (0..=collision.page_cells).step_by(3) {
                    let st = [f64::from(i) / cells, f64::from(j) / cells];
                    let reference = recipe.evaluate(chart.direction(st), texel).unwrap();
                    let gpu = page.sample(st);
                    let dh = (gpu.height_m - reference.height_m).abs();
                    assert!(
                        dh <= HEIGHT_TOLERANCE_M
                            + HEIGHT_TOLERANCE_RELATIVE * reference.height_m.abs(),
                        "{} page {:?} ({i},{j}): |dh| {dh}",
                        body.name(),
                        pages[k]
                    );
                    let degrees = gpu
                        .normal
                        .dot(reference.normal)
                        .clamp(-1.0, 1.0)
                        .acos()
                        .to_degrees();
                    // Collider normals are stored as f32, not quantised.
                    assert!(degrees <= 0.1, "{} normal {degrees} deg", body.name());
                    worst = (worst.0.max(dh), worst.1.max(degrees));
                }
            }
            // A query between samples is bilinear in the page.
            let (_, st) = page_at(*direction, level).unwrap();
            let reference = recipe.evaluate(*direction, texel).unwrap();
            let between = (page.sample(st).height_m - reference.height_m).abs();
            println!(
                "{} {:?}: query under direction |dh| {between:.4} m (bilinear between {:.3} m samples)",
                body.name(),
                pages[k],
                texel
            );
        }
        println!(
            "{}: physics level {level}, texel {texel:.3} m, {} pages in {submissions} submissions; \
             max |dh| {:.6} m, max normal {:.4} deg",
            body.name(),
            pages.len(),
            worst.0,
            worst.1
        );
    }
}

#[test]
fn gpu_lattice_hash_is_bit_identical_to_the_cpu_oracle() {
    let Some(context) = common::gpu() else {
        return;
    };
    let mut rng = common::Rng(0x0dd_c0ffee);
    let mut inputs: Vec<([i32; 3], u32)> = (0..65_536)
        .map(|_| {
            let cell = [0, 1, 2].map(|_| rng.next() as i32 >> rng.below(31));
            (cell, rng.next() as u32)
        })
        .collect();
    inputs.extend([
        ([0, 0, 0], 0),
        ([i32::MIN, i32::MAX, -1], u32::MAX),
        ([-1, -1, -1], 1),
    ]);
    let gpu =
        astrum_renderer::lattice_hash_for_validation(&context.device, &context.queue, &inputs)
            .unwrap();
    let mismatches = inputs
        .iter()
        .zip(&gpu)
        .filter(|((cell, seed), hash)| noise::lattice_bits(*cell, *seed) != **hash)
        .count();
    println!(
        "lattice hash: {} inputs, {mismatches} mismatches",
        inputs.len()
    );
    assert_eq!(mismatches, 0);
}

/// Ladder noise (M2 design §3): octave seeds, sub-cell offsets, the 4D lattice
/// hash and the integer split cells are bit-identical to the CPU; the f32
/// split fraction and the 3D/4D octave values and gradients match the f64
/// oracle at tile-sized offsets, for every ladder and level −11..=19 on tile
/// centres at Rust's radius and 1,000 km.
#[test]
fn gpu_ladder_noise_matches_the_cpu_oracle() {
    use astrum_renderer::LadderNoiseProbe;
    use astrum_world::terrain::ladder::{self, LadderOctave};
    let Some(context) = common::gpu() else {
        return;
    };
    let mut rng = common::Rng(0x1add_e125);
    let mut unit = move || (rng.next() >> 11) as f64 / (1u64 << 53) as f64;
    let mut probes = Vec::new();
    // (centre, octave, local in metres, w)
    let mut cases = Vec::new();
    for radius in [338_950.0, 1_000_000.0] {
        for _ in 0..24 {
            let z = 2.0 * unit() - 1.0;
            let a = std::f64::consts::TAU * unit();
            let r = (1.0 - z * z).sqrt();
            let centre = DVec3::new(r * a.cos(), r * a.sin(), z) * radius;
            let anchors = ladder::tile_anchors(centre).unwrap();
            for ladder_index in 0..4u8 {
                for level in ladder::MIN_LADDER_LEVEL..=ladder::MAX_LADDER_LEVEL {
                    let octave = LadderOctave {
                        ladder: ladder_index,
                        level,
                    };
                    // Band limiting keeps an octave's tile extent below ~50 cells.
                    let extent = 40.0 * octave.wavelength_m();
                    let local =
                        DVec3::new(unit() - 0.5, unit() - 0.5, 0.05 * (unit() - 0.5)) * extent;
                    let local = local.as_vec3();
                    let w = (unit() - 0.5) * 300.0;
                    let m = usize::from(ladder_index);
                    let salt = rng_word(&mut unit);
                    probes.push(LadderNoiseProbe {
                        anchor_cell: anchors.cells[m],
                        anchor_residual: anchors.residual[m],
                        ladder: u32::from(ladder_index),
                        level,
                        salt,
                        local: local.to_array(),
                        w_cell: w.floor() as i32,
                        w_frac: (w - w.floor()) as f32,
                        cell4: [0, 1, 2, 3].map(|_| rng_word(&mut unit) as i32),
                    });
                    cases.push((centre, anchors, octave, local, w));
                }
            }
        }
    }
    let gpu =
        astrum_renderer::ladder_noise_for_validation(&context.device, &context.queue, &probes)
            .unwrap();
    let (mut worst_fraction, mut worst3, mut worst4) = (0.0f64, 0.0f64, 0.0f64);
    let (mut worst_gradient, mut worst_dw) = (0.0f64, 0.0f64);
    for ((probe, result), (centre, anchors, octave, local, w)) in
        probes.iter().zip(&gpu).zip(&cases)
    {
        let seed = ladder::octave_seed(probe.salt, *octave);
        assert_eq!(result.seed, seed, "{octave:?}");
        assert_eq!(
            result.offset,
            ladder::octave_offset(seed).as_vec3().to_array()
        );
        assert_eq!(
            result.bits4,
            ladder::lattice_bits4(probe.cell4, probe.salt),
            "{probe:?}"
        );
        let (cell, fraction) = ladder::ladder_split(anchors, *octave, local.as_dvec3());
        assert_eq!(result.cell, cell, "{octave:?}");
        for axis in 0..3 {
            worst_fraction =
                worst_fraction.max((f64::from(result.fraction[axis]) - fraction[axis]).abs());
        }
        let p = *centre + local.as_dvec3();
        let f = octave.frequency_per_m();
        let (v3, g3) = ladder::octave_noise3(p, *octave, seed);
        let (v4, g4, dw) = ladder::octave_noise4(p, *w, *octave, seed);
        let gpu_g3 = DVec3::new(
            result.noise3[1].into(),
            result.noise3[2].into(),
            result.noise3[3].into(),
        );
        let gpu_g4 = DVec3::new(
            result.noise4[1].into(),
            result.noise4[2].into(),
            result.noise4[3].into(),
        );
        worst3 = worst3.max((f64::from(result.noise3[0]) - v3).abs());
        worst4 = worst4.max((f64::from(result.noise4[0]) - v4).abs());
        // Gradients in lattice units (per cell).
        worst_gradient = worst_gradient
            .max((gpu_g3 - g3).length() / f)
            .max((gpu_g4 - g4).length() / f);
        worst_dw = worst_dw.max((f64::from(result.noise4[4]) - dw).abs());
    }
    println!(
        "ladder noise: {} probes; worst split fraction {worst_fraction:.3e} cell, value 3D \
         {worst3:.3e}, 4D {worst4:.3e}, gradient {worst_gradient:.3e} per cell, d/dw {worst_dw:.3e}",
        probes.len()
    );
    // f32 lattice coordinates up to ~25 cells from the anchor: ~2e-6 of a
    // cell, times noise slopes of a few per cell.
    assert!(worst_fraction < 1.0e-5, "{worst_fraction}");
    assert!(worst3 < 5.0e-5 && worst4 < 5.0e-5, "{worst3} {worst4}");
    assert!(
        worst_gradient < 1.0e-3 && worst_dw < 1.0e-3,
        "{worst_gradient} {worst_dw}"
    );
}

fn rng_word(unit: &mut impl FnMut() -> f64) -> u32 {
    (unit() * 4_294_967_296.0) as u32
}

#[test]
fn shader_sources_never_hash_floats_with_fract_sin() {
    let shaders = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut checked = 0;
    let mut stack = vec![shaders];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if !path.ends_with("target") {
                    stack.push(path);
                }
            } else if path.extension().is_some_and(|e| e == "wgsl") {
                let source = std::fs::read_to_string(&path).unwrap();
                let compact: String = source.split_whitespace().collect();
                assert!(
                    !compact.contains("fract(sin("),
                    "{} hashes floats with fract(sin(...)) (pipeline §17.1)",
                    path.display()
                );
                checked += 1;
            }
        }
    }
    assert!(checked >= 10, "only {checked} shaders found");
}
