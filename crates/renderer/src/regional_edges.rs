//! Canonical CPU preparation for shared edges of resident regional tile covers.
//!
//! The output is disposable geometry preparation. It contains no animation or
//! per-frame transition state; callers rebuild it when the resident topology or
//! its tile payloads change.

use crate::resident_tile::{TileData, TileGeometryError};
use glam::DVec3;
use mundaris_math::surface::{CubePatchAddress, CubeSampleKey, PatchEdge};
use std::collections::BTreeMap;
use std::sync::Arc;

/// One f64 endpoint relative to the owning tile's chart-center anchor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoundaryVertex {
    pub position_local_m: DVec3,
    /// Raw varying; normalization happens after triangle interpolation.
    pub normal_varying_body: DVec3,
    pub material: [f64; 4],
}

/// Closed regular-grid perimeter, ordered by [`PatchEdge::ALL`].
#[derive(Debug, Clone, PartialEq)]
pub struct TileBoundary {
    pub edges: [Vec<BoundaryVertex>; 4],
}

impl TileBoundary {
    pub fn edge(&self, edge: PatchEdge) -> &[BoundaryVertex] {
        &self.edges[edge_index(edge)]
    }
}

/// Validate and prepare canonical shared boundary vertices for a resident cover.
///
/// Tiles must use the same authority identity and same power-of-two grid. Grids
/// above 32 cells use recursively subdivided virtual patches for canonical keys.
/// Cover edges without a resident neighbor are treated as the regional perimeter.
pub fn build_boundaries(
    tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
) -> Result<BTreeMap<CubePatchAddress, TileBoundary>, TileGeometryError> {
    if tiles.is_empty() {
        return Err(TileGeometryError::InvalidTile);
    }
    let mut cells = None;
    let mut identity = None;
    for (address, tile) in tiles {
        tile.validate()?;
        if tile.key.address != *address || !tile.key.cells.is_power_of_two() {
            return Err(TileGeometryError::InvalidTile);
        }
        let tile_identity = (
            tile.key.body_identity,
            &tile.key.definition_words,
            tile.key.radius_bits,
            tile.key.surface_revision,
            tile.key.material_revision,
            tile.key.format_version,
            tile.key.filter_version,
        );
        if cells
            .replace(tile.key.cells)
            .is_some_and(|old| old != tile.key.cells)
            || identity
                .replace(tile_identity)
                .is_some_and(|old| old != tile_identity)
        {
            return Err(TileGeometryError::InvalidTile);
        }
    }
    validate_cover_edges(tiles)?;

    // The record retains the source tile/local value so translations between
    // tile anchors are formed by f64 anchor subtraction, never absolute f32.
    let mut canonical = BTreeMap::<CubeSampleKey, SampleRecord>::new();
    for (address, tile) in tiles {
        let n = tile.key.cells;
        for edge in PatchEdge::ALL {
            for along in 0..=n {
                let grid = edge.grid(along, n);
                let key = canonical_sample_key(*address, grid, n)?;
                let candidate = sample_record(tile, grid)?;
                match canonical.get(&key) {
                    Some(current) if source_order(current) <= (address.level(), *address) => {}
                    _ => {
                        canonical.insert(key, candidate);
                    }
                }
            }
        }
    }

    let mut result = BTreeMap::new();
    for (address, tile) in tiles {
        let n = tile.key.cells;
        let mut edges: [Vec<BoundaryVertex>; 4] =
            std::array::from_fn(|_| Vec::with_capacity(n as usize + 1));
        for edge in PatchEdge::ALL {
            let neighbor = address.neighbor(edge);
            let coarse =
                find_coarse_neighbor(tiles, neighbor.address, neighbor.edge, neighbor.reversed);
            for along in 0..=n {
                let vertex = if let Some((owner_address, owner_edge, reversed, child_half)) = coarse
                    .filter(|(owner_address, _, _, _)| owner_address.level() < address.level())
                {
                    let owner = tiles
                        .get(&owner_address)
                        .ok_or(TileGeometryError::InvalidTile)?;
                    let local_t = f64::from(along) / f64::from(n);
                    let owner_t = (f64::from(child_half)
                        + if reversed { 1.0 - local_t } else { local_t })
                        * 0.5;
                    interpolate_edge(owner, owner_edge, owner_t, &canonical, tile)?
                } else {
                    let key = canonical_sample_key(*address, edge.grid(along, n), n)?;
                    canonical
                        .get(&key)
                        .ok_or(TileGeometryError::InvalidTile)?
                        .for_tile(tile)?
                };
                edges[edge_index(edge)].push(vertex);
            }
        }
        result.insert(*address, TileBoundary { edges });
    }
    Ok(result)
}

