use glam::DVec3;
use mundaris_math::surface::{CubeFace, CubePatchAddress, PatchEdge};
use mundaris_renderer::regional_edges::{
    BoundaryVertex, build_boundaries, evaluate_node, evaluate_triangle,
};
use mundaris_renderer::resident_tile::{TileData, TileKey, TileTexel};
use std::collections::BTreeMap;
use std::sync::Arc;

fn tile(address: CubePatchAddress, profile: f32) -> TileData {
    tile_with_cells(address, profile, 4)
}

fn tile_with_cells(address: CubePatchAddress, profile: f32, cells: u32) -> TileData {
    let stride = cells as usize + 3;
    let mut texels = Vec::with_capacity(stride * stride);
    for y in 0..stride {
        for x in 0..stride {
            let u = (x as f32 - 1.0) / cells as f32;
            let v = (y as f32 - 1.0) / cells as f32;
            let material = [profile, 1.0 - profile, 0.0, 0.0];
            texels.push(TileTexel {
                radial_offset_m: profile * 80.0 + u * 3.0 + v * 7.0,
                material,
            });
        }
    }
    TileData {
        key: TileKey {
            body_identity: 17,
            definition_words: vec![3, 9],
            radius_bits: 6_000_000.0_f64.to_bits(),
            surface_revision: 4,
            material_revision: 2,
            format_version: 1,
            filter_version: 1,
            address,
            cells,
        },
        anchor_radius_m: 6_000_000.0,
        min_max_radial_offset_m: [-1000.0, 1000.0],
        texels,
    }
}

fn close_vertex(a: BoundaryVertex, b: BoundaryVertex) {
    assert!((a.position_local_m - b.position_local_m).length() < 1.0e-8);
    assert!((a.normal_varying_body - b.normal_varying_body).length() < 1.0e-8);
    for (a, b) in a.material.into_iter().zip(b.material) {
        assert!((a - b).abs() < 1.0e-8);
    }
}

fn translated(vertex: BoundaryVertex, from: &TileData, to: &TileData) -> BoundaryVertex {
    BoundaryVertex {
        position_local_m: vertex.position_local_m + from.anchor_position_body().unwrap()
            - to.anchor_position_body().unwrap(),
        ..vertex
    }
}

#[test]
fn fine_edges_interpolate_the_actual_coarse_polyline_and_raw_attributes() {
    let coarse = CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 0, 0).unwrap();
    let neighbor_region = CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 1, 0).unwrap();
    let [lower, _, upper, _] = neighbor_region.children().unwrap();
    let mut tiles = BTreeMap::new();
    tiles.insert(coarse, Arc::new(tile(coarse, 0.15)));
    tiles.insert(lower, Arc::new(tile(lower, 0.70)));
    tiles.insert(upper, Arc::new(tile(upper, 0.92)));

    let boundaries = build_boundaries(&tiles).unwrap();
    let coarse_tile = &tiles[&coarse];
    let coarse_boundary = &boundaries[&coarse];
    for (fine_address, reversed, half) in [(lower, false, 0.0), (upper, false, 0.5)] {
        let fine = &tiles[&fine_address];
        let fine_edge = boundaries[&fine_address].edge(PatchEdge::UMin);
        for (i, actual) in fine_edge.iter().copied().enumerate() {
            let local = i as f64 / 4.0;
            let coarse_t = half + (if reversed { 1.0 - local } else { local }) * 0.5;
            let expected =
                evaluate_triangle(coarse_tile, Some(coarse_boundary), [1.0, coarse_t]).unwrap();
            close_vertex(actual, translated(expected, coarse_tile, fine));
        }
    }
}

#[test]
fn virtual_sample_keys_preserve_shared_nodes_for_64_and_128_cell_tiles() {
    for cells in [64, 128] {
        let coarse = CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 0, 0).unwrap();
        let neighbor_region = CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 1, 0).unwrap();
        let [lower, _, upper, _] = neighbor_region.children().unwrap();
        let tiles = BTreeMap::from([
            (coarse, Arc::new(tile_with_cells(coarse, 0.12, cells))),
            (lower, Arc::new(tile_with_cells(lower, 0.81, cells))),
            (upper, Arc::new(tile_with_cells(upper, 0.94, cells))),
        ]);
        let boundaries = build_boundaries(&tiles).unwrap();
        for (fine_address, offset) in [(lower, 0), (upper, cells / 2)] {
            let fine = &tiles[&fine_address];
            let fine_edge = boundaries[&fine_address].edge(PatchEdge::UMin);
            let coarse_edge = boundaries[&coarse].edge(PatchEdge::UMax);
            for along in (0..=cells).step_by(2) {
                let coarse_index = offset + along / 2;
                close_vertex(
                    fine_edge[along as usize],
                    translated(coarse_edge[coarse_index as usize], &tiles[&coarse], fine),
                );
            }
        }
    }
}

