//! Opt-in single-tile comparison fixture for the Slice 2A production path.
use std::sync::Arc;

use anyhow::{Result, bail, ensure};
use glam::{DMat3, DVec3};
use mundaris_math::surface::{CubeFace, CubePatchAddress, SurfaceLocation};
use mundaris_renderer::planet_surface::{
    ActiveSurfacePatch, GeneratedSurfacePatch, SurfaceErrorContributions, SurfaceExtent,
    SurfaceGeometrySample, SurfaceTopology,
};
use mundaris_renderer::{ResidentTileReport, TileData};
use mundaris_world::{
    BodyId,
    terrain::{SurfaceAlgorithm, SurfaceGenerator},
};

use crate::resident_terrain::{TileApproximationMetrics, TileBuildDiagnostics};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ResidentTileConfig {
    pub family: SurfaceAlgorithm,
    pub seed: u64,
    pub radius_m: f64,
    pub address: CubePatchAddress,
    pub cells: u32,
    /// A stable content salt represented in the published surface identity.
    pub revision: u64,
}

impl ResidentTileConfig {
    #[allow(clippy::too_many_arguments)] // Mirrors the flat developer protocol command.
    pub fn parse(
        family: &str,
        seed: u64,
        radius_m: f64,
        face: &str,
        level: u8,
        x: u32,
        y: u32,
        cells: u32,
        revision: u64,
    ) -> Result<Self> {
        ensure!(
            radius_m.is_finite() && radius_m > 0.0,
            "invalid resident tile radius"
        );
        ensure!(
            cells.is_power_of_two() && (1..=128).contains(&cells),
            "resident tile cells must be a power of two in 1..=128"
        );
        let family = match family.to_ascii_lowercase().as_str() {
            "rocky_v5" | "rockyv5" => SurfaceAlgorithm::RockyV5,
            "icy_v3" | "icyv3" => SurfaceAlgorithm::IcyV3,
            "volcanic_v3" | "volcanicv3" => SurfaceAlgorithm::VolcanicV3,
            _ => bail!("unsupported resident tile family"),
        };
        let face = match face.to_ascii_lowercase().as_str() {
            "positive_x" | "+x" => CubeFace::PositiveX,
            "negative_x" | "-x" => CubeFace::NegativeX,
            "positive_y" | "+y" => CubeFace::PositiveY,
            "negative_y" | "-y" => CubeFace::NegativeY,
            "positive_z" | "+z" => CubeFace::PositiveZ,
            "negative_z" | "-z" => CubeFace::NegativeZ,
            _ => bail!("unsupported cube face"),
        };
        let address = CubePatchAddress::try_new(face, level, x, y)?;
        Ok(Self {
            family,
            seed,
            radius_m,
            address,
            cells,
            revision,
        })
    }

    pub fn family_name(&self) -> &'static str {
        match self.family {
            SurfaceAlgorithm::RockyV5 => "RockyV5",
            SurfaceAlgorithm::IcyV3 => "IcyV3",
            SurfaceAlgorithm::VolcanicV3 => "VolcanicV3",
            _ => "unsupported",
        }
    }
}

#[derive(Clone)]
pub(super) struct RestoreAuthority {
    pub body: BodyId,
    pub radius_m: f64,
    pub legacy: Option<mundaris_world::terrain::TerrainDefinition>,
    pub surface: Option<mundaris_world::terrain::SurfaceDefinition>,
    pub camera: crate::celestial_camera::CelestialCamera,
}

pub(super) struct ResidentTileFixture {
    pub enabled: bool,
    pub body: Option<BodyId>,
    pub config: Option<ResidentTileConfig>,
    pub tile: Option<Arc<TileData>>,
    pub publication: Option<mundaris_renderer::TilePublicationToken>,
    pub cpu_reference: Option<CpuReference>,
    pub build: Option<TileBuildDiagnostics>,
    pub approximation: Option<TileApproximationMetrics>,
    pub restore: Option<RestoreAuthority>,
    pub mode: u32,
    pub camera_offset_m: [f64; 3],
    pub sun_direction_body: DVec3,
    pub reference_cpu: bool,
    pub validation_pending: bool,
    pub gpu_validation: Option<serde_json::Value>,
    pub stale_reason: Option<String>,
    pub slot_state: mundaris_renderer::TileSlotState,
}

