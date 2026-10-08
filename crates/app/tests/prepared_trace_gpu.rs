//! Prepared crater sampled-to-drawn trace (crater-milestone-v2 acceptance 3).
//!
//! For three consecutive LOD levels of the cube-face tiles around crater-A this
//! builds the production resident tiles from the shared test scene's prepared
//! Moon surface, draws them through the production WGSL reconstruction, reads
//! the vertices back from a real GPU and compares them with the f64 world
//! authority at the same node directions.
#![cfg(feature = "terrain-capture")]

use std::{num::NonZeroU64, path::PathBuf, sync::Arc};

use anyhow::{Context, Result, ensure};
use glam::{DMat3, DVec3};
use mundaris_app::{
    resident_terrain::{ResidentTileBuilder, SharedDerivedField, TileBuildIdentity},
    solar_system::SolarSystemPreset,
};
use mundaris_math::{
    Direction3, FrameId, FramePose, FramePosition, FrameTree, LocalPosition, UnitRotation,
    surface::{CubeFace, CubePatchAddress, SurfaceLocation},
};
use mundaris_renderer::{
    CelestialFrame, CelestialProjection, CelestialStaging, Icosphere, PreparedView,
    ReconstructedTileVertex, RenderPrecisionBudget, TileData, TileDraw, TileSlotState,
    terrain_capture::TerrainCaptureRenderer,
};
use mundaris_world::terrain::{PreparedSurface, SurfaceGenerator};

/// Stable Moon body identity used by the production developer/native paths.
const MOON_BODY_IDENTITY: u64 = 5_931_033_225_171_238_913;
/// `gravity_orbits/planetary.rs` binds the planetary resident path with 32 cells
/// (`configure_planetary(..., 32)`); `DEFAULT_TILE_CELLS` (64) is only the
/// prototype default of the builder.
const PRODUCTION_CELLS: u32 = 32;
/// Three consecutive levels: tile widths of ~6.8, ~3.4 and ~1.7 km at the
/// gameplay-scale Moon radius (2R / 2^level at the face centre).
const LEVELS: [u8; 3] = [5, 6, 7];
/// Gameplay-scale Moon radius named by the milestone contract.
const CONTRACT_RADIUS_M: f64 = 109_081.776_8;
const CRATER_LOCAL_RADIUS_M: f64 = 700.0;
const RIM_RING_M: [f64; 2] = [550.0, 750.0];
const FLOOR_RADIUS_M: f64 = 300.0;
const DENSE_MAX_RADIUS_M: f64 = 800.0;
const FINEST_LEVEL_RELIEF_TOLERANCE: f64 = 0.05;
const DEFAULT_EVIDENCE_PATH: &str =
    "D:/Mundaris/ai/tasks/crater-milestone-v2/trace/prepared-trace-gpu.json";

/// Residuals of one GPU tile against one reference, accumulated over nodes.
#[derive(Default, Clone)]
struct Residuals {
    max_local_m: f64,
    sum_local_sq: f64,
    max_normal_rad: f64,
    sum_normal_sq: f64,
    max_material_l2: f64,
    sum_material_sq: f64,
    samples: u64,
}

impl Residuals {
    fn record(&mut self, local_m: f64, normal_rad: f64, material_l2: f64) {
        self.max_local_m = self.max_local_m.max(local_m);
        self.sum_local_sq += local_m * local_m;
        self.max_normal_rad = self.max_normal_rad.max(normal_rad);
        self.sum_normal_sq += normal_rad * normal_rad;
        self.max_material_l2 = self.max_material_l2.max(material_l2);
        self.sum_material_sq += material_l2 * material_l2;
        self.samples += 1;
    }

    fn merge(&mut self, other: &Self) {
        self.max_local_m = self.max_local_m.max(other.max_local_m);
        self.sum_local_sq += other.sum_local_sq;
        self.max_normal_rad = self.max_normal_rad.max(other.max_normal_rad);
        self.sum_normal_sq += other.sum_normal_sq;
        self.max_material_l2 = self.max_material_l2.max(other.max_material_l2);
        self.sum_material_sq += other.sum_material_sq;
        self.samples += other.samples;
    }