#[test]
fn cube_corner_uses_the_coarsest_incident_source_across_face_seams() {
    let mut tiles = BTreeMap::new();
    let coarse = CubePatchAddress::root(CubeFace::PositiveZ);
    let source = Arc::new(tile(coarse, 0.23));
    tiles.insert(coarse, source.clone());
    for face in CubeFace::ALL {
        if face == CubeFace::PositiveZ {
            continue;
        }
        for child in CubePatchAddress::root(face).children().unwrap() {
            tiles.insert(
                child,
                Arc::new(tile(child, 0.41 + child.coordinates()[0] as f32 * 0.1)),
            );
        }
    }

    let boundaries = build_boundaries(&tiles).unwrap();
    let source_boundary = &boundaries[&coarse];
    let n = source.key.cells;
    for edge in PatchEdge::ALL {
        for along in [0, n] {
            let source_value = source_boundary.edge(edge)[along as usize];
            let source_key = coarse
                .sample_key(edge.grid(along, n)[0], edge.grid(along, n)[1], n)
                .unwrap();
            for (address, tile) in &tiles {
                for other_edge in PatchEdge::ALL {
                    for other_along in [0, tile.key.cells] {
                        let grid = other_edge.grid(other_along, tile.key.cells);
                        if address
                            .sample_key(grid[0], grid[1], tile.key.cells)
                            .unwrap()
                            == source_key
                        {
                            let actual = boundaries[address].edge(other_edge)[other_along as usize];
                            close_vertex(actual, translated(source_value, &source, tile));
                        }
                    }
                }
            }
        }
    }

    // Every root/fine seam, including reversed face transitions, has identical
    // canonical values at all exact shared nodes.
    for (address, tile) in &tiles {
        for edge in PatchEdge::ALL {
            let neighbor = address.neighbor(edge);
            let Some(other) = tiles.get(&neighbor.address) else {
                continue;
            };
            if other.key.address.level() != address.level() {
                continue;
            }
            for along in 0..=tile.key.cells {
                let mapped = if neighbor.reversed {
                    tile.key.cells - along
                } else {
                    along
                };
                let a = boundaries[address].edge(edge)[along as usize];
                let b = boundaries[&neighbor.address].edge(neighbor.edge)[mapped as usize];
                let anchor_delta =
                    tile.anchor_position_body().unwrap() - other.anchor_position_body().unwrap();
                close_vertex(
                    a,
                    BoundaryVertex {
                        position_local_m: b.position_local_m - anchor_delta,
                        ..b
                    },
                );
            }
        }
    }
}

#[test]
fn triangle_evaluation_uses_corrected_boundary_nodes() {
    let address = CubePatchAddress::root(CubeFace::PositiveZ);
    let tile = tile(address, 0.34);
    let mut boundaries =
        build_boundaries(&BTreeMap::from([(address, Arc::new(tile.clone()))])).unwrap();
    let boundary = boundaries.get_mut(&address).unwrap();
    boundary.edges[0][2].material = [0.0, 0.0, 1.0, 0.0];
    boundary.edges[0][2].normal_varying_body = DVec3::X * 3.0;
    let node = evaluate_node(&tile, Some(boundary), [0, 2]).unwrap();
    assert_eq!(node.material, [0.0, 0.0, 1.0, 0.0]);
    assert_eq!(node.normal_varying_body, DVec3::X * 3.0);
    let interpolated = evaluate_triangle(&tile, Some(boundary), [0.0, 0.5]).unwrap();
    assert!(interpolated.material[2] > 0.99);
    assert!(interpolated.normal_varying_body.x > 2.99);
}

#[test]
fn overlapping_or_unbalanced_cover_is_rejected() {
    let parent = CubePatchAddress::root(CubeFace::PositiveZ);
    let child = parent.children().unwrap()[0];
    let overlap = BTreeMap::from([
        (parent, Arc::new(tile(parent, 0.2))),
        (child, Arc::new(tile(child, 0.3))),
    ]);
    assert!(build_boundaries(&overlap).is_err());

    let coarse = CubePatchAddress::root(CubeFace::PositiveZ);
    let deep = CubePatchAddress::try_new(CubeFace::PositiveZ, 2, 0, 0).unwrap();
    let unbalanced = BTreeMap::from([
        (coarse, Arc::new(tile(coarse, 0.2))),
        (deep, Arc::new(tile(deep, 0.3))),
    ]);
    assert!(build_boundaries(&unbalanced).is_err());
}
