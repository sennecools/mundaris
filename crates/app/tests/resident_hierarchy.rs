use std::sync::{Arc, OnceLock};

use glam::{DMat3, DVec3};
use mundaris_app::resident_terrain::{ResidentTileBuilder, TileBuildIdentity};
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_renderer::{HierarchyVertex, ResidentHierarchyDraw, TileDraw, TileSlotState};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};

const CELLS: u32 = 8;
static CANONICAL_HIERARCHY: OnceLock<ResidentHierarchyDraw> = OnceLock::new();

fn generator(radius_m: f64) -> (SurfaceDefinition, SurfaceGenerator) {
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(0x2b00_0001),
        TerrainSeed(0x2b00_0002),
        SurfaceAlgorithm::RockyV5,
    );
    let generator = SurfaceGenerator::new(&definition, radius_m).unwrap();
    (definition, generator)
}

fn draw(tile: Arc<mundaris_app::resident_terrain::TileData>, anchor_view_m: DVec3) -> TileDraw {
    let mut slot = TileSlotState::default();
    let publication = slot.request(&tile.key).unwrap();
    TileDraw {
        tile,
        publication,
        anchor_view_m,
        body_to_view: DMat3::IDENTITY,
        mode: 0,
        sun_body: DVec3::Z,
    }
}

fn build_hierarchy(radius_m: f64, level: u8, x: u32, y: u32) -> ResidentHierarchyDraw {
    let (definition, generator) = generator(radius_m);
    let address = CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y).unwrap();
    let identity = TileBuildIdentity {
        body_identity: definition.identity().0,
        surface_revision: 2,
        material_revision: definition.material_identity(),
    };
    let (parent, _) = ResidentTileBuilder::build(&generator, identity, address, CELLS).unwrap();
    let parent = Arc::new(parent);
    let parent_draw = draw(Arc::clone(&parent), DVec3::ZERO);
    let children_addresses = address.children().unwrap();
    let children = std::array::from_fn(|index| {
        let (child, _) =
            ResidentTileBuilder::build(&generator, identity, children_addresses[index], CELLS)
                .unwrap();
        let child = Arc::new(child);
        let anchor_delta =
            child.anchor_position_body().unwrap() - parent.anchor_position_body().unwrap();
        Some(draw(child, anchor_delta))
    });
    ResidentHierarchyDraw {
        parent: parent_draw,
        children,
        draw_children: true,
        morph_fraction: 0.0,
    }
}

fn canonical_hierarchy() -> ResidentHierarchyDraw {
    CANONICAL_HIERARCHY
        .get_or_init(|| build_hierarchy(80_000.0, 9, 157, 39))
        .clone()
}

fn angle(a: DVec3, b: DVec3) -> f64 {
    a.cross(b).length().atan2(a.dot(b))
}

fn child_endpoint(
    hierarchy: &ResidentHierarchyDraw,
    child: usize,
    grid: [u32; 2],
) -> HierarchyVertex {
    let draw = hierarchy.children[child].as_ref().unwrap();
    let st = grid.map(|value| f64::from(value) / f64::from(CELLS));
    HierarchyVertex {
        position_parent_local_m: hierarchy.child_anchor_delta_body(child).unwrap()
            + draw.tile.position_local(st).unwrap(),
        normal_body: draw.tile.normal_local(grid).unwrap(),
        normal_varying_body: draw.tile.normal_local(grid).unwrap(),
        material: draw.tile.material(st).unwrap().map(f64::from),
    }
}

/// Independent manual barycentric interpolation over renderer topology
/// [a,b,c,b,d,c]. Deliberately reads tile nodes directly rather than calling
/// ResidentHierarchyDraw::parent_triangle.
fn manual_parent_triangle(hierarchy: &ResidentHierarchyDraw, st: [f64; 2]) -> HierarchyVertex {
    let cells = hierarchy.parent.tile.key.cells;
    let gx = st[0] * f64::from(cells);
    let gy = st[1] * f64::from(cells);
    let ix = (gx.floor() as u32).min(cells - 1);
    let iy = (gy.floor() as u32).min(cells - 1);
    let u = gx - f64::from(ix);
    let v = gy - f64::from(iy);
    let nodes = [[ix, iy], [ix + 1, iy], [ix, iy + 1], [ix + 1, iy + 1]];
    let (triangle, weights) = if u + v <= 1.0 {
        ([0usize, 1, 2], [1.0 - u - v, u, v])
    } else {
        ([1usize, 3, 2], [1.0 - v, u + v - 1.0, 1.0 - u])
    };
    let mut result = HierarchyVertex {
        position_parent_local_m: DVec3::ZERO,
        normal_body: DVec3::ZERO,
        normal_varying_body: DVec3::ZERO,
        material: [0.0; 4],
    };
    for (vertex, weight) in triangle.into_iter().zip(weights) {
        let grid = nodes[vertex];
        let node_st = grid.map(|value| f64::from(value) / f64::from(cells));
        result.position_parent_local_m +=
            hierarchy.parent.tile.position_local(node_st).unwrap() * weight;
        result.normal_varying_body += hierarchy.parent.tile.normal_local(grid).unwrap() * weight;
        let material = hierarchy.parent.tile.material(node_st).unwrap();
        for (channel, value) in material.into_iter().enumerate() {
            result.material[channel] += f64::from(value) * weight;
        }
    }
    result.normal_body = result.normal_varying_body.normalize();
    result
}

