//! Bounded GPU residency for a regional adaptive cover.
//!
//! Selection and worker scheduling live upstream. This module describes one
//! render transaction: desired uploads, the patches drawn from resident slots,
//! and the static edge endpoints that make mixed-level boundaries watertight.

use crate::regional_edges::{TileBoundary, evaluate_triangle};
use crate::resident_tile::{TileData, TileDraw, TileKey};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// Static endpoints published with a resident topology. `own_coarse` and
/// `own_fine` are expressed in the owning tile's local anchor; `parent` is the
/// outgoing perimeter of the parent tile, also in the parent's local anchor.
#[derive(Debug, Clone, PartialEq)]
pub struct RegionalBoundaryEndpoints {
    pub version: u64,
    pub own_coarse: TileBoundary,
    pub own_fine: TileBoundary,
    pub parent: TileBoundary,
}

/// One upload candidate for a physical regional slot. The caller retains the
/// CPU tile and publication token; the renderer reports when residency changes.
#[derive(Debug, Clone)]
pub struct RegionalTileUpload {
    pub slot: usize,
    pub tile: TileDraw,
}

/// One currently drawable patch and its coarse reconstruction dependency.
#[derive(Debug, Clone)]
pub struct RegionalPatchDraw {
    pub own_slot: usize,
    pub parent_slot: usize,
    pub own: TileDraw,
    pub parent: TileDraw,
    pub morph_fraction: f32,
    /// Fraction along the published start/end edge endpoints. This can move
    /// opposite to `morph_fraction` during a local merge.
    pub boundary_fraction: f32,
    /// Parent quadrant for a child transition. `None` draws the tile itself.
    pub quadrant: Option<[u32; 2]>,
    /// The resident ancestor supplies quality coverage while the requested
    /// descendant tile is unavailable.
    pub quality_fallback: bool,
    pub boundary_endpoints: Arc<RegionalBoundaryEndpoints>,
}

/// One bounded upload/draw transaction for a regional adaptive cover.
#[derive(Debug, Clone)]
pub struct RegionalResidentDraw {
    pub capacity: usize,
    pub cells: u32,
    pub uploads: Vec<RegionalTileUpload>,
    pub patches: Vec<RegionalPatchDraw>,
}

/// GPU residency and upload accounting for the latest regional transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegionalResidentReport {
    pub capacity: usize,
    pub resident_count: usize,
    pub pinned_count: usize,
    pub in_flight_count: usize,
    pub active_morph_count: usize,
    pub fallback_active: bool,
    pub evictable_count: usize,
    /// Number of physical slot objects currently retained by the renderer.
    pub allocated_slot_count: usize,
    pub tile_capacity_bytes: u64,
    pub boundary_capacity_bytes: u64,
    /// Capacity belonging to the currently configured logical slot pool.
    pub active_tile_capacity_bytes: u64,
    pub active_boundary_capacity_bytes: u64,
    pub metadata_capacity_bytes: u64,
    pub grid_capacity_bytes: u64,
    pub validation_capacity_bytes: u64,
    pub tile_upload_bytes: u64,
    pub boundary_upload_bytes: u64,
    pub metadata_upload_bytes: u64,
    pub tile_upload_count: u64,
    pub boundary_upload_count: u64,
    pub deferred_upload_count: u64,
    pub transfer_staging_bytes: u64,
    pub cumulative_tile_upload_bytes: u64,
    pub cumulative_boundary_upload_bytes: u64,
    pub cumulative_tile_upload_count: u64,
    pub cumulative_boundary_upload_count: u64,
    pub slots: Vec<RegionalSlotReport>,
    pub validation_readback_bytes: u64,
}

/// Observable state of one configurable physical GPU tile slot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegionalSlotReport {
    pub key: Option<TileKey>,
    pub generation: u64,
    pub reuse_safe: bool,
    pub pinned: bool,
    pub in_flight: bool,
}

