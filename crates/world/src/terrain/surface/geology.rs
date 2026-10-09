//! Deterministic icy and volcanic surface fields.
//!
//! Local features are evaluated from a fixed Cartesian shell halo. Regional
//! structure and feature masks carry analytic derivatives so the returned
//! tangent gradient describes the complete composed height field.

use super::{GeologicalParameters, SurfaceAlgorithm, SurfaceQueryWork, TerrainError};
use astrum_math::noise::gradient_noise;
use glam::{DMat3, DVec3};

const MIN_EDGE_M: f64 = 8.0;
const CELL_JITTER: f64 = 0.2;
const SHELL_FRACTION: f64 = 0.12;
const SUPPORT_FRACTION: f64 = 0.65;
const MAX_RADIUS_M: f64 = 1.0e8;
// A skipped cell's centre remains outside support even after jitter and shell projection.
const _: () = assert!(1.0 - CELL_JITTER - SHELL_FRACTION > SUPPORT_FRACTION);

#[derive(Debug, Clone, Copy)]
pub(super) struct GeologySample {
    pub(super) height_m: f64,
    /// Derivative in metres per unit change of the normalized body direction.
    pub(super) gradient_m: DVec3,
    pub(super) weights: [f64; 4],
    pub(super) work: SurfaceQueryWork,
}

#[derive(Debug, Clone)]
pub(super) struct GeologyField {
    algorithm: SurfaceAlgorithm,
    parameters: GeologicalParameters,
    seed: u64,
    radius_m: f64,
    rotation: DMat3,
    edge_m: [f64; 4],
    bound_m: f64,
}

impl GeologyField {
    pub(super) fn new(
        algorithm: SurfaceAlgorithm,
        parameters: GeologicalParameters,
        seed: u64,
        radius_m: f64,
    ) -> Result<Self, TerrainError> {
        if !matches!(
            algorithm,
            SurfaceAlgorithm::IcyV1 | SurfaceAlgorithm::VolcanicV1
        ) {
            return Err(TerrainError::InvalidConfig);
        }
        if !radius_m.is_finite() || radius_m <= 0.0 || radius_m > MAX_RADIUS_M {
            return Err(TerrainError::InvalidRadius);
        }
        parameters.validate()?;
        let salt = mix(seed ^ algorithm_tag(algorithm));
        let angle = |tag| unit(salt ^ tag) * std::f64::consts::TAU;
        let rotation = DMat3::from_rotation_z(angle(1))
            * DMat3::from_rotation_y(angle(2))
            * DMat3::from_rotation_x(angle(3))
            * DMat3::from_rotation_z(parameters.orientation_radians);
        let largest_edge = (radius_m * parameters.feature_scale_fraction).max(MIN_EDGE_M);
        let edge_m = [
            largest_edge,
            largest_edge * 0.22,
            largest_edge * 0.045,
            largest_edge * 0.012,
        ]
        .map(|edge| edge.max(MIN_EDGE_M));
        // Regional structure and four bounded local epochs have explicit
        // envelopes. Epochs replace within themselves; only their budgets add.
        let relief = outward_product(radius_m, parameters.relief_fraction);
        let bound_fraction = match algorithm {
            // Icy: regional blocks <= .838, body-scale paired bands <= .064,
            // noisy fractures <= .004, compact cellular fracture pairs <=
            // .636*(.09+.08+.07+.06)=.1908, and retained impacts <= .302.
            // Their sum is 1.3988 relief units; 1.74 remains conservative.
            SurfaceAlgorithm::IcyV1 => 1.74,
            // Volcanic: plains <= .307, local flow ripples <= .012, each
            // emplacement <= 1.07 with budgets summing to .81, and impacts
            // <= .84*(.07+.045+.025+.015)=.131. Total < 1.32.
            SurfaceAlgorithm::VolcanicV1 => 1.62,
            SurfaceAlgorithm::PreparedV1
            | SurfaceAlgorithm::WorldV1
            | SurfaceAlgorithm::RockyV3
            | SurfaceAlgorithm::RockyV4
            | SurfaceAlgorithm::RockyV5
            | SurfaceAlgorithm::IcyV2
            | SurfaceAlgorithm::IcyV3
            | SurfaceAlgorithm::VolcanicV2
            | SurfaceAlgorithm::VolcanicV3
            | SurfaceAlgorithm::MoonFieldsV1
            | SurfaceAlgorithm::MoonProfileV1 => return Err(TerrainError::InvalidConfig),
        };
        let bound_m = if relief == 0.0 {
            0.0
        } else {
            (relief * bound_fraction).next_up()
        };
        if !bound_m.is_finite() || bound_m >= radius_m * 0.1 {
            return Err(TerrainError::InvalidRadius);
        }
        Ok(Self {
            algorithm,
            parameters,
            seed,
            radius_m,
            rotation,
            edge_m,
            bound_m,
        })
    }

    pub(super) fn absolute_height_bound_m(&self) -> f64 {
        self.bound_m
    }

    /// Select one already-supported feature in each fixed scale epoch and
    /// return directions immediately inside/outside its compact profile edge.
    /// This diagnostic is bounded by the same local cell halo as evaluation.
    pub(super) fn diagnostic_boundary_probes(
        &self,
        near: DVec3,
    ) -> Result<Vec<(&'static str, DVec3)>, TerrainError> {
        if !near.is_finite() || near.length_squared() <= f64::MIN_POSITIVE {
            return Err(TerrainError::InvalidConfig);
        }
        let near = near.normalize();
        let icy_labels = [
            (
                "icy-impact-0-in",
                "icy-impact-0-out",
                "icy-fracture-0-in",
                "icy-fracture-0-out",
            ),
            (
                "icy-impact-1-in",
                "icy-impact-1-out",
                "icy-fracture-1-in",
                "icy-fracture-1-out",
            ),
            (
                "icy-impact-2-in",
                "icy-impact-2-out",
                "icy-fracture-2-in",
                "icy-fracture-2-out",
            ),
            (
                "icy-impact-3-in",
                "icy-impact-3-out",
                "icy-fracture-3-in",
                "icy-fracture-3-out",
            ),
        ];
        let volcanic_labels = [
            (
                "volcanic-impact-0-in",
                "volcanic-impact-0-out",
                "volcanic-flow-0-in",
                "volcanic-flow-0-out",
            ),
            (
                "volcanic-impact-1-in",
                "volcanic-impact-1-out",
                "volcanic-flow-1-in",
                "volcanic-flow-1-out",
            ),
            (
                "volcanic-impact-2-in",
                "volcanic-impact-2-out",
                "volcanic-flow-2-in",
                "volcanic-flow-2-out",
            ),
            (
                "volcanic-impact-3-in",
                "volcanic-impact-3-out",
                "volcanic-flow-3-in",
                "volcanic-flow-3-out",
            ),
        ];
        let labels = match self.algorithm {
            SurfaceAlgorithm::IcyV1 => &icy_labels,
            SurfaceAlgorithm::VolcanicV1 => &volcanic_labels,
            SurfaceAlgorithm::PreparedV1
            | SurfaceAlgorithm::WorldV1
            | SurfaceAlgorithm::RockyV3
            | SurfaceAlgorithm::RockyV4
            | SurfaceAlgorithm::RockyV5
            | SurfaceAlgorithm::IcyV2
            | SurfaceAlgorithm::IcyV3
            | SurfaceAlgorithm::VolcanicV2
            | SurfaceAlgorithm::VolcanicV3
            | SurfaceAlgorithm::MoonFieldsV1
            | SurfaceAlgorithm::MoonProfileV1 => return Err(TerrainError::InvalidConfig),
        };
        let mut probes = Vec::with_capacity(labels.len() * 4);
        for (epoch, epoch_labels) in labels.iter().enumerate() {
            let mut work = SurfaceQueryWork {
                accepted_features: Some(0),
                ..SurfaceQueryWork::default()
            };
            let mut selected_impact = None;
            let mut selected_surface = None;
            self.visit_features(near, epoch, 1, &mut work, |center, key| {
                let edge = self.edge_m[epoch];
                if selected_impact.is_none() {
                    let support = match self.algorithm {
                        SurfaceAlgorithm::IcyV1 => {
                            edge * (0.13 + 0.06 * unit(key ^ 0x4943_595f_5241_4401)) * 1.75
                        }
                        SurfaceAlgorithm::VolcanicV1 => {
                            edge * (0.13 + 0.06 * unit(key ^ 0x4943_595f_5241_4401)) * 1.75
                        }
                        SurfaceAlgorithm::PreparedV1
                        | SurfaceAlgorithm::WorldV1
                        | SurfaceAlgorithm::RockyV3
                        | SurfaceAlgorithm::RockyV4
                        | SurfaceAlgorithm::RockyV5
                        | SurfaceAlgorithm::IcyV2
                        | SurfaceAlgorithm::IcyV3
                        | SurfaceAlgorithm::VolcanicV2
                        | SurfaceAlgorithm::VolcanicV3
                        | SurfaceAlgorithm::MoonFieldsV1
                        | SurfaceAlgorithm::MoonProfileV1 => 0.0,
                    };
                    selected_impact = Some((center.normalize(), support));
                }
                let present = match self.algorithm {
                    SurfaceAlgorithm::IcyV1 => {
                        unit(key ^ 0x4943_595f_4445_4e53) < 0.40 + 0.54 * self.parameters.activity
                    }
                    SurfaceAlgorithm::VolcanicV1 => true,
                    SurfaceAlgorithm::PreparedV1
                    | SurfaceAlgorithm::WorldV1
                    | SurfaceAlgorithm::RockyV3
                    | SurfaceAlgorithm::RockyV4
                    | SurfaceAlgorithm::RockyV5
                    | SurfaceAlgorithm::IcyV2
                    | SurfaceAlgorithm::IcyV3
                    | SurfaceAlgorithm::VolcanicV2
                    | SurfaceAlgorithm::VolcanicV3
                    | SurfaceAlgorithm::MoonFieldsV1
                    | SurfaceAlgorithm::MoonProfileV1 => false,
                };
                if selected_surface.is_none() && present {
                    selected_surface = Some(center.normalize());
                }
            })?;
            let mut append = |center_direction: DVec3, support_m: f64, first: usize| {
                let helper = if center_direction.dot(DVec3::X).abs() < 0.8 {
                    DVec3::X
                } else {
                    DVec3::Y
                };
                let tangent = center_direction.cross(helper).normalize();
                let (inside_label, outside_label) = if first == 0 {
                    (epoch_labels.0, epoch_labels.1)
                } else {
                    (epoch_labels.2, epoch_labels.3)
                };
                for (label, multiplier) in
                    [(inside_label, 1.0 - 1.0e-4), (outside_label, 1.0 + 1.0e-4)]
                {
                    let chord_m = support_m * multiplier;
                    let half_angle = (chord_m / (2.0 * self.radius_m)).clamp(0.0, 1.0).asin();
                    let angle = half_angle * 2.0;
                    probes.push((
                        label,
                        center_direction * angle.cos() + tangent * angle.sin(),
                    ));
                }
            };
            if let Some((center, support)) = selected_impact {
                append(center, support, 0);
            }
            if let Some(center) = selected_surface {
                append(center, self.edge_m[epoch] * SUPPORT_FRACTION, 2);
            }
        }
        Ok(probes)
    }

