//! Fixed parent/four-child reconstruction for the resident terrain prototype.
//!
//! Positions share the parent's body-fixed anchor. The coarse endpoint is the
//! actual indexed parent triangle, including its interpolated vertex attributes.

use crate::resident_tile::{ResidentTileReport, TileDraw, TileGeometryError, TileKey};
use glam::DVec3;

/// One fixed resident parent with independently available children. Publication
/// of child draw ownership is atomic; incomplete children never replace coverage.
#[derive(Debug, Clone)]
pub struct ResidentHierarchyDraw {
    pub parent: TileDraw,
    pub children: [Option<TileDraw>; 4],
    pub draw_children: bool,
    pub morph_fraction: f32,
}

/// CPU reconstruction in the common parent anchor and body-fixed normal axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HierarchyVertex {
    pub position_parent_local_m: DVec3,
    /// Normalized shading normal at this point, as consumed by the fragment.
    pub normal_body: DVec3,
    /// Raw smooth-normal varying. Preserve this through subdivision and only
    /// normalize after interpolation, so the coarse endpoint preserves shading.
    pub normal_varying_body: DVec3,
    pub material: [f64; 4],
}

/// Latest submitted fixed-hierarchy resources and physical-slot observations.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResidentHierarchyReport {
    pub resources: ResidentTileReport,
    /// Additional transient mapped buffer requested by the last diagnostic.
    /// Zero before validation; this is not an ordinary-frame live allocation.
    pub validation_readback_bytes: u64,
    pub resident_keys: [Option<TileKey>; 5],
    pub slot_generations: [u64; 5],
    pub slot_content_upload_bytes: [u64; 5],
    pub parent_pinned: bool,
    pub draw_children: bool,
    pub morph_fraction: f32,
}

impl ResidentHierarchyDraw {
    /// Canonical child order: lower-left, lower-right, upper-left, upper-right.
    pub fn quadrant(child_index: usize) -> Result<[u32; 2], TileGeometryError> {
        if child_index >= 4 {
            return Err(TileGeometryError::InvalidCoordinate);
        }
        Ok([(child_index % 2) as u32, (child_index / 2) as u32])
    }

    /// Validate fixed topology, shared authority, and the common view boundary.
    /// Full texel scans are intentionally performed only on changed publication.
    pub fn validate(&self) -> Result<(), TileGeometryError> {
        if !self.morph_fraction.is_finite() || !(0.0..=1.0).contains(&self.morph_fraction) {
            return Err(TileGeometryError::InvalidTile);
        }
        self.parent.tile.validate_layout()?;
        // Match the builder's supported power-of-two resolution policy.
        // Equal-resolution child grids preserve the parent lines and diagonals.
        if !self.parent.tile.key.cells.is_power_of_two() {
            return Err(TileGeometryError::InvalidTile);
        }
        self.parent
            .validate_view_transform(crate::RenderPrecisionBudget::near_debug())
            .map_err(|_| TileGeometryError::InvalidTile)?;
        if self.parent.publication.key() != &self.parent.tile.key {
            return Err(TileGeometryError::InvalidTile);
        }
        let addresses = self
            .parent
            .tile
            .key
            .address
            .children()
            .map_err(|_| TileGeometryError::InvalidTile)?;
        for (index, child) in self.children.iter().enumerate() {
            let Some(child) = child else {
                if self.draw_children {
                    return Err(TileGeometryError::InvalidTile);
                }
                continue;
            };
            child.tile.validate_layout()?;
            let key = &child.tile.key;
            let parent_key = &self.parent.tile.key;
            if key.address != addresses[index]
                || key.body_identity != parent_key.body_identity
                || key.definition_words != parent_key.definition_words
                || key.radius_bits != parent_key.radius_bits
                || key.surface_revision != parent_key.surface_revision
                || key.material_revision != parent_key.material_revision
                || key.format_version != parent_key.format_version
                || key.filter_version != parent_key.filter_version
                || key.cells != parent_key.cells
                || child.publication.key() != &child.tile.key
                || child.mode != self.parent.mode
                || child.sun_body != self.parent.sun_body
                || child
                    .body_to_view
                    .to_cols_array()
                    .iter()
                    .zip(self.parent.body_to_view.to_cols_array())
                    .any(|(a, b)| (*a - b).abs() > 1.0e-10)
            {
                return Err(TileGeometryError::InvalidTile);
            }
            child
                .validate_view_transform(crate::RenderPrecisionBudget::near_debug())
                .map_err(|_| TileGeometryError::InvalidTile)?;
            let delta = self.child_anchor_delta_body(index)?;
            crate::RenderPrecisionBudget::near_debug()
                .try_view_relative_position(delta)
                .map_err(|_| TileGeometryError::InvalidTile)?;
            let expected_view = self.parent.anchor_view_m + self.parent.body_to_view * delta;
            if (expected_view - child.anchor_view_m).length() > 1.0e-6 {
                return Err(TileGeometryError::InvalidTile);
            }
        }
        Ok(())
    }

    /// f64 subtraction precedes narrowing; no planetary anchors reach the GPU.
    pub fn child_anchor_delta_body(&self, index: usize) -> Result<DVec3, TileGeometryError> {
        let child = self
            .children
            .get(index)
            .and_then(Option::as_ref)
            .ok_or(TileGeometryError::InvalidCoordinate)?;
        let delta = child.tile.anchor_position_body()? - self.parent.tile.anchor_position_body()?;
        if !delta.is_finite() {
            return Err(TileGeometryError::NonFinite);
        }
        Ok(delta)
    }

