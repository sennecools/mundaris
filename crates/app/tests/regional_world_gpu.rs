#![cfg(feature = "developer-tools")]

use std::{collections::BTreeMap, num::NonZeroU64, path::PathBuf, sync::Arc, time::Instant};

use anyhow::{Context, Result, ensure};
use glam::{DMat3, DVec3};
use mundaris_app::resident_terrain::{
    ResidentTileBuilder, TileBuildDiagnostics, TileBuildIdentity,
};
use mundaris_math::{
    Direction3, Displacement3, FrameId, FramePose, FramePosition, FrameState, FrameTree,
    LocalPosition, RigidTransform, UnitRotation,
    surface::{CubeFace, CubePatchAddress, PatchEdge},
};
use mundaris_renderer::{
    CelestialFrame, CelestialProjection, CelestialStaging, Icosphere, PreparedView,
    RegionalBoundaryEndpoints, RegionalPatchDraw, RegionalResidentDraw, RegionalTileUpload,
    RenderPrecisionBudget, TileData, TileDraw, TileSlotState,
    regional_edges::{TileBoundary, build_boundaries, subdivided_parent_boundary},
    regional_resident::reconstruct_patch_node,
    terrain_capture::TerrainCaptureRenderer,
};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};

const BODY_IDENTITY: u64 = 5_931_033_225_171_238_913;
const CELLS: u32 = 16;
const MORPH_FRACTIONS: [f32; 5] = [0.0, 0.25, 0.5, 0.75, 1.0];
type TileCover = BTreeMap<CubePatchAddress, Arc<TileData>>;
type CoverTransition = (TileCover, TileCover, CubePatchAddress);

#[derive(Default)]
struct Residuals {
    max_local_m: f64,
    max_view_m: f64,
    max_normal_rad: f64,
    max_material_l2: f64,
    samples: u64,
}

impl Residuals {
    fn record(&mut self, local_m: f64, view_m: f64, normal_rad: f64, material_l2: f64) {
        self.max_local_m = self.max_local_m.max(local_m);
        self.max_view_m = self.max_view_m.max(view_m);
        self.max_normal_rad = self.max_normal_rad.max(normal_rad);
        self.max_material_l2 = self.max_material_l2.max(material_l2);
        self.samples += 1;
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "samples": self.samples,
            "max_local_position_m": self.max_local_m,
            "max_view_position_m": self.max_view_m,
            "max_normal_angle_radians": self.max_normal_rad,
            "max_material_l2": self.max_material_l2,
        })
    }
}

#[derive(Default)]
struct SeamResiduals {
    max_position_m: f64,
    max_normal_rad: f64,
    max_material_l2: f64,
    compared_nodes: u64,
    coarse_fine_odd_nodes: u64,
}

impl SeamResiduals {
    fn record(&mut self, position_m: f64, normal_rad: f64, material_l2: f64, odd: bool) {
        self.max_position_m = self.max_position_m.max(position_m);
        self.max_normal_rad = self.max_normal_rad.max(normal_rad);
        self.max_material_l2 = self.max_material_l2.max(material_l2);
        self.compared_nodes += 1;
        if odd {
            self.coarse_fine_odd_nodes += 1;
        }
    }

    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "compared_nodes": self.compared_nodes,
            "coarse_fine_odd_nodes": self.coarse_fine_odd_nodes,
            "max_view_position_m": self.max_position_m,
            "max_normal_angle_radians": self.max_normal_rad,
            "max_material_l2": self.max_material_l2,
        })
    }
}

struct Fixture {
    label: &'static str,
    radius_m: f64,
    level: u8,
    x: u32,
    y: u32,
    split_child: usize,
    cover: CoverShape,
    common_offset_m: f64,
}

#[derive(Clone, Copy)]
enum CoverShape {
    FourChildren,
    ParentWithUMaxNeighbor,
    ParentWithUMaxAndVMaxNeighbors,
}