#[test]
fn hierarchy_tiles_match_child_addresses_and_scaled_face_edge_corner_regions() {
    // Fixed +Z face samples cover an interior patch, a face edge, and a cube corner
    // at the relevant scaled radius/LOD fixtures.
    for (radius, level, x, y) in [
        (80_000.0, 9, 157, 39),
        (6_371_000.0, 15, (1 << 15) - 1, (1 << 14) / 2),
        (70_000_000.0, 19, 0, 0),
    ] {
        let hierarchy = build_hierarchy(radius, level, x, y);
        hierarchy.validate().unwrap();
        let expected = CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y)
            .unwrap()
            .children()
            .unwrap();
        for (child, expected_address) in expected.iter().enumerate() {
            let tile = &hierarchy.children[child].as_ref().unwrap().tile;
            assert_eq!(tile.key.address, *expected_address);
            assert_eq!(tile.key.cells, CELLS);
            for node in [[0, 0], [CELLS, 0], [0, CELLS], [CELLS, CELLS]] {
                let child_st = node.map(|value| f64::from(value) / f64::from(CELLS));
                let q = ResidentHierarchyDraw::quadrant(child).unwrap();
                let parent_st = [
                    (f64::from(q[0]) + child_st[0]) * 0.5,
                    (f64::from(q[1]) + child_st[1]) * 0.5,
                ];
                let child_position =
                    tile.anchor_position_body().unwrap() + tile.position_local(child_st).unwrap();
                let parent_position = hierarchy.parent.tile.anchor_position_body().unwrap()
                    + hierarchy.parent.tile.position_local(parent_st).unwrap();
                assert!(
                    child_position
                        .normalize()
                        .distance(parent_position.normalize())
                        < 2.0e-12,
                    "child {child} node {node:?} differs in cube-chart direction"
                );
            }
        }
    }
}

#[test]
fn children_share_filtered_boundaries_through_parent_anchor_and_child_endpoint() {
    let mut hierarchy = canonical_hierarchy();
    hierarchy.morph_fraction = 1.0;
    hierarchy.validate().unwrap();
    let edges = [
        (0usize, [CELLS, 0], 1usize, [0, 0], [0, 1]),
        (0, [0, CELLS], 2, [0, 0], [1, 0]),
        (1, [0, CELLS], 3, [0, 0], [1, 0]),
        (2, [CELLS, 0], 3, [0, 0], [0, 1]),
    ];
    for (left, left_start, right, right_start, step) in edges {
        for t in 0..=CELLS {
            let left_grid = [left_start[0] + step[0] * t, left_start[1] + step[1] * t];
            let right_grid = [right_start[0] + step[0] * t, right_start[1] + step[1] * t];
            let a = hierarchy.reconstruct_patch(left + 1, left_grid).unwrap();
            let b = hierarchy.reconstruct_patch(right + 1, right_grid).unwrap();
            assert!(
                (a.position_parent_local_m - b.position_parent_local_m).length() <= 0.001,
                "child border position mismatch {left}/{right} at {t}"
            );
            assert!(
                angle(a.normal_body, b.normal_body) <= 0.001,
                "child border normal mismatch {left}/{right} at {t}"
            );
            assert!(
                (a.normal_varying_body - b.normal_varying_body).length() <= 0.001,
                "child border raw normal mismatch {left}/{right} at {t}"
            );
            let material_error = a
                .material
                .iter()
                .zip(b.material)
                .map(|(x, y)| (x - y).powi(2))
                .sum::<f64>()
                .sqrt();
            assert!(
                material_error <= 1.0e-6,
                "child border material mismatch {left}/{right} at {t}: {material_error}"
            );

            let own_a = child_endpoint(&hierarchy, left, left_grid);
            let own_b = child_endpoint(&hierarchy, right, right_grid);
            assert!((a.position_parent_local_m - own_a.position_parent_local_m).length() < 1.0e-9);
            assert!(angle(a.normal_body, own_a.normal_body) < 1.0e-10);
            assert!((b.position_parent_local_m - own_b.position_parent_local_m).length() < 1.0e-9);
            assert!(angle(b.normal_body, own_b.normal_body) < 1.0e-10);
        }
    }
    // The central four-child corner is checked independently from edge interiors.
    let center = [
        hierarchy.reconstruct_patch(1, [CELLS, CELLS]).unwrap(),
        hierarchy.reconstruct_patch(2, [0, CELLS]).unwrap(),
        hierarchy.reconstruct_patch(3, [CELLS, 0]).unwrap(),
        hierarchy.reconstruct_patch(4, [0, 0]).unwrap(),
    ];
    for candidate in &center[1..] {
        assert!(
            (center[0].position_parent_local_m - candidate.position_parent_local_m).length()
                <= 0.001
        );
        assert!(angle(center[0].normal_body, candidate.normal_body) <= 0.001);
        let material_error = center[0]
            .material
            .iter()
            .zip(candidate.material)
            .map(|(x, y)| (x - y).powi(2))
            .sum::<f64>()
            .sqrt();
        assert!(material_error <= 1.0e-6);
    }
}