    fn report(&self) -> serde_json::Value {
        let n = self.samples.max(1) as f64;
        serde_json::json!({
            "samples": self.samples,
            "max_local_position_m": self.max_local_m,
            "rms_local_position_m": (self.sum_local_sq / n).sqrt(),
            "max_normal_angle_radians": self.max_normal_rad,
            "rms_normal_angle_radians": (self.sum_normal_sq / n).sqrt(),
            "max_material_l2": self.max_material_l2,
            "rms_material_l2": (self.sum_material_sq / n).sqrt(),
        })
    }
}

/// World authority at one tile node direction.
struct AuthorityNode {
    position_body: DVec3,
    height_m: f64,
    normal: DVec3,
    material: [f64; 4],
    /// Great-circle distance to the crater-A centre, in metres.
    crater_r_m: f64,
    /// Body-x component of the unit direction (crater-B lies towards +x).
    dir_x: f64,
}

fn angle(a: DVec3, b: DVec3) -> f64 {
    let a = a.normalize();
    let b = b.normalize();
    a.cross(b).length().atan2(a.dot(b))
}

fn material_l2(a: &[f32; 4], b: [f64; 4]) -> f64 {
    a.iter()
        .zip(b)
        .map(|(left, right)| (f64::from(*left) - right).powi(2))
        .sum::<f64>()
        .sqrt()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn arc_m(radius_m: f64, a: DVec3, b: DVec3) -> f64 {
    radius_m * angle(a, b)
}

fn view_direction(view: &PreparedView<'_>, body: FrameId, axis: DVec3) -> Result<DVec3> {
    Ok(view
        .prepare_source(body)?
        .view_direction(Direction3::try_new(axis)?)?
        .unit())
}

/// Reconstruct one resident tile on the GPU after a real frame submission.
fn gpu_vertices(
    renderer: &mut TerrainCaptureRenderer,
    staging: &mut CelestialStaging,
    slot: &mut TileSlotState,
    sphere: &Icosphere,
    projection: CelestialProjection,
    namespace: u64,
    tile: &Arc<TileData>,
) -> Result<Vec<ReconstructedTileVertex>> {
    let anchor = tile.anchor_position_body()?;
    let tree = FrameTree::new(NonZeroU64::new(namespace).unwrap());
    let body = tree.root();
    let observer = FramePose::new(
        FramePosition::new(
            body,
            LocalPosition::try_metres(anchor + anchor.normalize() * 1_000.0)?,
        ),
        UnitRotation::identity(),
    );
    let evaluation = tree.evaluate();
    let view = PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug())?;
    let source = view.prepare_source(body)?;
    let anchor_view_m = source
        .view_displacement(FramePosition::new(body, LocalPosition::try_metres(anchor)?))?
        .metres();
    let body_to_view = DMat3::from_cols(
        view_direction(&view, body, DVec3::X)?,
        view_direction(&view, body, DVec3::Y)?,
        view_direction(&view, body, DVec3::Z)?,
    );
    let publication = slot
        .request(&tile.key)
        .context("requesting tile publication")?;
    let draw = TileDraw {
        tile: Arc::clone(tile),
        publication,
        anchor_view_m,
        body_to_view,
        mode: 0,
        sun_body: DVec3::new(0.3, 0.7, 0.5).normalize(),
    };
    {
        let mut frame = CelestialFrame::new(&view, staging, projection, sphere);
        frame
            .set_resident_tile(draw.clone())
            .context("staging resident tile")?;
        renderer.render(&frame).context("rendering resident tile")?;
    }
    renderer
        .validate_resident_tile(&draw)
        .context("GPU tile readback")
}

/// Authority at every node of `address` plus the largest tangent gradient seen
/// at nodes and cell centres (m per radian), used for the per-LOD tolerance.
fn authority_nodes(
    generator: &SurfaceGenerator,
    address: CubePatchAddress,
    cells: u32,
    reference_radius_m: f64,
) -> Result<(Vec<AuthorityNode>, f64)> {
    let crater = DVec3::Z;
    let mut nodes = Vec::with_capacity(((cells + 1) * (cells + 1)) as usize);
    let mut max_gradient = 0.0_f64;
    for j in 0..=cells {
        for i in 0..=cells {
            let st = [
                f64::from(i) / f64::from(cells),
                f64::from(j) / f64::from(cells),
            ];
            let direction = address.face().direction(address.face_uv(st)?)?;
            let location = SurfaceLocation::new(direction);
            let sample = generator.evaluate_point(location)?;
            let unit = direction.unit();
            max_gradient = max_gradient.max(
                sample
                    .terrain()
                    .tangent_gradient_m_per_unit_direction()
                    .length(),
            );
            nodes.push(AuthorityNode {
                position_body: sample.position(location),
                height_m: sample.radius_m() - reference_radius_m,
                normal: sample.normal(),
                material: sample.material_weights(),
                crater_r_m: arc_m(reference_radius_m, unit, crater),
                dir_x: unit.x,
            });
        }
    }
    for j in 0..cells {
        for i in 0..cells {
            let st = [
                (f64::from(i) + 0.5) / f64::from(cells),
                (f64::from(j) + 0.5) / f64::from(cells),
            ];
            let direction = address.face().direction(address.face_uv(st)?)?;
            let sample = generator.evaluate_point(SurfaceLocation::new(direction))?;
            max_gradient = max_gradient.max(
                sample
                    .terrain()
                    .tangent_gradient_m_per_unit_direction()
                    .length(),
            );
        }
    }
    Ok((nodes, max_gradient))
}