    /// Parent coordinates at a child's reusable-grid node.
    pub fn parent_st(
        &self,
        child_index: usize,
        grid: [u32; 2],
    ) -> Result<[f64; 2], TileGeometryError> {
        let quadrant = Self::quadrant(child_index)?;
        let cells = self.parent.tile.key.cells;
        if grid.iter().any(|value| *value > cells) {
            return Err(TileGeometryError::InvalidCoordinate);
        }
        Ok(std::array::from_fn(|axis| {
            (f64::from(quadrant[axis]) + f64::from(grid[axis]) / f64::from(cells)) * 0.5
        }))
    }

    /// Evaluate the actual parent triangle at an arbitrary closed patch UV.
    /// This uses `[a,b,c,b,d,c]`, exactly the resident index buffer's diagonal.
    pub fn parent_triangle(&self, st: [f64; 2]) -> Result<HierarchyVertex, TileGeometryError> {
        if st
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return Err(TileGeometryError::InvalidCoordinate);
        }
        let cells = self.parent.tile.key.cells;
        if cells == 0 {
            return Err(TileGeometryError::InvalidTile);
        }
        let coordinates = st.map(|v| v * f64::from(cells));
        let base = coordinates.map(|v| (v.floor() as u32).min(cells - 1));
        let sx = coordinates[0] - f64::from(base[0]);
        let sy = coordinates[1] - f64::from(base[1]);
        let a = base;
        let b = [base[0] + 1, base[1]];
        let c = [base[0], base[1] + 1];
        let d = [base[0] + 1, base[1] + 1];
        let (nodes, weights) = if sx + sy <= 1.0 {
            ([a, b, c], [1.0 - sx - sy, sx, sy])
        } else {
            ([b, d, c], [1.0 - sy, sx + sy - 1.0, 1.0 - sx])
        };
        let mut result = HierarchyVertex {
            position_parent_local_m: DVec3::ZERO,
            normal_body: DVec3::ZERO,
            normal_varying_body: DVec3::ZERO,
            material: [0.0; 4],
        };
        for (node, weight) in nodes.into_iter().zip(weights) {
            let vertex = self.parent_node(node)?;
            result.position_parent_local_m += vertex.position_parent_local_m * weight;
            result.normal_varying_body += vertex.normal_varying_body * weight;
            for (destination, value) in result.material.iter_mut().zip(vertex.material) {
                *destination += value * weight;
            }
        }
        result.normal_body = result
            .normal_varying_body
            .try_normalize()
            .ok_or(TileGeometryError::NonFinite)?;
        Ok(result)
    }

    /// Patch 0 is the parent, patches 1–4 are the canonical children. Child
    /// results blend actual parent triangles with the child's derived endpoint.
    pub fn reconstruct_patch(
        &self,
        patch_index: usize,
        grid: [u32; 2],
    ) -> Result<HierarchyVertex, TileGeometryError> {
        if patch_index == 0 {
            return self.parent_node(grid);
        }
        let child_index = patch_index
            .checked_sub(1)
            .filter(|index| *index < 4)
            .ok_or(TileGeometryError::InvalidCoordinate)?;
        let child = self.children[child_index]
            .as_ref()
            .ok_or(TileGeometryError::InvalidCoordinate)?;
        let cells = child.tile.key.cells;
        if grid.iter().any(|value| *value > cells) {
            return Err(TileGeometryError::InvalidCoordinate);
        }
        let coarse = self.parent_triangle(self.parent_st(child_index, grid)?)?;
        let st = grid.map(|value| f64::from(value) / f64::from(cells));
        let own_normal = child.tile.normal_local(grid)?;
        let own = HierarchyVertex {
            position_parent_local_m: self.child_anchor_delta_body(child_index)?
                + child.tile.position_local(st)?,
            normal_body: own_normal,
            normal_varying_body: own_normal,
            material: child.tile.material(st)?.map(f64::from),
        };
        let t = f64::from(self.morph_fraction);
        let normal_varying_body =
            coarse.normal_varying_body * (1.0 - t) + own.normal_varying_body * t;
        let normal_body = normal_varying_body
            .try_normalize()
            .ok_or(TileGeometryError::NonFinite)?;
        Ok(HierarchyVertex {
            position_parent_local_m: coarse.position_parent_local_m * (1.0 - t)
                + own.position_parent_local_m * t,
            normal_body,
            normal_varying_body,
            material: std::array::from_fn(|i| coarse.material[i] * (1.0 - t) + own.material[i] * t),
        })
    }

    pub fn position_view(&self, vertex: &HierarchyVertex) -> DVec3 {
        self.parent.anchor_view_m + self.parent.body_to_view * vertex.position_parent_local_m
    }

    fn parent_node(&self, grid: [u32; 2]) -> Result<HierarchyVertex, TileGeometryError> {
        let cells = self.parent.tile.key.cells;
        if cells == 0 || grid.iter().any(|value| *value > cells) {
            return Err(TileGeometryError::InvalidCoordinate);
        }
        let st = grid.map(|value| f64::from(value) / f64::from(cells));
        let normal_body = self.parent.tile.normal_local(grid)?;
        Ok(HierarchyVertex {
            position_parent_local_m: self.parent.tile.position_local(st)?,
            normal_body,
            normal_varying_body: normal_body,
            material: self.parent.tile.material(st)?.map(f64::from),
        })
    }
}
