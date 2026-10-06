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

/// Incremental canonical boundary preparation for immutable resident covers.
/// The cache retains only the latest cover, and clears itself on any failure.
#[derive(Default)]
pub struct RegionalBoundaryCache {
    tiles: BTreeMap<CubePatchAddress, Arc<TileData>>,
    sources:
        std::collections::HashMap<CubeSampleKey, BTreeMap<(u8, CubePatchAddress), SampleRecord>>,
    canonical: BTreeMap<CubeSampleKey, SampleRecord>,
    samples: std::collections::HashMap<CubePatchAddress, std::collections::HashSet<CubeSampleKey>>,
    dependencies:
        std::collections::HashMap<CubePatchAddress, std::collections::HashSet<CubeSampleKey>>,
    dependents:
        std::collections::HashMap<CubeSampleKey, std::collections::HashSet<CubePatchAddress>>,
    boundaries: BTreeMap<CubePatchAddress, TileBoundary>,
    last_rebuilt: usize,
}

impl RegionalBoundaryCache {
    pub fn build(
        &mut self,
        tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
    ) -> Result<BTreeMap<CubePatchAddress, TileBoundary>, TileGeometryError> {
        let result = self.update(tiles, 1);
        if result.is_err() {
            *self = Self::default();
        }
        result
    }

    /// Build a boundary cover while calculating independent dirty tile
    /// boundaries on at most two scoped workers. Cache/index mutation remains
    /// serialized, and results are applied in address order for determinism.
    /// Worker counts outside 1..=2 are rejected and clear this candidate cache.
    pub fn build_parallel(
        &mut self,
        tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
        max_workers: usize,
    ) -> Result<BTreeMap<CubePatchAddress, TileBoundary>, TileGeometryError> {
        if !(1..=2).contains(&max_workers) {
            *self = Self::default();
            return Err(TileGeometryError::InvalidTile);
        }
        let result = self.update(tiles, max_workers);
        if result.is_err() {
            *self = Self::default();
        }
        result
    }

    pub fn last_rebuilt_count(&self) -> usize {
        self.last_rebuilt
    }

    /// Retained buffers and index storage, excluding aliased tile payloads.
    /// Tree-node overhead is conservatively accounted per entry; this is not RSS.
    pub fn accounted_owned_bytes(&self) -> usize {
        use std::mem::size_of;
        fn hash_bytes<K, V>(capacity: usize) -> usize {
            capacity * (size_of::<K>() + size_of::<V>() + 1)
        }
        let source_entries = self.sources.values().map(BTreeMap::len).sum::<usize>();
        let sample_capacity = self
            .samples
            .values()
            .map(|keys| keys.capacity())
            .sum::<usize>();
        let dependency_capacity = self
            .dependencies
            .values()
            .map(|keys| keys.capacity())
            .sum::<usize>();
        let dependent_capacity = self
            .dependents
            .values()
            .map(|keys| keys.capacity())
            .sum::<usize>();
        self.boundaries
            .values()
            .map(|boundary| {
                size_of::<TileBoundary>()
                    + 64
                    + boundary
                        .edges
                        .iter()
                        .map(|edge| edge.capacity() * size_of::<BoundaryVertex>())
                        .sum::<usize>()
            })
            .sum::<usize>()
            + self.tiles.len() * (size_of::<CubePatchAddress>() + size_of::<Arc<TileData>>() + 64)
            + source_entries
                * (size_of::<(u8, CubePatchAddress)>() + size_of::<SampleRecord>() + 64)
            + self.canonical.len() * (size_of::<CubeSampleKey>() + size_of::<SampleRecord>() + 64)
            + hash_bytes::<CubeSampleKey, BTreeMap<(u8, CubePatchAddress), SampleRecord>>(
                self.sources.capacity(),
            )
            + hash_bytes::<CubePatchAddress, std::collections::HashSet<CubeSampleKey>>(
                self.samples.capacity() + self.dependencies.capacity(),
            )
            + hash_bytes::<CubeSampleKey, std::collections::HashSet<CubePatchAddress>>(
                self.dependents.capacity(),
            )
            + hash_bytes::<CubeSampleKey, ()>(sample_capacity + dependency_capacity)
            + hash_bytes::<CubePatchAddress, ()>(dependent_capacity)
    }