    /// Select a genuine small-scale feature from the same fixed local cell
    /// halo used by field evaluation. Returned directions lie on its fracture
    /// core/shoulders or volcanic caldera/front profile, not on a synthetic
    /// sampling grid.
    pub(super) fn diagnostic_landmark_probes(
        &self,
        near: DVec3,
    ) -> Result<Vec<(&'static str, DVec3)>, TerrainError> {
        if !near.is_finite() || near.length_squared() <= f64::MIN_POSITIVE {
            return Err(TerrainError::InvalidConfig);
        }
        if self.parameters.relief_fraction == 0.0
            || (self.algorithm == SurfaceAlgorithm::VolcanicV1 && self.parameters.activity == 0.0)
        {
            return Ok(Vec::new());
        }
        let near = near.normalize();
        let mut output = Vec::with_capacity(3);
        let epoch = match self.algorithm {
            SurfaceAlgorithm::IcyV1 => 3,
            SurfaceAlgorithm::VolcanicV1 => 2,
            SurfaceAlgorithm::PreparedV1
            | SurfaceAlgorithm::WorldV1
            | SurfaceAlgorithm::RockyV3
            | SurfaceAlgorithm::RockyV4
            | SurfaceAlgorithm::RockyV5
            | SurfaceAlgorithm::IcyV2
            | SurfaceAlgorithm::IcyV3
            | SurfaceAlgorithm::VolcanicV2
            | SurfaceAlgorithm::VolcanicV3
            | SurfaceAlgorithm::MoonFieldsV1
            | SurfaceAlgorithm::MoonProfileV1 => return Err(TerrainError::InvalidConfig),
        };
        let mut work = SurfaceQueryWork {
            accepted_features: Some(0),
            ..SurfaceQueryWork::default()
        };
        let mut chosen = None;
        self.visit_features(near, epoch, 1, &mut work, |center, key| {
            if chosen.is_some() {
                return;
            }
            match self.algorithm {
                SurfaceAlgorithm::IcyV1 => {
                    let density = 0.40 + 0.54 * self.parameters.activity;
                    if unit(key ^ 0x4943_595f_4445_4e53) < density {
                        let preference_a =
                            self.rotation.transpose() * self.ridge_axis(0x4943_595f_4652_4141);
                        let preference_b =
                            self.rotation.transpose() * self.ridge_axis(0x4943_595f_4652_4142);
                        let frame = icy_fracture_frame(center, key, preference_a, preference_b);
                        let phase = unit(key ^ 0x4943_595f_4245_4e44) * std::f64::consts::TAU;
                        let bend =
                            phase.sin() * (0.035 + 0.035 * unit(key ^ 0x4943_595f_4355_5256));
                        let point = |lateral: f64| {
                            (center
                                + frame.across
                                    * (self.edge_m[epoch] * (bend + frame.width * lateral)))
                                .normalize()
                        };
                        chosen = Some(vec![
                            ("icy-fracture-epoch3-core", point(0.0)),
                            ("icy-fracture-epoch3-shoulder-a", point(2.1)),
                            ("icy-fracture-epoch3-shoulder-b", point(-2.1)),
                        ]);
                    }
                }
                SurfaceAlgorithm::VolcanicV1 => {
                    let frame = volcanic_frame(center, key);
                    let edge = self.edge_m[epoch];
                    let caldera_radius = 0.17 + 0.11 * unit(key ^ 31);
                    let caldera =
                        (center + frame.across * (edge * frame.minor * caldera_radius)).normalize();
                    let mut lo = 0.25;
                    let mut hi = 1.45;
                    for _ in 0..40 {
                        let mid = (lo + hi) * 0.5;
                        let (_radial, footprint) =
                            volcanic_footprint(key, self.parameters.activity, mid, 0.0);
                        if footprint < 0.82 {
                            lo = mid;
                        } else {
                            hi = mid;
                        }
                    }
                    let along = (lo + hi) * 0.5;
                    let front = (center + frame.axis * (edge * frame.major * along)).normalize();
                    chosen = Some(vec![
                        ("volcanic-epoch2-caldera-rim", caldera),
                        ("volcanic-epoch2-lobed-front", front),
                    ]);
                }
                SurfaceAlgorithm::PreparedV1
                | SurfaceAlgorithm::WorldV1
                | SurfaceAlgorithm::RockyV3
                | SurfaceAlgorithm::RockyV4
                | SurfaceAlgorithm::RockyV5
                | SurfaceAlgorithm::IcyV2
                | SurfaceAlgorithm::IcyV3
                | SurfaceAlgorithm::VolcanicV2
                | SurfaceAlgorithm::VolcanicV3
                | SurfaceAlgorithm::MoonFieldsV1
                | SurfaceAlgorithm::MoonProfileV1 => {}
            }
        })?;
        if let Some(probes) = chosen {
            output.extend(probes);
        }
        Ok(output)
    }

    pub(super) fn evaluate(&self, direction: DVec3) -> Result<GeologySample, TerrainError> {
        self.evaluate_with_halo(direction, 1)
    }

