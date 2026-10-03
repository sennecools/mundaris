//! CPU inspection of the published triangles, distinct from procedural terrain truth.
//! This radial debug probe is not collision detection or a navigation controller.
use anyhow::Result;
use glam::DVec3;
use mundaris_math::surface::{CubePatchAddress, SurfaceLocation};

use crate::planet_terrain::AdaptiveTerrainCover;

#[derive(Debug, Clone, Copy)]
pub struct ReadyMeshProbe {
    pub patch: CubePatchAddress,
    pub radius_m: f64,
    pub footprint_m: f64,
    pub morphing: bool,
}

/// Logical source-cover leaf under a body-fixed direction, including invisible leaves.
pub fn patch_at_location(
    cover: &AdaptiveTerrainCover,
    location: SurfaceLocation,
) -> Option<CubePatchAddress> {
    let (face, uv) = location.face_uv();
    cover.active().iter().find_map(|p| {
        (p.address.face() == face && p.address.patch_local(uv).is_ok()).then_some(p.address)
    })
}

/// Intersect the outward radial ray with actual stitched/morph triangles. Only the
/// logical leaf and affected overlays at that location are tested; no samples generated.
pub fn ready_mesh_probe(
    cover: &AdaptiveTerrainCover,
    location: SurfaceLocation,
) -> Result<Option<ReadyMeshProbe>> {
    let Some(address) = patch_at_location(cover, location) else {
        return Ok(None);
    };
    let (Some(surface), Some(topology)) = (cover.surface(), cover.topology()) else {
        return Ok(None);
    };
    let Some(index) = surface
        .patches()
        .iter()
        .position(|p| p.address() == address)
    else {
        return Ok(None);
    };
    let patch = &surface.patches()[index];
    let direction = location.direction().unit();
    let mut radius: Option<f64> = None;
    let mut include = |points| {
        if let Some(hit) = radial_triangle_radius(direction, points) {
            radius = Some(radius.map_or(hit, |r| r.max(hit)));
        }
    };
    let morph = cover.transition();
    let affected = morph.is_some_and(|(mesh, _)| mesh.affected_old().contains(&address));
    if affected {
        if let Some((mesh, fraction)) = morph {
            for triangle in mesh.triangles() {
                if triangle[0].old_reference.address != address {
                    continue;
                }
                include([
                    triangle[0].sample(fraction)?.position_body_m,
                    triangle[1].sample(fraction)?.position_body_m,
                    triangle[2].sample(fraction)?.position_body_m,
                ]);
            }
        }
    } else {
        let mask = surface.stitch_mask(index).unwrap_or(0);
        for triangle in topology.indices(mask).as_chunks::<3>().0 {
            include(triangle.map(|i| patch.samples()[usize::from(i)].position_body_m));
        }
    }
    Ok(radius.map(|radius_m| ReadyMeshProbe {
        patch: address,
        radius_m,
        footprint_m: patch.footprint_m(),
        morphing: affected,
    }))
}

fn radial_triangle_radius(direction: DVec3, points: [DVec3; 3]) -> Option<f64> {
    let [a, b, c] = points;
    let e1 = b - a;
    let e2 = c - a;
    let cross = direction.cross(e2);
    let det = e1.dot(cross);
    if !det.is_finite() || det.abs() <= f64::EPSILON * e1.length() * e2.length() {
        return None;
    }
    let inv = det.recip();
    let u = -a.dot(cross) * inv;
    let q = (-a).cross(e1);
    let v = direction.dot(q) * inv;
    // Direction samples at dyadic boundaries belong to either adjacent triangle.
    let tolerance = 1e-8;
    if u < -tolerance || v < -tolerance || u + v > 1.0 + tolerance {
        return None;
    }
    let radius = e2.dot(q) * inv;
    (radius.is_finite() && radius > 0.0).then_some(radius)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radial_probe_is_two_sided_and_rejects_misses() {
        let triangle = [
            DVec3::new(-1.0, -1.0, 10.0),
            DVec3::new(1.0, -1.0, 10.0),
            DVec3::new(0.0, 1.0, 10.0),
        ];
        assert_eq!(radial_triangle_radius(DVec3::Z, triangle), Some(10.0));
        assert_eq!(
            radial_triangle_radius(DVec3::Z, [triangle[2], triangle[1], triangle[0]]),
            Some(10.0)
        );
        assert_eq!(radial_triangle_radius(-DVec3::Z, triangle), None);
        assert_eq!(radial_triangle_radius(DVec3::X, triangle), None);
    }
}
