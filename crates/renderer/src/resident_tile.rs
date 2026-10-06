//! A small renderer-owned, reusable regular-grid tile and its CPU reconstruction.
//!
//! The tile contains only derived displacement and normalized material samples.
//! It is not a terrain authority and deliberately has no dependency on world-side
//! surface generation.

use glam::{DMat3, DVec3};
use mundaris_math::surface::CubePatchAddress;
use std::sync::Arc;

mod gpu;
pub(crate) use gpu::ResidentTileRenderer;

pub const TILE_FORMAT_VERSION: u32 = 1;
pub const TILE_FILTER_VERSION: u32 = 1;

/// Versioned address and complete authority identity for one derived tile.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TileKey {
    pub body_identity: u64,
    pub definition_words: Vec<u64>,
    pub radius_bits: u64,
    pub surface_revision: u64,
    pub material_revision: u64,
    pub format_version: u32,
    pub filter_version: u32,
    pub address: CubePatchAddress,
    pub cells: u32,
}

/// One derived radial displacement and normalized material sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileTexel {
    pub radial_offset_m: f32,
    pub material: [f32; 4],
}

/// CPU payload for one cube-chart tile. Texels are row-major and include a one
/// sample halo: the dimensions are `(cells + 3) × (cells + 3)`.
#[derive(Debug, Clone, PartialEq)]
pub struct TileData {
    pub key: TileKey,
    pub anchor_radius_m: f64,
    pub min_max_radial_offset_m: [f64; 2],
    pub texels: Vec<TileTexel>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TileGeometryError {
    #[error("tile resolution or halo payload is invalid")]
    InvalidTile,
    #[error("tile coordinate is outside the supported chart and halo")]
    InvalidCoordinate,
    #[error("tile reconstruction produced a non-finite value")]
    NonFinite,
}

impl TileData {
    pub const MAX_CELLS: u32 = 128;

    /// Validate the checked dimensions and finite values required by CPU/GPU use.
    pub fn validate(&self) -> Result<(), TileGeometryError> {
        self.validate_layout()?;
        if self.texels.iter().any(|texel| {
            !texel.radial_offset_m.is_finite()
                || texel.material.iter().any(|value| !value.is_finite())
                || texel
                    .material
                    .iter()
                    .any(|value| !(0.0..=1.0).contains(value))
                || (texel.material.iter().sum::<f32>() - 1.0).abs() > 1.0e-4
        }) {
            return Err(TileGeometryError::InvalidTile);
        }
        Ok(())
    }

    pub(crate) fn validate_layout(&self) -> Result<(), TileGeometryError> {
        let cells = self.key.cells;
        if cells == 0
            || cells > Self::MAX_CELLS
            || self.key.definition_words.is_empty()
            || !f64::from_bits(self.key.radius_bits).is_finite()
            || f64::from_bits(self.key.radius_bits) <= 0.0
            || !self.anchor_radius_m.is_finite()
            || self.anchor_radius_m <= 0.0
            || !(self.anchor_radius_m as f32).is_finite()
            || self.anchor_radius_m as f32 <= 0.0
            || !self.min_max_radial_offset_m[0].is_finite()
            || !self.min_max_radial_offset_m[1].is_finite()
            || !(self.min_max_radial_offset_m[0] as f32).is_finite()
            || !(self.min_max_radial_offset_m[1] as f32).is_finite()
            || self.min_max_radial_offset_m[0] > self.min_max_radial_offset_m[1]
            || self.texels.len() != ((cells + 3) as usize).saturating_pow(2)
        {
            return Err(TileGeometryError::InvalidTile);
        }
        Ok(())
    }

    /// Body-fixed chart center multiplied by the tile's radial anchor.
    pub fn anchor_position_body(&self) -> Result<DVec3, TileGeometryError> {
        self.validate_layout()?;
        let [x, y] = self.key.address.coordinates();
        let scale = 2.0_f64.powi(i32::from(self.key.address.level()));
        let [normal, axis_u, axis_v] = self.key.address.face().basis();
        let center = normal
            + axis_u * (2.0 * (f64::from(x) + 0.5) / scale - 1.0)
            + axis_v * (2.0 * (f64::from(y) + 0.5) / scale - 1.0);
        let result = center.normalize() * self.anchor_radius_m;
        result
            .is_finite()
            .then_some(result)
            .ok_or(TileGeometryError::NonFinite)
    }