    fn evaluate_with_halo(
        &self,
        direction: DVec3,
        halo: i64,
    ) -> Result<GeologySample, TerrainError> {
        if !direction.is_finite() || direction.length_squared() <= f64::MIN_POSITIVE {
            return Err(TerrainError::InvalidConfig);
        }
        let n = direction.normalize();
        let (height, gradient, mut weights, work) = match self.algorithm {
            SurfaceAlgorithm::IcyV1 => self.icy(n, halo)?,
            SurfaceAlgorithm::VolcanicV1 => self.volcanic(n, halo)?,
            SurfaceAlgorithm::PreparedV1
            | SurfaceAlgorithm::WorldV1
            | SurfaceAlgorithm::RockyV3
            | SurfaceAlgorithm::RockyV4
            | SurfaceAlgorithm::RockyV5
            | SurfaceAlgorithm::IcyV2
            | SurfaceAlgorithm::IcyV3
            | SurfaceAlgorithm::VolcanicV2
            | SurfaceAlgorithm::VolcanicV3
            | SurfaceAlgorithm::MoonFieldsV1
            | SurfaceAlgorithm::MoonProfileV1 => return Err(TerrainError::InvalidConfig),
        };
        normalize_weights(&mut weights);
        if !height.is_finite()
            || !gradient.is_finite()
            || weights.iter().any(|weight| !weight.is_finite())
        {
            return Err(TerrainError::NonFiniteResult);
        }
        Ok(GeologySample {
            height_m: height,
            gradient_m: tangent_project(n, gradient),
            weights,
            work,
        })
    }

    fn icy(
        &self,
        n: DVec3,
        halo: i64,
    ) -> Result<(f64, DVec3, [f64; 4], SurfaceQueryWork), TerrainError> {
        let p = self.rotation * n;
        let (regional, regional_gradient) = self.icy_regional(p)?;
        let scale = self.radius_m * self.parameters.relief_fraction;
        let mut height = regional * scale;
        let mut gradient = regional_gradient * scale;
        let mut old = self.parameters.age;
        let tectonic_strength = 0.35 + 0.65 * self.parameters.activity;
        let chaos = self.icy_chaos(p)?;
        let block_noise = noise(self.salt(0x4943_595f_424c_4f43), p, 52.0)?
            + noise(self.salt(0x4943_595f_424c_4f32), p, 87.0)? * 0.38;
        let block_field = (block_noise - 0.08).smoothstep(-0.22, 0.24);
        let chaos_blocks = chaos * (block_field - 0.5) * 0.22;
        height += chaos_blocks.value * scale;
        gradient += self.rotation.transpose() * chaos_blocks.gradient * scale;
        let contamination = noise(self.salt(0x4943_595f_4445_4252), p, 5.0)?.smoothstep(0.18, 0.72)
            * (0.10 + self.parameters.resurfacing_fraction * 0.32);
        let mut work = SurfaceQueryWork {
            accepted_features: Some(0),
            ..SurfaceQueryWork::default()
        };

        // Weak body-scale pairs retain regional orientation cues. The visible
        // ridges are compact curved segments below, so a few angular periods
        // cannot dominate the silhouette as globe-wide corduroy.
        let ridge_mask = Diff::constant(1.0) - chaos * 0.58;
        for (frequency, amplitude, phase) in [
            (58.0, 0.025, 0x4943_595f_5249_4401),
            (151.0, 0.014, 0x4943_595f_5249_4402),
            (389.0, 0.006, 0x4943_595f_5249_4403),
        ] {
            let axis = self.ridge_axis(phase);
            let coordinate = p.dot(axis);
            let warped = self.icy_warp(p, frequency, phase)?;
            let phase_value = Diff {
                value: coordinate * frequency + warped.value * 0.72,
                gradient: axis * frequency + warped.gradient * 0.72,
            };
            let pair = paired_ridge(phase_value);
            let term = pair * ridge_mask * (amplitude * tectonic_strength);
            height += term.value * scale;
            gradient += self.rotation.transpose() * term.gradient * scale;
        }

        // Smooth old impacts are low-frequency remnants, composed as bounded
        // epochs so their overlap cannot sum without limit.
        let micro = noise(self.salt(0x4943_595f_4d49_4352), p, 1_200.0)?;
        let fracture =
            (Diff::constant(0.075) - (micro * micro + 0.0004).sqrt()).smoothstep(0.0, 0.06);
        let fracture_term = fracture * chaos * 0.004;
        height += fracture_term.value * scale;
        gradient += self.rotation.transpose() * fracture_term.gradient * scale;

        let impact_budgets = [0.16, 0.105, 0.06, 0.035];
        let fracture_budgets = [0.09, 0.08, 0.07, 0.06];
        let mut fracture_disruption = Diff::constant(0.0);
        let preference_a = self.rotation.transpose() * self.ridge_axis(0x4943_595f_4652_4141);
        let preference_b = self.rotation.transpose() * self.ridge_axis(0x4943_595f_4652_4142);
        for epoch in 0..impact_budgets.len() {
            let mut impact_residual = Diff::constant(0.0);
            let mut fracture_residual = Diff::constant(0.0);
            self.visit_features(n, epoch, halo, &mut work, |center, feature_key| {
                let impact = icy_impact(
                    n * self.radius_m,
                    center,
                    self.edge_m[epoch],
                    feature_key,
                    old,
                );
                impact_residual = impact_residual * (Diff::constant(1.0) - impact.influence)
                    + impact.height * impact.influence;
                let fracture = icy_fracture(
                    n * self.radius_m,
                    center,
                    self.edge_m[epoch],
                    feature_key,
                    self.parameters.activity,
                    self.parameters.resurfacing_fraction,
                    preference_a,
                    preference_b,
                );
                fracture_residual = fracture_residual * (Diff::constant(1.0) - fracture.influence)
                    + fracture.height * fracture.influence;
                fracture_disruption = fracture_disruption
                    + fracture.influence * (Diff::constant(1.0) - fracture_disruption);
            })?;
            let retained = self.parameters.impact_retention * (0.35 + 0.65 * old);
            let impact_budget = impact_budgets[epoch];
            height += impact_residual.value * scale * impact_budget * retained;
            gradient +=
                impact_residual.gradient * (self.radius_m * scale * impact_budget * retained);
            let fracture_budget = fracture_budgets[epoch];
            let fracture_strength = 0.45 + 0.55 * self.parameters.resurfacing_fraction;
            height += fracture_residual.value * scale * fracture_budget * fracture_strength;
            gradient += fracture_residual.gradient
                * (self.radius_m * scale * fracture_budget * fracture_strength);
            old *= 0.73;
        }
        let disruption = (chaos * 0.65 + fracture_disruption * 0.35).clamp01();
        let weights = [
            ((Diff::constant(1.0) - disruption - contamination) * (1.0 - self.parameters.age))
                .clamp01()
                .value,
            ((Diff::constant(1.0) - disruption) * self.parameters.age * 0.55)
                .clamp01()
                .value,
            disruption.value,
            contamination.clamp01().value,
        ];
        // Regional ridging and bounded impact epochs use the same relief scale.
        Ok((height, gradient, weights, work))
    }

    fn icy_regional(&self, p: DVec3) -> Result<(f64, DVec3), TerrainError> {
        let a = noise(self.salt(0x4943_595f_504c_4149), p, 3.4)?;
        let b = noise(self.salt(0x4943_595f_4241_4e44), p, 11.0)?;
        let c = noise(self.salt(0x4943_595f_5245_4749), p, 26.0)?;
        let age = self.parameters.age;
        let resurfacing = self.parameters.resurfacing_fraction;
        let old_relaxation = 0.25 + 0.75 * age;
        let band = (a * 0.66 + b * 0.34).smoothstep(-0.22, 0.24);
        let paired = (a * 2.6 + c * 0.38).sin();
        let regional = a * (0.12 + 0.12 * age)
            + (band - 0.5) * (0.30 + 0.28 * resurfacing)
            + paired * (0.14 * (1.0 - 0.55 * age))
            + c * (0.075 * old_relaxation);
        Ok((
            regional.value,
            self.rotation.transpose() * regional.gradient,
        ))
    }

    fn icy_chaos(&self, p: DVec3) -> Result<Diff, TerrainError> {
        let broad = noise(self.salt(0x4943_595f_4348_414f), p, 8.0)?;
        let fine = noise(self.salt(0x4943_595f_4348_4141), p, 17.0)?;
        let field = broad * 0.78 + fine * 0.22;
        let mask = field.smoothstep(-0.12, 0.30);
        Ok(mask
            * (0.20
                + 0.52 * self.parameters.resurfacing_fraction
                + 0.12 * self.parameters.activity))
    }

    fn icy_warp(&self, p: DVec3, frequency: f64, tag: u64) -> Result<Diff, TerrainError> {
        let a = noise(self.salt(tag ^ 1), p, frequency * 0.035)?;
        let b = noise(self.salt(tag ^ 2), p, frequency * 0.027)?;
        Ok(a * 0.42 + b * 0.28)
    }