pub(super) struct CpuReference {
    pub mesh: GeneratedSurfacePatch,
    pub patch: ActiveSurfacePatch,
    pub topology: SurfaceTopology,
}

impl CpuReference {
    pub fn build(generator: &SurfaceGenerator, address: CubePatchAddress) -> Result<Self> {
        let topology = SurfaceTopology::new();
        let mut samples = Vec::with_capacity(17 * 17);
        let mut minimum = f64::INFINITY;
        let mut maximum = f64::NEG_INFINITY;
        for y in 0..=16 {
            for x in 0..=16 {
                let location = SurfaceLocation::new(address.sample_direction(x, y, 16)?);
                let sample = generator.evaluate_point(location)?;
                minimum = minimum.min(sample.radius_m() - generator.radius_m());
                maximum = maximum.max(sample.radius_m() - generator.radius_m());
                samples.push(SurfaceGeometrySample {
                    position_body_m: sample.position(location),
                    normal_body: sample.normal(),
                });
            }
        }
        let reference_radius = generator.radius_m();
        let extent = SurfaceExtent {
            min_height_m: minimum,
            max_height_m: maximum,
            guaranteed_opaque_radius_m: 0.0,
        };
        let footprint_m = 2.0 * reference_radius / 2.0_f64.powi(i32::from(address.level()))
            * std::f64::consts::SQRT_2;
        let mesh = GeneratedSurfacePatch::new(
            address,
            reference_radius,
            footprint_m,
            samples,
            extent,
            SurfaceErrorContributions::default(),
        )?;
        let patch = ActiveSurfacePatch {
            address,
            stitch_mask: 0,
            metadata: mundaris_renderer::planet_surface::PatchMetadata::build(address, &topology)?,
            error_pixels: 0.0,
        };
        Ok(Self {
            mesh,
            patch,
            topology,
        })
    }
}

impl Default for ResidentTileFixture {
    fn default() -> Self {
        Self {
            enabled: false,
            body: None,
            config: None,
            tile: None,
            publication: None,
            cpu_reference: None,
            build: None,
            approximation: None,
            restore: None,
            mode: 0,
            camera_offset_m: [0.0; 3],
            sun_direction_body: DVec3::new(0.3, 0.8, 0.5).normalize(),
            reference_cpu: false,
            validation_pending: false,
            gpu_validation: None,
            stale_reason: None,
            slot_state: mundaris_renderer::TileSlotState::default(),
        }
    }
}

impl ResidentTileFixture {
    pub fn set_view(
        &mut self,
        mode: u8,
        camera_offset_m: [f64; 3],
        sun_direction_body: [f64; 3],
        reference_cpu: bool,
    ) -> Result<()> {
        ensure!(mode <= 10, "resident tile mode must be in 0..=10");
        ensure!(
            camera_offset_m.iter().all(|v| v.is_finite()),
            "nonfinite camera offset"
        );
        ensure!(
            sun_direction_body.iter().all(|v| v.is_finite()),
            "nonfinite tile light"
        );
        let sun = DVec3::from_array(sun_direction_body);
        ensure!(
            sun.length_squared() > 0.0,
            "resident tile light direction is zero"
        );
        self.mode = u32::from(mode);
        self.camera_offset_m = camera_offset_m;
        self.sun_direction_body = sun.normalize();
        self.reference_cpu = reference_cpu;
        Ok(())
    }

