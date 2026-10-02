//! Disposable reference policy and conic geometry. No guide advances a physical body.
use anyhow::{Result, ensure};
use glam::DVec3;
use mundaris_simulation::{
    ConicClass, GRAVITATIONAL_CONSTANT_M3_KG_S2 as G, TwoBodyElements, osculating_elements,
};
use mundaris_world::{BodyId, CelestialSystem, SimulationInstant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrbitGuideReference {
    Automatic,
    Explicit(BodyId),
    None,
}
#[derive(Debug, Clone)]
pub struct OrbitGuide {
    pub body: BodyId,
    pub reference: Option<BodyId>,
    pub automatic_reference: Option<BodyId>,
    pub sampled_time: SimulationInstant,
    pub revision: u64,
    pub elements: Option<TwoBodyElements>,
    pub perturbation_ratio: Option<f64>,
    pub diagnostic: Option<String>,
}
impl OrbitGuide {
    /// Start at 64 segments and deterministically double until screen chord error
    /// meets 0.5 physical pixels, or the caller's bounded cap reports coarse geometry.
    pub fn tessellate(
        &self,
        cap: usize,
        mut project: impl FnMut(DVec3) -> Result<Option<[f64; 2]>>,
        output: &mut Vec<DVec3>,
    ) -> Result<bool> {
        ensure!(cap == 512 || cap == 1024, "invalid tessellation cap");
        let Some(elements) = self.elements.filter(|e| e.class() == ConicClass::Elliptic) else {
            output.clear();
            return Ok(false);
        };
        let mut segments = 64;
        loop {
            let mut coarse = false;
            for i in 0..segments {
                let a = elements
                    .elliptic_position_m(i as f64 * std::f64::consts::TAU / segments as f64)?;
                let b = elements.elliptic_position_m(
                    (i + 1) as f64 * std::f64::consts::TAU / segments as f64,
                )?;
                let midpoint = elements.elliptic_position_m(
                    (i as f64 + 0.5) * std::f64::consts::TAU / segments as f64,
                )?;
                match (project(a)?, project(b)?, project(midpoint)?) {
                    (Some(a), Some(b), Some(m)) => {
                        coarse |= screen_chord_error(a, b, m) > 0.5;
                    }
                    (None, None, None) => {}
                    _ => coarse = true,
                }
            }
            if !coarse || segments >= cap {
                self.vertices(segments, output)?;
                return Ok(coarse);
            }
            segments *= 2;
        }
    }
    pub fn vertices(&self, segments: usize, output: &mut Vec<DVec3>) -> Result<()> {
        ensure!(
            (64..=1024).contains(&segments),
            "invalid guide segment budget"
        );
        output.clear();
        if let Some(e) = self.elements
            && e.class() == ConicClass::Elliptic
        {
            for i in 0..=segments {
                output.push(
                    e.elliptic_position_m(i as f64 * std::f64::consts::TAU / segments as f64)?,
                );
            }
        }
        Ok(())
    }
}
pub(crate) fn screen_chord_error(a: [f64; 2], b: [f64; 2], p: [f64; 2]) -> f64 {
    let delta = [b[0] - a[0], b[1] - a[1]];
    let norm = delta[0] * delta[0] + delta[1] * delta[1];
    let t = if norm > 0.0 {
        (((p[0] - a[0]) * delta[0] + (p[1] - a[1]) * delta[1]) / norm).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p[0] - a[0] - delta[0] * t).hypot(p[1] - a[1] - delta[1] * t)
}
#[derive(Default)]
pub struct OrbitGuides {
    guides: Vec<OrbitGuide>,
    overrides: Vec<(BodyId, OrbitGuideReference)>,
}
impl OrbitGuides {
    pub fn guides(&self) -> &[OrbitGuide] {
        &self.guides
    }
    pub fn set_reference(
        &mut self,
        system: &CelestialSystem,
        body: BodyId,
        reference: OrbitGuideReference,
    ) -> Result<()> {
        system.body(body)?;
        if let OrbitGuideReference::Explicit(id) = reference {
            system.body(id)?;
            ensure!(id != body, "self guide unavailable");
        }
        if let Some(entry) = self.overrides.iter_mut().find(|e| e.0 == body) {
            entry.1 = reference;
        } else {
            self.overrides.push((body, reference));
        }
        Ok(())
    }
    pub fn update(&mut self, system: &CelestialSystem) {
        let mut next = Vec::with_capacity(system.body_count());
        for (id, body) in system.bodies() {
            let position = body.state().center_in_system().metres();
            let mut candidates = Vec::new();
            let mut strongest = 0.0_f64;
            for (reference, other) in system.bodies() {
                if id == reference {
                    continue;
                }
                let r = (position - other.state().center_in_system().metres()).length();
                let attraction = (G / r) / r * other.properties().mass_kg();
                if attraction.is_finite() {
                    strongest = strongest.max(attraction);
                }
                if other.properties().mass_kg() <= body.properties().mass_kg() {
                    continue;
                }
                if let Ok((elements, eta)) = pair_elements(system, id, reference)
                    && elements.class() == ConicClass::Elliptic
                    && eta <= 0.05
                {
                    candidates.push((reference, attraction));
                }
            }
            let mut automatic = candidates
                .iter()
                .filter(|c| c.1 >= strongest * (1.0 - 1e-12))
                .max_by(|a, b| a.1.total_cmp(&b.1))
                .map(|c| c.0);
            if candidates
                .iter()
                .filter(|c| c.1 >= strongest * (1.0 - 1e-12))
                .count()
                > 1
            {
                automatic = None;
            }
            if let Some(previous) = self
                .guides
                .iter()
                .find(|g| g.body == id)
                .and_then(|g| g.automatic_reference)
                && let Some(old) = candidates.iter().find(|c| c.0 == previous)
                && strongest <= old.1 * 1.25
            {
                automatic = Some(previous);
            }
            let policy = self
                .overrides
                .iter()
                .find(|e| e.0 == id)
                .map_or(OrbitGuideReference::Automatic, |e| e.1);
            let reference = match policy {
                OrbitGuideReference::Automatic => automatic,
                OrbitGuideReference::Explicit(id) => Some(id),
                OrbitGuideReference::None => None,
            };
            let mut guide = OrbitGuide {
                body: id,
                reference,
                automatic_reference: automatic,
                sampled_time: system.sample_time(),
                revision: system.revision(),
                elements: None,
                perturbation_ratio: None,
                diagnostic: None,
            };
            if let Some(reference) = reference {
                match pair_elements(system, id, reference) {
                    Ok((elements, eta)) => {
                        guide.elements = Some(elements);
                        guide.perturbation_ratio = Some(eta);
                        if elements.class() != ConicClass::Elliptic {
                            guide.diagnostic =
                                Some(format!("{:?}; closed guide unavailable", elements.class()));
                        } else if eta > 0.05 {
                            guide.diagnostic =
                                Some("Instantaneous, strongly perturbed guide".into());
                        }
                    }
                    Err(error) => guide.diagnostic = Some(error.to_string()),
                }
            } else {
                guide.diagnostic = Some("No automatic reference; choose an explicit pair".into());
            }
            next.push(guide);
        }
        self.guides = next;
    }
    pub fn subsystem(&self, selected: BodyId) -> Vec<BodyId> {
        std::iter::once(selected)
            .chain(
                self.guides
                    .iter()
                    .filter(|g| g.automatic_reference == Some(selected))
                    .map(|g| g.body),
            )
            .collect()
    }
}
fn pair_elements(
    system: &CelestialSystem,
    body: BodyId,
    reference: BodyId,
) -> Result<(TwoBodyElements, f64)> {
    let b = system.body(body)?;
    let q = system.body(reference)?;
    let qp = q.state().center_in_system().metres();
    let r = b.state().center_in_system().metres() - qp;
    let v = b.state().center_velocity_in_system().metres_per_second()
        - q.state().center_velocity_in_system().metres_per_second();
    let elements = osculating_elements(r, v, b.properties().mass_kg(), q.properties().mass_kg())?;
    let mut eta = perturbation(system, body, reference, r)?;
    if elements.class() == ConicClass::Elliptic {
        for i in 0..8 {
            eta = eta.max(perturbation(
                system,
                body,
                reference,
                elements.elliptic_position_m(i as f64 * std::f64::consts::TAU / 8.0)?,
            )?);
        }
    }
    Ok((elements, eta))
}
fn perturbation(
    system: &CelestialSystem,
    body: BodyId,
    reference: BodyId,
    r: DVec3,
) -> Result<f64> {
    let b = system.body(body)?;
    let q = system.body(reference)?;
    let origin = q.state().center_in_system().metres();
    let mut external = DVec3::ZERO;
    let acceleration = |delta: DVec3, mass: f64| -> Result<DVec3> {
        let d = delta.x.hypot(delta.y).hypot(delta.z);
        ensure!(
            d.is_normal(),
            "unavailable perturbation at coincident sample"
        );
        let a = (delta / d) * ((G / d) / d * mass);
        ensure!(a.is_finite(), "invalid perturbation arithmetic");
        Ok(a)
    };
    for (id, other) in system.bodies() {
        if id == body || id == reference {
            continue;
        }
        let delta = other.state().center_in_system().metres() - origin;
        external += acceleration(delta - r, other.properties().mass_kg())?
            - acceleration(delta, other.properties().mass_kg())?;
    }
    let d = r.x.hypot(r.y).hypot(r.z);
    let pair = ((G * b.properties().mass_kg() + G * q.properties().mass_kg()) / d) / d;
    let eta = external.x.hypot(external.y).hypot(external.z) / pair;
    ensure!(eta.is_finite(), "invalid differential perturbation");
    Ok(eta)
}