fn fixtures() -> Vec<Fixture> {
    vec![
        Fixture {
            label: "small_body_interior",
            radius_m: 80_000.0,
            level: 9,
            x: 157,
            y: 39,
            split_child: 0,
            cover: CoverShape::FourChildren,
            common_offset_m: 0.0,
        },
        Fixture {
            label: "earth_scale_face_edge",
            radius_m: 6_371_000.0,
            level: 16,
            x: (1 << 16) - 1,
            y: 1 << 15,
            split_child: 1,
            cover: CoverShape::FourChildren,
            common_offset_m: 0.0,
        },
        Fixture {
            label: "large_body_face_corner",
            radius_m: 70_000_000.0,
            level: 20,
            x: 0,
            y: 0,
            split_child: 0,
            cover: CoverShape::FourChildren,
            common_offset_m: 0.0,
        },
        Fixture {
            label: "earth_scale_across_face_umax",
            radius_m: 6_371_000.0,
            level: 16,
            x: (1 << 16) - 1,
            y: 1 << 15,
            split_child: 0,
            cover: CoverShape::ParentWithUMaxNeighbor,
            common_offset_m: 0.0,
        },
        Fixture {
            label: "large_body_across_face_corner",
            radius_m: 70_000_000.0,
            level: 20,
            x: (1 << 20) - 1,
            y: (1 << 20) - 1,
            split_child: 0,
            cover: CoverShape::ParentWithUMaxAndVMaxNeighbors,
            common_offset_m: 0.0,
        },
        Fixture {
            label: "large_body_across_face_corner_offset_1e13m",
            radius_m: 70_000_000.0,
            level: 20,
            x: (1 << 20) - 1,
            y: (1 << 20) - 1,
            split_child: 0,
            cover: CoverShape::ParentWithUMaxAndVMaxNeighbors,
            common_offset_m: 1.0e13,
        },
    ]
}

fn direction(view: &PreparedView<'_>, body: FrameId, axis: DVec3) -> Result<DVec3> {
    Ok(view
        .prepare_source(body)?
        .view_direction(Direction3::try_new(axis)?)?
        .unit())
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

fn point_at(
    points: &[mundaris_renderer::ReconstructedTileVertex],
    grid: [u32; 2],
) -> &mundaris_renderer::ReconstructedTileVertex {
    &points[(grid[1] * (CELLS + 1) + grid[0]) as usize]
}

fn write_evidence(evidence: &serde_json::Value) -> Result<PathBuf> {
    let path = std::env::var_os("MUNDARIS_REGIONAL_WORLD_GPU_EVIDENCE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/terrain-redesign/slice2c/worldgpu.json"));
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(evidence)?)?;
    Ok(path)
}

fn make_tile_draw(
    tile: Arc<TileData>,
    publication: mundaris_renderer::TilePublicationToken,
    reference_anchor: DVec3,
    reference_view: DVec3,
    body_to_view: DMat3,
) -> Result<TileDraw> {
    let anchor = tile.anchor_position_body()?;
    Ok(TileDraw {
        tile,
        publication,
        anchor_view_m: reference_view + body_to_view * (anchor - reference_anchor),
        body_to_view,
        mode: 0,
        sun_body: DVec3::new(0.3, 0.7, 0.5).normalize(),
    })
}