    fn ridge_axis(&self, tag: u64) -> DVec3 {
        let a = unit_vector(self.salt(tag));
        let b = unit_vector(self.salt(tag ^ 0x9e37_79b9));
        (a + b * 0.4).normalize()
    }

    fn volcanic(
        &self,
        n: DVec3,
        halo: i64,
    ) -> Result<(f64, DVec3, [f64; 4], SurfaceQueryWork), TerrainError> {
        let p = self.rotation * n;
        let scale = self.radius_m * self.parameters.relief_fraction;
        let activity = self.parameters.activity;
        let resurfacing = self.parameters.resurfacing_fraction;
        let mut height = 0.0;
        let mut gradient = DVec3::ZERO;
        let mut work = SurfaceQueryWork {
            accepted_features: Some(0),
            ..SurfaceQueryWork::default()
        };

        let broad = noise(self.salt(0x564f_4c43_504c_4149), p, 3.0)?;
        let regional = noise(self.salt(0x564f_4c43_5245_4749), p, 15.0)?;
        let plains_mask = (broad * 0.65 + regional * 0.35).smoothstep(-0.18, 0.26);
        let plains = broad * 0.12 + (plains_mask - 0.45) * (0.23 + activity * 0.11);
        height += plains.value * scale;
        gradient += self.rotation.transpose() * plains.gradient * scale;

        // Narrow paired lobes preserve flow orientation at near-surface scale.
        let flow_axis = self.ridge_axis(0x564f_4c43_464c_4f57);
        let flow_warp = noise(self.salt(0x564f_4c43_5741_5250), p, 32.0)?;
        let flow_frequency = 680.0 + unit(self.salt(0x564f_4c43_4652_4551)) * 520.0;
        let flow_phase = Diff {
            value: p.dot(flow_axis) * flow_frequency + flow_warp.value * 2.2,
            gradient: flow_axis * flow_frequency + flow_warp.gradient * 2.2,
        };
        let flow_mask = plains_mask * (0.28 + activity * 0.32) + activity * 0.12;
        let flow_ripples = flow_phase.sin() * flow_mask * 0.012;
        height += flow_ripples.value * scale;
        gradient += self.rotation.transpose() * flow_ripples.gradient * scale;

        let budgets = [0.36, 0.24, 0.15, 0.06];
        let mut freshness = Diff::constant(0.0);
        for (epoch, budget) in budgets.into_iter().enumerate() {
            let mut residual = Diff::constant(0.0);
            let mut coverage = Diff::constant(0.0);
            let mut epoch_fresh: f64 = 0.0;
            self.visit_features(n, epoch, halo, &mut work, |center, feature_key| {
                let feature = volcanic_emplacement(
                    n * self.radius_m,
                    center,
                    self.edge_m[epoch],
                    feature_key,
                    activity,
                    self.parameters.age,
                );
                residual = residual * (Diff::constant(1.0) - feature.influence)
                    + feature.height * feature.influence;
                coverage = coverage + feature.influence * (Diff::constant(1.0) - coverage);
                epoch_fresh = epoch_fresh.max(feature.freshness * feature.influence.value);
            })?;
            // Young flow fields overprint older topography with a smooth
            // emplacement mask. Old shields persist as attenuated remnants.
            let epoch_activity = activity * (0.35 + 0.65 * resurfacing);
            let old_factor = 0.30 + 0.70 * (1.0 - self.parameters.age);
            let amount = budget * epoch_activity * old_factor;
            let overprint_m = coverage * (activity * resurfacing * 0.78);
            let overprint = Diff {
                value: overprint_m.value,
                gradient: overprint_m.gradient * self.radius_m,
            };
            let previous = Diff {
                value: height,
                gradient,
            };
            let displacement = Diff {
                value: residual.value * scale * amount,
                gradient: residual.gradient * (self.radius_m * scale * amount),
            };
            let composed = previous * (Diff::constant(1.0) - overprint) + displacement;
            height = composed.value;
            gradient = composed.gradient;
            freshness = freshness.max(Diff::constant(epoch_fresh * (1.0 - epoch as f64 * 0.23)));
        }

        // Impact craters remain visible on old substrate according to the
        // explicit retention input, without turning emplacements into craters.
        let mut impact = Diff::constant(0.0);
        for (epoch, budget) in [0.07, 0.045, 0.025, 0.015].into_iter().enumerate() {
            let mut residual = Diff::constant(0.0);
            self.visit_features(n, epoch, halo, &mut work, |center, feature_key| {
                let feature = icy_impact(
                    n * self.radius_m,
                    center,
                    self.edge_m[epoch],
                    feature_key,
                    0.9,
                );
                residual = residual * (Diff::constant(1.0) - feature.influence)
                    + feature.height * feature.influence;
            })?;
            let retained = self.parameters.impact_retention * (0.25 + 0.75 * self.parameters.age);
            height += residual.value * scale * budget * retained;
            gradient += residual.gradient * (self.radius_m * scale * budget * retained);
            impact = impact.max(Diff::constant(residual.value.abs() * retained));
        }

        let fresh_weight = (freshness * (0.65 + 0.35 * activity)).clamp01();
        let old_plain_weight = (plains_mask * (1.0 - activity * 0.58)).clamp01();
        let impact_weight = (impact * 1.7).clamp01();
        let old_substrate =
            (Diff::constant(1.0) - fresh_weight - old_plain_weight * 0.42).clamp01();
        let weights = [
            fresh_weight.value,
            old_plain_weight.value,
            old_substrate.value,
            impact_weight.value,
        ];
        Ok((height, gradient, weights, work))
    }

    fn visit_features(
        &self,
        p: DVec3,
        epoch: usize,
        halo: i64,
        work: &mut SurfaceQueryWork,
        mut visit: impl FnMut(DVec3, u64),
    ) -> Result<(), TerrainError> {
        let edge = self.edge_m[epoch];
        let local = self.rotation * (p * self.radius_m);
        let center_cell = checked_cell(local / edge)?;
        for z in -halo..=halo {
            for y in -halo..=halo {
                for x in -halo..=halo {
                    let cell = [center_cell[0] + x, center_cell[1] + y, center_cell[2] + z];
                    work.cells_visited = work.cells_visited.saturating_add(1);
                    let key = cell_key(self.seed, self.algorithm, epoch, cell);
                    let jitter = DVec3::new(
                        unit(key ^ 0x4a49_5454_4552_0001),
                        unit(key ^ 0x4a49_5454_4552_0002),
                        unit(key ^ 0x4a49_5454_4552_0003),
                    ) * (2.0 * CELL_JITTER)
                        - DVec3::splat(CELL_JITTER);
                    let raw = (DVec3::new(cell[0] as f64, cell[1] as f64, cell[2] as f64) + jitter)
                        * edge;
                    let raw_radius = raw.length();
                    if raw_radius <= 0.0
                        || (raw_radius - self.radius_m).abs() > edge * SHELL_FRACTION
                    {
                        continue;
                    }
                    work.candidate_features = work.candidate_features.saturating_add(1);
                    let surface_center = raw * (self.radius_m / raw_radius);
                    let distance = (local - surface_center).length();
                    if distance >= edge * SUPPORT_FRACTION {
                        continue;
                    }
                    work.accepted_features =
                        Some(work.accepted_features.unwrap_or(0).saturating_add(1));
                    visit(self.rotation.transpose() * surface_center, key);
                }
            }
        }
        Ok(())
    }

    fn salt(&self, tag: u64) -> u64 {
        mix(self.seed ^ algorithm_tag(self.algorithm) ^ tag)
    }
}

#[derive(Debug, Clone, Copy)]
struct Feature {
    height: Diff,
    influence: Diff,
    freshness: f64,
}

fn icy_impact(p: DVec3, center: DVec3, edge: f64, key: u64, age: f64) -> Feature {
    let delta = p - center;
    let length = delta.length();
    let radius = edge * (0.13 + 0.06 * unit(key ^ 0x4943_595f_5241_4401));
    let q = Diff {
        value: length / radius,
        gradient: if length > 0.0 {
            delta / (length * radius)
        } else {
            DVec3::ZERO
        },
    };
    let rough = 0.82 + 0.18 * unit(key ^ 0x4943_595f_524f_5547);
    let bowl = (Diff::constant(1.0) - q.smoothstep(0.50, 1.18)) * -0.62;
    // The distance norm is not differentiable at the center. Remove the
    // Gaussian ring's tiny central tail so the profile is flat there.
    let rim = ((q - 1.0) * 5.0).gaussian() * q.smoothstep(0.05, 0.15) * 0.22;
    let relaxed = 0.48 + 0.52 * age;
    let height = (bowl + rim) * (rough * relaxed);
    let influence = (Diff::constant(1.0) - q.smoothstep(1.25, 1.75)).clamp01();
    Feature {
        height,
        influence,
        freshness: 0.0,
    }
}

