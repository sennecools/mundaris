//! Unified selection identity and headless label/marker/reference-sphere picking.
use crate::celestial_labels::ScreenRect;
use anyhow::{Result, ensure};
use glam::DVec3;
use astrum_renderer::CelestialProjection;
use astrum_world::{BodyId, CelestialSystem};
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