/// (distance to crater-A centre in m, body-x of the unit direction, height in m).
type ReliefPoint = (f64, f64, f64);

/// Dense polar sweep around crater-A: (r_m, body-x of direction, height_m).
fn dense_authority_sweep(
    generator: &SurfaceGenerator,
    reference_radius_m: f64,
) -> Result<(Vec<ReliefPoint>, f64, f64)> {
    let centre = DVec3::Z;
    let (east, north) = (DVec3::X, DVec3::Y);
    let mut context = generator.prepared_query_context();
    let mut points = Vec::new();
    let mut locations = Vec::new();
    for ring in 0..=(DENSE_MAX_RADIUS_M / 2.0) as u32 {
        let r_m = f64::from(ring) * 2.0;
        let theta = r_m / reference_radius_m;
        let azimuths = if ring == 0 { 1 } else { 180 };
        for step in 0..azimuths {
            let phi = f64::from(step) * std::f64::consts::TAU / 180.0;
            let tangent = east * phi.cos() + north * phi.sin();
            let unit = centre * theta.cos() + tangent * theta.sin();
            locations.push(SurfaceLocation::new(Direction3::try_new(unit)?));
            points.push((r_m, unit.x));
        }
    }
    let first = generator.evaluate_point(locations[0])?;
    let mut samples = vec![first; locations.len()];
    context.evaluate_batch(&locations, &mut samples)?;
    let max_gradient = samples
        .iter()
        .map(|sample| {
            sample
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .length()
        })
        .fold(0.0_f64, f64::max);
    let centre_height_m = samples[0].radius_m() - reference_radius_m;
    let dense = points
        .into_iter()
        .zip(&samples)
        .map(|((r_m, x), sample)| (r_m, x, sample.radius_m() - reference_radius_m))
        .collect();
    Ok((dense, max_gradient, centre_height_m))
}

/// (max height on the rim ring, min height on the floor, their difference).
fn relief(points: &[ReliefPoint], west_only: bool) -> serde_json::Value {
    let mut ring_max = f64::NEG_INFINITY;
    let mut floor_min = f64::INFINITY;
    for &(r_m, x, h) in points {
        if west_only && x >= 0.0 {
            continue;
        }
        if (RIM_RING_M[0]..=RIM_RING_M[1]).contains(&r_m) {
            ring_max = ring_max.max(h);
        }
        if r_m < FLOOR_RADIUS_M {
            floor_min = floor_min.min(h);
        }
    }
    if ring_max.is_finite() && floor_min.is_finite() {
        serde_json::json!({
            "rim_ring_max_height_m": ring_max,
            "floor_min_height_m": floor_min,
            "relief_m": ring_max - floor_min,
        })
    } else {
        serde_json::json!({ "relief_m": null })
    }
}

fn relief_m(value: &serde_json::Value) -> Option<f64> {
    value["relief_m"].as_f64()
}

fn relative_to_authority(drawn: &serde_json::Value, authority_relief_m: f64) -> Option<f64> {
    relief_m(drawn).map(|drawn| (drawn - authority_relief_m) / authority_relief_m)
}