    fn update(
        &mut self,
        tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
        max_workers: usize,
    ) -> Result<BTreeMap<CubePatchAddress, TileBoundary>, TileGeometryError> {
        use std::collections::HashSet;
        let first = tiles
            .values()
            .next()
            .ok_or(TileGeometryError::InvalidTile)?;
        for (address, tile) in tiles {
            if tile.key.address != *address
                || !tile.key.cells.is_power_of_two()
                || tile.key.cells != first.key.cells
                || !same_authority(tile, first)
            {
                return Err(TileGeometryError::InvalidTile);
            }
        }
        validate_cover_edges(tiles)?;
        let removed: Vec<_> = self
            .tiles
            .iter()
            .filter_map(|(address, tile)| {
                (!tiles
                    .get(address)
                    .is_some_and(|next| Arc::ptr_eq(tile, next)))
                .then_some(*address)
            })
            .collect();
        let added: Vec<_> = tiles
            .iter()
            .filter_map(|(address, tile)| {
                (!self
                    .tiles
                    .get(address)
                    .is_some_and(|old| Arc::ptr_eq(tile, old)))
                .then_some(*address)
            })
            .collect();
        let mut changed_keys = HashSet::new();
        let mut dirty: HashSet<_> = added.iter().copied().collect();
        for address in &removed {
            if let Some(keys) = self.samples.remove(address) {
                for key in keys {
                    changed_keys.insert(key);
                    if let Some(sources) = self.sources.get_mut(&key) {
                        sources.remove(&(address.level(), *address));
                    }
                }
            }
            self.boundaries.remove(address);
        }
        for address in &added {
            let tile = &tiles[address];
            tile.validate()?;
            let mut keys = HashSet::new();
            for edge in PatchEdge::ALL {
                for along in 0..=tile.key.cells {
                    let grid = edge.grid(along, tile.key.cells);
                    let key = canonical_sample_key(*address, grid, tile.key.cells)?;
                    if keys.insert(key) {
                        self.sources
                            .entry(key)
                            .or_default()
                            .insert((address.level(), *address), sample_record(tile, grid)?);
                        changed_keys.insert(key);
                    }
                }
            }
            self.samples.insert(*address, keys);
        }
        // Dependencies include both coarse interpolation endpoints, including
        // the zero-weight endpoint. Topology changes can change the owner even
        // when the winning endpoint happens to have the same value.
        for key in &changed_keys {
            if let Some(dependents) = self.dependents.get(key) {
                dirty.extend(dependents);
            }
            if let Some(record) = self
                .sources
                .get(key)
                .and_then(|sources| sources.values().next())
            {
                self.canonical.insert(*key, record.clone());
            } else {
                self.canonical.remove(key);
                self.sources.remove(key);
            }
        }
        // Explicitly include all touching old/new edge neighbors. This covers
        // a coarse owner change even when its sample dependencies are replaced.
        for address in removed.iter().chain(&added) {
            for edge in PatchEdge::ALL {
                let neighbor = address.neighbor(edge).address;
                for candidate in std::iter::once(neighbor)
                    .chain(neighbor.parent())
                    .chain(neighbor.children().into_iter().flatten())
                {
                    if tiles.contains_key(&candidate) {
                        dirty.insert(candidate);
                    }
                }
            }
        }
        for address in removed.iter().chain(dirty.iter()) {
            if let Some(keys) = self.dependencies.remove(address) {
                for key in keys {
                    if let Some(dependents) = self.dependents.get_mut(&key) {
                        dependents.remove(address);
                        if dependents.is_empty() {
                            self.dependents.remove(&key);
                        }
                    }
                }
            }
        }
        self.last_rebuilt = 0;
        let mut dirty: Vec<_> = dirty.into_iter().collect();
        dirty.retain(|address| tiles.contains_key(address));
        dirty.sort_unstable();
        let rebuilt = calculate_dirty_boundaries(&dirty, tiles, &self.canonical, max_workers)?;
        for (address, boundary, keys) in rebuilt {
            for key in &keys {
                self.dependents.entry(*key).or_default().insert(address);
            }
            self.dependencies.insert(address, keys);
            self.boundaries.insert(address, boundary);
            self.last_rebuilt += 1;
        }
        self.tiles = tiles.clone();
        Ok(self.boundaries.clone())
    }
}

