//! Read-only point terrain diagnostics; this module never creates render geometry.
//!
//! Runtime callers measure clearance through a [`SurfaceQuery`] backed by
//! read-back GPU colliders ([`clearance_from_query`]); under ADR 0023 the GPU
//! producer is the terrain authority. The functions that evaluate the CPU
//! surface generator directly are the test oracle for headless tests and
//! tools.
use anyhow::Result;
use astrum_math::{Direction3, FramePose, surface::SurfaceLocation};
use astrum_world::terrain::{
    SurfaceGenerator, TerrainDefinition, TerrainFootprint, TerrainGenerator, TerrainQuery,
    surface_query::{QueryResult, SurfaceQuery},
};
use astrum_world::{BodyId, CelestialBody, CoherentCelestialView};
use glam::DVec3;

/// Clearance of a body-fixed position above the authoritative surface, from
/// `query`. `Pending` while the surface there has not been read back yet.
pub fn clearance_from_query(
    query: &mut impl SurfaceQuery,
    celestial: &CelestialBody,
    position: DVec3,
    body: BodyId,
) -> Result<QueryResult<TerrainClearance>> {
    anyhow::ensure!(
        position.is_finite() && position.length() > 0.0,
        "invalid camera position"
    );
    let radius = celestial.properties().reference_radius_m();
    let location = SurfaceLocation::new(Direction3::try_new(position)?);
    let QueryResult::Ready(surface) = query.height_at(body, location.direction().unit()) else {
        return Ok(QueryResult::Pending);
    };
    let radial = location.direction().unit();
    let slope = surface
        .normal
        .cross(radial)
        .length()
        .atan2(surface.normal.dot(radial));
    let camera_radius_m = position.length();
    let surface_radius_m = radius + surface.height_m;
    Ok(QueryResult::Ready(TerrainClearance {
        body,
        location,
        camera_radius_m,
        sphere_altitude_m: camera_radius_m - radius,
        terrain_elevation_m: surface.height_m,
        surface_radius_m,
        clearance_m: camera_radius_m - surface_radius_m,
        slope_angle_rad: slope,
    }))
}

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

/// CPU test oracle: measure a camera against the complete, unfiltered terrain
/// definition, if present. Runtime code uses [`clearance_from_query`].
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

/// CPU test oracle: measure a body-fixed position against the complete CPU
/// surface evaluation (shape and relief together). Runtime code uses
/// [`clearance_from_query`].
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

/// CPU test oracle: evaluate complete legacy terrain at a body-fixed position.
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

/// M1 flat water (pipeline §13 until M3): world-map bodies with a sea level
/// draw opaque water at the reference radius over ground below it, so the
/// camera treats that surface as its floor. Colliders stay on the sea floor.
pub fn with_flat_water(body: &CelestialBody, mut clearance: TerrainClearance) -> TerrainClearance {
    let has_ocean = body
        .surface_definition()
        .and_then(|d| d.world())
        .is_some_and(|w| {
            w.archetype
                .stages
                .contains(&astrum_world::terrain::archetype::TierAStage::SeaLevel)
        });
    if has_ocean && clearance.terrain_elevation_m < 0.0 {
        clearance.surface_radius_m -= clearance.terrain_elevation_m;
        clearance.terrain_elevation_m = 0.0;
        clearance.clearance_m = clearance.camera_radius_m - clearance.surface_radius_m;
        clearance.slope_angle_rad = 0.0;
    }
    clearance
}

#[cfg(test)]
mod tests {
    /// ADR 0023: only this module's labelled oracle functions evaluate the CPU
    /// surface generator; every other runtime caller goes through
    /// `clearance_from_query`. Lines after a `#[cfg(test)]` marker are tests.
    #[test]
    fn runtime_code_never_evaluates_the_cpu_surface_generator() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root];
        let mut checked = 0;
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                    continue;
                }
                if path.extension().is_none_or(|e| e != "rs")
                    || path.ends_with("terrain_inspection.rs")
                {
                    continue;
                }
                let source = std::fs::read_to_string(&path).unwrap();
                let runtime = ["#[cfg(test)]\nmod ", "#[cfg(test)]\npub(crate) mod "]
                    .iter()
                    .filter_map(|marker| source.find(marker))
                    .min()
                    .map_or(source.as_str(), |end| &source[..end]);
                for pattern in [
                    ".evaluate_point(",
                    "clearance_at_body_position(",
                    "terrain_inspection::terrain_clearance(",
                ] {
                    let offenders: Vec<_> = runtime
                        .lines()
                        .enumerate()
                        .filter(|(_, line)| line.contains(pattern))
                        .filter(|(_, line)| {
                            // The camera's explicit CPU-oracle source for headless tests.
                            !(path.ends_with("celestial_camera.rs")
                                && line.contains("clearance_at_body_position(celestial, p, body)"))
                        })
                        .map(|(n, _)| n + 1)
                        .collect();
                    assert!(
                        offenders.is_empty(),
                        "{} calls {pattern} at lines {offenders:?}",
                        path.display()
                    );
                }
                checked += 1;
            }
        }
        assert!(checked > 40, "{checked}");
    }

    #[test]
    fn flat_water_is_the_camera_floor_over_oceans_only() {
        use astrum_math::{Direction3, surface::SurfaceLocation};
        let loaded = crate::shared_system::SharedTestSystem::load_canonical(
            std::num::NonZeroU64::new(31).unwrap(),
        )
        .unwrap();
        let body = |name: &str| {
            loaded
                .system
                .bodies()
                .find(|(_, b)| b.name() == name)
                .map(|(id, b)| (id, b.clone()))
                .unwrap()
        };
        let clearance = |id, elevation: f64| super::TerrainClearance {
            body: id,
            location: SurfaceLocation::new(Direction3::try_new(glam::DVec3::Y).unwrap()),
            camera_radius_m: 1_000_100.0,
            sphere_altitude_m: 100.0,
            terrain_elevation_m: elevation,
            surface_radius_m: 1_000_000.0 + elevation,
            clearance_m: 100.0 - elevation,
            slope_angle_rad: 0.3,
        };
        let (rust_id, rust) = body("Rust");
        // Over 2 km of sea the camera is 100 m above the water, flat.
        let water = super::with_flat_water(&rust, clearance(rust_id, -2000.0));
        assert_eq!(water.clearance_m, 100.0);
        assert_eq!(water.surface_radius_m, 1_000_000.0);
        assert_eq!(water.slope_angle_rad, 0.0);
        // Land and airless bodies are unchanged.
        let land = clearance(rust_id, 500.0);
        assert_eq!(super::with_flat_water(&rust, land), land);
        let (moon_id, moon) = body("Moon");
        let crater = clearance(moon_id, -2000.0);
        assert_eq!(super::with_flat_water(&moon, crater), crater);
    }
}