    /// Patch-local regular-grid UV bounds.
    pub const fn grid_uv_bounds(&self) -> [[f32; 2]; 2] {
        [[0.0, 0.0], [1.0, 1.0]]
    }

    /// Reconstruct a body-centered local position relative to the chart-center
    /// anchor. `st` is patch-local and may extend one grid step into the halo.
    pub fn position_local(&self, st: [f64; 2]) -> Result<DVec3, TileGeometryError> {
        self.validate_layout()?;
        let offset = self.sample_radial_offset(st)?;
        let (n0, diff) = self.chart_direction_delta(st)?;
        let position = diff * self.anchor_radius_m + (n0 + diff) * offset;
        position
            .is_finite()
            .then_some(position)
            .ok_or(TileGeometryError::NonFinite)
    }

    /// Geometric normal from central secants through the one-texel halo.
    pub fn normal_local(&self, grid_index: [u32; 2]) -> Result<DVec3, TileGeometryError> {
        self.validate_layout()?;
        let cells = self.key.cells;
        if grid_index[0] > cells || grid_index[1] > cells {
            return Err(TileGeometryError::InvalidCoordinate);
        }
        let step = 1.0 / f64::from(cells);
        let st = [
            f64::from(grid_index[0]) * step,
            f64::from(grid_index[1]) * step,
        ];
        let left = self.position_local([st[0] - step, st[1]])?;
        let right = self.position_local([st[0] + step, st[1]])?;
        let down = self.position_local([st[0], st[1] - step])?;
        let up = self.position_local([st[0], st[1] + step])?;
        let normal = (right - left).cross(up - down).normalize();
        normal
            .is_finite()
            .then_some(normal)
            .ok_or(TileGeometryError::NonFinite)
    }

    /// Bilinearly reconstructed normalized material weights.
    pub fn material(&self, st: [f64; 2]) -> Result<[f32; 4], TileGeometryError> {
        self.validate_layout()?;
        let [x, y] = self.texel_coordinates(st)?;
        let stride = self.key.cells as usize + 3;
        let x0 = (x.floor() as usize).min(stride - 2);
        let y0 = (y.floor() as usize).min(stride - 2);
        let tx = (x - x0 as f64) as f32;
        let ty = (y - y0 as f64) as f32;
        let blend =
            |a: [f32; 4], b: [f32; 4], t: f32| std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t);
        let a = blend(
            self.texels[y0 * stride + x0].material,
            self.texels[y0 * stride + x0 + 1].material,
            tx,
        );
        let b = blend(
            self.texels[(y0 + 1) * stride + x0].material,
            self.texels[(y0 + 1) * stride + x0 + 1].material,
            tx,
        );
        let material = blend(a, b, ty);
        material
            .iter()
            .all(|value| value.is_finite())
            .then_some(material)
            .ok_or(TileGeometryError::NonFinite)
    }

    pub fn retained_payload_bytes(&self) -> usize {
        self.texels.len() * std::mem::size_of::<TileTexel>()
    }

    fn sample_radial_offset(&self, st: [f64; 2]) -> Result<f64, TileGeometryError> {
        let [x, y] = self.texel_coordinates(st)?;
        let stride = self.key.cells as usize + 3;
        let x0 = (x.floor() as usize).min(stride - 2);
        let y0 = (y.floor() as usize).min(stride - 2);
        let tx = x - x0 as f64;
        let ty = y - y0 as f64;
        let at = |x: usize, y: usize| f64::from(self.texels[y * stride + x].radial_offset_m);
        let top = at(x0, y0) + (at(x0 + 1, y0) - at(x0, y0)) * tx;
        let bottom = at(x0, y0 + 1) + (at(x0 + 1, y0 + 1) - at(x0, y0 + 1)) * tx;
        let value = top + (bottom - top) * ty;
        value
            .is_finite()
            .then_some(value)
            .ok_or(TileGeometryError::NonFinite)
    }