/// Curved, paired fracture/trough segment in the feature center's tangent
/// plane. The compact mask fades before the .65-edge cell support boundary.
#[allow(clippy::too_many_arguments)]
fn icy_fracture(
    p: DVec3,
    center: DVec3,
    edge: f64,
    key: u64,
    activity: f64,
    resurfacing: f64,
    preference_a: DVec3,
    preference_b: DVec3,
) -> Feature {
    let density = 0.40 + 0.54 * activity;
    if unit(key ^ 0x4943_595f_4445_4e53) >= density {
        return Feature {
            height: Diff::constant(0.0),
            influence: Diff::constant(0.0),
            freshness: 0.0,
        };
    }

    let frame = icy_fracture_frame(center, key, preference_a, preference_b);
    let axis = frame.axis;
    let across_axis = frame.across;
    let delta = p - center;
    let along = Diff {
        value: delta.dot(axis) / edge,
        gradient: axis / edge,
    };
    let across = Diff {
        value: delta.dot(across_axis) / edge,
        gradient: across_axis / edge,
    };
    let half_length = frame.half_length;
    let width = frame.width;
    let phase = unit(key ^ 0x4943_595f_4245_4e44) * std::f64::consts::TAU;
    let bend = (along * (std::f64::consts::PI / half_length) + phase).sin()
        * (0.035 + 0.035 * unit(key ^ 0x4943_595f_4355_5256));
    let lateral = (across - bend) * (1.0 / width);
    let core = lateral.gaussian();
    let shoulders = ((lateral - 2.1).gaussian() + (lateral + 2.1).gaussian()) * 0.22;
    let paired_profile = core * -0.62 + shoulders;

    let start = (along + half_length).smoothstep(-0.08, -0.015);
    let end = (Diff::constant(half_length) - along).smoothstep(-0.08, -0.015);
    let segment = start * end;
    let distance = delta.length();
    let normalized_distance = Diff {
        value: distance / edge,
        gradient: if distance > 0.0 {
            delta / (distance * edge)
        } else {
            DVec3::ZERO
        },
    };
    let support =
        (Diff::constant(1.0) - normalized_distance.smoothstep(0.53, SUPPORT_FRACTION)).clamp01();
    let influence = segment * support;
    let height = paired_profile * (0.6 * (0.55 + 0.45 * resurfacing));
    Feature {
        height,
        influence,
        freshness: 0.0,
    }
}

#[derive(Clone, Copy)]
struct IcyFractureFrame {
    axis: DVec3,
    across: DVec3,
    half_length: f64,
    width: f64,
}

fn icy_fracture_frame(
    center: DVec3,
    key: u64,
    preference_a: DVec3,
    preference_b: DVec3,
) -> IcyFractureFrame {
    let radial = center.normalize();
    let family = if mix(key ^ 0x4943_595f_4641_4d49) & 1 == 0 {
        preference_a
    } else {
        preference_b
    };
    let random_axis = unit_vector(key ^ 0x4943_595f_4158_4953);
    let project_axis = |axis: DVec3| {
        let projected = tangent_project(radial, axis);
        if projected.length_squared() > 1.0e-12 {
            projected.normalize()
        } else {
            let random_projected = tangent_project(radial, random_axis);
            if random_projected.length_squared() > 1.0e-12 {
                random_projected.normalize()
            } else {
                let helper = if radial.x.abs() < 0.8 {
                    DVec3::X
                } else {
                    DVec3::Y
                };
                radial.cross(helper).normalize()
            }
        }
    };
    let preferred = project_axis(family);
    let perturbed = project_axis(random_axis);
    let axis = (preferred * 0.78 + perturbed * 0.22).normalize();
    IcyFractureFrame {
        axis,
        across: radial.cross(axis).normalize(),
        half_length: 0.20 + 0.11 * unit(key ^ 0x4943_595f_4c45_4e47),
        width: 0.020 + 0.025 * unit(key ^ 0x4943_595f_5749_4454),
    }
}

fn volcanic_emplacement(
    p: DVec3,
    center: DVec3,
    edge: f64,
    key: u64,
    activity: f64,
    age: f64,
) -> Feature {
    let delta = p - center;
    let distance = delta.length();
    let frame = volcanic_frame(center, key);
    // A broad, flattened shield aligns its caldera and outer front with one
    // seeded tangent direction. Multiple axial harmonics break the ring into
    // protruding and receding lobes instead of an isolated circular island.
    let axis = frame.axis;
    let across_axis = frame.across;
    let major = frame.major;
    let minor = frame.minor;
    let along = Diff {
        value: delta.dot(axis) / (edge * major),
        gradient: axis / (edge * major),
    };
    let across = Diff {
        value: delta.dot(across_axis) / (edge * minor),
        gradient: across_axis / (edge * minor),
    };
    let radial = (along * along + across * across + 1.0e-12).sqrt();
    let lobes = volcanic_lobes_diff(key, along, across);
    let lobe_gate = radial.smoothstep(0.12, 0.30);
    let edge_warp = lobes * lobe_gate * (0.06 + 0.10 * activity);
    let footprint = radial + edge_warp;
    let shield = (Diff::constant(1.0) - footprint.smoothstep(0.58, 1.0)) * 0.62;
    let caldera_radius = 0.17 + 0.11 * unit(key ^ 31);
    let caldera = (Diff::constant(1.0) - (radial * (1.0 / caldera_radius)).smoothstep(0.72, 1.08))
        * -(0.12 + 0.13 * activity);
    let rim = ((radial - caldera_radius) * 13.0).gaussian()
        * radial.smoothstep(0.04, 0.08)
        * (0.035 + 0.045 * activity);
    let lobed_front = (footprint - 0.82).gaussian()
        * lobe_gate
        * (0.045 + 0.075 * activity)
        * (lobes * 0.35 + 0.65);
    let relaxed = 0.48 + 0.52 * (1.0 - age);
    let height = (shield + caldera + rim + lobed_front) * relaxed;
    let local_support = (Diff::constant(1.0)
        - Diff {
            value: distance / edge,
            gradient: if distance > 0.0 {
                delta / (distance * edge)
            } else {
                DVec3::ZERO
            },
        }
        .smoothstep(0.53, SUPPORT_FRACTION))
    .clamp01();
    let influence = (Diff::constant(1.0) - radial.smoothstep(1.18, 1.58)).clamp01() * local_support;
    let freshness =
        ((0.35 + 0.65 * activity) * (1.0 - 0.72 * age) * influence.value).clamp(0.0, 1.0);
    Feature {
        height,
        influence,
        freshness,
    }
}

#[derive(Clone, Copy)]
struct VolcanicFrame {
    axis: DVec3,
    across: DVec3,
    major: f64,
    minor: f64,
}

fn volcanic_frame(center: DVec3, key: u64) -> VolcanicFrame {
    let center_direction = center.normalize();
    let authored_axis = unit_vector(key ^ 0x564f_4c43_4158_4953);
    let projected_axis = tangent_project(center_direction, authored_axis);
    let fallback_axis = if center_direction.x.abs() < 0.8 {
        DVec3::X
    } else {
        DVec3::Y
    };
    let axis = if projected_axis.length_squared() > 1.0e-12 {
        projected_axis.normalize()
    } else {
        center_direction.cross(fallback_axis).normalize()
    };
    VolcanicFrame {
        axis,
        across: center_direction.cross(axis).normalize(),
        major: 0.30 + 0.12 * unit(key ^ 0x564f_4c43_5349_5a45),
        minor: 0.18 + 0.08 * unit(key ^ 0x564f_4c43_5749_4454),
    }
}

fn volcanic_footprint(key: u64, activity: f64, along: f64, across: f64) -> (f64, f64) {
    let radial = (along * along + across * across + 1.0e-12).sqrt();
    let lobes = volcanic_lobes(key, along, across);
    let lobe_gate = smoothstep_value(radial, 0.12, 0.30);
    (
        radial,
        radial + lobes * lobe_gate * (0.06 + 0.10 * activity),
    )
}