/// Evaluate a regular-grid node, replacing its value when it lies on a prepared
/// boundary. Interior nodes are reconstructed directly from the tile.
pub fn evaluate_node(
    tile: &TileData,
    boundary: Option<&TileBoundary>,
    grid: [u32; 2],
) -> Result<BoundaryVertex, TileGeometryError> {
    let n = tile.key.cells;
    if n == 0 || grid.iter().any(|index| *index > n) {
        return Err(TileGeometryError::InvalidCoordinate);
    }
    if let Some(boundary) = boundary {
        let matching = [
            (grid[0] == 0, PatchEdge::UMin, grid[1]),
            (grid[0] == n, PatchEdge::UMax, grid[1]),
            (grid[1] == 0, PatchEdge::VMin, grid[0]),
            (grid[1] == n, PatchEdge::VMax, grid[0]),
        ];
        for (is_on_edge, edge, along) in matching {
            if is_on_edge {
                return boundary
                    .edge(edge)
                    .get(along as usize)
                    .copied()
                    .ok_or(TileGeometryError::InvalidTile);
            }
        }
    }
    sample_record(tile, grid)?.for_tile(tile)
}

/// Evaluate the tile's actual regular-grid triangles (`[a,b,c,b,d,c]`) at UV.
/// Optional prepared edge values replace boundary nodes before interpolation.
pub fn evaluate_triangle(
    tile: &TileData,
    boundary: Option<&TileBoundary>,
    st: [f64; 2],
) -> Result<BoundaryVertex, TileGeometryError> {
    let n = tile.key.cells;
    if n == 0
        || st
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err(TileGeometryError::InvalidCoordinate);
    }
    let p = st.map(|v| v * f64::from(n));
    let base = p.map(|v| (v.floor() as u32).min(n - 1));
    let [sx, sy] = [p[0] - f64::from(base[0]), p[1] - f64::from(base[1])];
    let a = base;
    let b = [base[0] + 1, base[1]];
    let c = [base[0], base[1] + 1];
    let d = [base[0] + 1, base[1] + 1];
    let (nodes, weights) = if sx + sy <= 1.0 {
        ([a, b, c], [1.0 - sx - sy, sx, sy])
    } else {
        ([b, d, c], [1.0 - sy, sx + sy - 1.0, 1.0 - sx])
    };
    let vertices = nodes
        .map(|node| evaluate_node(tile, boundary, node))
        .into_iter()
        .collect::<Result<Vec<_>, _>>()?;
    Ok(weight_vertices(&vertices, &weights))
}

/// Subdivide a parent's corrected surface onto one of its four child perimeters.
/// The returned positions are relative to the child's own f64 anchor.
pub fn subdivided_parent_boundary(
    parent: &TileData,
    outgoing: &TileBoundary,
    child: &TileData,
) -> Result<TileBoundary, TileGeometryError> {
    parent.validate_layout()?;
    child.validate_layout()?;
    if parent.key.cells != child.key.cells
        || parent.key.cells == 0
        || parent
            .key
            .address
            .children()
            .map_err(|_| TileGeometryError::InvalidTile)?
            .iter()
            .all(|address| *address != child.key.address)
        || !same_authority(parent, child)
    {
        return Err(TileGeometryError::InvalidTile);
    }
    let quadrant = child.key.address.coordinates().map(|value| value & 1);
    let delta = parent.anchor_position_body()? - child.anchor_position_body()?;
    let n = parent.key.cells;
    let mut edges: [Vec<BoundaryVertex>; 4] =
        std::array::from_fn(|_| Vec::with_capacity(n as usize + 1));
    for edge in PatchEdge::ALL {
        for along in 0..=n {
            let grid = edge.grid(along, n);
            let parent_st = [
                (f64::from(quadrant[0]) + f64::from(grid[0]) / f64::from(n)) * 0.5,
                (f64::from(quadrant[1]) + f64::from(grid[1]) / f64::from(n)) * 0.5,
            ];
            let mut vertex = evaluate_triangle(parent, Some(outgoing), parent_st)?;
            vertex.position_local_m += delta;
            edges[edge_index(edge)].push(vertex);
        }
    }
    Ok(TileBoundary { edges })
}

