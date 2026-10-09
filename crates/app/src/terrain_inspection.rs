//! Read-only point terrain diagnostics; this module never creates render geometry.
use anyhow::Result;
use glam::DVec3;
use astrum_math::{Direction3, FramePose, surface::SurfaceLocation};
use astrum_world::terrain::{
    SurfaceGenerator, TerrainDefinition, TerrainFootprint, TerrainGenerator, TerrainQuery,
};
use astrum_world::{BodyId, CelestialBody, CoherentCelestialView};

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
    if !celestial.has_surface() {
        return Ok(None);
    }
    let fixed = pair.projection().frames_for(body)?.body_fixed;
    let position = pair
        .evaluation()
        .convert_position(pose.position(), fixed)?
        .local()
        .metres();
    clearance_at_body_position(celestial, position, body)
}

/// Measure a body-fixed position against whichever complete surface authority
/// the body currently publishes. Compositional shape and relief are evaluated
/// together by the same world generator used for other complete queries.
pub fn clearance_at_body_position(
    celestial: &CelestialBody,
    position: DVec3,
    body: BodyId,
) -> Result<Option<TerrainClearance>> {
    if !celestial.has_surface() {
        return Ok(None);
    }
    anyhow::ensure!(
        position.is_finite() && position.length() > 0.0,
        "invalid camera position"
    );
    let radius = celestial.properties().reference_radius_m();
    let location = SurfaceLocation::new(Direction3::try_new(position)?);
    let camera_radius_m = position.length();
    let (terrain_elevation_m, surface_radius_m, slope_angle_rad) =
        if let Some(definition) = celestial.surface_definition() {
            let sample = SurfaceGenerator::new(definition, radius)?.evaluate_point(location)?;
            let radial = location.direction().unit();
            let normal = sample.normal();
            let slope = normal.cross(radial).length().atan2(normal.dot(radial));
            (sample.terrain().height_m(), sample.radius_m(), slope)
        } else {
            let Some(definition) = celestial.terrain() else {
                return Ok(None);
            };
            let evaluator = TerrainGenerator::new(definition, radius)?;
            let sample = evaluator.evaluate_point(TerrainQuery {
                location,
                footprint: TerrainFootprint::COMPLETE,
            })?;
            (
                sample.height_m(),
                radius + sample.height_m(),
                sample.slope_angle_rad(radius)?,
            )
        };
    Ok(Some(TerrainClearance {
        body,
        location,
        camera_radius_m,
        sphere_altitude_m: camera_radius_m - radius,
        terrain_elevation_m,
        surface_radius_m,
        clearance_m: camera_radius_m - surface_radius_m,
        slope_angle_rad,
    }))
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