    fn texel_coordinates(&self, st: [f64; 2]) -> Result<[f64; 2], TileGeometryError> {
        let cells = self.key.cells;
        let halo = 1.0 / f64::from(cells);
        if st
            .iter()
            .any(|value| !value.is_finite() || *value < -halo || *value > 1.0 + halo)
        {
            return Err(TileGeometryError::InvalidCoordinate);
        }
        let max = f64::from(cells + 2);
        Ok([
            (st[0] * f64::from(cells) + 1.0).clamp(0.0, max),
            (st[1] * f64::from(cells) + 1.0).clamp(0.0, max),
        ])
    }

    fn chart_direction_delta(&self, st: [f64; 2]) -> Result<(DVec3, DVec3), TileGeometryError> {
        let address = self.key.address;
        let [normal, axis_u, axis_v] = address.face().basis();
        let [x, y] = address.coordinates();
        let inverse_scale = 1.0 / 2.0_f64.powi(i32::from(address.level()));
        let patch_width = 2.0 * inverse_scale;
        let q0 = normal
            + axis_u * ((2.0 * (f64::from(x) + 0.5) * inverse_scale) - 1.0)
            + axis_v * ((2.0 * (f64::from(y) + 0.5) * inverse_scale) - 1.0);
        let q0_length = q0.length();
        if !q0_length.is_finite() || q0_length <= 0.0 {
            return Err(TileGeometryError::NonFinite);
        }
        let n0 = q0 / q0_length;
        let tangent = (axis_u * ((st[0] - 0.5) * patch_width)
            + axis_v * ((st[1] - 0.5) * patch_width))
            / q0_length;
        let s = 2.0 * n0.dot(tangent) + tangent.length_squared();
        let root = (1.0 + s).sqrt();
        if !root.is_finite() || root <= 0.0 {
            return Err(TileGeometryError::NonFinite);
        }
        let k = 1.0 / root;
        let diff = tangent * k - n0 * (s * k / (1.0 + root));
        Ok((n0, diff))
    }
}

/// One selected resident tile and its observer-relative presentation state.
#[derive(Debug, Clone)]
pub struct TileDraw {
    pub tile: Arc<TileData>,
    /// Request-generation capability produced by a persistent `TileSlotState`.
    pub publication: TilePublicationToken,
    /// View-relative chart-center anchor, narrowed only in the shader boundary.
    pub anchor_view_m: DVec3,
    /// Body-fixed axes expressed in view coordinates.
    pub body_to_view: DMat3,
    /// 0 lit, 1 height, 2 normal, 3 material, 4 UV, 5 grid.
    pub mode: u32,
    pub sun_body: DVec3,
}

impl TileDraw {
    /// Check the f64-to-f32 view boundary before this draw enters staging.
    pub fn validate_view_transform(
        &self,
        budget: crate::RenderPrecisionBudget,
    ) -> Result<(), crate::RenderPreparationError> {
        budget.try_view_relative_position(self.anchor_view_m)?;
        let columns = self.body_to_view.to_cols_array();
        let rotation = self.body_to_view;
        let sun_f32 = self.sun_body.as_vec3();
        let tolerance = 1.0e-8;
        let orthonormal = rotation.is_finite()
            && (rotation.x_axis.length_squared() - 1.0).abs() <= tolerance
            && (rotation.y_axis.length_squared() - 1.0).abs() <= tolerance
            && (rotation.z_axis.length_squared() - 1.0).abs() <= tolerance
            && rotation.x_axis.dot(rotation.y_axis).abs() <= tolerance
            && rotation.x_axis.dot(rotation.z_axis).abs() <= tolerance
            && rotation.y_axis.dot(rotation.z_axis).abs() <= tolerance
            && (rotation.determinant() - 1.0).abs() <= 4.0 * tolerance
            && columns.iter().all(|value| value.is_finite())
            && columns
                .iter()
                .map(|value| *value as f32)
                .all(f32::is_finite)
            && sun_f32.is_finite()
            && sun_f32.length_squared().is_finite()
            && sun_f32.length_squared() > 0.0
            && {
                let narrowed = glam::Mat3::from_cols_array(&columns.map(|value| value as f32));
                let x = narrowed.x_axis;
                let y = narrowed.y_axis;
                let z = narrowed.z_axis;
                (x.length_squared() - 1.0).abs() <= 2.0e-6
                    && (y.length_squared() - 1.0).abs() <= 2.0e-6
                    && (z.length_squared() - 1.0).abs() <= 2.0e-6
                    && x.dot(y).abs() <= 2.0e-6
                    && x.dot(z).abs() <= 2.0e-6
                    && y.dot(z).abs() <= 2.0e-6
                    && (narrowed.determinant() - 1.0).abs() <= 4.0e-6
            };
        if !orthonormal {
            return Err(crate::RenderPreparationError::InvalidResidentTile);
        }
        Ok(())
    }
}