#[derive(Clone)]
struct SampleRecord {
    source_address: CubePatchAddress,
    source_anchor: DVec3,
    value: BoundaryVertex,
}

impl SampleRecord {
    fn for_tile(&self, tile: &TileData) -> Result<BoundaryVertex, TileGeometryError> {
        let translation = self.source_anchor - tile.anchor_position_body()?;
        let position = translation + self.value.position_local_m;
        position
            .is_finite()
            .then_some(BoundaryVertex {
                position_local_m: position,
                ..self.value
            })
            .ok_or(TileGeometryError::NonFinite)
    }
}

fn sample_record(tile: &TileData, grid: [u32; 2]) -> Result<SampleRecord, TileGeometryError> {
    let n = tile.key.cells;
    let st = [
        f64::from(grid[0]) / f64::from(n),
        f64::from(grid[1]) / f64::from(n),
    ];
    let position_local_m = tile.position_local(st)?;
    let normal_varying_body = tile.normal_local(grid)?;
    let material = tile.material(st)?.map(f64::from);
    if !position_local_m.is_finite()
        || !normal_varying_body.is_finite()
        || material.iter().any(|v| !v.is_finite())
    {
        return Err(TileGeometryError::NonFinite);
    }
    Ok(SampleRecord {
        source_address: tile.key.address,
        source_anchor: tile.anchor_position_body()?,
        value: BoundaryVertex {
            position_local_m,
            normal_varying_body,
            material,
        },
    })
}

fn source_order(record: &SampleRecord) -> (u8, CubePatchAddress) {
    (record.source_address.level(), record.source_address)
}

fn edge_index(edge: PatchEdge) -> usize {
    match edge {
        PatchEdge::UMin => 0,
        PatchEdge::UMax => 1,
        PatchEdge::VMin => 2,
        PatchEdge::VMax => 3,
    }
}

fn canonical_sample_key(
    address: CubePatchAddress,
    grid: [u32; 2],
    cells: u32,
) -> Result<CubeSampleKey, TileGeometryError> {
    if cells <= 32 {
        return address
            .sample_key(grid[0], grid[1], cells)
            .map_err(|_| TileGeometryError::InvalidTile);
    }
    if !cells.is_power_of_two() || cells > TileData::MAX_CELLS || grid.iter().any(|i| *i > cells) {
        return Err(TileGeometryError::InvalidTile);
    }
    let half = cells / 2;
    let quadrant = [u32::from(grid[0] >= half), u32::from(grid[1] >= half)];
    let local = [grid[0] - quadrant[0] * half, grid[1] - quadrant[1] * half];
    let index = (quadrant[0] + 2 * quadrant[1]) as usize;
    let child = address
        .children()
        .map_err(|_| TileGeometryError::InvalidTile)?[index];
    canonical_sample_key(child, local, half)
}

fn same_authority(a: &TileData, b: &TileData) -> bool {
    a.key.body_identity == b.key.body_identity
        && a.key.definition_words == b.key.definition_words
        && a.key.radius_bits == b.key.radius_bits
        && a.key.surface_revision == b.key.surface_revision
        && a.key.material_revision == b.key.material_revision
        && a.key.format_version == b.key.format_version
        && a.key.filter_version == b.key.filter_version
}

