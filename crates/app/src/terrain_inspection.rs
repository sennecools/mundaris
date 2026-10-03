//! Read-only point terrain diagnostics; this module never creates render geometry.
use anyhow::Result;
use glam::DVec3;
use mundaris_math::{Direction3, FramePose, surface::SurfaceLocation};
use mundaris_world::terrain::{
    TerrainDefinition, TerrainFootprint, TerrainGenerator, TerrainQuery,
};
use mundaris_world::{BodyId, CoherentCelestialView};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TerrainClearance {
    pub body: BodyId,
    pub location: SurfaceLocation,
    pub camera_radius_m: f64,
    pub sphere_altitude_m: f64,
    pub terrain_elevation_m: f64,
    pub surface_radius_m: f64,
    pub clearance_m: f64,
    pub slope_angle_rad: f64,
}

/// Measure a camera against the complete, unfiltered terrain definition, if present.
pub fn terrain_clearance(
    pair: &CoherentCelestialView<'_>,
    pose: FramePose,
    body: BodyId,
) -> Result<Option<TerrainClearance>> {
    let celestial = pair.system().body(body)?;
    let Some(definition) = celestial.terrain() else {
        return Ok(None);
    };
    let radius = celestial.properties().reference_radius_m();
    let fixed = pair.projection().frames_for(body)?.body_fixed;
    let position = pair
        .evaluation()
        .convert_position(pose.position(), fixed)?
        .local()
        .metres();
    Ok(Some(clearance_at_position(
        definition, radius, position, body,
    )?))
}

/// Evaluate complete terrain at a body-fixed camera position.
pub fn clearance_at_position(
    definition: &TerrainDefinition,
    radius_m: f64,
    position: DVec3,
    body: BodyId,
) -> Result<TerrainClearance> {
    anyhow::ensure!(
        position.is_finite() && position.length() > 0.0,
        "invalid camera position"
    );
    let camera_radius_m = position.length();
    let location = SurfaceLocation::new(Direction3::try_new(position)?);
    let evaluator = TerrainGenerator::new(definition, radius_m)?;
    let sample = evaluator.evaluate_point(TerrainQuery {
        location,
        footprint: TerrainFootprint::COMPLETE,
    })?;
    let terrain_elevation_m = sample.height_m();
    let surface_radius_m = radius_m + terrain_elevation_m;
    Ok(TerrainClearance {
        body,
        location,
        camera_radius_m,
        sphere_altitude_m: camera_radius_m - radius_m,
        terrain_elevation_m,
        surface_radius_m,
        clearance_m: camera_radius_m - surface_radius_m,
        slope_angle_rad: sample.slope_angle_rad(radius_m)?,
    })
}