#[test]
fn morph_endpoints_match_actual_parent_triangles_and_child_tiles() {
    let mut hierarchy = canonical_hierarchy();
    hierarchy.morph_fraction = 0.0;
    hierarchy.validate().unwrap();
    // Every child grid node at t=0 preserves the parent's actual triangle
    // position, smooth-normal varying, and linearly filtered material.
    for child in 0..4 {
        for j in 0..=CELLS {
            for i in 0..=CELLS {
                let grid = [i, j];
                let q = ResidentHierarchyDraw::quadrant(child).unwrap();
                let st = [
                    (f64::from(q[0]) + f64::from(i) / f64::from(CELLS)) * 0.5,
                    (f64::from(q[1]) + f64::from(j) / f64::from(CELLS)) * 0.5,
                ];
                let actual = hierarchy.reconstruct_patch(child + 1, grid).unwrap();
                let manual = manual_parent_triangle(&hierarchy, st);
                assert!(
                    (actual.position_parent_local_m - manual.position_parent_local_m).length()
                        < 1.0e-8
                );
                assert!(
                    (actual.normal_varying_body - manual.normal_varying_body).length() < 1.0e-12
                );
                assert!(
                    actual
                        .material
                        .iter()
                        .zip(manual.material)
                        .all(|(a, b)| (a - b).abs() < 1.0e-12)
                );
            }
        }
    }
    let probes = [(0usize, [1, 1]), (1, [3, 5]), (2, [5, 3]), (3, [7, 7])];
    for fraction in [0.0, 0.25, 0.5, 0.75, 1.0] {
        hierarchy.morph_fraction = fraction;
        hierarchy.validate().unwrap();
        for (child, grid) in probes {
            let vertex = hierarchy.reconstruct_patch(child + 1, grid).unwrap();
            let q = ResidentHierarchyDraw::quadrant(child).unwrap();
            let st = [
                (f64::from(q[0]) + f64::from(grid[0]) / f64::from(CELLS)) * 0.5,
                (f64::from(q[1]) + f64::from(grid[1]) / f64::from(CELLS)) * 0.5,
            ];
            let parent = manual_parent_triangle(&hierarchy, st);
            let own = child_endpoint(&hierarchy, child, grid);
            let expected_position = parent.position_parent_local_m * (1.0 - f64::from(fraction))
                + own.position_parent_local_m * f64::from(fraction);
            assert!((vertex.position_parent_local_m - expected_position).length() < 1.0e-8);
            let expected_raw_normal = parent.normal_varying_body * (1.0 - f64::from(fraction))
                + own.normal_varying_body * f64::from(fraction);
            assert!((vertex.normal_varying_body - expected_raw_normal).length() < 1.0e-12);
            assert!(angle(vertex.normal_body, expected_raw_normal.normalize()) < 1.0e-10);
            let expected_material: [f64; 4] = std::array::from_fn(|channel| {
                parent.material[channel] * (1.0 - f64::from(fraction))
                    + own.material[channel] * f64::from(fraction)
            });
            assert!(
                vertex
                    .material
                    .iter()
                    .zip(expected_material)
                    .all(|(a, b)| (a - b).abs() < 1.0e-12)
            );
            if fraction == 0.0 {
                assert!(
                    (vertex.position_parent_local_m - parent.position_parent_local_m).length()
                        < 1.0e-8
                );
                assert!(angle(vertex.normal_body, parent.normal_body) < 1.0e-10);
                assert!(
                    (vertex.normal_varying_body - parent.normal_varying_body).length() < 1.0e-12
                );
                assert!(
                    vertex
                        .material
                        .iter()
                        .zip(parent.material)
                        .all(|(a, b)| (a - b).abs() < 1.0e-12)
                );
            }
            if fraction == 1.0 {
                assert!(
                    (vertex.position_parent_local_m - own.position_parent_local_m).length()
                        < 1.0e-8
                );
                assert!(angle(vertex.normal_body, own.normal_body) < 1.0e-10);
                assert!((vertex.normal_varying_body - own.normal_varying_body).length() < 1.0e-12);
                assert!(
                    vertex
                        .material
                        .iter()
                        .zip(own.material)
                        .all(|(a, b)| (a - b).abs() < 1.0e-7)
                );
            }
        }
    }
}