fn validate_cover_edges(
    tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
) -> Result<(), TileGeometryError> {
    for address in tiles.keys() {
        for edge in PatchEdge::ALL {
            let adjacent = address.neighbor(edge);
            let mut neighbors = Vec::with_capacity(4);
            if tiles.contains_key(&adjacent.address) {
                neighbors.push(adjacent.address);
            }
            if let Some(parent) = adjacent.address.parent().filter(|p| tiles.contains_key(p)) {
                neighbors.push(parent);
            }
            if let Ok(children) = adjacent.address.children() {
                neighbors.extend(
                    children
                        .into_iter()
                        .filter(|child| tiles.contains_key(child)),
                );
            }
            for neighbor in neighbors {
                if neighbor == *address || address.contains(neighbor) || neighbor.contains(*address)
                {
                    return Err(TileGeometryError::InvalidTile);
                }
                if address.level().abs_diff(neighbor.level()) > 1 {
                    return Err(TileGeometryError::InvalidTile);
                }
            }
        }
    }
    // A balanced quadtree has no ancestor/descendant overlap, including on a
    // face interior where no edge query would otherwise expose the conflict.
    let addresses: Vec<_> = tiles.keys().copied().collect();
    for address in &addresses {
        let mut parent = address.parent();
        while let Some(candidate) = parent {
            if tiles.contains_key(&candidate) {
                return Err(TileGeometryError::InvalidTile);
            }
            parent = candidate.parent();
        }
    }
    Ok(())
}

fn find_coarse_neighbor(
    tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
    same_level_neighbor: CubePatchAddress,
    neighbor_edge: PatchEdge,
    reversed: bool,
) -> Option<(CubePatchAddress, PatchEdge, bool, u32)> {
    let address = same_level_neighbor
        .parent()
        .filter(|parent| tiles.contains_key(parent))?;
    if address.level() + 1 != same_level_neighbor.level() {
        return None;
    }
    let [x, y] = same_level_neighbor.coordinates();
    let (edge, half) = match neighbor_edge {
        PatchEdge::UMin if x & 1 == 0 => (PatchEdge::UMin, y & 1),
        PatchEdge::UMax if x & 1 == 1 => (PatchEdge::UMax, y & 1),
        PatchEdge::VMin if y & 1 == 0 => (PatchEdge::VMin, x & 1),
        PatchEdge::VMax if y & 1 == 1 => (PatchEdge::VMax, x & 1),
        _ => return None,
    };
    Some((address, edge, reversed, half))
}

fn interpolate_edge(
    owner: &TileData,
    edge: PatchEdge,
    t: f64,
    canonical: &BTreeMap<CubeSampleKey, SampleRecord>,
    destination: &TileData,
) -> Result<BoundaryVertex, TileGeometryError> {
    let n = owner.key.cells;
    let coordinate = t.clamp(0.0, 1.0) * f64::from(n);
    let i0 = (coordinate.floor() as u32).min(n);
    let i1 = (i0 + 1).min(n);
    let blend = coordinate - f64::from(i0);
    let node = |index: u32| -> Result<BoundaryVertex, TileGeometryError> {
        let grid = edge.grid(index, n);
        let key = canonical_sample_key(owner.key.address, grid, n)?;
        canonical
            .get(&key)
            .ok_or(TileGeometryError::InvalidTile)?
            .for_tile(destination)
    };
    let a = node(i0)?;
    if i0 == i1 {
        return Ok(a);
    }
    let b = node(i1)?;
    Ok(weight_vertices(&[a, b], &[1.0 - blend, blend]))
}

fn weight_vertices(vertices: &[BoundaryVertex], weights: &[f64]) -> BoundaryVertex {
    let mut result = BoundaryVertex {
        position_local_m: DVec3::ZERO,
        normal_varying_body: DVec3::ZERO,
        material: [0.0; 4],
    };
    for (vertex, weight) in vertices.iter().zip(weights) {
        result.position_local_m += vertex.position_local_m * *weight;
        result.normal_varying_body += vertex.normal_varying_body * *weight;
        for (out, value) in result.material.iter_mut().zip(vertex.material) {
            *out += value * *weight;
        }
    }
    result
}