fn calculate_dirty_boundaries(
    dirty: &[CubePatchAddress],
    tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
    canonical: &BTreeMap<CubeSampleKey, SampleRecord>,
    max_workers: usize,
) -> Result<
    Vec<(
        CubePatchAddress,
        TileBoundary,
        std::collections::HashSet<CubeSampleKey>,
    )>,
    TileGeometryError,
> {
    if max_workers == 1 || dirty.len() < 2 {
        return calculate_dirty_subset(dirty, tiles, canonical);
    }

    let midpoint = dirty.len().div_ceil(2);
    let (first_addresses, second_addresses) = dirty.split_at(midpoint);
    let (first, second) = std::thread::scope(|scope| {
        let first_addresses = first_addresses.to_vec();
        let first_worker = std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn_scoped(scope, move || {
                calculate_dirty_subset(&first_addresses, tiles, canonical)
            })
            .map_err(|_| TileGeometryError::InvalidTile)?;
        let second_addresses = second_addresses.to_vec();
        let second_worker = match std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn_scoped(scope, move || {
                calculate_dirty_subset(&second_addresses, tiles, canonical)
            }) {
            Ok(worker) => worker,
            Err(_) => {
                let _ = first_worker.join();
                return Err(TileGeometryError::InvalidTile);
            }
        };
        let first_result = first_worker
            .join()
            .map_err(|_| TileGeometryError::InvalidTile);
        let second_result = second_worker
            .join()
            .map_err(|_| TileGeometryError::InvalidTile);
        let first = first_result??;
        let second = second_result??;
        Ok((first, second))
    })?;

    let mut rebuilt = first;
    rebuilt.extend(second);
    rebuilt.sort_unstable_by_key(|(address, _, _)| *address);
    Ok(rebuilt)
}

fn calculate_dirty_subset(
    addresses: &[CubePatchAddress],
    tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
    canonical: &BTreeMap<CubeSampleKey, SampleRecord>,
) -> Result<
    Vec<(
        CubePatchAddress,
        TileBoundary,
        std::collections::HashSet<CubeSampleKey>,
    )>,
    TileGeometryError,
> {
    let mut rebuilt = Vec::with_capacity(addresses.len());
    for address in addresses {
        let Some(tile) = tiles.get(address) else {
            continue;
        };
        let (boundary, keys) = cached_tile_boundary(*address, tile, tiles, canonical)?;
        rebuilt.push((*address, boundary, keys));
    }
    Ok(rebuilt)
}