#[test]
fn parent_stays_valid_when_children_are_missing_and_rejects_key_mismatch() {
    let mut hierarchy = canonical_hierarchy();
    hierarchy.children[1] = None;
    hierarchy.draw_children = false;
    hierarchy.validate().unwrap();
    assert!(
        hierarchy
            .reconstruct_patch(0, [CELLS / 2, CELLS / 2])
            .is_ok()
    );
    assert!(hierarchy.reconstruct_patch(2, [0, 0]).is_err());
    hierarchy.draw_children = true;
    assert!(hierarchy.validate().is_err());

    let mut mismatch = canonical_hierarchy();
    let (definition, generator) = generator(80_000.0);
    let parent_address = mismatch.parent.tile.key.address;
    let child_address = parent_address.children().unwrap()[0];
    let identity = TileBuildIdentity {
        body_identity: definition.identity().0,
        surface_revision: 2,
        material_revision: definition.material_identity(),
    };
    let (wrong_cells, _) =
        ResidentTileBuilder::build(&generator, identity, child_address, CELLS / 2).unwrap();
    let wrong_cells = Arc::new(wrong_cells);
    let anchor_delta = wrong_cells.anchor_position_body().unwrap()
        - mismatch.parent.tile.anchor_position_body().unwrap();
    mismatch.children[0] = Some(draw(wrong_cells, anchor_delta));
    assert!(mismatch.validate().is_err());
}

#[test]
fn coarse_child_triangles_preserve_parent_planes_and_raw_shading_fields() {
    let hierarchy = canonical_hierarchy();
    for child in 0..4 {
        for y in 0..CELLS {
            for x in 0..CELLS {
                for nodes in [
                    [[x, y], [x + 1, y], [x, y + 1]],
                    [[x + 1, y], [x + 1, y + 1], [x, y + 1]],
                ] {
                    // Raster interpolation at the child triangle's centroid
                    // must preserve the entire coarse triangle field, beyond
                    // just matching the newly inserted child vertices.
                    let mut st = [0.0; 2];
                    let mut position = DVec3::ZERO;
                    let mut raw_normal = DVec3::ZERO;
                    let mut material = [0.0; 4];
                    for node in nodes {
                        let uv = hierarchy.parent_st(child, node).unwrap();
                        st[0] += uv[0] / 3.0;
                        st[1] += uv[1] / 3.0;
                        let vertex = hierarchy.reconstruct_patch(child + 1, node).unwrap();
                        position += vertex.position_parent_local_m / 3.0;
                        raw_normal += vertex.normal_varying_body / 3.0;
                        for (a, b) in material.iter_mut().zip(vertex.material) {
                            *a += b / 3.0;
                        }
                    }
                    let parent = manual_parent_triangle(&hierarchy, st);
                    assert!(position.distance(parent.position_parent_local_m) < 1.0e-8);
                    assert!(raw_normal.distance(parent.normal_varying_body) < 1.0e-12);
                    assert!(
                        material
                            .iter()
                            .zip(parent.material)
                            .all(|(a, b)| (a - b).abs() < 1.0e-12)
                    );
                }
            }
        }
    }
}