impl RegionalResidentDraw {
    /// Validate the whole transaction before the renderer mutates any slot.
    pub fn validate(&self) -> Result<(), crate::resident_tile::TileGeometryError> {
        use crate::resident_tile::TileGeometryError;

        if self.capacity == 0
            || self.cells == 0
            || self.cells > TileData::MAX_CELLS
            || !self.cells.is_power_of_two()
            || self.patches.is_empty()
            || self.patches.len() > self.capacity
            || self.uploads.len() > self.capacity
        {
            return Err(TileGeometryError::InvalidTile);
        }
        let mut upload_slots = BTreeSet::new();
        for upload in &self.uploads {
            if upload.slot >= self.capacity
                || !upload_slots.insert(upload.slot)
                || upload.tile.tile.key.cells != self.cells
                || upload.tile.publication.key() != &upload.tile.tile.key
            {
                return Err(TileGeometryError::InvalidTile);
            }
            upload.tile.tile.validate_layout()?;
        }

        let mut own_slots = BTreeSet::new();
        let mut expected_uploads = BTreeMap::<usize, &TileKey>::new();
        let mut parent_keys = BTreeMap::<usize, &TileKey>::new();
        let mut parent_boundaries = BTreeMap::<usize, (&TileKey, &TileBoundary)>::new();
        for patch in &self.patches {
            if patch.own_slot >= self.capacity
                || patch.parent_slot >= self.capacity
                || !own_slots.insert(patch.own_slot)
                || patch.own.tile.key.cells != self.cells
                || patch.parent.tile.key.cells != self.cells
                || patch.own.publication.key() != &patch.own.tile.key
                || patch.parent.publication.key() != &patch.parent.tile.key
                || !patch.morph_fraction.is_finite()
                || !(0.0..=1.0).contains(&patch.morph_fraction)
                || !patch.boundary_fraction.is_finite()
                || !(0.0..=1.0).contains(&patch.boundary_fraction)
                || patch.own.mode != patch.parent.mode
                || patch.own.sun_body != patch.parent.sun_body
                || patch
                    .own
                    .body_to_view
                    .to_cols_array()
                    .iter()
                    .zip(patch.parent.body_to_view.to_cols_array())
                    .any(|(a, b)| (*a - b).abs() > 1.0e-10)
            {
                return Err(TileGeometryError::InvalidTile);
            }
            patch.own.tile.validate_layout()?;
            patch.parent.tile.validate_layout()?;
            patch
                .own
                .validate_view_transform(crate::RenderPrecisionBudget::near_debug())
                .map_err(|_| TileGeometryError::InvalidTile)?;
            patch
                .parent
                .validate_view_transform(crate::RenderPrecisionBudget::near_debug())
                .map_err(|_| TileGeometryError::InvalidTile)?;
            let anchor_delta = patch.own.tile.anchor_position_body()?
                - patch.parent.tile.anchor_position_body()?;
            crate::RenderPrecisionBudget::near_debug()
                .try_view_relative_position(anchor_delta)
                .map_err(|_| TileGeometryError::InvalidTile)?;
            let expected_anchor =
                patch.parent.anchor_view_m + patch.parent.body_to_view * anchor_delta;
            if (expected_anchor - patch.own.anchor_view_m).length() > 1.0e-6 {
                return Err(TileGeometryError::InvalidTile);
            }
            let endpoints = &patch.boundary_endpoints;
            validate_boundary(&endpoints.own_coarse, self.cells)?;
            validate_boundary(&endpoints.own_fine, self.cells)?;
            validate_boundary(&endpoints.parent, self.cells)?;

            if let Some((parent_key, parent_boundary)) = parent_boundaries.get(&patch.parent_slot) {
                if *parent_key != &patch.parent.tile.key || *parent_boundary != &endpoints.parent {
                    return Err(TileGeometryError::InvalidTile);
                }
            } else {
                parent_boundaries.insert(
                    patch.parent_slot,
                    (&patch.parent.tile.key, &endpoints.parent),
                );
            }

            if let Some(quadrant) = patch.quadrant {
                if quadrant.iter().any(|value| *value > 1)
                    || patch.own.tile.key.address.parent() != Some(patch.parent.tile.key.address)
                {
                    return Err(TileGeometryError::InvalidTile);
                }
                let expected = patch
                    .parent
                    .tile
                    .key
                    .address
                    .children()
                    .map_err(|_| TileGeometryError::InvalidTile)?;
                let index = (quadrant[1] * 2 + quadrant[0]) as usize;
                if expected[index] != patch.own.tile.key.address {
                    return Err(TileGeometryError::InvalidTile);
                }
            } else if patch.own.tile.key.address != patch.parent.tile.key.address {
                return Err(TileGeometryError::InvalidTile);
            }
            if !same_authority(&patch.own.tile.key, &patch.parent.tile.key) {
                return Err(TileGeometryError::InvalidTile);
            }
            if patch.own_slot == patch.parent_slot && patch.own.tile.key != patch.parent.tile.key {
                return Err(TileGeometryError::InvalidTile);
            }

            expected_uploads.insert(patch.own_slot, &patch.own.tile.key);
            if parent_keys
                .get(&patch.parent_slot)
                .is_some_and(|key| **key != patch.parent.tile.key)
            {
                return Err(TileGeometryError::InvalidTile);
            }
            parent_keys.insert(patch.parent_slot, &patch.parent.tile.key);
        }
        for upload in &self.uploads {
            if expected_uploads
                .get(&upload.slot)
                .is_some_and(|key| **key != upload.tile.tile.key)
                || parent_keys
                    .get(&upload.slot)
                    .is_some_and(|key| **key != upload.tile.tile.key)
            {
                return Err(TileGeometryError::InvalidTile);
            }
        }
        Ok(())
    }
}

