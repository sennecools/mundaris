//! Disposable reference policy and conic geometry. No guide advances a physical body.
use anyhow::{Result, ensure};
use glam::DVec3;
use astrum_simulation::{
    ConicClass, GRAVITATIONAL_CONSTANT_M3_KG_S2 as G, TwoBodyElements, osculating_elements,
};
use astrum_world::{
    BodyId, CelestialMotionDefinition, CelestialSystem, CelestialTranslation, EllipticOrbit,
    SimulationInstant,
};

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
    /// Authored ellipse used directly for prescribed-motion navigation geometry.
    pub authored_orbit: Option<EllipticOrbit>,
    pub perturbation_ratio: Option<f64>,
    pub diagnostic: Option<String>,
}
impl OrbitGuide {
    pub fn has_geometry(&self) -> bool {
        self.authored_orbit.is_some()
            || self
                .elements
                .is_some_and(|e| e.class() == ConicClass::Elliptic)
    }
    pub fn bounds_m(&self) -> Result<Option<[DVec3; 2]>> {
        if let Some(orbit) = self.authored_orbit {
            let rotation = orbit.plane_to_system().quaternion();
            let x = rotation * DVec3::X * orbit.semi_major_axis_m();
            let y = rotation
                * DVec3::Y
                * (orbit.semi_major_axis_m()
                    * ((1.0 - orbit.eccentricity()) * (1.0 + orbit.eccentricity())).sqrt());
            let center = -x * orbit.eccentricity();
            let extent = DVec3::new(x.x.hypot(y.x), x.y.hypot(y.y), x.z.hypot(y.z));
            ensure!(
                center.is_finite() && extent.is_finite(),
                "unrepresentable authored guide bounds"
            );
            return Ok(Some([center - extent, center + extent]));
        }
        self.elements
            .filter(|e| e.class() == ConicClass::Elliptic)
            .map(|e| e.elliptic_bounds_m().map_err(anyhow::Error::new))
            .transpose()
    }
    /// Start at 64 segments and deterministically double until screen chord error
    /// meets 0.5 physical pixels, or the caller's bounded cap reports coarse geometry.
    pub fn tessellate(
        &self,
        cap: usize,
        mut project: impl FnMut(DVec3) -> Result<Option<[f64; 2]>>,
        output: &mut Vec<DVec3>,
    ) -> Result<bool> {
        ensure!(cap == 512 || cap == 1024, "invalid tessellation cap");
        if let Some(orbit) = self.authored_orbit {
            let mut segments = 64;
            loop {
                let mut coarse = false;
                for i in 0..segments {
                    let a = authored_position(
                        orbit,
                        i as f64 * std::f64::consts::TAU / segments as f64,
                    );
                    let b = authored_position(
                        orbit,
                        (i + 1) as f64 * std::f64::consts::TAU / segments as f64,
                    );
                    let m = authored_position(
                        orbit,
                        (i as f64 + 0.5) * std::f64::consts::TAU / segments as f64,
                    );
                    match (project(a)?, project(b)?, project(m)?) {
                        (Some(a), Some(b), Some(m)) => coarse |= screen_chord_error(a, b, m) > 0.5,
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
        if let Some(orbit) = self.authored_orbit {
            for i in 0..=segments {
                output.push(authored_position(
                    orbit,
                    i as f64 * std::f64::consts::TAU / segments as f64,
                ));
            }
            return Ok(());
        }
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
fn authored_position(orbit: EllipticOrbit, eccentric_anomaly: f64) -> DVec3 {
    let e = orbit.eccentricity();
    let a = orbit.semi_major_axis_m();
    let (sin_e, cos_e) = eccentric_anomaly.sin_cos();
    orbit.plane_to_system().quaternion()
        * DVec3::new(a * (cos_e - e), a * (1.0 - e * e).sqrt() * sin_e, 0.0)
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
                authored_orbit: None,
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
    /// Rebuild navigation guides from authored prescribed translations, without
    /// deriving osculating/gravitational elements.
    pub fn update_analytic(
        &mut self,
        system: &CelestialSystem,
        definition: &CelestialMotionDefinition,
    ) {
        let mut next = Vec::with_capacity(system.body_count());
        for motion in definition.definitions() {
            let authored_orbit = match motion.translation {
                CelestialTranslation::Elliptic(orbit) => Some(orbit),
                CelestialTranslation::Stationary(_) => None,
            };
            let automatic = authored_orbit.map(EllipticOrbit::reference);
            let policy = self
                .overrides
                .iter()
                .find(|e| e.0 == motion.body)
                .map_or(OrbitGuideReference::Automatic, |e| e.1);
            let requested = match policy {
                OrbitGuideReference::Automatic => automatic,
                OrbitGuideReference::Explicit(id) => Some(id),
                OrbitGuideReference::None => None,
            };
            let matches = authored_orbit.is_none_or(|orbit| Some(orbit.reference()) == requested);
            let mut guide = OrbitGuide {
                body: motion.body,
                reference: requested,
                automatic_reference: automatic,
                sampled_time: system.sample_time(),
                revision: system.revision(),
                elements: None,
                authored_orbit: authored_orbit.filter(|_| matches),
                perturbation_ratio: None,
                diagnostic: None,
            };
            if let (Some(orbit), Some(reference)) = (authored_orbit, requested) {
                if reference != orbit.reference() {
                    guide.diagnostic = Some(format!(
                        "Explicit reference {reference:?} does not match authored orbit reference {:?}; authored guide unavailable",
                        orbit.reference()
                    ));
                }
            } else if authored_orbit.is_none() && requested.is_some() {
                guide.diagnostic = Some("Stationary body has no authored orbit geometry".into());
            } else if requested.is_none() && authored_orbit.is_some() {
                guide.diagnostic = Some("No guide reference selected".into());
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