#[test]
#[ignore = "requires a real GPU adapter; prepared crater sampled-to-drawn trace"]
fn prepared_crater_sampled_to_drawn_trace() -> Result<()> {
    // Scene radius: exactly what the shared test scene binds for the Moon.
    let (system, _motion) =
        SolarSystemPreset::gameplay().create_analytic(NonZeroU64::new(734).unwrap())?;
    let moon = system
        .bodies()
        .find(|(_, body)| body.name() == "Moon")
        .expect("Moon exists in the shared scene")
        .0;
    let state = system.body(moon)?;
    let radius_m = state.properties().reference_radius_m();
    let definition = state
        .surface_definition()
        .expect("the shared Moon binds a surface")
        .clone();
    let prepared = Arc::clone(
        definition
            .prepared()
            .expect("the shared Moon binds the prepared surface"),
    );
    // Independent derivation: catalogue Moon radius (1.7374e6 m) times the
    // gameplay preset's 400 km Earth radius scale.
    let derived_radius_m = 1.7374e6 * (400_000.0 / 6_371_000.0);
    ensure!(
        (radius_m - derived_radius_m).abs() < 1e-6,
        "scene radius derivation"
    );
    ensure!(
        (radius_m - CONTRACT_RADIUS_M).abs() < 1e-3,
        "contract radius {radius_m}"
    );
    // The scene's prepared surface is the on-disk definition.
    let on_disk = PreparedSurface::load(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../assets/prepared/composition.native.json"),
    )?;
    ensure!(
        on_disk.content_identity() == prepared.content_identity(),
        "scene prepared surface differs from assets/prepared/composition.native.json"
    );
    let content_identity = hex(&prepared.content_identity());

    let generator = SurfaceGenerator::new(&definition, radius_m)?;
    let revision = state.terrain_revision().value();
    let identity = TileBuildIdentity {
        body_identity: MOON_BODY_IDENTITY,
        surface_revision: revision,
        material_revision: revision,
    };
    let derived_field = SharedDerivedField::new(&generator)?;
    let mut query_context = generator.prepared_query_context();

    let mut renderer = TerrainCaptureRenderer::new(32, 32)
        .expect("GPU adapter required; acceptance gate must not silently skip");
    let mut staging = CelestialStaging::default();
    // One persistent renderer slot: each request bumps the publication generation.
    let mut slot = TileSlotState::default();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1)?;

    let (dense, dense_gradient, centre_height_m) = dense_authority_sweep(&generator, radius_m)?;
    let authority_relief_all = relief(&dense, false);
    let authority_relief_west = relief(&dense, true);
    let authority_relief = relief_m(&authority_relief_all).expect("dense authority relief");
    let authority_relief_west_m = relief_m(&authority_relief_west).expect("dense west relief");

    let mut failures: Vec<String> = Vec::new();
    let mut level_reports = Vec::new();
    let mut namespace = 80_000_u64;

    for level in LEVELS {
        let mid = 1u32 << (level - 1);
        // Crater-A's centre is the shared corner of these four tiles at every
        // level (dyadic boundary at face coordinate 0), so all four are drawn.
        let addresses: Vec<CubePatchAddress> = [
            (mid - 1, mid - 1),
            (mid, mid - 1),
            (mid - 1, mid),
            (mid, mid),
        ]
        .into_iter()
        .map(|(x, y)| CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y))
        .collect::<Result<_, _>>()?;
        let authority: Vec<_> = addresses
            .iter()
            .map(|address| authority_nodes(&generator, *address, PRODUCTION_CELLS, radius_m))
            .collect::<Result<_>>()?;
        let spacing_rad = 2.0 / (f64::from(1u32 << level) * f64::from(PRODUCTION_CELLS));
        let nominal_width_m = radius_m * 2.0 / f64::from(1u32 << level);
        let gradient_bound = 1.25
            * authority
                .iter()
                .map(|(_, g)| *g)
                .fold(dense_gradient, f64::max);
        // Per-LOD tolerance of node position against the world authority.
        // Drawn minus authority = (authority -> CPU tile) + (CPU tile -> GPU).
        // The second term is f32 storage relative to the tile anchor, bounded
        // by 1e-3 m (the existing GPU tests' local-position gate). The first is
        // the representation approximation, which the builder measures itself
        // (max_texel_radial_error_m over the same nodes), so a node must agree
        // within that certificate + 1e-3 m. As an independent a-priori ceiling
        // (rigorous but loose because crater walls are steep) the 7-tap average
        // (0.25 centre + 6 x 0.125, taps 0.25 spacing away) moves a node by at
        // most 0.75 * G * 0.25 * spacing for the exact filtered tile, and by at
        // most 0.375 * G * spacing for the production derived tile, whose taps
        // are bilinearly interpolated from raw nodes up to sqrt(2) spacings
        // away (non-centre corner weight <= 0.354, tap weight sum 0.75). G is
        // the largest sampled tangent gradient (m/rad), with a 1.25 margin.
        let ceiling_exact_m = 1.0e-3 + 0.1875 * gradient_bound * spacing_rad;
        let ceiling_derived_m = 1.0e-3 + 0.375 * gradient_bound * spacing_rad;

        let mut representation_reports = Vec::new();
        for (name, derived) in [("production_derived", true), ("exact_filtered", false)] {
            let ceiling_m = if derived {
                ceiling_derived_m
            } else {
                ceiling_exact_m
            };
            let mut level_tolerance_m = 0.0_f64;
            let mut aggregate_authority = Residuals::default();
            let mut aggregate_tile_cpu = Residuals::default();
            let mut crater_max_m = 0.0_f64;
            let mut crater_sum_sq = 0.0_f64;
            let mut crater_nodes = 0_u64;
            let mut drawn_points: Vec<ReliefPoint> = Vec::new();
            let mut authority_node_points: Vec<ReliefPoint> = Vec::new();
            let mut tile_reports = Vec::new();

            for (address, (nodes, _)) in addresses.iter().zip(&authority) {
                let (tile, _diagnostics) = if derived {
                    ResidentTileBuilder::build_derived(
                        &generator,
                        identity,
                        *address,
                        PRODUCTION_CELLS,
                        &mut query_context,
                        &derived_field,
                        || false,
                    )?
                } else {
                    ResidentTileBuilder::build(&generator, identity, *address, PRODUCTION_CELLS)?
                };
                let certificate = ResidentTileBuilder::measure_approximation(&generator, &tile)?;
                let tolerance_m = (certificate.max_texel_radial_error_m + 1.0e-3).min(ceiling_m);
                level_tolerance_m = level_tolerance_m.max(tolerance_m);
                let tile = Arc::new(tile);
                let anchor = tile.anchor_position_body()?;
                namespace += 1;
                let vertices = gpu_vertices(
                    &mut renderer,
                    &mut staging,
                    &mut slot,
                    &sphere,
                    projection,
                    namespace,
                    &tile,
                )?;
                ensure!(vertices.len() == nodes.len(), "vertex count");

                let mut vs_authority = Residuals::default();
                let mut vs_tile = Residuals::default();
                let mut tile_crater_max_m = 0.0_f64;
                let mut tile_crater_nodes = 0_u64;
                let mut tile_failures = 0_u64;
                for (index, (gpu, node)) in vertices.iter().zip(nodes).enumerate() {
                    let i = index as u32 % (PRODUCTION_CELLS + 1);
                    let j = index as u32 / (PRODUCTION_CELLS + 1);
                    let st = [
                        f64::from(i) / f64::from(PRODUCTION_CELLS),
                        f64::from(j) / f64::from(PRODUCTION_CELLS),
                    ];
                    let gpu_local = DVec3::from_array(gpu.position_local_m.map(f64::from));
                    let gpu_normal = DVec3::from_array(gpu.normal_body.map(f64::from));
                    let finite = gpu
                        .position_local_m
                        .iter()
                        .chain(&gpu.normal_body)
                        .chain(&gpu.material)
                        .all(|value| value.is_finite());
                    let valid = finite
                        && (gpu_normal.length() - 1.0).abs() <= 1.0e-4
                        && gpu.material.iter().all(|value| (0.0..=1.0).contains(value))
                        && (gpu.material.iter().sum::<f32>() - 1.0).abs() <= 1.0e-4;
                    if !valid {
                        failures.push(format!("{name} L{level} node {i},{j}: invalid GPU vertex"));
                        tile_failures += 1;
                        continue;
                    }

                    // Representation fidelity: GPU against the CPU tile itself.
                    let cpu_local = tile.position_local(st)?;
                    let tile_local = (gpu_local - cpu_local).length();
                    let tile_normal = angle(gpu_normal, tile.normal_local([i, j])?);
                    let tile_material =
                        material_l2(&gpu.material, tile.material(st)?.map(f64::from));
                    vs_tile.record(tile_local, tile_normal, tile_material);
                    if tile_local > 1.0e-3 || tile_normal > 1.0e-3 || tile_material > 1.0e-5 {
                        failures.push(format!(
                            "{name} L{level} node {i},{j}: GPU vs tile local {tile_local}m normal {tile_normal}rad material {tile_material}"
                        ));
                        tile_failures += 1;
                    }

                    // Sampled to drawn: GPU against the complete world authority.
                    let authority_local = node.position_body - anchor;
                    let local_error = (gpu_local - authority_local).length();
                    vs_authority.record(
                        local_error,
                        angle(gpu_normal, node.normal),
                        material_l2(&gpu.material, node.material),
                    );
                    if local_error > tolerance_m {
                        failures.push(format!(
                            "{name} L{level} node {i},{j}: GPU vs authority {local_error}m exceeds {tolerance_m}m"
                        ));
                        tile_failures += 1;
                    }

                    let drawn_height_m = (anchor + gpu_local).length() - radius_m;
                    if node.crater_r_m <= DENSE_MAX_RADIUS_M {
                        drawn_points.push((node.crater_r_m, node.dir_x, drawn_height_m));
                        authority_node_points.push((node.crater_r_m, node.dir_x, node.height_m));
                    }
                    if node.crater_r_m <= CRATER_LOCAL_RADIUS_M {
                        let height_error = (drawn_height_m - node.height_m).abs();
                        tile_crater_max_m = tile_crater_max_m.max(height_error);
                        crater_sum_sq += height_error * height_error;
                        tile_crater_nodes += 1;
                    }
                }
                crater_max_m = crater_max_m.max(tile_crater_max_m);
                crater_nodes += tile_crater_nodes;
                aggregate_authority.merge(&vs_authority);
                aggregate_tile_cpu.merge(&vs_tile);
                let [x, y] = address.coordinates();
                tile_reports.push(serde_json::json!({
                    "address": { "face": "positive_z", "level": level, "x": x, "y": y },
                    "gpu_vs_authority_f64": vs_authority.report(),
                    "gpu_vs_cpu_tile": vs_tile.report(),
                    "position_tolerance_vs_authority_m": tolerance_m,
                    "builder_certificate_cpu_tile_vs_authority": {
                        "max_texel_radial_error_m": certificate.max_texel_radial_error_m,
                        "rms_texel_radial_error_m": certificate.rms_texel_radial_error_m,
                        "max_triangle_centroid_error_m": certificate.max_triangle_centroid_error_m,
                        "rms_triangle_centroid_error_m": certificate.rms_triangle_centroid_error_m,
                        "max_normal_angular_error_radians": certificate.max_normal_angular_error_radians,
                        "rms_normal_angular_error_radians": certificate.rms_normal_angular_error_radians,
                    },
                    "crater_local_nodes": tile_crater_nodes,
                    "crater_local_max_abs_height_error_m": tile_crater_max_m,
                    "failed_nodes": tile_failures,
                }));
            }

            let drawn_all = relief(&drawn_points, false);
            let drawn_west = relief(&drawn_points, true);
            let node_all = relief(&authority_node_points, false);
            let node_west = relief(&authority_node_points, true);
            let relative_difference = relative_to_authority(&drawn_all, authority_relief);
            let relative_difference_west =
                relative_to_authority(&drawn_west, authority_relief_west_m);
            // Asserted at the finest level for the production representation.
            // Crater-A is judged on the half-plane away from crater-B: B's
            // narrow rim lies inside A's rim ring on the +x side and is a
            // separate, smaller feature. The literal all-azimuth figure is
            // reported (and its pass/fail recorded) but not asserted.
            if derived && level == *LEVELS.last().unwrap() {
                match relative_difference_west {
                    Some(difference) if difference.abs() <= FINEST_LEVEL_RELIEF_TOLERANCE => {}
                    other => failures.push(format!(
                        "finest level L{level} {name}: crater-A drawn relief {:?} m vs authority {authority_relief_west_m} m (relative {other:?}) exceeds {FINEST_LEVEL_RELIEF_TOLERANCE}",
                        relief_m(&drawn_west)
                    )),
                }
            }
            representation_reports.push(serde_json::json!({
                "representation": name,
                "position_tolerance_vs_authority_m_max_over_tiles": level_tolerance_m,
                "position_a_priori_ceiling_m": ceiling_m,
                "gpu_vs_authority_f64": aggregate_authority.report(),
                "gpu_vs_cpu_tile": aggregate_tile_cpu.report(),
                "crater_local": {
                    "radius_m": CRATER_LOCAL_RADIUS_M,
                    "nodes": crater_nodes,
                    "max_abs_height_error_m": crater_max_m,
                    "rms_abs_height_error_m": (crater_sum_sq / crater_nodes.max(1) as f64).sqrt(),
                },
                "relief": {
                    "definition": "max drawn height on rim ring r in [550,750] m minus min drawn height at r < 300 m",
                    "drawn_all_azimuths": drawn_all,
                    "drawn_west_half_away_from_crater_b": drawn_west,
                    "authority_at_same_nodes_all_azimuths": node_all,
                    "authority_at_same_nodes_west_half": node_west,
                    "authority_dense_all_azimuths": authority_relief_all,
                    "authority_dense_west_half": authority_relief_west,
                    "drawn_minus_dense_authority_relative_all_azimuths": relative_difference,
                    "drawn_minus_dense_authority_relative_west_half": relative_difference_west,
                    "literal_all_azimuth_within_5_percent": relative_difference.map(|d| d.abs() <= FINEST_LEVEL_RELIEF_TOLERANCE),
                    "west_half_within_5_percent": relative_difference_west.map(|d| d.abs() <= FINEST_LEVEL_RELIEF_TOLERANCE),
                },
                "tiles": tile_reports,
            }));
        }

        // Measured widths from the node directions of the first tile edge.
        let (first_nodes, _) = &authority[3];
        let cells = PRODUCTION_CELLS as usize;
        let measured_width_m = arc_m(
            radius_m,
            first_nodes[0].position_body,
            first_nodes[cells].position_body,
        );
        level_reports.push(serde_json::json!({
            "level": level,
            "tile_count": addresses.len(),
            "cells": PRODUCTION_CELLS,
            "nominal_tile_width_m": nominal_width_m,
            "measured_tile_edge_arc_m": measured_width_m,
            "cell_spacing_m": measured_width_m / f64::from(PRODUCTION_CELLS),
            "max_sampled_tangent_gradient_m_per_rad_with_margin": gradient_bound,
            "position_a_priori_ceiling_exact_filtered_m": ceiling_exact_m,
            "position_a_priori_ceiling_production_derived_m": ceiling_derived_m,
            "representations": representation_reports,
        }));
    }

    let evidence = serde_json::json!({
        "schema": "mundaris.prepared_trace_gpu.v1",
        "gpu_adapter": renderer.adapter_name(),
        "gpu_backend": renderer.adapter_backend(),
        "scene": {
            "body": "Moon",
            "radius_m": radius_m,
            "radius_derivation": "SolarSystemPreset::gameplay().create_analytic(..): catalogue Moon real_mean_radius_m 1.7374e6 x (400000/6371000); read back from BodyProperties::reference_radius_m()",
            "radius_derived_independently_m": derived_radius_m,
            "contract_radius_m": CONTRACT_RADIUS_M,
            "terrain_revision": revision,
        },
        "prepared_definition_content_identity_sha256": content_identity,
        "tile_cells": PRODUCTION_CELLS,
        "tile_cells_source": "gravity_orbits/planetary.rs configure_planetary(.., 32) -> RegionalTerrain::new_derived -> ResidentTileBuilder::build_derived",
        "levels": LEVELS,
        "crater_a_center_direction": [0.0, 0.0, 1.0],
        "authority_dense_sweep": {
            "centre_absolute_height_m": centre_height_m,
            "implied_control_base_height_m_if_depth_is_5x_minus_25_6": centre_height_m - 5.0 * -25.6,
            "max_tangent_gradient_m_per_rad": dense_gradient,
            "radial_step_m": 2.0,
            "azimuth_step_degrees": 2.0,
        },
        "morph_fraction": "not applicable: single-tile validate path draws own-level geometry (equivalent to morph 0)",
        "level_reports": level_reports,
        "failures": failures.len(),
        "first_failures": failures.iter().take(20).collect::<Vec<_>>(),
    });
    let text = serde_json::to_string_pretty(&evidence)?;
    println!("{text}");
    let path = std::env::var_os("MUNDARIS_PREPARED_TRACE_EVIDENCE")
        .map_or_else(|| PathBuf::from(DEFAULT_EVIDENCE_PATH), PathBuf::from);
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, text.as_bytes())?;
    println!("wrote prepared trace evidence to {}", path.display());

    assert!(
        failures.is_empty(),
        "{} trace assertion failures, first: {:?}",
        failures.len(),
        failures.iter().take(5).collect::<Vec<_>>()
    );
    Ok(())
}