    pub fn draw(
        &self,
        view: &mundaris_renderer::PreparedView<'_>,
        body_frame: mundaris_math::FrameId,
    ) -> Result<mundaris_renderer::TileDraw> {
        let tile = self
            .tile
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("resident tile is not prepared"))?;
        let publication = self
            .publication
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("tile publication token is unavailable"))?;
        let source = view.prepare_source(body_frame)?;
        let anchor = tile.anchor_position_body()?;
        let anchor_view_m = source
            .view_displacement(mundaris_math::FramePosition::new(
                body_frame,
                mundaris_math::LocalPosition::try_metres(anchor)?,
            ))?
            .metres();
        let direction = |v: DVec3| -> Result<DVec3> {
            Ok(source
                .view_direction(mundaris_math::Direction3::try_new(v)?)?
                .unit())
        };
        let body_to_view = DMat3::from_cols(
            direction(DVec3::X)?,
            direction(DVec3::Y)?,
            direction(DVec3::Z)?,
        );
        Ok(mundaris_renderer::TileDraw {
            tile: Arc::clone(tile),
            anchor_view_m,
            body_to_view,
            mode: self.mode,
            sun_body: self.sun_direction_body,
            publication: publication.clone(),
        })
    }

    pub fn camera_pose(
        &self,
        body_frame: mundaris_math::FrameId,
    ) -> Result<mundaris_math::FramePose> {
        let tile = self
            .tile
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("resident tile is not prepared"))?;
        let anchor = tile.anchor_position_body()?;
        let radial = anchor.normalize();
        let tangent = mundaris_math::surface::SurfaceTangentBasis::new(
            mundaris_math::Direction3::try_new(radial)?,
        );
        let offset = DVec3::from_array(self.camera_offset_m);
        let footprint_m =
            2.0 * tile.anchor_radius_m / 2.0_f64.powi(i32::from(tile.key.address.level()));
        let terrain_above_anchor = tile.min_max_radial_offset_m[1].max(0.0);
        let camera_height_m =
            (0.7 * footprint_m).max(terrain_above_anchor + 0.15 * footprint_m + 10.0);
        let camera = anchor
            + radial * (camera_height_m + offset.z)
            + tangent.east().unit() * offset.x
            + tangent.north().unit() * offset.y;
        let orientation = glam::DQuat::from_mat3(&DMat3::from_cols(
            tangent.east().unit(),
            tangent.north().unit(),
            radial,
        ));
        Ok(mundaris_math::FramePose::new(
            mundaris_math::FramePosition::new(
                body_frame,
                mundaris_math::LocalPosition::try_metres(camera)?,
            ),
            mundaris_math::UnitRotation::try_from_quaternion(orientation)?,
        ))
    }

    pub fn snapshot(
        &self,
        report: Option<&ResidentTileReport>,
        frame_number: u64,
    ) -> Option<serde_json::Value> {
        if !self.enabled {
            return None;
        }
        let config = self.config.as_ref()?;
        let tile = self.tile.as_ref()?;
        let key = &tile.key;
        let address = key.address;
        let world_to_tile = self.approximation.map(|v| {
            serde_json::json!({
                "texel_query_count": v.texel_query_count,
                "triangle_centroid_query_count": v.triangle_centroid_query_count,
                "max_texel_radial_error_m": v.max_texel_radial_error_m,
                "rms_texel_radial_error_m": v.rms_texel_radial_error_m,
                "max_normal_angular_error_radians": v.max_normal_angular_error_radians,
                "rms_normal_angular_error_radians": v.rms_normal_angular_error_radians,
                "max_triangle_centroid_error_m": v.max_triangle_centroid_error_m,
                "rms_triangle_centroid_error_m": v.rms_triangle_centroid_error_m,
            })
        });
        let gpu = report.map(|v| serde_json::json!({
            "tile_content_upload_bytes": v.tile_content_upload_bytes,
            "tile_content_upload_count": v.tile_content_upload_count,
            "cumulative_content_upload_bytes": v.cumulative_content_upload_bytes,
            "cumulative_content_upload_count": v.cumulative_content_upload_count,
            "tile_content_pack_bytes": v.tile_content_pack_bytes,
            "tile_content_pack_ms": v.tile_content_pack_duration.as_secs_f64() * 1000.0,
            "tile_content_upload_api_ms": v.tile_content_upload_api_duration.as_secs_f64() * 1000.0,
            "validation_content_upload_bytes": v.validation_content_upload_bytes,
            "validation_content_upload_count": v.validation_content_upload_count,
            "cumulative_validation_upload_bytes": v.cumulative_validation_upload_bytes,
            "cumulative_validation_upload_count": v.cumulative_validation_upload_count,
            "validation_pack_ms": v.validation_pack_duration.as_secs_f64() * 1000.0,
            "validation_upload_api_ms": v.validation_upload_api_duration.as_secs_f64() * 1000.0,
            "metadata_upload_bytes": v.metadata_upload_bytes,
            "validation_metadata_upload_bytes": v.validation_metadata_upload_bytes,
            "allocation_count": v.allocation_count,
            "allocation_capacity_bytes": v.allocation_capacity_bytes,
            "gpu_tile_payload_bytes": v.gpu_tile_payload_bytes,
            "cpu_retained_payload_bytes": v.cpu_retained_payload_bytes,
            "validation_output_buffer_bytes": v.validation_output_buffer_bytes,
            "grid_bytes": v.grid_bytes,
            "slot_generation": v.slot_generation,
            "resident_key_matches": v.resident_key.as_ref() == Some(key),
        }));
        let builder = serde_json::json!({
            "cpu_build_ms": self.build.map(|v| v.elapsed.as_secs_f64() * 1000.0),
            "authoritative_query_count": self.build.map(|v| v.authoritative_query_count),
            "builder_stack_scratch_bytes_estimate": self.build.map(|v| v.builder_stack_scratch_bytes_estimate),
            "surface_query_heap_scratch_bytes": self.build.map(|v| v.surface_query_heap_scratch_bytes),
            "surface_generator_retained_heap_bytes": self.build.map(|v| v.surface_generator_retained_heap_bytes),
            "surface_generator_heap_bound_bytes": self.build.map(|v| v.surface_generator_heap_bound_bytes),
            "texel_dimensions": self.build.map(|v| v.texel_dimensions),
            "patch_grid_vertex_dimensions": self.build.map(|v| v.patch_grid_vertex_dimensions),
            "nominal_grid_spacing_radians": self.build.map(|v| v.nominal_grid_spacing_radians),
            "nominal_grid_spacing_m": self.build.map(|v| v.nominal_grid_spacing_m),
            "actual_chart_center_grid_spacing_m": self.build.map(|v| v.actual_chart_center_grid_spacing_m),
            "filter_step_radians": self.build.map(|v| v.filter_step_radians),
            "filter_nominal_width_m": self.build.map(|v| v.filter_nominal_width_m),
            "filter_actual_surface_offsets_m": self.build.map(|v| v.filter_actual_surface_offsets_m),
        });
        let identity = serde_json::json!({
            "family": config.family_name(),
            "seed": config.seed,
            "body_identity": key.body_identity,
            "surface_revision": key.surface_revision,
            "material_revision": key.material_revision,
            "definition_words": key.definition_words,
            "radius_m": f64::from_bits(key.radius_bits),
            "format_version": key.format_version,
            "filter_version": key.filter_version,
        });
        let address = serde_json::json!({
            "face": format!("{:?}", address.face()),
            "level": address.level(),
            "xy": address.coordinates(),
            "cells": key.cells,
            "payload_bytes": tile.retained_payload_bytes(),
            "anchor_radius_m": tile.anchor_radius_m,
            "min_max_radial_offset_m": tile.min_max_radial_offset_m,
        });
        Some(serde_json::json!({
            "enabled": true,
            "frame_number": frame_number,
            "scope": "one_opt_in_resident_tile; not whole_body_quality_or_visual_acceptance",
            "identity": identity,
            "address": address,
            "ordinary_frame_tile_builds": 0,
            "ordinary_frame_terrain_build_waits": 0,
            "stale": self.stale_reason.is_some(),
            "stale_reason": self.stale_reason,
            "builder": builder,
            "world_to_tile": world_to_tile,
            "tile_to_gpu": self.gpu_validation,
            "gpu_residency": gpu,
            "view_mode": self.mode,
            "sun_direction_body": self.sun_direction_body.to_array(),
            "camera_offset_m": self.camera_offset_m,
            "reference_cpu_enabled": self.reference_cpu,
            "cpu_reference_cells": 16,
            "cpu_reference_density_note": "old_generated_surface_path_uses_grid16;gpu_path_uses_configured_grid",
        }))
    }
}
