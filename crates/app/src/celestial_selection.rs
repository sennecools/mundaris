//! Unified selection identity and headless label/marker/reference-sphere picking.
use crate::celestial_labels::ScreenRect;
use anyhow::{Result, ensure};
use astrum_renderer::CelestialProjection;
use astrum_world::{BodyId, CelestialSystem};
use glam::{DQuat, DVec3};
#[derive(Default)]
pub struct BodySelection(Option<BodyId>);
impl BodySelection {
    pub fn selected(&self) -> Option<BodyId> {
        self.0
    }
    pub fn select(&mut self, system: &CelestialSystem, body: BodyId) -> Result<()> {
        system.body(body)?;
        self.0 = Some(body);
        Ok(())
    }
}
#[derive(Debug, Clone, Copy)]
pub struct BodyHitTarget {
    pub body: BodyId,
    pub marker: Option<[f64; 2]>,
    pub marker_radius_pixels: f64,
    pub label: Option<ScreenRect>,
    pub center_in_view_m: DVec3,
    pub radius_m: f64,
    pub occluded_overlay: bool,
}
#[derive(Debug, Clone, PartialEq)]
pub struct PickResult {
    pub candidates: Vec<BodyId>,
    pub overlay: bool,
}
/// Stable input body order resolves residual ties. Every overlapping marker is returned.
pub fn pick_body(
    targets: &[BodyHitTarget],
    pointer: [f64; 2],
    projection: CelestialProjection,
) -> Result<PickResult> {
    ensure!(pointer.iter().all(|p| p.is_finite()), "invalid pointer");
    for t in targets {
        ensure!(
            t.center_in_view_m.is_finite() && t.radius_m.is_finite() && t.radius_m > 0.0,
            "invalid pick sphere"
        );
    }
    if let Some(t) = targets
        .iter()
        .find(|t| t.label.is_some_and(|r| r.contains(pointer)))
    {
        return Ok(PickResult {
            candidates: vec![t.body],
            overlay: t.occluded_overlay,
        });
    }
    let mut markers: Vec<_> = targets
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            t.marker
                .map(|p| (i, t, (p[0] - pointer[0]).hypot(p[1] - pointer[1])))
        })
        .filter(|(_, t, d)| *d <= t.marker_radius_pixels)
        .collect();
    markers.sort_by(|(ai, a, ad), (bi, b, bd)| {
        ad.total_cmp(bd)
            .then((-a.center_in_view_m.z).total_cmp(&(-b.center_in_view_m.z)))
            .then(ai.cmp(bi))
    });
    if !markers.is_empty() {
        return Ok(PickResult {
            overlay: markers.iter().any(|(_, t, _)| t.occluded_overlay),
            candidates: markers.iter().map(|(_, t, _)| t.body).collect(),
        });
    }
    let ray = projection.unproject_ray(pointer)?;
    let hit = targets
        .iter()
        .filter_map(|t| sphere_hit(ray, t.center_in_view_m, t.radius_m).map(|d| (t.body, d)))
        .min_by(|a, b| a.1.total_cmp(&b.1));
    Ok(PickResult {
        candidates: hit.map_or_else(Vec::new, |(id, _)| vec![id]),
        overlay: false,
    })
}
fn sphere_hit(ray: DVec3, center: DVec3, radius: f64) -> Option<f64> {
    let along = center.dot(ray);
    let perpendicular = (center - ray * along).length();
    if perpendicular > radius {
        return None;
    }
    let half = radius * (1.0 - (perpendicular / radius).powi(2)).max(0.0).sqrt();
    let near = along - half;
    let far = along + half;
    if near > 0.0 {
        Some(near)
    } else if far > 0.0 {
        Some(far)
    } else {
        None
    }
}
/// A reference sphere the pointer ray may land on, with the view-to-body-fixed
/// rotation needed to express the hit in that body's own frame.
#[derive(Debug, Clone, Copy)]
pub struct SurfacePickTarget {
    pub body: BodyId,
    pub center_in_view_m: DVec3,
    pub radius_m: f64,
    pub body_fixed_from_view: DQuat,
}
/// Nearest reference-sphere hit under the pointer, in the hit body's body-fixed frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceHit {
    pub body: BodyId,
    pub point_body_m: DVec3,
}
/// Intersects the pointer ray with every reference sphere in front of the camera and
/// returns the nearest. Misses, and spheres entirely behind the camera, give `None`.
pub fn pick_surface_point(
    targets: &[SurfacePickTarget],
    pointer: [f64; 2],
    projection: CelestialProjection,
) -> Result<Option<SurfaceHit>> {
    ensure!(pointer.iter().all(|p| p.is_finite()), "invalid pointer");
    for t in targets {
        ensure!(
            t.center_in_view_m.is_finite()
                && t.radius_m.is_finite()
                && t.radius_m > 0.0
                && t.body_fixed_from_view.is_finite(),
            "invalid pick sphere"
        );
    }
    let ray = projection.unproject_ray(pointer)?;
    Ok(targets
        .iter()
        .filter_map(|t| sphere_hit(ray, t.center_in_view_m, t.radius_m).map(|d| (t, d)))
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(t, distance)| SurfaceHit {
            body: t.body,
            point_body_m: t.body_fixed_from_view * (ray * distance - t.center_in_view_m),
        }))
}
#[cfg(test)]
mod surface_pick_tests {
    use super::*;
    use astrum_math::*;
    use astrum_world::BodyProperties;
    use astrum_world::BodyState;
    fn ids() -> [BodyId; 2] {
        let mut system = CelestialSystem::new(
            std::num::NonZeroU64::new(1).unwrap(),
            SimulationInstant::ZERO,
        );
        let mut add = |name| {
            system
                .insert_body(
                    name,
                    BodyProperties::new(1.0, 1.0).unwrap(),
                    BodyState::new(
                        LocalPosition::origin(),
                        LinearVelocity3::zero(),
                        UnitRotation::identity(),
                        AngularVelocity3::zero(),
                    ),
                )
                .unwrap()
        };
        [add("a"), add("b")]
    }
    fn projection() -> CelestialProjection {
        CelestialProjection::try_new(800, 600, 60.0_f64.to_radians(), 0.1).unwrap()
    }
    fn centre() -> [f64; 2] {
        [400.0, 300.0]
    }
    fn target(body: BodyId, z: f64, radius: f64) -> SurfacePickTarget {
        SurfacePickTarget {
            body,
            center_in_view_m: DVec3::new(0.0, 0.0, z),
            radius_m: radius,
            body_fixed_from_view: DQuat::IDENTITY,
        }
    }
    #[test]
    fn centre_ray_hits_near_pole_in_body_frame() {
        let [a, _] = ids();
        let hit = pick_surface_point(&[target(a, -10.0, 2.0)], centre(), projection())
            .unwrap()
            .unwrap();
        assert_eq!(hit.body, a);
        assert!((hit.point_body_m - DVec3::new(0.0, 0.0, 2.0)).length() < 1e-9);
    }
    #[test]
    fn hit_is_rotated_into_the_body_frame() {
        let [a, _] = ids();
        let mut t = target(a, -10.0, 2.0);
        t.body_fixed_from_view = DQuat::from_rotation_y(std::f64::consts::FRAC_PI_2);
        let hit = pick_surface_point(&[t], centre(), projection())
            .unwrap()
            .unwrap();
        assert!((hit.point_body_m - DVec3::new(2.0, 0.0, 0.0)).length() < 1e-9);
    }
    #[test]
    fn miss_returns_none() {
        let [a, _] = ids();
        let corner = [790.0, 10.0];
        assert!(
            pick_surface_point(&[target(a, -10.0, 1.0)], corner, projection())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn nearest_of_two_spheres_wins_regardless_of_order() {
        let [a, b] = ids();
        let near = target(b, -6.0, 1.0);
        let far = target(a, -20.0, 5.0);
        for list in [[far, near], [near, far]] {
            let hit = pick_surface_point(&list, centre(), projection())
                .unwrap()
                .unwrap();
            assert_eq!(hit.body, b);
        }
    }
    #[test]
    fn sphere_behind_camera_is_ignored() {
        let [a, _] = ids();
        assert!(
            pick_surface_point(&[target(a, 10.0, 2.0)], centre(), projection())
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn limb_inside_hits_and_outside_misses() {
        let [a, _] = ids();
        let p = projection();
        let limb = (1.0_f64 / 10.0).asin();
        let at = |angle: f64| [400.0 + angle.tan() * p.focal_pixels(), 300.0];
        let t = [target(a, -10.0, 1.0)];
        let inside = pick_surface_point(&t, at(limb * 0.999), p)
            .unwrap()
            .unwrap();
        // Near the silhouette the hit normal is almost perpendicular to the view ray.
        assert!(inside.point_body_m.z.abs() < 0.2);
        assert!(
            pick_surface_point(&t, at(limb * 1.001), p)
                .unwrap()
                .is_none()
        );
    }
}
#[derive(Default)]
pub struct PickCycle {
    last: Vec<BodyId>,
    pointer: Option<[f64; 2]>,
    index: usize,
}
impl PickCycle {
    pub fn choose(&mut self, pick: &PickResult, pointer: [f64; 2]) -> Option<BodyId> {
        if pick.candidates.is_empty() {
            return None;
        }
        if self.last == pick.candidates
            && self
                .pointer
                .is_some_and(|p| (p[0] - pointer[0]).hypot(p[1] - pointer[1]) <= 4.0)
        {
            self.index = (self.index + 1) % pick.candidates.len();
        } else {
            self.index = 0;
        }
        self.last.clone_from(&pick.candidates);
        self.pointer = Some(pointer);
        Some(self.last[self.index])
    }
}