fn cached_tile_boundary(
    address: CubePatchAddress,
    tile: &TileData,
    tiles: &BTreeMap<CubePatchAddress, Arc<TileData>>,
    canonical: &BTreeMap<CubeSampleKey, SampleRecord>,
) -> Result<(TileBoundary, std::collections::HashSet<CubeSampleKey>), TileGeometryError> {
    let n = tile.key.cells;
    let mut dependencies = std::collections::HashSet::new();
    let mut edges: [Vec<BoundaryVertex>; 4] =
        std::array::from_fn(|_| Vec::with_capacity(n as usize + 1));
    for edge in PatchEdge::ALL {
        let neighbor = address.neighbor(edge);
        let coarse =
            find_coarse_neighbor(tiles, neighbor.address, neighbor.edge, neighbor.reversed);
        for along in 0..=n {
            let vertex = if let Some((owner_address, owner_edge, reversed, child_half)) =
                coarse.filter(|(owner_address, _, _, _)| owner_address.level() < address.level())
            {
                let owner = tiles
                    .get(&owner_address)
                    .ok_or(TileGeometryError::InvalidTile)?;
                let local_t = f64::from(along) / f64::from(n);
                let owner_t =
                    (f64::from(child_half) + if reversed { 1.0 - local_t } else { local_t }) * 0.5;
                let coordinate = owner_t.clamp(0.0, 1.0) * f64::from(n);
                let i0 = (coordinate.floor() as u32).min(n);
                let i1 = (i0 + 1).min(n);
                dependencies.insert(canonical_sample_key(
                    owner_address,
                    owner_edge.grid(i0, n),
                    n,
                )?);
                dependencies.insert(canonical_sample_key(
                    owner_address,
                    owner_edge.grid(i1, n),
                    n,
                )?);
                interpolate_edge(owner, owner_edge, owner_t, canonical, tile)?
            } else {
                let key = canonical_sample_key(address, edge.grid(along, n), n)?;
                dependencies.insert(key);
                canonical
                    .get(&key)
                    .ok_or(TileGeometryError::InvalidTile)?
                    .for_tile(tile)?
            };
            edges[edge_index(edge)].push(vertex);
        }
    }
    Ok((TileBoundary { edges }, dependencies))
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    use crate::{TILE_FILTER_VERSION, TILE_FORMAT_VERSION, TileKey, TileTexel};
    use mundaris_math::surface::CubeFace;

    fn tile(address: CubePatchAddress, cells: u32) -> Arc<TileData> {
        let side = cells + 3;
        let [x, y] = address.coordinates();
        let count = 2.0_f64.powi(i32::from(address.level()));
        let texels = (0..side)
            .flat_map(|j| {
                (0..side).map(move |i| {
                    let u = (f64::from(x) + (f64::from(i) - 1.0) / f64::from(cells)) / count;
                    let v = (f64::from(y) + (f64::from(j) - 1.0) / f64::from(cells)) / count;
                    let a = (0.22 + 0.04 * u) as f32;
                    let b = (0.31 - 0.03 * u) as f32;
                    let c = (0.12 + 0.02 * v) as f32;
                    TileTexel {
                        radial_offset_m: (3.0 * (9.0 * u).sin() + 2.0 * (7.0 * v).cos()) as f32,
                        material: [a, b, c, 1.0 - a - b - c],
                    }
                })
            })
            .collect();
        Arc::new(TileData {
            key: TileKey {
                body_identity: 911,
                definition_words: vec![7, 19, 23],
                radius_bits: 70_000_000.0_f64.to_bits(),
                surface_revision: 3,
                material_revision: 5,
                format_version: TILE_FORMAT_VERSION,
                filter_version: TILE_FILTER_VERSION,
                address,
                cells,
            },
            anchor_radius_m: 70_000_000.0,
            min_max_radial_offset_m: [-5.0, 5.0],
            texels,
        })
    }

    #[test]
    fn incremental_boundaries_match_reference_through_cross_face_split_merge_and_stale_retry() {
        for cells in [2, 8, 32, 64] {
            let mut tiles: BTreeMap<_, _> = CubeFace::ALL
                .into_iter()
                .map(|face| {
                    let address = CubePatchAddress::root(face);
                    (address, tile(address, cells))
                })
                .collect();
            let roots = tiles.clone();
            let mut cache = RegionalBoundaryCache::default();
            let mut parallel_cache = RegionalBoundaryCache::default();
            assert_eq!(
                cache.build(&tiles).unwrap(),
                build_boundaries(&tiles).unwrap()
            );
            assert_eq!(
                parallel_cache.build_parallel(&tiles, 2).unwrap(),
                cache.boundaries
            );
            let mut history = vec![tiles.clone()];
            let corner_parent = CubePatchAddress::root(CubeFace::PositiveZ);
            let mut corner_split = tiles.clone();
            corner_split.remove(&corner_parent);
            for child in corner_parent.children().unwrap() {
                corner_split.insert(child, tile(child, cells));
            }
            assert!(validate_cover_edges(&corner_split).is_ok());
            let corner_expected = build_boundaries(&corner_split).unwrap();
            assert_eq!(cache.build(&corner_split).unwrap(), corner_expected);
            assert_eq!(
                parallel_cache.build_parallel(&corner_split, 2).unwrap(),
                corner_expected
            );
            assert_eq!(
                parallel_cache.build_parallel(&tiles, 2).unwrap(),
                build_boundaries(&tiles).unwrap()
            );
            assert_eq!(
                parallel_cache.build_parallel(&corner_split, 2).unwrap(),
                corner_expected
            );
            tiles = corner_split;
            history.push(tiles.clone());
            let mut random = 123456789_u64;
            for _ in 0..90 {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                let address = *tiles.keys().nth(random as usize % tiles.len()).unwrap();
                if address.level() >= 3 {
                    continue;
                }
                let mut next = tiles.clone();
                next.remove(&address);
                for child in address.children().unwrap() {
                    next.insert(child, tile(child, cells));
                }
                if validate_cover_edges(&next).is_err() {
                    continue;
                }
                let expected = build_boundaries(&next).unwrap();
                assert_eq!(cache.build(&next).unwrap(), expected);
                assert_eq!(parallel_cache.build_parallel(&next, 2).unwrap(), expected);
                // Stale candidates may be discarded; rebuilding from the old
                // acknowledged cover must undo every cached ownership change.
                assert_eq!(
                    cache.build(&tiles).unwrap(),
                    build_boundaries(&tiles).unwrap()
                );
                assert_eq!(
                    parallel_cache.build_parallel(&tiles, 2).unwrap(),
                    cache.boundaries
                );
                assert_eq!(cache.build(&next).unwrap(), expected);
                assert_eq!(parallel_cache.build_parallel(&next, 2).unwrap(), expected);
                tiles = next;
                history.push(tiles.clone());
            }
            for previous in history.into_iter().rev() {
                assert_eq!(
                    cache.build(&previous).unwrap(),
                    build_boundaries(&previous).unwrap()
                );
                assert_eq!(
                    parallel_cache.build_parallel(&previous, 2).unwrap(),
                    cache.boundaries
                );
                assert_eq!(cache.tiles.len(), previous.len());
                assert_eq!(cache.boundaries.len(), previous.len());
                assert_eq!(cache.dependencies.len(), previous.len());
                assert!(
                    cache
                        .dependents
                        .values()
                        .flatten()
                        .all(|address| previous.contains_key(address))
                );
            }
            assert_eq!(cache.tiles.len(), roots.len());
            cache.build(&roots).unwrap();
            parallel_cache.build_parallel(&roots, 2).unwrap();
            assert_eq!(cache.last_rebuilt_count(), 0);
            assert_eq!(parallel_cache.last_rebuilt_count(), 0);
        }
    }

    #[test]
    fn boundary_cache_checks_changed_payload_authority_and_recovers_after_failure() {
        let address = CubePatchAddress::root(CubeFace::PositiveZ);
        let mut tiles = BTreeMap::from([(address, tile(address, 8))]);
        let mut cache = RegionalBoundaryCache::default();
        let mut parallel_cache = RegionalBoundaryCache::default();
        cache.build(&tiles).unwrap();
        parallel_cache.build_parallel(&tiles, 2).unwrap();
        let mut changed = (*tiles[&address]).clone();
        changed.texels[9].radial_offset_m += 1.0;
        tiles.insert(address, Arc::new(changed));
        assert_eq!(
            cache.build(&tiles).unwrap(),
            build_boundaries(&tiles).unwrap()
        );
        assert_eq!(
            parallel_cache.build_parallel(&tiles, 2).unwrap(),
            cache.boundaries
        );
        assert_eq!(cache.last_rebuilt_count(), 1);
        let valid = tiles.clone();
        let mut invalid = (*tiles[&address]).clone();
        invalid.texels[0].material[0] = f32::NAN;
        tiles.insert(address, Arc::new(invalid));
        assert!(cache.build(&tiles).is_err());
        assert!(parallel_cache.build_parallel(&tiles, 2).is_err());
        assert!(cache.tiles.is_empty() && cache.sources.is_empty() && cache.canonical.is_empty());
        assert!(parallel_cache.tiles.is_empty() && parallel_cache.sources.is_empty());
        assert_eq!(
            cache.build(&valid).unwrap(),
            build_boundaries(&valid).unwrap()
        );
        assert_eq!(
            parallel_cache.build_parallel(&valid, 2).unwrap(),
            cache.boundaries
        );
        let other = CubePatchAddress::root(CubeFace::NegativeZ);
        let mut invalid = (*tile(other, 8)).clone();
        invalid.key.surface_revision += 1;
        tiles = valid;
        tiles.insert(other, Arc::new(invalid));
        assert!(cache.build(&tiles).is_err());
        assert!(parallel_cache.build_parallel(&tiles, 2).is_err());
    }

    #[test]
    fn parallel_boundary_cache_rejects_unsupported_worker_counts_and_recovers() {
        let address = CubePatchAddress::root(CubeFace::PositiveZ);
        let tiles = BTreeMap::from([(address, tile(address, 8))]);
        let mut cache = RegionalBoundaryCache::default();
        assert_eq!(
            cache.build_parallel(&tiles, 1).unwrap(),
            build_boundaries(&tiles).unwrap()
        );
        for workers in [0, 3, usize::MAX] {
            assert_eq!(
                cache.build_parallel(&tiles, workers),
                Err(TileGeometryError::InvalidTile)
            );
            assert!(cache.tiles.is_empty());
            assert_eq!(
                cache.build(&tiles).unwrap(),
                build_boundaries(&tiles).unwrap()
            );
        }
    }

    #[test]
    fn a_local_edit_rebuilds_only_boundary_dependents_in_a_large_cover() {
        let mut tiles = BTreeMap::new();
        for face in CubeFace::ALL {
            for x in 0..8 {
                for y in 0..8 {
                    let address = CubePatchAddress::try_new(face, 3, x, y).unwrap();
                    tiles.insert(address, tile(address, 8));
                }
            }
        }
        let mut cache = RegionalBoundaryCache::default();
        cache.build(&tiles).unwrap();
        let parent = CubePatchAddress::try_new(CubeFace::PositiveZ, 3, 0, 0).unwrap();
        tiles.remove(&parent);
        for child in parent.children().unwrap() {
            tiles.insert(child, tile(child, 8));
        }
        assert_eq!(
            cache.build(&tiles).unwrap(),
            build_boundaries(&tiles).unwrap()
        );
        assert!(cache.last_rebuilt_count() < 24);
        cache.build(&tiles).unwrap();
        assert_eq!(cache.last_rebuilt_count(), 0);
    }

    #[test]
    #[ignore = "matched CPU boundary preparation benchmark"]
    fn boundary_publication_cpu_benchmark() {
        let mut tiles = BTreeMap::new();
        for face in CubeFace::ALL {
            for x in 0..16 {
                for y in 0..16 {
                    let address = CubePatchAddress::try_new(face, 4, x, y).unwrap();
                    tiles.insert(address, tile(address, 32));
                }
            }
        }
        let parent = CubePatchAddress::try_new(CubeFace::PositiveZ, 4, 0, 0).unwrap();
        let mut target = tiles.clone();
        target.remove(&parent);
        for child in parent.children().unwrap() {
            target.insert(child, tile(child, 32));
        }
        let mut cache = RegionalBoundaryCache::default();
        cache.build(&tiles).unwrap();
        for iteration in 0..5 {
            let started = std::time::Instant::now();
            let expected = build_boundaries(&target).unwrap();
            let reference_ms = started.elapsed().as_secs_f64() * 1000.0;
            let started = std::time::Instant::now();
            let actual = cache.build(&target).unwrap();
            let owned_bytes = cache.accounted_owned_bytes();
            let cached_ms = started.elapsed().as_secs_f64() * 1000.0;
            assert_eq!(actual, expected);
            eprintln!(
                "boundary benchmark iteration={iteration} cells=32 cover={} reference_ms={reference_ms:.4} cached_ms={cached_ms:.4} rebuilt={} accounted_cache_bytes={owned_bytes}",
                target.len(),
                cache.last_rebuilt_count()
            );
            cache.build(&tiles).unwrap();
        }
    }
}