/// Last submitted resident-tile resource accounting.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResidentTileReport {
    /// Content bytes uploaded by the most recent ordinary render submission.
    pub tile_content_upload_bytes: u64,
    pub tile_content_upload_count: u64,
    pub cumulative_content_upload_bytes: u64,
    pub cumulative_content_upload_count: u64,
    pub tile_content_pack_bytes: u64,
    pub tile_content_pack_duration: std::time::Duration,
    pub tile_content_upload_api_duration: std::time::Duration,
    /// Diagnostic-only uploads never overwrite the last ordinary draw metrics.
    pub validation_content_upload_bytes: u64,
    pub validation_content_upload_count: u64,
    pub cumulative_validation_upload_bytes: u64,
    pub cumulative_validation_upload_count: u64,
    pub validation_pack_duration: std::time::Duration,
    pub validation_upload_api_duration: std::time::Duration,
    pub metadata_upload_bytes: u64,
    pub validation_metadata_upload_bytes: u64,
    pub allocation_count: u32,
    pub allocation_capacity_bytes: u64,
    pub cpu_retained_payload_bytes: usize,
    pub gpu_tile_payload_bytes: u64,
    pub validation_output_buffer_bytes: u64,
    pub grid_bytes: u64,
    pub slot_generation: u64,
    pub resident_key: Option<TileKey>,
}

/// Vertex returned by the focused GPU reconstruction validator.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReconstructedTileVertex {
    pub position_local_m: [f32; 3],
    pub position_view_m: [f32; 3],
    pub normal_body: [f32; 3],
    pub material: [f32; 4],
}

/// A specific request generation for publishing one exact tile key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TilePublicationToken {
    generation: u64,
    key: TileKey,
}
impl TilePublicationToken {
    pub fn generation(&self) -> u64 {
        self.generation
    }
    pub fn key(&self) -> &TileKey {
        &self.key
    }
}

/// CPU-testable desired-key and stale-publication guard for one logical slot.
#[derive(Debug, Default, Clone)]
pub struct TileSlotState {
    generation: u64,
    requested_key: Option<TileKey>,
}
impl TileSlotState {
    /// Select the desired content key. Repeating the same key keeps its token.
    pub fn request(&mut self, key: &TileKey) -> Result<TilePublicationToken, TileGeometryError> {
        if self.requested_key.as_ref() != Some(key) {
            self.generation = self
                .generation
                .checked_add(1)
                .ok_or(TileGeometryError::InvalidTile)?;
            self.requested_key = Some(key.clone());
        }
        Ok(TilePublicationToken {
            generation: self.generation,
            key: key.clone(),
        })
    }

    /// Accept only the latest matching request. This guard runs before a GPU
    /// slot upload, so delayed CPU completions cannot replace newer content.
    pub fn accept_publication(&mut self, token: &TilePublicationToken, key: &TileKey) -> bool {
        if token.key != *key || token.generation < self.generation {
            return false;
        }
        if token.generation == self.generation {
            return self.requested_key.as_ref() == Some(key);
        }
        self.generation = token.generation;
        self.requested_key = Some(key.clone());
        true
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }
}
