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
    /// Planetary presentation uses a footprint-scaled narrowing budget. Finite
    /// precision fixtures retain the original 10 km / 1 mm boundary.
    pub planetary: bool,
    pub capacity: usize,
    pub cells: u32,
    pub uploads: Vec<RegionalTileUpload>,
    pub patches: Vec<RegionalPatchDraw>,
}

/// GPU residency and upload accounting for the latest regional transaction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegionalResidentReport {
    /// Host time spent validating, admitting uploads and preparing GPU draws.
    pub preparation_micros: u64,
    /// Completed host time for transaction validation and slot dependency checks.
    /// `None` means the stage did not complete (for example, validation returned an error).
    pub validation_dependency_micros: Option<u64>,
    /// Host time in buffer/grid/slot allocation calls. This does not include GPU execution.
    pub resource_allocation_micros: Option<u64>,
    /// Host time packing tile/boundary payloads and issuing their queue writes.
    /// Queue writes are asynchronous; this is not GPU execution time.
    pub gpu_upload_preparation_micros: Option<u64>,
    /// Residual host time for drawable state/metadata work and report assembly after
    /// subtracting the separately measured stages from the total preparation time.
    pub drawable_metadata_micros: Option<u64>,
    /// Narrow timing details; these can overlap the retained aggregate buckets.
    pub cached_endpoint_validation_micros: Option<u64>,
    pub proposed_slot_state_micros: Option<u64>,
    pub slot_dependency_check_micros: Option<u64>,
    pub metadata_pack_queue_write_micros: Option<u64>,
    pub draw_retention_micros: Option<u64>,
    pub report_assembly_micros: Option<u64>,
    /// Input transaction work counts for the latest regional preparation.
    pub prepare_patch_count: usize,
    pub prepare_upload_count: usize,
    pub prepare_distinct_slot_count: usize,
    /// The latest preparation reused an exactly matching, previously validated cover.
    pub preparation_cache_hit: bool,
    /// Endpoint references reused from or newly validated in the exact cache.
    pub endpoint_validation_cache_hits: u64,
    pub endpoint_validation_cache_misses: u64,
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
    /// Actual retained buffer capacities in each (overlapping) slot state.
    pub resident_tile_capacity_bytes: u64,
    pub pinned_tile_capacity_bytes: u64,
    pub in_flight_tile_capacity_bytes: u64,
    pub evictable_tile_capacity_bytes: u64,
    pub metadata_capacity_bytes: u64,
    /// Host cache of exact presentation bytes used to skip unchanged uploads.
    pub metadata_cpu_capacity_bytes: u64,
    /// Cached parent boundary buffers; endpoint references alias the draw cover.
    pub boundary_cpu_capacity_bytes: u64,
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
    /// Whether two draws have identical immutable content and presentation inputs.
    /// This is deliberately exact: cached validation is valid only while every
    /// value that can affect validation or packed GPU parameters is unchanged.
    pub(crate) fn same_render_inputs(&self, other: &Self) -> bool {
        self.planetary == other.planetary
            && self.capacity == other.capacity
            && self.cells == other.cells
            && self.uploads.len() == other.uploads.len()
            && self
                .uploads
                .iter()
                .zip(&other.uploads)
                .all(|(a, b)| a.slot == b.slot && same_tile_draw(&a.tile, &b.tile))
            && self.patches.len() == other.patches.len()
            && self.patches.iter().zip(&other.patches).all(|(a, b)| {
                a.own_slot == b.own_slot
                    && a.parent_slot == b.parent_slot
                    && same_tile_draw(&a.own, &b.own)
                    && same_tile_draw(&a.parent, &b.parent)
                    && a.morph_fraction.to_bits() == b.morph_fraction.to_bits()
                    && a.boundary_fraction.to_bits() == b.boundary_fraction.to_bits()
                    && a.quadrant == b.quadrant
                    && a.quality_fallback == b.quality_fallback
                    && Arc::ptr_eq(&a.boundary_endpoints, &b.boundary_endpoints)
            })
    }

    pub fn precision_budget(&self, tile: &crate::TileDraw) -> crate::RenderPrecisionBudget {
        if self.planetary {
            tile.planetary_precision_budget()
        } else {
            crate::RenderPrecisionBudget::near_debug()
        }
    }
    /// Validate the whole transaction before the renderer mutates any slot.
    pub fn validate(&self) -> Result<(), crate::resident_tile::TileGeometryError> {
        self.validate_inner(&mut |endpoints, cells| {
            validate_boundary(&endpoints.own_coarse, cells)?;
            validate_boundary(&endpoints.own_fine, cells)?;
            validate_boundary(&endpoints.parent, cells)
        })
    }

    fn validate_inner(
        &self,
        validate_endpoints: &mut impl FnMut(
            &Arc<RegionalBoundaryEndpoints>,
            u32,
        )
            -> Result<(), crate::resident_tile::TileGeometryError>,
    ) -> Result<(), crate::resident_tile::TileGeometryError> {
        use crate::resident_tile::TileGeometryError;

        if self.capacity == 0
            || self.cells == 0
            || self.cells > TileData::MAX_CELLS
            || !self.cells.is_power_of_two()
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
        let mut cover_appearance = None;
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
                || patch.own.appearance != patch.parent.appearance
                || cover_appearance.is_some_and(|appearance| appearance != patch.own.appearance)
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
            cover_appearance = Some(patch.own.appearance);
            patch.own.tile.validate_layout()?;
            patch.parent.tile.validate_layout()?;
            patch
                .own
                .validate_view_transform(self.precision_budget(&patch.own))
                .map_err(|_| TileGeometryError::InvalidTile)?;
            patch
                .parent
                .validate_view_transform(self.precision_budget(&patch.parent))
                .map_err(|_| TileGeometryError::InvalidTile)?;
            let anchor_delta = patch.own.tile.anchor_position_body()?
                - patch.parent.tile.anchor_position_body()?;
            self.precision_budget(&patch.parent)
                .try_view_relative_position(anchor_delta)
                .map_err(|_| TileGeometryError::InvalidTile)?;
            let expected_anchor =
                patch.parent.anchor_view_m + patch.parent.body_to_view * anchor_delta;
            if (expected_anchor - patch.own.anchor_view_m).length() > 1.0e-6 {
                return Err(TileGeometryError::InvalidTile);
            }
            let endpoints = &patch.boundary_endpoints;
            validate_endpoints(endpoints, self.cells)?;

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

fn same_tile_draw(a: &TileDraw, b: &TileDraw) -> bool {
    Arc::ptr_eq(&a.tile, &b.tile)
        && a.tile.key == b.tile.key
        && a.publication.key() == b.publication.key()
        && a.publication.generation() == b.publication.generation()
        && a.anchor_view_m.to_array().map(f64::to_bits)
            == b.anchor_view_m.to_array().map(f64::to_bits)
        && a.body_to_view.to_cols_array().map(f64::to_bits)
            == b.body_to_view.to_cols_array().map(f64::to_bits)
        && a.mode == b.mode
        && a.sun_body.to_array().map(f64::to_bits) == b.sun_body.to_array().map(f64::to_bits)
        && a.appearance == b.appearance
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

/// Exact immutable endpoint validation retained only for the latest draw cover.
/// Strong references prevent in-place mutation while their result is cached.
#[derive(Default)]
pub(crate) struct RegionalEndpointValidationCache {
    cells: u32,
    endpoints: std::collections::HashMap<usize, Arc<RegionalBoundaryEndpoints>>,
}

impl RegionalEndpointValidationCache {
    pub(crate) fn capacity_bytes(&self) -> u64 {
        (self.endpoints.capacity()
            * (std::mem::size_of::<usize>()
                + std::mem::size_of::<Arc<RegionalBoundaryEndpoints>>()
                + 1)) as u64
    }
}

impl RegionalResidentDraw {
    #[cfg(test)]
    pub(crate) fn validate_cached(
        &self,
        cache: &mut RegionalEndpointValidationCache,
    ) -> Result<(), crate::resident_tile::TileGeometryError> {
        self.validate_cached_profiled(cache).0
    }

    pub(crate) fn validate_cached_profiled(
        &self,
        cache: &mut RegionalEndpointValidationCache,
    ) -> (
        Result<(), crate::resident_tile::TileGeometryError>,
        EndpointValidationCacheCounts,
    ) {
        let mut counts = EndpointValidationCacheCounts::default();
        if cache.cells != self.cells {
            cache.endpoints.clear();
            cache.cells = self.cells;
        }
        let mut used = std::collections::HashSet::with_capacity(self.patches.len());
        let result = self.validate_inner(&mut |endpoints, cells| {
            let identity = Arc::as_ptr(endpoints) as usize;
            match cache.endpoints.entry(identity) {
                std::collections::hash_map::Entry::Occupied(_) => {
                    counts.hits = counts.hits.saturating_add(1);
                }
                std::collections::hash_map::Entry::Vacant(entry) => {
                    counts.misses = counts.misses.saturating_add(1);
                    validate_boundary(&endpoints.own_coarse, cells)?;
                    validate_boundary(&endpoints.own_fine, cells)?;
                    validate_boundary(&endpoints.parent, cells)?;
                    entry.insert(Arc::clone(endpoints));
                }
            }
            used.insert(identity);
            Ok(())
        });
        if result.is_ok() {
            cache
                .endpoints
                .retain(|identity, _| used.contains(identity));
        } else {
            cache.endpoints.clear();
        }
        (result, counts)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct EndpointValidationCacheCounts {
    pub hits: u64,
    pub misses: u64,
}

#[cfg(test)]
mod endpoint_cache_tests {
    use super::*;
    use crate::{TILE_FILTER_VERSION, TILE_FORMAT_VERSION, TileSlotState, TileTexel};
    use glam::{DMat3, DVec3};
    use mundaris_math::surface::{CubeFace, CubePatchAddress};

    fn draw() -> RegionalResidentDraw {
        let address = CubePatchAddress::root(CubeFace::PositiveZ);
        let tile = Arc::new(TileData {
            key: TileKey {
                body_identity: 1,
                definition_words: vec![1],
                radius_bits: 100_000.0f64.to_bits(),
                surface_revision: 1,
                material_revision: 1,
                format_version: TILE_FORMAT_VERSION,
                filter_version: TILE_FILTER_VERSION,
                address,
                cells: 2,
            },
            anchor_radius_m: 100_000.0,
            min_max_radial_offset_m: [0.0, 0.0],
            texels: vec![
                TileTexel {
                    radial_offset_m: 0.0,
                    material: [1.0, 0.0, 0.0, 0.0]
                };
                25
            ],
        });
        let publication = TileSlotState::default().request(&tile.key).unwrap();
        let boundary = crate::regional_edges::build_boundaries(&BTreeMap::from([(
            address,
            Arc::clone(&tile),
        )]))
        .unwrap()
        .remove(&address)
        .unwrap();
        let tile_draw = TileDraw {
            tile,
            publication,
            anchor_view_m: DVec3::new(0.0, 0.0, -1000.0),
            body_to_view: DMat3::IDENTITY,
            mode: 0,
            sun_body: DVec3::Z,
            appearance: Default::default(),
        };
        RegionalResidentDraw {
            planetary: true,
            capacity: 1,
            cells: 2,
            uploads: vec![],
            patches: vec![RegionalPatchDraw {
                own_slot: 0,
                parent_slot: 0,
                own: tile_draw.clone(),
                parent: tile_draw,
                morph_fraction: 1.0,
                boundary_fraction: 1.0,
                quadrant: None,
                quality_fallback: false,
                boundary_endpoints: Arc::new(RegionalBoundaryEndpoints {
                    version: 1,
                    own_coarse: boundary.clone(),
                    own_fine: boundary.clone(),
                    parent: boundary,
                }),
            }],
        }
    }

    #[test]
    fn exact_endpoint_cache_preserves_dynamic_checks_and_rejects_replacement_payloads() {
        let original = draw();
        let mut cache = RegionalEndpointValidationCache::default();
        let (first, first_counts) = original.validate_cached_profiled(&mut cache);
        first.unwrap();
        assert_eq!(
            first_counts,
            EndpointValidationCacheCounts { hits: 0, misses: 1 }
        );
        let (second, second_counts) = original.validate_cached_profiled(&mut cache);
        second.unwrap();
        assert_eq!(
            second_counts,
            EndpointValidationCacheCounts { hits: 1, misses: 0 }
        );
        assert_eq!(cache.endpoints.len(), 1);
        let mut invalid = original.clone();
        invalid.patches[0].morph_fraction = f32::NAN;
        assert_eq!(invalid.validate_cached(&mut cache), invalid.validate());
        assert!(cache.endpoints.is_empty());
        original.validate_cached(&mut cache).unwrap();
        let mut endpoints = (*original.patches[0].boundary_endpoints).clone();
        endpoints.parent.edges[0][0].material[0] = f64::NAN;
        invalid = original.clone();
        invalid.patches[0].boundary_endpoints = Arc::new(endpoints);
        assert!(invalid.validate_cached(&mut cache).is_err());
        assert!(cache.endpoints.is_empty());
        original.validate_cached(&mut cache).unwrap();
        invalid = original.clone();
        invalid.cells = 4;
        assert!(invalid.validate_cached(&mut cache).is_err());
        assert!(cache.endpoints.is_empty());
        original.validate_cached(&mut cache).unwrap();
        let mut empty = original;
        empty.patches.clear();
        empty.validate_cached(&mut cache).unwrap();
        assert!(cache.endpoints.is_empty());
    }

    #[test]
    fn cached_draw_inputs_require_exact_content_slots_and_presentation() {
        let original = draw();
        assert!(original.same_render_inputs(&original.clone()));

        let mut changed = original.clone();
        changed.capacity += 1;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.planetary = false;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.uploads.push(RegionalTileUpload {
            slot: 0,
            tile: changed.patches[0].own.clone(),
        });
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].own_slot = 1;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].parent_slot = 1;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        let tile = Arc::clone(&changed.patches[0].own.tile);
        changed.patches[0].own.tile = Arc::new((*tile).clone());
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].own.anchor_view_m.x = -0.0;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].own.body_to_view.x_axis.x = -0.0;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].own.mode = 1;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].own.appearance.material_colors[0][0] += 0.01;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].own.sun_body.x = -0.0;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].morph_fraction = f32::from_bits(1.0f32.to_bits() + 1);
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].boundary_fraction = f32::from_bits(1.0f32.to_bits() - 1);
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].quadrant = Some([0, 0]);
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches[0].quality_fallback = true;
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        let endpoints = Arc::clone(&changed.patches[0].boundary_endpoints);
        changed.patches[0].boundary_endpoints = Arc::new((*endpoints).clone());
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        let mut state = TileSlotState::default();
        let other_key = {
            let mut key = changed.patches[0].own.tile.key.clone();
            key.definition_words.push(9);
            key
        };
        state.request(&changed.patches[0].own.tile.key).unwrap();
        state.request(&other_key).unwrap();
        changed.patches[0].own.publication =
            state.request(&changed.patches[0].own.tile.key).unwrap();
        assert!(!original.same_render_inputs(&changed));

        changed = original.clone();
        changed.patches.clear();
        assert!(!original.same_render_inputs(&changed));
    }
}