fn build_cover(
    generator: &SurfaceGenerator,
    identity: TileBuildIdentity,
    parent_address: CubePatchAddress,
    split_child: usize,
    builds: &mut Vec<serde_json::Value>,
) -> Result<CoverTransition> {
    let initial_addresses = parent_address.children()?;
    ensure!(split_child < 4, "split child index outside quadtree");
    let split_parent = initial_addresses[split_child];
    let mut addresses: Vec<_> = initial_addresses.into_iter().collect();
    addresses.extend(split_parent.children()?);
    addresses.sort();
    addresses.dedup();

    let mut all = BTreeMap::new();
    for address in addresses {
        let started = Instant::now();
        let (tile, diagnostics) = ResidentTileBuilder::build(generator, identity, address, CELLS)?;
        let elapsed = started.elapsed();
        builds.push(build_record(address, &tile, diagnostics, elapsed));
        all.insert(address, Arc::new(tile));
    }
    let initial = initial_addresses
        .into_iter()
        .map(|address| {
            Ok((
                address,
                Arc::clone(all.get(&address).context("initial tile missing")?),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let target = all
        .iter()
        .filter(|(address, _)| *address != &split_parent)
        .map(|(address, tile)| (*address, Arc::clone(tile)))
        .collect::<BTreeMap<_, _>>();
    ensure!(
        initial.len() == 4 && target.len() == 7 && all.len() == 8,
        "fixture must contain four initial tiles and seven target leaves"
    );
    Ok((initial, target, split_parent))
}

fn build_parent_neighbor_cover(
    generator: &SurfaceGenerator,
    identity: TileBuildIdentity,
    parent: CubePatchAddress,
    neighbor_edges: &[PatchEdge],
    builds: &mut Vec<serde_json::Value>,
) -> Result<CoverTransition> {
    let mut neighbors = BTreeMap::new();
    for edge in neighbor_edges {
        let neighbor = parent.neighbor(*edge).address;
        ensure!(
            neighbor != parent,
            "cube edge neighbor resolved to same patch"
        );
        neighbors.insert(neighbor, ());
    }
    let mut initial_addresses = vec![parent];
    initial_addresses.extend(neighbors.keys().copied());
    let mut target_addresses = parent.children()?.to_vec();
    target_addresses.extend(neighbors.keys().copied());
    initial_addresses.sort();
    target_addresses.sort();
    let mut all_addresses = initial_addresses.clone();
    all_addresses.extend(target_addresses.iter().copied());
    all_addresses.sort();
    all_addresses.dedup();

    let mut all = BTreeMap::new();
    for address in all_addresses {
        let started = Instant::now();
        let (tile, diagnostics) = ResidentTileBuilder::build(generator, identity, address, CELLS)?;
        builds.push(build_record(address, &tile, diagnostics, started.elapsed()));
        all.insert(address, Arc::new(tile));
    }
    let initial = initial_addresses
        .into_iter()
        .map(|address| {
            Ok((
                address,
                Arc::clone(all.get(&address).context("initial patch missing")?),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    let target = target_addresses
        .into_iter()
        .map(|address| {
            Ok((
                address,
                Arc::clone(all.get(&address).context("target patch missing")?),
            ))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    Ok((initial, target, parent))
}

fn build_record(
    address: CubePatchAddress,
    tile: &TileData,
    diagnostics: TileBuildDiagnostics,
    wall_elapsed: std::time::Duration,
) -> serde_json::Value {
    serde_json::json!({
        "address": address_json(address),
        "body_identity": tile.key.body_identity,
        "definition_words": tile.key.definition_words,
        "radius_bits": tile.key.radius_bits,
        "surface_revision": tile.key.surface_revision,
        "material_revision": tile.key.material_revision,
        "cells": tile.key.cells,
        "authoritative_query_count": diagnostics.authoritative_query_count,
        "payload_bytes": diagnostics.payload_bytes,
        "builder_elapsed_ms": diagnostics.elapsed.as_secs_f64() * 1000.0,
        "measured_wall_elapsed_ms": wall_elapsed.as_secs_f64() * 1000.0,
    })
}

fn address_json(address: CubePatchAddress) -> serde_json::Value {
    let [x, y] = address.coordinates();
    serde_json::json!({
        "face": format!("{:?}", address.face()),
        "level": address.level(),
        "x": x,
        "y": y,
    })
}

fn neighbor_json(address: CubePatchAddress, edge: PatchEdge) -> serde_json::Value {
    let neighbor = address.neighbor(edge);
    serde_json::json!({
        "source_address": address_json(address),
        "source_edge": format!("{edge:?}"),
        "neighbor_address": address_json(neighbor.address),
        "neighbor_edge": format!("{:?}", neighbor.edge),
        "reversed": neighbor.reversed,
    })
}

fn create_patches(
    initial: &BTreeMap<CubePatchAddress, Arc<TileData>>,
    target: &BTreeMap<CubePatchAddress, Arc<TileData>>,
    split_parent: CubePatchAddress,
    draws: &BTreeMap<CubePatchAddress, TileDraw>,
    slots: &BTreeMap<CubePatchAddress, usize>,
    old_boundaries: &BTreeMap<CubePatchAddress, TileBoundary>,
    target_boundaries: &BTreeMap<CubePatchAddress, TileBoundary>,
) -> Result<Vec<RegionalPatchDraw>> {
    let split_children = split_parent.children()?;
    let split_children_set: std::collections::BTreeSet<_> = split_children.into_iter().collect();
    let old_parent_boundary = old_boundaries
        .get(&split_parent)
        .context("split parent outgoing boundary missing")?;
    let mut patches = Vec::with_capacity(target.len());
    for (address, tile) in target {
        let own = draws.get(address).context("own draw missing")?.clone();
        let (parent_address, quadrant, own_coarse, parent_boundary) =
            if split_children_set.contains(address) {
                let parent_tile = initial
                    .get(&split_parent)
                    .context("split parent tile missing")?;
                let coarse = subdivided_parent_boundary(parent_tile, old_parent_boundary, tile)?;
                let [x, y] = address.coordinates();
                (
                    split_parent,
                    Some([x & 1, y & 1]),
                    coarse,
                    old_parent_boundary.clone(),
                )
            } else {
                let old = old_boundaries
                    .get(address)
                    .context("stable tile outgoing boundary missing")?;
                (*address, None, old.clone(), old.clone())
            };
        patches.push(RegionalPatchDraw {
            quality_fallback: false,
            own_slot: *slots.get(address).context("own slot missing")?,
            parent_slot: *slots.get(&parent_address).context("parent slot missing")?,
            own,
            parent: draws
                .get(&parent_address)
                .context("reconstruction parent draw missing")?
                .clone(),
            morph_fraction: 0.0,
            boundary_fraction: 0.0,
            quadrant,
            boundary_endpoints: Arc::new(RegionalBoundaryEndpoints {
                version: 1,
                own_coarse,
                own_fine: target_boundaries
                    .get(address)
                    .context("target tile boundary missing")?
                    .clone(),
                parent: parent_boundary,
            }),
        });
    }
    Ok(patches)
}

fn gpu_seams(
    target: &BTreeMap<CubePatchAddress, Arc<TileData>>,
    patches: &[RegionalPatchDraw],
    points: &[Vec<mundaris_renderer::ReconstructedTileVertex>],
) -> SeamResiduals {
    let patch_indices: BTreeMap<_, _> = patches
        .iter()
        .enumerate()
        .map(|(index, patch)| (patch.own.tile.key.address, index))
        .collect();
    let mut result = SeamResiduals::default();
    for address in target.keys() {
        let own_index = patch_indices[address];
        for edge in PatchEdge::ALL {
            let adjacent = address.neighbor(edge);
            if let Some(&other_index) = patch_indices.get(&adjacent.address) {
                if *address >= adjacent.address {
                    continue;
                }
                for along in 0..=CELLS {
                    let other_along = if adjacent.reversed {
                        CELLS - along
                    } else {
                        along
                    };
                    let a = point_at(&points[own_index], edge.grid(along, CELLS));
                    let b = point_at(&points[other_index], adjacent.edge.grid(other_along, CELLS));
                    record_seam(&mut result, a, b, false);
                }
                continue;
            }
            if address.level() == 0 {
                continue;
            }
            let Some(coarse_address) = adjacent.address.parent() else {
                continue;
            };
            let Some(&coarse_index) = patch_indices.get(&coarse_address) else {
                continue;
            };
            let [x, y] = adjacent.address.coordinates();
            let (coarse_edge, half) = match adjacent.edge {
                PatchEdge::UMin if x & 1 == 0 => (PatchEdge::UMin, y & 1),
                PatchEdge::UMax if x & 1 == 1 => (PatchEdge::UMax, y & 1),
                PatchEdge::VMin if y & 1 == 0 => (PatchEdge::VMin, x & 1),
                PatchEdge::VMax if y & 1 == 1 => (PatchEdge::VMax, x & 1),
                _ => continue,
            };
            if target.get(&coarse_address).is_none() {
                continue;
            }
            for along in 0..=CELLS {
                let local_t = f64::from(along) / f64::from(CELLS);
                let coarse_t = (f64::from(half)
                    + if adjacent.reversed {
                        1.0 - local_t
                    } else {
                        local_t
                    })
                    * 0.5;
                let coordinate = coarse_t * f64::from(CELLS);
                let i0 = (coordinate.floor() as u32).min(CELLS);
                let i1 = (i0 + 1).min(CELLS);
                let t = coordinate - f64::from(i0);
                let a = point_at(&points[coarse_index], coarse_edge.grid(i0, CELLS));
                let b = point_at(&points[coarse_index], coarse_edge.grid(i1, CELLS));
                let coarse_value = interpolate_gpu(a, b, t as f32);
                let fine_value = point_at(&points[own_index], edge.grid(along, CELLS));
                record_seam(&mut result, fine_value, &coarse_value, along % 2 == 1);
            }
        }
    }
    result
}

fn interpolate_gpu(
    a: &mundaris_renderer::ReconstructedTileVertex,
    b: &mundaris_renderer::ReconstructedTileVertex,
    t: f32,
) -> mundaris_renderer::ReconstructedTileVertex {
    let mix3 = |a: [f32; 3], b: [f32; 3]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
    let mix4 = |a: [f32; 4], b: [f32; 4]| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
    mundaris_renderer::ReconstructedTileVertex {
        position_local_m: mix3(a.position_local_m, b.position_local_m),
        position_view_m: mix3(a.position_view_m, b.position_view_m),
        normal_body: mix3(a.normal_body, b.normal_body),
        material: mix4(a.material, b.material),
    }
}

fn record_seam(
    output: &mut SeamResiduals,
    a: &mundaris_renderer::ReconstructedTileVertex,
    b: &mundaris_renderer::ReconstructedTileVertex,
    odd: bool,
) {
    let position = (DVec3::from_array(a.position_view_m.map(f64::from))
        - DVec3::from_array(b.position_view_m.map(f64::from)))
    .length();
    let normal = angle(
        DVec3::from_array(a.normal_body.map(f64::from)),
        DVec3::from_array(b.normal_body.map(f64::from)),
    );
    let material = a
        .material
        .iter()
        .zip(b.material)
        .map(|(left, right)| (f64::from(*left) - f64::from(right)).powi(2))
        .sum::<f64>()
        .sqrt();
    output.record(position, normal, material, odd);
}

#[test]
#[ignore = "requires a real GPU adapter; checks world-generated mixed-LOD regional reconstruction"]
fn rocky_world_mixed_lod_regional_gpu_matches_f64_oracle_and_shared_edges() -> Result<()> {
    let mut renderer = TerrainCaptureRenderer::new(32, 32)
        .expect("GPU adapter required; this acceptance gate must not silently skip");
    let mut staging = CelestialStaging::default();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1)?;
    let mut fixtures_report = Vec::new();
    let mut violations = Vec::new();
    let mut corner_baseline: Option<Vec<Vec<Vec<mundaris_renderer::ReconstructedTileVertex>>>> =
        None;
    let mut slot_states: Vec<_> = (0..8).map(|_| TileSlotState::default()).collect();

    for (fixture_index, fixture) in fixtures().into_iter().enumerate() {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(BODY_IDENTITY),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, fixture.radius_m)?;
        let identity = TileBuildIdentity {
            body_identity: BODY_IDENTITY,
            surface_revision: 2,
            material_revision: definition.material_identity(),
        };
        let parent_address =
            CubePatchAddress::try_new(CubeFace::PositiveZ, fixture.level, fixture.x, fixture.y)?;
        let mut build_records = Vec::new();
        let (initial, target, split_parent) = match fixture.cover {
            CoverShape::FourChildren => build_cover(
                &generator,
                identity,
                parent_address,
                fixture.split_child,
                &mut build_records,
            )?,
            CoverShape::ParentWithUMaxNeighbor => build_parent_neighbor_cover(
                &generator,
                identity,
                parent_address,
                &[PatchEdge::UMax],
                &mut build_records,
            )?,
            CoverShape::ParentWithUMaxAndVMaxNeighbors => build_parent_neighbor_cover(
                &generator,
                identity,
                parent_address,
                &[PatchEdge::UMax, PatchEdge::VMax],
                &mut build_records,
            )?,
        };
        let old_boundaries = build_boundaries(&initial)?;
        let target_boundaries = build_boundaries(&target)?;
        let all_tiles: BTreeMap<_, _> = initial
            .iter()
            .chain(target.iter())
            .map(|(address, tile)| (*address, Arc::clone(tile)))
            .collect();
        ensure!(
            all_tiles.len() <= slot_states.len(),
            "fixture exceeds allocated regional upload slots"
        );

        let split_tile = initial
            .get(&split_parent)
            .context("split parent tile missing")?;
        let reference_anchor = split_tile.anchor_position_body()?;
        let mut tree = FrameTree::new(NonZeroU64::new(90_000 + fixture_index as u64).unwrap());
        let root = tree.root();
        let common = tree.insert(
            root,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(DVec3::new(fixture.common_offset_m, 0.0, 0.0))?,
                UnitRotation::identity(),
            )),
        )?;
        let body = tree.insert(
            common,
            FrameState::stationary(RigidTransform::new(
                Displacement3::zero(),
                UnitRotation::identity(),
            )),
        )?;
        let radial = reference_anchor.normalize();
        let observer_frame = tree.insert(
            common,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(reference_anchor + radial * 1_000.0)?,
                UnitRotation::identity(),
            )),
        )?;
        let observer = FramePose::new(
            FramePosition::new(observer_frame, LocalPosition::try_metres(DVec3::ZERO)?),
            UnitRotation::identity(),
        );
        let evaluation = tree.evaluate();
        let view = PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug())?;
        let source = view.prepare_source(body)?;
        let reference_view = source
            .view_displacement(FramePosition::new(
                body,
                LocalPosition::try_metres(reference_anchor)?,
            ))?
            .metres();
        let body_to_view = DMat3::from_cols(
            direction(&view, body, DVec3::X)?,
            direction(&view, body, DVec3::Y)?,
            direction(&view, body, DVec3::Z)?,
        );

        let mut draws = BTreeMap::new();
        let mut uploads = Vec::with_capacity(all_tiles.len());
        let mut slots = BTreeMap::new();
        for (slot, (address, tile)) in all_tiles.iter().enumerate() {
            let publication = slot_states[slot].request(&tile.key)?;
            let draw = make_tile_draw(
                Arc::clone(tile),
                publication,
                reference_anchor,
                reference_view,
                body_to_view,
            )?;
            slots.insert(*address, slot);
            draws.insert(*address, draw.clone());
            uploads.push(RegionalTileUpload { slot, tile: draw });
        }
        let patches = create_patches(
            &initial,
            &target,
            split_parent,
            &draws,
            &slots,
            &old_boundaries,
            &target_boundaries,
        )?;
        let mut draw = RegionalResidentDraw {
            planetary: false,
            capacity: all_tiles.len(),
            cells: CELLS,
            uploads,
            patches,
        };
        draw.validate()?;

        let mut residuals = Residuals::default();
        let mut seam_residuals = SeamResiduals::default();
        let mut tile_uploads = Vec::new();
        let mut boundary_uploads = Vec::new();
        let mut fixture_gpu_points = Vec::new();
        for fraction in MORPH_FRACTIONS {
            for patch in &mut draw.patches {
                patch.morph_fraction = fraction;
                patch.boundary_fraction = fraction;
            }
            {
                let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                frame.set_resident_regional(draw.clone())?;
                renderer.render(&frame)?;
            }
            let report = renderer.last_resident_regional_report();
            tile_uploads.push(report.tile_upload_bytes);
            boundary_uploads.push(report.boundary_upload_bytes);
            ensure!(
                report.resident_count == all_tiles.len(),
                "all parent and leaf tiles must be GPU resident"
            );

            let mut gpu_points = Vec::with_capacity(draw.patches.len());
            for (patch_index, patch) in draw.patches.iter().enumerate() {
                let points = renderer.validate_resident_regional(&draw, patch_index)?;
                ensure!(
                    points.len() == ((CELLS + 1) * (CELLS + 1)) as usize,
                    "regional GPU grid dimension mismatch"
                );
                for (index, gpu) in points.iter().enumerate() {
                    let grid = [index as u32 % (CELLS + 1), index as u32 / (CELLS + 1)];
                    let cpu = reconstruct_patch_node(patch, grid, CELLS)?;
                    // `reconstruct_patch_node` and the regional shader both
                    // express positions in the parent's local frame. The CPU
                    // reconstruction already applies the own-to-parent anchor
                    // delta for the child surface.
                    let expected_gpu_local = cpu.position_local_m;
                    let gpu_local = DVec3::from_array(gpu.position_local_m.map(f64::from));
                    let gpu_view = DVec3::from_array(gpu.position_view_m.map(f64::from));
                    let expected_view =
                        patch.parent.anchor_view_m + body_to_view * cpu.position_local_m;
                    let expected_normal = cpu
                        .normal_varying_body
                        .try_normalize()
                        .context("CPU normal could not be normalized")?;
                    let gpu_normal = DVec3::from_array(gpu.normal_body.map(f64::from));
                    let local_error = (gpu_local - expected_gpu_local).length();
                    let view_error = (gpu_view - expected_view).length();
                    let normal_error = angle(gpu_normal, expected_normal);
                    let material_error = material_l2(&gpu.material, cpu.material);
                    residuals.record(local_error, view_error, normal_error, material_error);
                    for (criterion, value, limit) in [
                        ("local", local_error, 1.0e-3),
                        ("view", view_error, 1.0e-3),
                        ("normal", normal_error, 1.0e-3),
                        ("material", material_error, 1.0e-5),
                    ] {
                        if value > limit {
                            violations.push(format!(
                                "{} t={fraction} patch={patch_index} grid={grid:?} {criterion}={value} limit={limit}",
                                fixture.label
                            ));
                        }
                    }
                }
                gpu_points.push(points);
            }
            let seam = gpu_seams(&target, &draw.patches, &gpu_points);
            seam_residuals.max_position_m = seam_residuals.max_position_m.max(seam.max_position_m);
            seam_residuals.max_normal_rad = seam_residuals.max_normal_rad.max(seam.max_normal_rad);
            seam_residuals.max_material_l2 =
                seam_residuals.max_material_l2.max(seam.max_material_l2);
            seam_residuals.compared_nodes += seam.compared_nodes;
            seam_residuals.coarse_fine_odd_nodes += seam.coarse_fine_odd_nodes;
            fixture_gpu_points.push(gpu_points.clone());
            for (criterion, value, limit) in [
                ("seam_view", seam.max_position_m, 1.0e-3),
                ("seam_normal", seam.max_normal_rad, 1.0e-3),
                ("seam_material", seam.max_material_l2, 1.0e-5),
            ] {
                if value > limit {
                    violations.push(format!(
                        "{} t={fraction} {criterion}={value} limit={limit}",
                        fixture.label
                    ));
                }
            }
        }

        let mut offset_residuals = Residuals::default();
        if fixture.label == "large_body_across_face_corner" {
            corner_baseline = Some(fixture_gpu_points.clone());
        } else if fixture.label == "large_body_across_face_corner_offset_1e13m" {
            let baseline = corner_baseline
                .as_ref()
                .context("untranslated cube-corner baseline is missing")?;
            ensure!(
                baseline.len() == fixture_gpu_points.len(),
                "translated corner fraction count changed"
            );
            for (baseline_fraction, offset_fraction) in baseline.iter().zip(&fixture_gpu_points) {
                ensure!(
                    baseline_fraction.len() == offset_fraction.len(),
                    "translated corner patch count changed"
                );
                for (baseline_patch, offset_patch) in baseline_fraction.iter().zip(offset_fraction)
                {
                    ensure!(
                        baseline_patch.len() == offset_patch.len(),
                        "translated corner node count changed"
                    );
                    for (baseline_node, offset_node) in baseline_patch.iter().zip(offset_patch) {
                        let local_m =
                            (DVec3::from_array(baseline_node.position_local_m.map(f64::from))
                                - DVec3::from_array(offset_node.position_local_m.map(f64::from)))
                            .length();
                        let view_m =
                            (DVec3::from_array(baseline_node.position_view_m.map(f64::from))
                                - DVec3::from_array(offset_node.position_view_m.map(f64::from)))
                            .length();
                        let normal_rad = angle(
                            DVec3::from_array(baseline_node.normal_body.map(f64::from)),
                            DVec3::from_array(offset_node.normal_body.map(f64::from)),
                        );
                        let material_l2 = (0..4)
                            .map(|channel| {
                                (f64::from(baseline_node.material[channel])
                                    - f64::from(offset_node.material[channel]))
                                .powi(2)
                            })
                            .sum::<f64>()
                            .sqrt();
                        offset_residuals.record(local_m, view_m, normal_rad, material_l2);
                        for (criterion, value, limit) in [
                            ("offset_local", local_m, 1.0e-3),
                            ("offset_view", view_m, 1.0e-3),
                            ("offset_normal", normal_rad, 1.0e-3),
                            ("offset_material", material_l2, 1.0e-5),
                        ] {
                            if value > limit {
                                violations.push(format!(
                                    "{} {criterion}={value} limit={limit}",
                                    fixture.label
                                ));
                            }
                        }
                    }
                }
            }
        }

        let build_time_ms: f64 = build_records
            .iter()
            .map(|record| record["measured_wall_elapsed_ms"].as_f64().unwrap_or(0.0))
            .sum();
        let authoritative_queries: u64 = build_records
            .iter()
            .map(|record| record["authoritative_query_count"].as_u64().unwrap_or(0))
            .sum();
        let transition_edges: &[PatchEdge] = match fixture.cover {
            CoverShape::FourChildren => &[],
            CoverShape::ParentWithUMaxNeighbor => &[PatchEdge::UMax],
            CoverShape::ParentWithUMaxAndVMaxNeighbors => &[PatchEdge::UMax, PatchEdge::VMax],
        };
        fixtures_report.push(serde_json::json!({
            "label": fixture.label,
            "body_identity": BODY_IDENTITY,
            "algorithm": "RockyV5",
            "seed": 0,
            "radius_m": fixture.radius_m,
            "common_offset_m": fixture.common_offset_m,
            "cover_shape": match fixture.cover {
                CoverShape::FourChildren => "four_children_then_split_one",
                CoverShape::ParentWithUMaxNeighbor => "parent_split_with_umax_neighbor",
                CoverShape::ParentWithUMaxAndVMaxNeighbors => "parent_split_with_umax_and_vmax_neighbors",
            },
            "parent_address": address_json(parent_address),
            "split_parent_address": address_json(split_parent),
            "initial_cover": initial.keys().copied().map(address_json).collect::<Vec<_>>(),
            "target_cover": target.keys().copied().map(address_json).collect::<Vec<_>>(),
            "face_transition_neighbors": transition_edges.iter().copied()
                .filter(|edge| parent_address.neighbor(*edge).address.face() != parent_address.face())
                .map(|edge| neighbor_json(parent_address, edge)).collect::<Vec<_>>(),
            "cells": CELLS,
            "initial_leaf_count": initial.len(),
            "mixed_leaf_count": target.len(),
            "resident_tile_count_including_parents": all_tiles.len(),
            "build_total_wall_ms": build_time_ms,
            "authoritative_query_count": authoritative_queries,
            "builds": build_records,
            "gpu_vs_world_f64_reconstruction": residuals.json(),
            "gpu_mixed_lod_seams": seam_residuals.json(),
            "translation_invariance": offset_residuals.json(),
            "tile_upload_bytes_by_fraction": tile_uploads,
            "boundary_upload_bytes_by_fraction": boundary_uploads,
        }));
    }

    let evidence = serde_json::json!({
        "schema": "mundaris.regional_world_gpu.v1",
        "gpu_adapter": renderer.adapter_name(),
        "gpu_backend": renderer.adapter_backend(),
        "position_local_contract": "GPU and reconstruct_patch_node both return parent-local; child-surface positions include the own-to-parent anchor delta",
        "limits": {
            "local_position_m": 1.0e-3,
            "view_position_m": 1.0e-3,
            "normal_angle_radians": 1.0e-3,
            "material_l2": 1.0e-5,
        },
        "fixtures": fixtures_report,
        "violations": violations,
    });
    println!("{}", serde_json::to_string_pretty(&evidence)?);
    let path = write_evidence(&evidence)?;
    println!("wrote regional world GPU evidence to {}", path.display());
    ensure!(
        violations.is_empty(),
        "{} GPU parity/seam criteria failed",
        violations.len()
    );
    Ok(())
}