fn volcanic_lobes(key: u64, along: f64, across: f64) -> f64 {
    let phase = unit(key ^ 29) * std::f64::consts::TAU;
    (along * 7.0 + phase).sin() * 0.62
        + (along * 13.0 + phase * 0.71).sin() * 0.25
        + (across * 5.0 + phase * 1.37).sin() * 0.13
}

fn volcanic_lobes_diff(key: u64, along: Diff, across: Diff) -> Diff {
    let phase = unit(key ^ 29) * std::f64::consts::TAU;
    (along * 7.0 + phase).sin() * 0.62
        + (along * 13.0 + phase * 0.71).sin() * 0.25
        + (across * 5.0 + phase * 1.37).sin() * 0.13
}

fn smoothstep_value(value: f64, lo: f64, hi: f64) -> f64 {
    let t = ((value - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Debug, Clone, Copy)]
struct Diff {
    value: f64,
    gradient: DVec3,
}
impl Diff {
    const fn constant(value: f64) -> Self {
        Self {
            value,
            gradient: DVec3::ZERO,
        }
    }
    fn sin(self) -> Self {
        Self {
            value: self.value.sin(),
            gradient: self.gradient * self.value.cos(),
        }
    }
    fn sqrt(self) -> Self {
        let value = self.value.sqrt();
        Self {
            value,
            gradient: if value > 0.0 {
                self.gradient * (0.5 / value)
            } else {
                DVec3::ZERO
            },
        }
    }
    fn gaussian(self) -> Self {
        let value = (-self.value * self.value).exp();
        Self {
            value,
            gradient: self.gradient * (-2.0 * self.value * value),
        }
    }
    fn smoothstep(self, lo: f64, hi: f64) -> Self {
        let t = ((self.value - lo) / (hi - lo)).clamp(0.0, 1.0);
        let value = t * t * (3.0 - 2.0 * t);
        let derivative = if self.value <= lo || self.value >= hi {
            0.0
        } else {
            6.0 * t * (1.0 - t) / (hi - lo)
        };
        Self {
            value,
            gradient: self.gradient * derivative,
        }
    }
    fn clamp01(self) -> Self {
        if self.value <= 0.0 {
            Self::constant(0.0)
        } else if self.value >= 1.0 {
            Self::constant(1.0)
        } else {
            self
        }
    }
    fn max(self, other: Self) -> Self {
        if self.value >= other.value {
            self
        } else {
            other
        }
    }
}
impl std::ops::Add for Diff {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self {
            value: self.value + b.value,
            gradient: self.gradient + b.gradient,
        }
    }
}
impl std::ops::Sub for Diff {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        self + -b
    }
}
impl std::ops::Neg for Diff {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            value: -self.value,
            gradient: -self.gradient,
        }
    }
}
impl std::ops::Mul for Diff {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self {
            value: self.value * b.value,
            gradient: self.gradient * b.value + b.gradient * self.value,
        }
    }
}
impl std::ops::Mul<f64> for Diff {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self {
            value: self.value * b,
            gradient: self.gradient * b,
        }
    }
}
impl std::ops::Add<f64> for Diff {
    type Output = Self;
    fn add(self, b: f64) -> Self {
        Self {
            value: self.value + b,
            gradient: self.gradient,
        }
    }
}
impl std::ops::Sub<f64> for Diff {
    type Output = Self;
    fn sub(self, b: f64) -> Self {
        self + -b
    }
}

