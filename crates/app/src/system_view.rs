//! Geometric navigation bounds: includes all scoped bodies, never a physics cutoff.
use crate::orbit_guides::OrbitGuide;
use anyhow::{Result, ensure};
use glam::DVec3;
use mundaris_renderer::CelestialProjection;
use mundaris_world::{BodyId, CelestialSystem};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverviewScope {
    WholeSystem,
    SelectedSubsystem(BodyId),
    ExplicitBodies(Vec<BodyId>),
}
#[derive(Debug, Clone)]
pub struct SystemViewBounds {
    center_m: DVec3,
    radius_m: f64,
    included: Vec<BodyId>,
    pub body_radius_m: f64,
    pub guide_radius_m: f64,
    pub history_outside_fit: bool,
    pub history_radius_m: f64,
}
impl SystemViewBounds {
    pub fn calculate(
        system: &CelestialSystem,
        scope: &OverviewScope,
        guides: &[OrbitGuide],
        history_points: &[DVec3],
        all_history: bool,
    ) -> Result<Self> {
        let included: Vec<_> = match scope {
            OverviewScope::WholeSystem => system.bodies().map(|(id, _)| id).collect(),
            OverviewScope::ExplicitBodies(ids) => ids.clone(),
            OverviewScope::SelectedSubsystem(id) => std::iter::once(*id)
                .chain(
                    guides
                        .iter()
                        .filter(|g| g.automatic_reference == Some(*id))
                        .map(|g| g.body),
                )
                .collect(),
        };
        for &id in &included {
            system.body(id)?;
        }
        let anchor = included.first().map_or(Ok(DVec3::ZERO), |&id| {
            system
                .body(id)
                .map(|b| b.state().center_in_system().metres())
        })?;
        let mut min = DVec3::splat(f64::MAX);
        let mut max = -min;
        let mut include = |a: DVec3, b: DVec3| -> Result<()> {
            ensure!(a.is_finite() && b.is_finite(), "invalid system bounds");
            min = min.min(a);
            max = max.max(b);
            Ok(())
        };
        for &id in &included {
            let b = system.body(id)?;
            let offset = b.state().center_in_system().metres() - anchor;
            let radius = DVec3::splat(b.properties().reference_radius_m());
            include(offset - radius, offset + radius)?;
        }
        if included.is_empty() {
            min = -DVec3::ONE;
            max = DVec3::ONE;
        }
        let body_radius_m = ((max - min) * 0.5).length();
        for g in guides {
            if !included.contains(&g.body) {
                continue;
            }
            // Local scope omits outer trajectories about references outside membership.
            if !matches!(scope, OverviewScope::WholeSystem)
                && g.reference.is_some_and(|r| !included.contains(&r))
            {
                continue;
            }
            if let (Some(reference), Some(elements)) = (g.reference, g.elements)
                && let Ok([a, b]) = elements.elliptic_bounds_m()
            {
                let offset = system.body(reference)?.state().center_in_system().metres() - anchor;
                min = min.min(offset + a);
                max = max.max(offset + b);
            }
        }
        let guide_radius_m = ((max - min) * 0.5).length();
        let core_center = min + (max - min) * 0.5;
        let core_radius = guide_radius_m.max(1.0);
        let mut history_outside_fit = false;
        let mut history_radius_m = 0.0_f64;
        for &point in history_points {
            let offset = point - anchor;
            let delta = offset - core_center;
            let distance = delta.length();
            ensure!(distance.is_finite(), "invalid history fit point");
            history_radius_m = history_radius_m.max(distance);
            let fitted = if !all_history && distance > 2.0 * core_radius {
                history_outside_fit = true;
                core_center + delta * (2.0 * core_radius / distance)
            } else {
                offset
            };
            min = min.min(fitted);
            max = max.max(fitted);
        }
        let center_m = anchor + (min + (max - min) * 0.5);
        let radius_m = ((max - min) * 0.5).length().max(1.0);
        ensure!(
            center_m.is_finite() && radius_m.is_finite(),
            "unrepresentable visual bounds"
        );
        Ok(Self {
            center_m,
            radius_m,
            included,
            body_radius_m,
            guide_radius_m,
            history_outside_fit,
            history_radius_m,
        })
    }
    pub fn center_m(&self) -> DVec3 {
        self.center_m
    }
    pub fn radius_m(&self) -> f64 {
        self.radius_m
    }
    pub fn included(&self) -> &[BodyId] {
        &self.included
    }
    pub fn fit_distance_m(&self, projection: CelestialProjection) -> Result<f64> {
        let [w, h] = projection.viewport().map(f64::from);
        let margin_x = 24.0_f64.max(0.1 * w);
        let margin_y = 24.0_f64.max(0.1 * h);
        ensure!(
            w > 2.0 * margin_x && h > 2.0 * margin_y,
            "viewport too small for navigation padding"
        );
        let tan = (projection.vertical_fov_rad() * 0.5).tan();
        let angle = (tan * (1.0 - 2.0 * margin_y / h))
            .min(tan * w / h * (1.0 - 2.0 * margin_x / w))
            .atan();
        let distance = (self.radius_m / angle.sin()).max(self.radius_m + projection.near_m());
        ensure!(
            distance.is_finite() && distance <= 1e15,
            "system fit exceeds 1e15 m navigation limit"
        );
        Ok(distance)
    }
}