/// CPU reconstruction matching the shader's actual parent triangles and edge
/// endpoint overrides. Used by focused tests and the explicit GPU diagnostic.
pub fn reconstruct_patch_node(
    patch: &RegionalPatchDraw,
    grid: [u32; 2],
    cells: u32,
) -> Result<crate::regional_edges::BoundaryVertex, crate::resident_tile::TileGeometryError> {
    use crate::resident_tile::TileGeometryError;
    if grid.iter().any(|value| *value > cells) || cells == 0 {
        return Err(TileGeometryError::InvalidCoordinate);
    }
    let own = crate::regional_edges::evaluate_node(
        &patch.own.tile,
        Some(&patch.boundary_endpoints.own_fine),
        grid,
    )?;
    let coarse_boundary = crate::regional_edges::evaluate_node(
        &patch.own.tile,
        Some(&patch.boundary_endpoints.own_coarse),
        grid,
    )?;
    let morph = f64::from(patch.morph_fraction);
    let boundary_fraction = f64::from(patch.boundary_fraction);
    let own_anchor_delta =
        patch.own.tile.anchor_position_body()? - patch.parent.tile.anchor_position_body()?;
    let own_parent = crate::regional_edges::BoundaryVertex {
        position_local_m: own.position_local_m + own_anchor_delta,
        ..own
    };
    let coarse_boundary = crate::regional_edges::BoundaryVertex {
        position_local_m: coarse_boundary.position_local_m + own_anchor_delta,
        ..coarse_boundary
    };
    let fine = blend_vertex(coarse_boundary, own_parent, boundary_fraction);
    let result = if let Some(quadrant) = patch.quadrant {
        let parent_st = [
            (f64::from(quadrant[0]) + f64::from(grid[0]) / f64::from(cells)) * 0.5,
            (f64::from(quadrant[1]) + f64::from(grid[1]) / f64::from(cells)) * 0.5,
        ];
        let coarse = evaluate_triangle(
            &patch.parent.tile,
            Some(&patch.boundary_endpoints.parent),
            parent_st,
        )?;
        let interior = blend_vertex(coarse, own_parent, morph);
        let is_boundary = grid[0] == 0 || grid[0] == cells || grid[1] == 0 || grid[1] == cells;
        if is_boundary { fine } else { interior }
    } else {
        fine
    };
    if !result.position_local_m.is_finite()
        || !result.normal_varying_body.is_finite()
        || result.material.iter().any(|value| !value.is_finite())
    {
        return Err(TileGeometryError::NonFinite);
    }
    Ok(result)
}

fn blend_vertex(
    a: crate::regional_edges::BoundaryVertex,
    b: crate::regional_edges::BoundaryVertex,
    t: f64,
) -> crate::regional_edges::BoundaryVertex {
    crate::regional_edges::BoundaryVertex {
        position_local_m: a.position_local_m.lerp(b.position_local_m, t),
        normal_varying_body: a.normal_varying_body.lerp(b.normal_varying_body, t),
        material: std::array::from_fn(|index| {
            a.material[index] + (b.material[index] - a.material[index]) * t
        }),
    }
}

fn validate_boundary(
    boundary: &TileBoundary,
    cells: u32,
) -> Result<(), crate::resident_tile::TileGeometryError> {
    use crate::resident_tile::TileGeometryError;
    if boundary.edges.iter().any(|edge| {
        edge.len() != cells as usize + 1
            || edge.iter().any(|vertex| {
                let narrowed_position = vertex.position_local_m.as_vec3();
                let narrowed_normal = vertex.normal_varying_body.as_vec3();
                !vertex.position_local_m.is_finite()
                    || !vertex.normal_varying_body.is_finite()
                    || vertex.normal_varying_body.length_squared() <= 0.0
                    || !narrowed_position.is_finite()
                    || !narrowed_normal.is_finite()
                    || vertex
                        .material
                        .iter()
                        .any(|value| !value.is_finite() || *value < 0.0 || *value > 1.0)
                    || (vertex.material.iter().sum::<f64>() - 1.0).abs() > 1.0e-4
            })
    }) {
        return Err(TileGeometryError::InvalidTile);
    }
    Ok(())
}

fn same_authority(a: &TileKey, b: &TileKey) -> bool {
    a.body_identity == b.body_identity
        && a.definition_words == b.definition_words
        && a.radius_bits == b.radius_bits
        && a.surface_revision == b.surface_revision
        && a.material_revision == b.material_revision
        && a.format_version == b.format_version
        && a.filter_version == b.filter_version
        && a.cells == b.cells
}