fn noise(seed: u64, p: DVec3, frequency: f64) -> Result<Diff, TerrainError> {
    let sample = gradient_noise(seed, p * frequency).map_err(|_| TerrainError::NonFiniteResult)?;
    Ok(Diff {
        value: sample.value,
        gradient: sample.gradient * frequency,
    })
}
fn paired_ridge(phase: Diff) -> Diff {
    // The first harmonic makes a trough between each paired positive ridge;
    // the second harmonic sharpens the two shoulders without a hard seam.
    phase.sin() + (phase * 2.0 + std::f64::consts::FRAC_PI_2).sin() * 0.42
}
fn checked_cell(p: DVec3) -> Result<[i64; 3], TerrainError> {
    if !p.is_finite() || p.abs().max_element() >= 9.0e15 {
        return Err(TerrainError::InvalidRadius);
    }
    Ok(p.floor().to_array().map(|v| v as i64))
}
fn cell_key(seed: u64, algorithm: SurfaceAlgorithm, epoch: usize, cell: [i64; 3]) -> u64 {
    let mut key =
        mix(seed ^ algorithm_tag(algorithm) ^ (epoch as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
    for (coordinate, tag) in cell.into_iter().zip([
        0xd6e8_feb8_6659_fd93,
        0xa5a3_56e4_27f8_862f,
        0x9e37_79b1_85eb_ca87,
    ]) {
        key = mix(key ^ (coordinate as u64).wrapping_mul(tag));
    }
    key
}
fn algorithm_tag(algorithm: SurfaceAlgorithm) -> u64 {
    match algorithm {
        SurfaceAlgorithm::IcyV1 => 0x4943_595f_5631_0001,
        SurfaceAlgorithm::VolcanicV1 => 0x564f_4c43_5631_0001,
        SurfaceAlgorithm::PreparedV1 | SurfaceAlgorithm::WorldV1 | SurfaceAlgorithm::RockyV3 => 0x524f_434b_5900_0003,
        SurfaceAlgorithm::RockyV4
        | SurfaceAlgorithm::RockyV5
        | SurfaceAlgorithm::IcyV2
        | SurfaceAlgorithm::IcyV3
        | SurfaceAlgorithm::VolcanicV2
        | SurfaceAlgorithm::VolcanicV3
        | SurfaceAlgorithm::MoonFieldsV1
        | SurfaceAlgorithm::MoonProfileV1 => 0,
    }
}
fn unit(key: u64) -> f64 {
    (mix(key) >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
}
fn unit_vector(key: u64) -> DVec3 {
    let component = |tag| unit(key ^ tag) * 2.0 - 1.0;
    let v = DVec3::new(component(1), component(2), component(3));
    if v.length_squared() <= f64::MIN_POSITIVE {
        DVec3::X
    } else {
        v.normalize()
    }
}
fn mix(mut value: u64) -> u64 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}
fn tangent_project(n: DVec3, value: DVec3) -> DVec3 {
    value - n * value.dot(n)
}
fn normalize_weights(weights: &mut [f64; 4]) {
    for weight in weights.iter_mut() {
        *weight = weight.clamp(0.0, 1.0);
    }
    let sum = weights.iter().sum::<f64>();
    if sum > 0.0 {
        for weight in weights.iter_mut() {
            *weight /= sum;
        }
    } else {
        weights[0] = 1.0;
    }
}
fn outward_product(a: f64, b: f64) -> f64 {
    if a == 0.0 {
        0.0
    } else {
        (a * b * (1.0 + 64.0 * f64::EPSILON)).next_up()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn params() -> GeologicalParameters {
        GeologicalParameters {
            age: 0.62,
            activity: 0.72,
            resurfacing_fraction: 0.35,
            impact_retention: 0.58,
            relief_fraction: 0.008,
            feature_scale_fraction: 0.08,
            orientation_radians: 0.37,
        }
    }
    fn field(algorithm: SurfaceAlgorithm) -> GeologyField {
        GeologyField::new(algorithm, params(), 0x5eed, 109_000.0).unwrap()
    }

    #[test]
    fn complete_fields_are_finite_bounded_and_deterministic() {
        for algorithm in [SurfaceAlgorithm::IcyV1, SurfaceAlgorithm::VolcanicV1] {
            let f = field(algorithm);
            for index in 0..96 {
                let z = 1.0 - 2.0 * (index as f64 + 0.5) / 96.0;
                let angle = index as f64 * 2.399_963_229_728_653;
                let r = (1.0 - z * z).sqrt();
                let n = DVec3::new(r * angle.cos(), r * angle.sin(), z);
                let a = f.evaluate(n).unwrap();
                let b = f.evaluate(n).unwrap();
                assert_eq!(a.height_m.to_bits(), b.height_m.to_bits());
                assert_eq!(a.gradient_m, b.gradient_m);
                assert!(a.height_m.abs() <= f.absolute_height_bound_m());
                assert!(a.gradient_m.is_finite());
                assert!((a.weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn icy_fracture_frame_handles_parallel_preferred_and_random_axes() {
        let key = 0x5eed;
        let radial = unit_vector(key ^ 0x4943_595f_4158_4953);
        let frame = icy_fracture_frame(radial * 10_000.0, key, radial, radial);
        assert!(frame.axis.is_finite() && frame.across.is_finite());
        assert!((frame.axis.length() - 1.0).abs() < 1.0e-12);
        assert!(frame.axis.dot(radial).abs() < 1.0e-12);
    }

    #[test]
    fn landmark_probes_require_nonzero_feature_displacement() {
        for algorithm in [SurfaceAlgorithm::IcyV1, SurfaceAlgorithm::VolcanicV1] {
            let mut parameters = params();
            parameters.relief_fraction = 0.0;
            let field = GeologyField::new(algorithm, parameters, 0x5eed, 109_000.0).unwrap();
            assert!(
                field
                    .diagnostic_landmark_probes(DVec3::Z)
                    .unwrap()
                    .is_empty()
            );
        }
        let mut parameters = params();
        parameters.activity = 0.0;
        let field =
            GeologyField::new(SurfaceAlgorithm::VolcanicV1, parameters, 0x5eed, 109_000.0).unwrap();
        assert!(
            field
                .diagnostic_landmark_probes(DVec3::Z)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn amplified_icy_fractures_stay_within_field_bounds_at_multiple_radii() {
        let mut parameters = params();
        parameters.relief_fraction = 0.01;
        parameters.feature_scale_fraction = 0.8;
        parameters.activity = 1.0;
        parameters.resurfacing_fraction = 1.0;
        for radius_m in [20_000.0, 109_000.0, 1_737_000.0] {
            let f =
                GeologyField::new(SurfaceAlgorithm::IcyV1, parameters, 0x5eed, radius_m).unwrap();
            let bound_fraction = 0.838 + 0.064 + 0.004 + 0.636 * 0.30 + 0.302;
            assert!(bound_fraction < 1.41);
            assert!(
                f.absolute_height_bound_m() <= (outward_product(radius_m, 0.01) * 1.74).next_up()
            );
            for n in [
                DVec3::Z,
                DVec3::X,
                DVec3::new(0.37, -0.51, 0.77).normalize(),
            ] {
                let sample = f.evaluate(n).unwrap();
                assert!(sample.height_m.abs() <= f.absolute_height_bound_m());
                assert!(sample.gradient_m.is_finite());
            }
        }
    }

    #[test]
    fn fine_landmark_probes_are_deterministic_active_and_differentiable() {
        for algorithm in [SurfaceAlgorithm::IcyV1, SurfaceAlgorithm::VolcanicV1] {
            let f = field(algorithm);
            let mut probes = Vec::new();
            let mut probe_near = DVec3::Z;
            for near in [
                DVec3::Z,
                DVec3::X,
                DVec3::Y,
                -DVec3::Z,
                -DVec3::X,
                -DVec3::Y,
            ] {
                probes = f.diagnostic_landmark_probes(near).unwrap();
                if !probes.is_empty() {
                    probe_near = near;
                    break;
                }
            }
            assert!(
                !probes.is_empty(),
                "no local fine landmarks for {algorithm:?}"
            );
            let repeat = f.diagnostic_landmark_probes(probe_near).unwrap();
            assert_eq!(probes, repeat);
            for (label, direction) in probes {
                assert!(match algorithm {
                    SurfaceAlgorithm::IcyV1 => label.starts_with("icy-fracture-epoch3-"),
                    SurfaceAlgorithm::VolcanicV1 => label.starts_with("volcanic-epoch2-"),
                    SurfaceAlgorithm::PreparedV1
                    | SurfaceAlgorithm::WorldV1
                    | SurfaceAlgorithm::RockyV3
                    | SurfaceAlgorithm::RockyV4
                    | SurfaceAlgorithm::RockyV5
                    | SurfaceAlgorithm::IcyV2
                    | SurfaceAlgorithm::IcyV3
                    | SurfaceAlgorithm::VolcanicV2
                    | SurfaceAlgorithm::VolcanicV3
                    | SurfaceAlgorithm::MoonFieldsV1
                    | SurfaceAlgorithm::MoonProfileV1 => false,
                });
                assert!((direction.length() - 1.0).abs() < 1.0e-12);
                let sample = f.evaluate(direction).unwrap();
                assert!(sample.gradient_m.is_finite());
                let projected = tangent_project(direction, sample.gradient_m);
                let tangent = if projected.length_squared() > 1.0e-18 {
                    projected.normalize()
                } else {
                    tangent_project(direction, DVec3::X).normalize()
                };
                let epsilon = 1.0e-9;
                let plus = f
                    .evaluate((direction + tangent * epsilon).normalize())
                    .unwrap()
                    .height_m;
                let minus = f
                    .evaluate((direction - tangent * epsilon).normalize())
                    .unwrap()
                    .height_m;
                let finite_difference = (plus - minus) / (2.0 * epsilon);
                let analytic = sample.gradient_m.dot(tangent);
                assert!(
                    (finite_difference - analytic).abs() < 0.02 + analytic.abs() * 0.02,
                    "{label}: finite difference {finite_difference}, analytic {analytic}"
                );
                let expanded = f.evaluate_with_halo(direction, 3).unwrap();
                assert_eq!(sample.height_m.to_bits(), expanded.height_m.to_bits());
                assert_eq!(sample.gradient_m, expanded.gradient_m);
            }
        }
    }

    #[test]
    fn analytic_tangent_gradient_matches_directional_difference() {
        for algorithm in [SurfaceAlgorithm::IcyV1, SurfaceAlgorithm::VolcanicV1] {
            let f = field(algorithm);
            for n in [
                DVec3::X,
                DVec3::Y,
                DVec3::new(0.31, -0.72, 0.55).normalize(),
            ] {
                let tangent = tangent_project(n, DVec3::new(0.21, 0.93, -0.17)).normalize();
                let epsilon = 1.0e-9;
                let center = f.evaluate(n).unwrap();
                let plus = f
                    .evaluate((n + tangent * epsilon).normalize())
                    .unwrap()
                    .height_m;
                let minus = f
                    .evaluate((n - tangent * epsilon).normalize())
                    .unwrap()
                    .height_m;
                let finite_difference = (plus - minus) / (2.0 * epsilon);
                let analytic = center.gradient_m.dot(tangent);
                assert!(
                    (finite_difference - analytic).abs() < 0.005 + analytic.abs() * 0.005,
                    "{algorithm:?}: finite difference {finite_difference}, analytic {analytic}"
                );
            }
        }
    }

    #[test]
    fn one_cell_halo_matches_expanded_oracle_at_shell_and_grid_boundaries() {
        // The construction's reach proof is: outside the 3x3x3 halo the raw
        // center is at least .8 edge away; shell projection removes at most
        // .12 edge, leaving .68 edge > .65 edge support.
        for algorithm in [SurfaceAlgorithm::IcyV1, SurfaceAlgorithm::VolcanicV1] {
            let f = field(algorithm);
            let mut directions = vec![DVec3::X, DVec3::Y, DVec3::new(0.3, -0.4, 0.7).normalize()];
            for edge in f.edge_m {
                let x = edge * (f.radius_m * 0.4 / edge).floor().max(1.0);
                let local =
                    DVec3::new(x, (f.radius_m * f.radius_m - x * x).sqrt(), 0.0) / f.radius_m;
                directions.push(f.rotation.transpose() * local);
            }
            for n in directions {
                let sample = f.evaluate(n).unwrap();
                let expanded = f.evaluate_with_halo(n, 3).unwrap();
                assert_eq!(sample.height_m.to_bits(), expanded.height_m.to_bits());
                assert_eq!(sample.gradient_m, expanded.gradient_m);
                assert_eq!(sample.weights, expanded.weights);
                assert!(sample.work.cells_visited <= 216);
                assert!(
                    sample.work.accepted_features.unwrap_or(0) <= sample.work.candidate_features
                );
            }
        }
    }

    #[test]
    fn compact_feature_cores_and_support_edges_have_independent_derivatives() {
        let key = 0x519f;
        let edge = 700.0;
        let center = DVec3::new(10_000.0, -4_000.0, 20_000.0);
        // One-sided differences expose an even radial cusp that a centered
        // difference would cancel at the feature center.
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            let icy_center = icy_impact(center, center, edge, key, 0.7);
            let volcanic_center = volcanic_emplacement(center, center, edge, key, 0.8, 0.5);
            for offset in [-1.0e-4, 1.0e-4] {
                let icy = icy_impact(center + axis * offset, center, edge, key, 0.7);
                let volcanic =
                    volcanic_emplacement(center + axis * offset, center, edge, key, 0.8, 0.5);
                assert_eq!(icy.height.value, icy_center.height.value);
                assert_eq!(volcanic.height.value, volcanic_center.height.value);
                assert_eq!(icy.height.gradient, DVec3::ZERO);
                assert_eq!(volcanic.height.gradient, DVec3::ZERO);
            }
        }
        for radius_fraction in [
            0.0, 0.12, 0.5, 0.99999, 1.00001, 1.24999, 1.25001, 1.74999, 1.75001,
        ] {
            let crater_radius = edge * (0.13 + 0.06 * unit(key ^ 0x4943_595f_5241_4401));
            let position = center + DVec3::X * (crater_radius * radius_fraction);
            let sample = icy_impact(position, center, edge, key, 0.7);
            let epsilon = 1.0e-4;
            let plus = icy_impact(position + DVec3::X * epsilon, center, edge, key, 0.7);
            let minus = icy_impact(position - DVec3::X * epsilon, center, edge, key, 0.7);
            let height_fd = (plus.height.value - minus.height.value) / (2.0 * epsilon);
            let mask_fd = (plus.influence.value - minus.influence.value) / (2.0 * epsilon);
            assert!(
                (height_fd - sample.height.gradient.x).abs() < 2.0e-6,
                "icy height at radius {radius_fraction}: fd={height_fd}, analytic={}",
                sample.height.gradient.x
            );
            assert!(
                (mask_fd - sample.influence.gradient.x).abs() < 2.0e-7,
                "icy mask at radius {radius_fraction}: fd={mask_fd}, analytic={}",
                sample.influence.gradient.x
            );
        }
        for radius_fraction in [0.0, 0.18, 0.76, 1.17999, 1.18001, 1.57999, 1.58001] {
            let position = center + DVec3::X * (edge * 0.3 * radius_fraction);
            let sample = volcanic_emplacement(position, center, edge, key, 0.8, 0.5);
            let epsilon = 1.0e-4;
            let plus =
                volcanic_emplacement(position + DVec3::X * epsilon, center, edge, key, 0.8, 0.5);
            let minus =
                volcanic_emplacement(position - DVec3::X * epsilon, center, edge, key, 0.8, 0.5);
            let height_fd = (plus.height.value - minus.height.value) / (2.0 * epsilon);
            let mask_fd = (plus.influence.value - minus.influence.value) / (2.0 * epsilon);
            assert!(
                (height_fd - sample.height.gradient.x).abs() < 2.0e-6,
                "volcanic height at radius {radius_fraction}: fd={height_fd}, analytic={}",
                sample.height.gradient.x
            );
            assert!(
                (mask_fd - sample.influence.gradient.x).abs() < 2.0e-7,
                "volcanic mask at radius {radius_fraction}: fd={mask_fd}, analytic={}",
                sample.influence.gradient.x
            );
        }
    }

    #[test]
    fn volcanic_profile_gradients_cover_physical_support_fade() {
        let edge = 700.0;
        let center = DVec3::new(10_000.0, -4_000.0, 20_000.0);
        let key = (0..10_000_u64)
            .find(|key| 0.30 + 0.12 * unit(*key ^ 0x564f_4c43_5349_5a45) > 0.412)
            .expect("fixed fixture includes a broad major axis");
        let center_direction = center.normalize();
        let raw_axis = unit_vector(key ^ 0x564f_4c43_4158_4953);
        let projected_axis = tangent_project(center_direction, raw_axis);
        let axis = if projected_axis.length_squared() > 1.0e-12 {
            projected_axis.normalize()
        } else {
            center_direction.cross(DVec3::Y).normalize()
        };
        let epsilon = edge * 1.0e-6;
        for boundary in [0.53, SUPPORT_FRACTION] {
            for offset in [-4.0 * epsilon, -epsilon, 0.0, epsilon, 4.0 * epsilon] {
                let position = center + axis * (edge * boundary + offset);
                let sample = volcanic_emplacement(position, center, edge, key, 0.8, 0.5);
                let plus =
                    volcanic_emplacement(position + axis * epsilon, center, edge, key, 0.8, 0.5);
                let minus =
                    volcanic_emplacement(position - axis * epsilon, center, edge, key, 0.8, 0.5);
                let composed = sample.height * sample.influence;
                let plus_composed = plus.height * plus.influence;
                let minus_composed = minus.height * minus.influence;
                for (name, finite_difference, analytic) in [
                    (
                        "height",
                        (plus.height.value - minus.height.value) / (2.0 * epsilon),
                        sample.height.gradient.dot(axis),
                    ),
                    (
                        "influence",
                        (plus.influence.value - minus.influence.value) / (2.0 * epsilon),
                        sample.influence.gradient.dot(axis),
                    ),
                    (
                        "composed",
                        (plus_composed.value - minus_composed.value) / (2.0 * epsilon),
                        composed.gradient.dot(axis),
                    ),
                ] {
                    assert!(
                        (finite_difference - analytic).abs() < 1.0e-8 + analytic.abs() * 0.003,
                        "{name} at support boundary {boundary} offset {offset}: fd={finite_difference}, analytic={analytic}"
                    );
                }
            }
        }
    }

    #[test]
    fn volcanic_diagnostic_support_probes_match_expanded_halo_oracle() {
        let f = field(SurfaceAlgorithm::VolcanicV1);
        let probes = f.diagnostic_boundary_probes(DVec3::Z).unwrap();
        let flow_probe_count = probes
            .iter()
            .filter(|(label, _)| label.starts_with("volcanic-flow-"))
            .count();
        assert!(
            flow_probe_count >= 2,
            "no active emplacement support probes: {probes:?}"
        );
        for (_, direction) in probes {
            let ordinary = f.evaluate_with_halo(direction, 1).unwrap();
            let expanded = f.evaluate_with_halo(direction, 3).unwrap();
            assert_eq!(ordinary.height_m.to_bits(), expanded.height_m.to_bits());
            assert_eq!(ordinary.gradient_m, expanded.gradient_m);
            assert_eq!(ordinary.weights, expanded.weights);
        }
    }

    #[test]
    fn curved_fractures_terminate_smoothly_and_compose_at_crossings() {
        let edge = 700.0;
        let center = DVec3::new(10_000.0, -4_000.0, 20_000.0);
        let key = (0..10_000_u64)
            .find(|key| unit(*key ^ 0x4943_595f_4445_4e53) < 0.93)
            .expect("fixed fracture fixture has an active cell");
        let field = |position, origin, key| {
            let feature = icy_fracture(position, origin, edge, key, 1.0, 0.9, DVec3::X, DVec3::Y);
            feature.height * feature.influence
        };
        let epsilon = 1.0e-3;
        for position in [
            center,
            center + DVec3::X * edge * 0.18,
            center + DVec3::Y * edge * 0.55,
            center + DVec3::Y * edge * 0.6499,
            center + DVec3::Y * edge * 0.6501,
        ] {
            let sample = field(position, center, key);
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                let plus = field(position + axis * epsilon, center, key);
                let minus = field(position - axis * epsilon, center, key);
                let finite_difference = (plus.value - minus.value) / (2.0 * epsilon);
                assert!(
                    (finite_difference - sample.gradient.dot(axis)).abs() < 1.0e-7,
                    "fracture gradient at {position:?} along {axis:?}: fd={finite_difference}, analytic={:?}",
                    sample.gradient
                );
            }
        }

        let second_center = center + DVec3::X * (edge * 0.22);
        let second_key = key ^ 0x8f23_57a1;
        let compose = |position: DVec3| {
            let older = field(position, center, key);
            let younger = field(position, second_center, second_key);
            older * (Diff::constant(1.0) - younger.clamp01()) + younger
        };
        let position = center + DVec3::X * edge * 0.12 + DVec3::Y * edge * 0.04;
        let sample = compose(position);
        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
            let plus = compose(position + axis * epsilon);
            let minus = compose(position - axis * epsilon);
            let finite_difference = (plus.value - minus.value) / (2.0 * epsilon);
            assert!((finite_difference - sample.gradient.dot(axis)).abs() < 1.0e-7);
        }
    }
}
