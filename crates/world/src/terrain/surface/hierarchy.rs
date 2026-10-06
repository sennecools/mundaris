//! Hierarchical geological residuals layered over the preserved province field.
//!
//! The inherited field remains the coarse authority. Three deterministic cell
//! bands add physical-regime morphology at about 256 m, 32 m and 8 m. Each
//! band's compact features carry process controls sampled once at their fixed
//! centres, so burial, age, activity and province history affect relief without
//! making a feature move as the query point changes.
use super::{
    GeologicalControls, GeologicalParameters, SurfaceAlgorithm, SurfaceQueryWork, TerrainError,
    director::DirectedSample,
    mix,
    provinces::{ProvinceField, ProvinceSample},
    unit,
};
use glam::{DMat3, DVec3};
use mundaris_math::noise::gradient_noise;
use std::ops::{Add, Div, Mul, Neg, Sub};

const SUPPORT: f64 = 0.52;
const JITTER: f64 = 0.16;
const SHELL: f64 = 0.30;
const LAYOUTS: usize = 2;
const BAND_EDGES_M: [f64; 3] = [256.0, 32.0, 8.0];
const BAND_BUDGETS_M: [f64; 3] = [30.72, 3.84, 0.96];
const BAND_RELIEF_FRACTIONS: [f64; 3] = [0.16, 0.085, 0.05];
const _: () = assert!(1.0 - JITTER - SHELL > SUPPORT);

#[derive(Debug, Clone, Copy)]
struct Differential {
    value: f64,
    gradient: DVec3,
}

impl Differential {
    fn constant(value: f64) -> Self {
        Self {
            value,
            gradient: DVec3::ZERO,
        }
    }

    fn new(value: f64, gradient: DVec3) -> Self {
        Self { value, gradient }
    }

    fn square(self) -> Self {
        self * self
    }

    fn cube(self) -> Self {
        self * self * self
    }

    fn exp(self) -> Self {
        let value = self.value.exp();
        Self::new(value, self.gradient * value)
    }

    fn gaussian(self, center: f64, width: f64) -> Self {
        (-((self - center) / width).square()).exp()
    }

    fn smoothstep(self, low: f64, high: f64) -> Self {
        if self.value <= low {
            Self::constant(0.0)
        } else if self.value >= high {
            Self::constant(1.0)
        } else {
            let t = (self - low) / (high - low);
            t.square() * (Differential::constant(3.0) - t * 2.0)
        }
    }

    fn sin(self) -> Self {
        Self::new(self.value.sin(), self.gradient * self.value.cos())
    }

    fn tanh(self) -> Self {
        let value = self.value.tanh();
        Self::new(value, self.gradient * (1.0 - value * value))
    }
}

impl Add for Differential {
    type Output = Self;
    fn add(self, rhs: Self) -> Self {
        Self::new(self.value + rhs.value, self.gradient + rhs.gradient)
    }
}
impl Sub for Differential {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.value - rhs.value, self.gradient - rhs.gradient)
    }
}
impl Mul for Differential {
    type Output = Self;
    fn mul(self, rhs: Self) -> Self {
        Self::new(
            self.value * rhs.value,
            self.gradient * rhs.value + rhs.gradient * self.value,
        )
    }
}
impl Div for Differential {
    type Output = Self;
    fn div(self, rhs: Self) -> Self {
        Self::new(
            self.value / rhs.value,
            (self.gradient * rhs.value - rhs.gradient * self.value) / rhs.value.powi(2),
        )
    }
}
impl Neg for Differential {
    type Output = Self;
    fn neg(self) -> Self {
        Self::new(-self.value, -self.gradient)
    }
}
impl Add<f64> for Differential {
    type Output = Self;
    fn add(self, rhs: f64) -> Self {
        self + Self::constant(rhs)
    }
}
impl Sub<f64> for Differential {
    type Output = Self;
    fn sub(self, rhs: f64) -> Self {
        self - Self::constant(rhs)
    }
}
impl Mul<f64> for Differential {
    type Output = Self;
    fn mul(self, rhs: f64) -> Self {
        Self::new(self.value * rhs, self.gradient * rhs)
    }
}
impl Div<f64> for Differential {
    type Output = Self;
    fn div(self, rhs: f64) -> Self {
        self * (1.0 / rhs)
    }
}

#[derive(Debug, Clone, Copy)]
struct CellFeature {
    center: DVec3,
    key: u64,
    lineage_key: u64,
}

/// Per-band work and hierarchical height decomposition from one authoritative
/// evaluation. Gradients are tangent derivatives in metres per unit direction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceDetailDiagnostics {
    pub inherited_height_m: f64,
    pub regional_height_m: f64,
    pub local_height_m: f64,
    pub fine_height_m: f64,
    pub inherited_gradient_m: DVec3,
    pub regional_gradient_m: DVec3,
    pub local_gradient_m: DVec3,
    pub fine_gradient_m: DVec3,
    pub parent_morphology: f64,
    pub parent_morphology_gradient_per_unit_direction: DVec3,
    pub band_work: [SurfaceQueryWork; 3],
    pub parent_process_strengths: [f64; 4],
}

#[derive(Debug, Clone, Copy)]
struct Evaluation {
    sample: ProvinceSample,
    diagnostics: SurfaceDetailDiagnostics,
}

#[derive(Debug, Clone)]
pub(super) struct HierarchicalField {
    algorithm: SurfaceAlgorithm,
    radius_m: f64,
    seed: u64,
    relief_fraction: f64,
    parent: ProvinceField,
    network_rotations: [DMat3; 3],
    bound_m: f64,
}

impl HierarchicalField {
    pub(super) fn new(
        algorithm: SurfaceAlgorithm,
        parameters: GeologicalParameters,
        seed: u64,
        radius_m: f64,
    ) -> Result<Self, TerrainError> {
        let parent_algorithm = match algorithm {
            SurfaceAlgorithm::RockyV5 => SurfaceAlgorithm::RockyV4,
            SurfaceAlgorithm::IcyV3 => SurfaceAlgorithm::IcyV2,
            SurfaceAlgorithm::VolcanicV3 => SurfaceAlgorithm::VolcanicV2,
            _ => return Err(TerrainError::InvalidConfig),
        };
        let parent = ProvinceField::new(parent_algorithm, parameters, seed, radius_m)?;
        let network_rotations = std::array::from_fn(|band| {
            let phase = unit(
                mix(seed ^ algorithm.code() ^ 0x4849_4552_4152_0001)
                    ^ (band as u64).wrapping_mul(0x9e37_79b9),
            );
            DMat3::from_rotation_z(phase * std::f64::consts::TAU)
                * DMat3::from_rotation_y(
                    unit(
                        mix(seed ^ algorithm.code() ^ 0x4849_4552_4152_0001)
                            ^ 0x4e45_5457
                            ^ band as u64,
                    ) * std::f64::consts::TAU,
                )
        });
        // Each band divides by 1 + sum(compact windows), and its profile is
        // algebraically bounded by one. Thus overlaps never exceed this sum.
        let residual_bound = BAND_EDGES_M
            .iter()
            .zip(BAND_BUDGETS_M)
            .zip(BAND_RELIEF_FRACTIONS)
            .map(|((_, budget), relief)| {
                budget.min(radius_m * parameters.relief_fraction * 1.2 * relief)
            })
            .sum::<f64>();
        let bound_m = (parent.absolute_height_bound_m() + residual_bound).next_up();
        if !bound_m.is_finite() || bound_m >= radius_m * 0.1 {
            return Err(TerrainError::InvalidRadius);
        }
        Ok(Self {
            algorithm,
            radius_m,
            seed: mix(seed ^ algorithm.code() ^ 0x4849_4552_4152_0001),
            relief_fraction: parameters.relief_fraction,
            parent,
            network_rotations,
            bound_m,
        })
    }

    pub(super) fn absolute_height_bound_m(&self) -> f64 {
        self.bound_m
    }

    /// Heap payload retained by the outer field box and the optional boxed
    /// Rocky V4 history in its parent. All other hierarchy state is inline;
    /// allocator bookkeeping is outside this byte count.
    pub(super) fn resident_heap_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.parent.resident_heap_bytes()
    }

    pub(super) fn controls(&self, n: DVec3) -> Result<GeologicalControls, TerrainError> {
        self.parent.controls(n)
    }

    pub(super) fn evaluate(&self, direction: DVec3) -> Result<ProvinceSample, TerrainError> {
        Ok(self.evaluate_components(direction)?.sample)
    }

    pub(super) fn diagnostics(
        &self,
        direction: DVec3,
    ) -> Result<SurfaceDetailDiagnostics, TerrainError> {
        Ok(self.evaluate_components(direction)?.diagnostics)
    }

    fn evaluate_components(&self, direction: DVec3) -> Result<Evaluation, TerrainError> {
        if !direction.is_finite() || direction.length_squared() <= f64::MIN_POSITIVE {
            return Err(TerrainError::InvalidConfig);
        }
        let n = direction.normalize();
        let (inherited, parent_context) = self.parent.evaluate_with_context(direction)?;
        let directed = parent_context.directed;
        let parent_controls = directed.controls;
        let point = n * self.radius_m;
        let parent_morphology =
            Differential::new(parent_context.morphology, parent_context.gradient);
        let mut contributions = [Differential::constant(0.0); 3];
        let mut band_work = [SurfaceQueryWork::default(); 3];
        contributions[0] =
            self.evaluate_band(point, 0, parent_morphology, &directed, &mut band_work[0])?;
        let regional_context =
            parent_morphology + normalize_contribution(n, contributions[0], self.band_budget(0));
        contributions[1] =
            self.evaluate_band(point, 1, regional_context, &directed, &mut band_work[1])?;
        let local_context =
            regional_context + normalize_contribution(n, contributions[1], self.band_budget(1));
        contributions[2] =
            self.evaluate_band(point, 2, local_context, &directed, &mut band_work[2])?;
        let total = Differential::new(inherited.height_m, inherited.gradient_m)
            + contributions[0]
            + contributions[1]
            + contributions[2];
        let gradient = total.gradient - n * n.dot(total.gradient);
        let component_gradients =
            contributions.map(|component| component.gradient - n * n.dot(component.gradient));
        if !total.value.is_finite() || !gradient.is_finite() || total.value.abs() > self.bound_m {
            return Err(TerrainError::NonFiniteResult);
        }
        let mut work = inherited.work;
        for band in band_work {
            work.cells_visited = work.cells_visited.saturating_add(band.cells_visited);
            work.candidate_features = work
                .candidate_features
                .saturating_add(band.candidate_features);
            work.accepted_features = match (work.accepted_features, band.accepted_features) {
                (Some(total), Some(additional)) => Some(total.saturating_add(additional)),
                _ => None,
            };
        }
        Ok(Evaluation {
            sample: ProvinceSample {
                height_m: total.value,
                gradient_m: gradient,
                weights: inherited.weights,
                work,
            },
            diagnostics: SurfaceDetailDiagnostics {
                inherited_height_m: inherited.height_m,
                regional_height_m: contributions[0].value,
                local_height_m: contributions[1].value,
                fine_height_m: contributions[2].value,
                inherited_gradient_m: inherited.gradient_m,
                regional_gradient_m: component_gradients[0],
                local_gradient_m: component_gradients[1],
                fine_gradient_m: component_gradients[2],
                parent_morphology: parent_context.morphology,
                parent_morphology_gradient_per_unit_direction: parent_context.gradient,
                band_work,
                parent_process_strengths: parent_controls.process_strengths,
            },
        })
    }

    fn evaluate_band(
        &self,
        point: DVec3,
        band: usize,
        parent_context: Differential,
        directed: &DirectedSample,
        work: &mut SurfaceQueryWork,
    ) -> Result<Differential, TerrainError> {
        let edge = BAND_EDGES_M[band].max(8.0);
        let support_radius = edge * SUPPORT;
        *work = SurfaceQueryWork {
            accepted_features: Some(0),
            ..SurfaceQueryWork::default()
        };
        let mut denominator = Differential::constant(1.0);
        let mut numerator = Differential::constant(0.0);
        for layout in 0..LAYOUTS {
            let shift = if layout == 0 {
                DVec3::ZERO
            } else {
                DVec3::new(0.31, -0.27, 0.19)
            };
            let base = (point / edge - shift).floor().to_array().map(|v| v as i64);
            for x in base[0] - 1..=base[0] + 1 {
                for y in base[1] - 1..=base[1] + 1 {
                    for z in base[2] - 1..=base[2] + 1 {
                        work.cells_visited += 1;
                        let Some(feature) = self.feature(x, y, z, band, layout, edge) else {
                            continue;
                        };
                        work.candidate_features += 1;
                        let delta = point - feature.center;
                        let q2 = Differential::new(
                            delta.length_squared() / support_radius.powi(2),
                            delta * (2.0 * self.radius_m / support_radius.powi(2)),
                        );
                        if q2.value >= 1.0 {
                            continue;
                        }
                        let accepted = work.accepted_features.get_or_insert(0);
                        *accepted = accepted.saturating_add(1);
                        let window = (Differential::constant(1.0) - q2).cube();
                        let controls = self.parent.controls(feature.center.normalize())?;
                        let profile =
                            self.profile(point, feature, edge, q2, controls, parent_context)?;
                        let amplitude = self.band_budget(band);
                        numerator = numerator + window * profile * amplitude;
                        denominator = denominator + window;
                    }
                }
            }
        }
        let feature_residual = numerator / denominator;
        let network =
            self.continuous_process(point / self.radius_m, band, parent_context, directed)?
                * self.band_budget(band);
        // The cell landmarks retain identifiable forms while a bounded,
        // family-specific geological network fills the gaps between supports.
        Ok((feature_residual + network) * 0.5)
    }

    fn band_budget(&self, band: usize) -> f64 {
        BAND_BUDGETS_M[band]
            .min(self.radius_m * self.relief_fraction * 1.2 * BAND_RELIEF_FRACTIONS[band])
    }

    fn continuous_process(
        &self,
        n: DVec3,
        band: usize,
        parent_context: Differential,
        directed: &DirectedSample,
    ) -> Result<Differential, TerrainError> {
        // The 8 m support is a parent structure, while each family's fine
        // process has a sub-support physical scale: fractured breccia joints,
        // intersecting ice faults, and narrow volcanic flow fronts.
        let wavelength = match (self.algorithm, band) {
            (SurfaceAlgorithm::RockyV5, 2) => 2.0,
            (SurfaceAlgorithm::IcyV3, 2) => 2.0,
            (SurfaceAlgorithm::VolcanicV3, 2) => 2.0,
            _ => BAND_EDGES_M[band],
        };
        let frequency = self.radius_m / wavelength;
        let rotation = self.network_rotations[band];
        let sample = gradient_noise(self.seed ^ 0x5052_4f43_4553_0001, rotation * n * frequency)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        let field = Differential::new(
            sample.value,
            rotation.transpose() * sample.gradient * frequency,
        );
        let controls = directed.controls;
        let province = std::array::from_fn::<_, 4, _>(|index| {
            Differential::new(
                controls.province_weights[index],
                directed.province_gradients[index],
            )
        });
        let activity = Differential::new(controls.activity, directed.control_gradients[1]);
        let resurfacing = Differential::new(controls.resurfacing, directed.control_gradients[2]);
        let retention = Differential::new(controls.impact_retention, directed.control_gradients[3]);
        let lineage_mask = parent_context.tanh();
        let process_strengths = match self.algorithm {
            SurfaceAlgorithm::RockyV5 => [
                retention,
                Differential::constant(0.0),
                Differential::constant(0.0),
                province[3] * 0.88 + 0.12,
            ],
            SurfaceAlgorithm::IcyV3 => [
                Differential::constant(0.0),
                (activity * 0.7 + province[1] * 0.65 + 0.18) / 1.53,
                Differential::constant(0.0),
                Differential::constant(0.0),
            ],
            SurfaceAlgorithm::VolcanicV3 => [
                Differential::constant(0.0),
                Differential::constant(0.0),
                (province[2] * 0.65 + resurfacing * 0.70 + 0.12) / 1.47,
                Differential::constant(0.0),
            ],
            _ => return Err(TerrainError::InvalidConfig),
        };
        let network = match self.algorithm {
            SurfaceAlgorithm::RockyV5 => {
                // Bedrock knobs and fractured ejecta exposures are concentrated
                // around signed contour bands in old retained highlands.
                let inherited_field = field + parent_context * 0.16;
                if band == 2 {
                    // Fine, connected joint troughs with unequal breccia lips;
                    // high impact retention preserves blocks while resurfacing
                    // buries them. The 2 m wavelength lives inside the 8 m cell.
                    let joints = -inherited_field.gaussian(0.0, 0.10)
                        + inherited_field.gaussian(-0.21, 0.075) * 0.42
                        + inherited_field.gaussian(0.24, 0.095) * 0.28;
                    joints
                        * (process_strengths[0] * 0.72 + process_strengths[3] * 0.28)
                        * (Differential::constant(1.0) - resurfacing * 0.82)
                        * (lineage_mask * 0.24 + 0.76)
                } else {
                    (inherited_field.gaussian(0.0, 0.17) - 0.32)
                        * (process_strengths[0] * 0.52 + process_strengths[3] * 0.48)
                        * (Differential::constant(1.0) - resurfacing * 0.82)
                        * (lineage_mask * 0.24 + 0.76)
                }
            }
            SurfaceAlgorithm::IcyV3 => {
                // Unequal fault troughs and pressure shoulders form a tangled
                // stress network; this is one process field, not an octave stack.
                let inherited_field = field + parent_context * 0.22;
                let (fault, process_gain, activity_gain, lineage_gain) = if band == 2 {
                    // Narrow intersecting cryofaults and unequal compression lips.
                    // For |field| <= 1, the trough is at least -1 and the two
                    // shoulders sum to at most 0.82, so |fault| <= 1. The
                    // normalized tanh gain below remains bounded by one while
                    // increasing contrast in the low-amplitude contour flanks.
                    let profile = -inherited_field.gaussian(0.0, 0.075)
                        + inherited_field.gaussian(-0.19, 0.060) * 0.48
                        + inherited_field.gaussian(0.21, 0.070) * 0.34;
                    ((profile * 1.65).tanh() / 1.65_f64.tanh(), 0.56, 0.44, 0.22)
                } else {
                    let profile = -inherited_field.gaussian(0.0, 0.10)
                        + inherited_field.gaussian(-0.20, 0.075) * 0.31
                        + inherited_field.gaussian(0.22, 0.095) * 0.23;
                    (profile, 0.58, 0.22, 0.22)
                };
                fault
                    * (process_strengths[1] * process_gain + activity_gain)
                    * (Differential::constant(1.0) - resurfacing * 0.72)
                    * (lineage_mask * lineage_gain + (1.0 - lineage_gain))
            }
            SurfaceAlgorithm::VolcanicV3 => {
                // A nonperiodic lobate emplacement contour with a narrow
                // pressure ridge follows regional activity and resurfacing.
                let inherited_front = field + parent_context * 0.24;
                let (front, process_gain, activity_gain, ridge_gain, lineage_gain) = if band == 2 {
                    // The fine contour is a stepped flow front with a pressure
                    // lip and a trailing channel. Its conservative profile
                    // bound is [-0.64, 0.92] (smoothstep contributes [-0.5,0.5],
                    // ridge <= 0.42 and channel magnitude <= 0.14).
                    let stepped = inherited_front.smoothstep(-0.04, 0.055) - 0.5
                        + inherited_front.gaussian(0.105, 0.045) * 0.42
                        - inherited_front.gaussian(-0.12, 0.05) * 0.14;
                    (stepped, 0.60, 0.40, 0.25, 0.22)
                } else {
                    let broad = inherited_front.smoothstep(-0.08, 0.15) - 0.5
                        + inherited_front.gaussian(0.19, 0.065) * 0.28;
                    (broad, 0.62, 0.20, 0.25, 0.22)
                };
                front
                    * (process_strengths[2] * process_gain + activity_gain)
                    * (Differential::constant(1.0) - resurfacing * ridge_gain)
                    * (lineage_mask * lineage_gain + (1.0 - lineage_gain))
            }
            _ => return Err(TerrainError::InvalidConfig),
        };
        Ok(network)
    }

    fn feature(
        &self,
        x: i64,
        y: i64,
        z: i64,
        band: usize,
        layout: usize,
        edge: f64,
    ) -> Option<CellFeature> {
        let key = hash(
            self.seed
                ^ (band as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
                ^ (layout as u64).wrapping_mul(0xd1b5_4a32_d192_ed03),
            x,
            y,
            z,
        );
        let translation = if layout == 0 {
            DVec3::ZERO
        } else {
            DVec3::new(0.31, -0.27, 0.19)
        };
        let jitter = DVec3::new(unit(key ^ 1), unit(key ^ 2), unit(key ^ 3)) * 2.0 - DVec3::ONE;
        let raw = (DVec3::new(x as f64, y as f64, z as f64)
            + DVec3::splat(0.5)
            + translation
            + jitter * JITTER)
            * edge;
        if (raw.length() - self.radius_m).abs() > edge * SHELL
            || raw.length_squared() <= f64::MIN_POSITIVE
        {
            return None;
        }
        let lineage_key = if band == 0 {
            key
        } else {
            let coarse_shift = if layout == 0 {
                DVec3::ZERO
            } else {
                DVec3::new(0.31, -0.27, 0.19)
            };
            let parent_cell = (raw / BAND_EDGES_M[0] - coarse_shift)
                .floor()
                .to_array()
                .map(|value| value as i64);
            hash(
                self.seed ^ (layout as u64).wrapping_mul(0xd1b5_4a32_d192_ed03),
                parent_cell[0],
                parent_cell[1],
                parent_cell[2],
            )
        };
        Some(CellFeature {
            center: raw.normalize() * self.radius_m,
            key,
            lineage_key,
        })
    }

    fn profile(
        &self,
        point: DVec3,
        feature: CellFeature,
        edge: f64,
        q2: Differential,
        controls: GeologicalControls,
        parent_context: Differential,
    ) -> Result<Differential, TerrainError> {
        let center = feature.center.normalize();
        let mut axis =
            controls.structural_direction - center * center.dot(controls.structural_direction);
        if axis.length_squared() < 1e-18 {
            axis = center.cross(if center.y.abs() < 0.8 {
                DVec3::Y
            } else {
                DVec3::X
            });
        }
        axis = axis.normalize();
        let mut side = center.cross(axis).normalize();
        let lineage_turn = (unit(feature.lineage_key ^ 0x4c49_4e45_4147_0001) - 0.5) * 0.28;
        axis = axis * lineage_turn.cos() + side * lineage_turn.sin();
        side = center.cross(axis).normalize();
        let delta = point - feature.center;
        let along = Differential::new(delta.dot(axis) / edge, axis * (self.radius_m / edge));
        let across = Differential::new(delta.dot(side) / edge, side * (self.radius_m / edge));
        let process = controls.process_strengths;
        let burial = 1.0 - controls.resurfacing * 0.82;
        let age = controls.age;
        let phase = unit(feature.key ^ 0x4d4f_5250_484f_0001);
        // This signed, differentiable inherited morphology mask makes finer
        // processes respond to the actual parent floor/rim/front geometry.
        let inherited_shape = parent_context.tanh();
        let profile = match self.algorithm {
            SurfaceAlgorithm::RockyV5 => {
                if edge > 100.0 {
                    // Complex regional impacts retain an asymmetric raised rim.
                    let rim_offset = 0.37 + phase * 0.10;
                    (-q2.gaussian(0.0, 0.16) * (0.22 * process[0])
                        + q2.gaussian(rim_offset, 0.045) * (0.34 * process[0])
                        + q2.gaussian(0.82, 0.13) * (0.12 * process[0]))
                        * (0.50 + 0.50 * age)
                        * burial
                        * (inherited_shape * 0.50 + 0.50)
                        + (along * 4.0 + phase * 5.0).sin()
                            * q2.gaussian(0.58, 0.12)
                            * across.gaussian(0.0, 0.10)
                            * (0.10 * process[3])
                } else if edge > 16.0 {
                    // Local simple bowls and broken ejecta are tied to retained old crust.
                    (-q2.gaussian(0.0, 0.11) * (0.32 * process[0])
                        + q2.gaussian(0.48, 0.045) * (0.24 * process[0])
                        + q2.gaussian(0.79, 0.08) * (0.11 * process[0]))
                        * (0.35 + 0.65 * age)
                        * burial
                        * (inherited_shape * 0.50 + 0.50)
                        + across.gaussian(0.0, 0.09)
                            * along.gaussian(0.30 + phase * 0.12, 0.18)
                            * (0.14 * process[3])
                } else {
                    // Fine fractured breccia is stronger in old, unsmoothed uplands.
                    ((across.gaussian(0.0, 0.045) * (0.18 + 0.10 * phase)
                        - across.gaussian(0.10, 0.06) * 0.07)
                        * (along.gaussian(-0.10, 0.24) + along.gaussian(0.27, 0.16))
                        * process[3]
                        + q2.gaussian(0.28 + phase * 0.10, 0.08) * 0.12 * process[0])
                        * burial
                        * (inherited_shape * 0.50 + 0.50)
                }
            }
            SurfaceAlgorithm::IcyV3 => {
                // Stress-aligned fractures, unequal shoulders and intersecting
                // secondary faults strengthen with activity and relax under young ice.
                let main_trough =
                    across.gaussian(0.0, 0.055) * (along.gaussian(0.0, 0.42) * 0.45 + 0.55);
                let main_ridge = (across.gaussian(-0.10, 0.06) * 0.38
                    + across.gaussian(0.12, 0.075) * 0.27)
                    * (along.gaussian(0.0, 0.45) * 0.5 + 0.5);
                let crossing =
                    along.gaussian(0.04 + phase * 0.11, 0.07) * across.gaussian(0.0, 0.42);
                let blocks = q2.gaussian(0.45 + phase * 0.20, 0.12)
                    * (if edge < 16.0 { 0.18 } else { 0.08 });
                (-main_trough * (0.34 * process[1] + 0.10)
                    + main_ridge * (0.20 * process[1] + 0.07)
                    - crossing * (0.16 * process[1])
                    + blocks * process[3])
                    * (0.28 + 0.72 * controls.activity)
                    * (1.0 - controls.resurfacing * 0.72)
                    * (inherited_shape * 0.35 + 0.65)
            }
            SurfaceAlgorithm::VolcanicV3 => {
                // A deterministic lobate emplacement front with a constructional
                // apron and pressure ridge; phase changes the front and collapse.
                let front = along
                    + across * (0.18 + phase * 0.12)
                    + (across * (5.0 + phase * 3.0)).sin() * 0.045;
                let apron = front.smoothstep(-0.30, 0.12) - 0.5;
                let pressure = front.gaussian(0.10, 0.045) * 0.38;
                let margin = across.gaussian(0.0, 0.24)
                    * (along.gaussian(0.22, 0.23) + along.gaussian(-0.25, 0.22) * 0.6);
                let regional_construction = q2.gaussian(0.22, 0.22) * 0.17
                    - q2.gaussian(0.035, 0.025) * (0.15 + phase * 0.08);
                let emplacement = apron * 0.24 + pressure + margin * 0.12;
                let parent_strength = process[2] * 0.64 + controls.activity * 0.36;
                if edge > 100.0 {
                    (emplacement * parent_strength + regional_construction * process[1])
                        * (0.45 + 0.55 * parent_strength)
                        * (inherited_shape * 0.35 + 0.65)
                } else {
                    (emplacement * parent_strength + front.gaussian(0.10, 0.08) * 0.12 * process[2])
                        * (1.0 - controls.resurfacing * 0.28)
                        * (inherited_shape * 0.35 + 0.65)
                }
            }
            _ => return Err(TerrainError::InvalidConfig),
        };
        Ok(profile)
    }

    pub(super) fn diagnostic_boundary_probes(
        &self,
        near: DVec3,
    ) -> Result<Vec<(&'static str, DVec3)>, TerrainError> {
        let n = near.normalize();
        let mut probes = self.parent.diagnostic_boundary_probes(n)?;
        for (band, edge_m) in BAND_EDGES_M.into_iter().enumerate() {
            if let Some(feature) = self.nearby_feature(n, band) {
                let center = feature.center.normalize();
                let control = self.parent.controls(center)?;
                let axis = control.structural_direction.normalize();
                for multiplier in [1.0 - 1.0e-4, 1.0, 1.0 + 1.0e-4] {
                    let distance = SUPPORT * edge_m * multiplier / self.radius_m;
                    probes.push((
                        "hierarchical-detail-support",
                        (center * distance.cos() + axis * distance.sin()).normalize(),
                    ));
                }
            }
        }
        Ok(probes)
    }

    pub(super) fn diagnostic_landmark_probes(
        &self,
        near: DVec3,
    ) -> Result<Vec<(&'static str, DVec3)>, TerrainError> {
        let n = near.normalize();
        let mut probes = self.parent.diagnostic_landmark_probes(n)?;
        for band in 0..3 {
            if let Some(feature) = self.nearby_feature(n, band) {
                probes.push((
                    match (self.algorithm, band) {
                        (SurfaceAlgorithm::RockyV5, 0) => "rocky-regional-impact-rim",
                        (SurfaceAlgorithm::RockyV5, 1) => "rocky-local-simple-impact",
                        (SurfaceAlgorithm::RockyV5, _) => "rocky-fine-breccia-fracture",
                        (SurfaceAlgorithm::IcyV3, _) => "icy-stress-fracture-intersection",
                        (SurfaceAlgorithm::VolcanicV3, _) => "volcanic-lobate-emplacement-front",
                        _ => "hierarchical-detail-feature",
                    },
                    feature.center.normalize(),
                ));
            }
        }
        Ok(probes)
    }

    fn nearby_feature(&self, near: DVec3, band: usize) -> Option<CellFeature> {
        let edge = BAND_EDGES_M[band].max(8.0);
        let point = near.normalize() * self.radius_m;
        let mut best: Option<(f64, CellFeature)> = None;
        for layout in 0..LAYOUTS {
            let shift = if layout == 0 {
                DVec3::ZERO
            } else {
                DVec3::new(0.31, -0.27, 0.19)
            };
            let base = (point / edge - shift).floor().to_array().map(|v| v as i64);
            for x in base[0] - 1..=base[0] + 1 {
                for y in base[1] - 1..=base[1] + 1 {
                    for z in base[2] - 1..=base[2] + 1 {
                        let Some(feature) = self.feature(x, y, z, band, layout, edge) else {
                            continue;
                        };
                        let distance = (point - feature.center).length_squared();
                        if distance < (edge * SUPPORT).powi(2)
                            && best.is_none_or(|(old_distance, _)| distance < old_distance)
                        {
                            best = Some((distance, feature));
                        }
                    }
                }
            }
        }
        best.map(|(_, feature)| feature)
    }
}

fn hash(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    mix(seed ^ mix(x as u64) ^ mix((y as u64).rotate_left(21)) ^ mix((z as u64).rotate_left(42)))
}

fn normalize_contribution(n: DVec3, value: Differential, budget_m: f64) -> Differential {
    if budget_m > f64::MIN_POSITIVE {
        Differential::new(
            value.value / budget_m,
            (value.gradient - n * n.dot(value.gradient)) / budget_m,
        )
    } else {
        Differential::constant(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(algorithm: SurfaceAlgorithm) -> HierarchicalField {
        let seed = 0x5eed_cafe;
        HierarchicalField::new(
            algorithm,
            GeologicalParameters::generated(algorithm, super::super::TerrainSeed(seed)),
            seed,
            109_000.0,
        )
        .unwrap()
    }

    #[test]
    fn support_halo_reaches_no_feature_outside_the_one_cell_search() {
        for algorithm in [
            SurfaceAlgorithm::RockyV5,
            SurfaceAlgorithm::IcyV3,
            SurfaceAlgorithm::VolcanicV3,
        ] {
            let field = field(algorithm);
            for (band, edge) in BAND_EDGES_M.into_iter().enumerate() {
                for direction in [
                    DVec3::Z,
                    DVec3::new(1.0, 1.0, 0.0).normalize(),
                    DVec3::ONE.normalize(),
                ] {
                    let point = direction * field.radius_m;
                    for layout in 0..LAYOUTS {
                        let translation = if layout == 0 {
                            DVec3::ZERO
                        } else {
                            DVec3::new(0.31, -0.27, 0.19)
                        };
                        let [bx, by, bz] = (point / edge - translation)
                            .floor()
                            .to_array()
                            .map(|v| v as i64);
                        for x in bx - 3..=bx + 3 {
                            for y in by - 3..=by + 3 {
                                for z in bz - 3..=bz + 3 {
                                    if (x - bx).abs() <= 1
                                        && (y - by).abs() <= 1
                                        && (z - bz).abs() <= 1
                                    {
                                        continue;
                                    }
                                    if let Some(feature) =
                                        field.feature(x, y, z, band, layout, edge)
                                    {
                                        assert!(
                                            (feature.center - point).length() >= edge * SUPPORT
                                        );
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn component_recomposition_and_band_work_are_exact_and_bounded() {
        for algorithm in [
            SurfaceAlgorithm::RockyV5,
            SurfaceAlgorithm::IcyV3,
            SurfaceAlgorithm::VolcanicV3,
        ] {
            let field = field(algorithm);
            for direction in [DVec3::Z, DVec3::X, DVec3::ONE.normalize()] {
                let sample = field.evaluate(direction).unwrap();
                let details = field.diagnostics(direction).unwrap();
                let height = details.inherited_height_m
                    + details.regional_height_m
                    + details.local_height_m
                    + details.fine_height_m;
                let gradient = details.inherited_gradient_m
                    + details.regional_gradient_m
                    + details.local_gradient_m
                    + details.fine_gradient_m;
                assert_eq!(sample.height_m, height);
                assert!((sample.gradient_m - gradient).length() < 1e-8);
                assert!(
                    details
                        .band_work
                        .iter()
                        .all(|work| work.cells_visited == 54)
                );
                assert!(sample.height_m.abs() <= field.absolute_height_bound_m());
            }
        }
    }

    #[test]
    fn supported_fine_morphology_covers_near_surface_queries() {
        let field = field(SurfaceAlgorithm::IcyV3);
        let mut supported = [0usize; 3];
        let mut effective = [0usize; 3];
        let samples = 512;
        let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
        for index in 0..samples {
            let y = 1.0 - 2.0 * (index as f64 + 0.5) / samples as f64;
            let radial = (1.0 - y * y).sqrt();
            let angle = golden * index as f64;
            let point = DVec3::new(radial * angle.cos(), y, radial * angle.sin()) * field.radius_m;
            let (_, context) = field.parent.evaluate_with_context(point).unwrap();
            for band in 0..3 {
                let mut work = SurfaceQueryWork::default();
                let residual = field
                    .evaluate_band(
                        point,
                        band,
                        Differential::constant(0.0),
                        &context.directed,
                        &mut work,
                    )
                    .unwrap();
                if work.accepted_features.unwrap_or_default() > 0 {
                    supported[band] += 1;
                }
                if residual.value.abs() > 1.0e-8 {
                    effective[band] += 1;
                }
            }
        }
        eprintln!("feature support/effective coverage: {supported:?}/{effective:?} / {samples}");
        assert!(
            supported[2] * 100 / samples >= 70,
            "fine feature supports unexpectedly sparse: {supported:?}"
        );
        assert!(
            effective.iter().all(|count| *count * 100 / samples >= 95),
            "continuous process field left smooth gaps: {effective:?}"
        );
    }

    #[test]
    fn derivatives_match_physical_tangent_differences() {
        for algorithm in [
            SurfaceAlgorithm::RockyV5,
            SurfaceAlgorithm::IcyV3,
            SurfaceAlgorithm::VolcanicV3,
        ] {
            let field = field(algorithm);
            let n = DVec3::new(0.36, -0.41, 0.84).normalize();
            let sample = field.evaluate(n).unwrap();
            let tangent = n.cross(DVec3::Y).normalize();
            let step = 0.0002 / field.radius_m;
            let plus = field.evaluate((n + tangent * step).normalize()).unwrap();
            let minus = field.evaluate((n - tangent * step).normalize()).unwrap();
            let numeric = (plus.height_m - minus.height_m) / (2.0 * step);
            assert!(
                (numeric - sample.gradient_m.dot(tangent)).abs() < 0.10,
                "derivative mismatch for {algorithm:?}: numeric={numeric}, analytic={}",
                sample.gradient_m.dot(tangent)
            );
        }
    }

    #[test]
    fn process_grammars_remain_distinct_and_suppressed_by_young_resurfacing() {
        let direction = DVec3::new(0.17, 0.43, 0.89).normalize();
        let rocky = field(SurfaceAlgorithm::RockyV5)
            .diagnostics(direction)
            .unwrap();
        let icy = field(SurfaceAlgorithm::IcyV3)
            .diagnostics(direction)
            .unwrap();
        let volcanic = field(SurfaceAlgorithm::VolcanicV3)
            .diagnostics(direction)
            .unwrap();
        assert_ne!(rocky.local_height_m, icy.local_height_m);
        assert_ne!(icy.fine_height_m, volcanic.fine_height_m);
        assert!(rocky.parent_process_strengths.iter().all(|v| v.is_finite()));

        let seed = 0x5eed_cafe;
        let mut old_ice = GeologicalParameters::generated(
            SurfaceAlgorithm::IcyV3,
            super::super::TerrainSeed(seed),
        );
        let mut young_ice = old_ice;
        old_ice.resurfacing_fraction = 0.05;
        young_ice.resurfacing_fraction = 0.95;
        let old_field =
            HierarchicalField::new(SurfaceAlgorithm::IcyV3, old_ice, seed, 109_000.0).unwrap();
        let young_field =
            HierarchicalField::new(SurfaceAlgorithm::IcyV3, young_ice, seed, 109_000.0).unwrap();
        let directions = [DVec3::Z, DVec3::X, DVec3::Y, DVec3::ONE.normalize()];
        let energy = |field: &HierarchicalField| {
            directions
                .iter()
                .map(|direction| {
                    let detail = field.diagnostics(*direction).unwrap();
                    detail.local_height_m.abs() + detail.fine_height_m.abs()
                })
                .sum::<f64>()
        };
        assert!(energy(&young_field) < energy(&old_field));
    }

    #[test]
    fn zero_relief_disables_all_residuals_and_parent_masks_track_coarse_processes() {
        for algorithm in [
            SurfaceAlgorithm::RockyV5,
            SurfaceAlgorithm::IcyV3,
            SurfaceAlgorithm::VolcanicV3,
        ] {
            let seed = 0x71a9;
            let mut parameters =
                GeologicalParameters::generated(algorithm, super::super::TerrainSeed(seed));
            parameters.relief_fraction = 0.0;
            let zero = HierarchicalField::new(algorithm, parameters, seed, 109_000.0).unwrap();
            let zero_details = zero
                .diagnostics(DVec3::new(0.4, -0.2, 0.8).normalize())
                .unwrap();
            assert_eq!(zero_details.regional_height_m, 0.0);
            assert_eq!(zero_details.local_height_m, 0.0);
            assert_eq!(zero_details.fine_height_m, 0.0);

            let live = field(algorithm);
            let mut strongest = (0.0_f64, DVec3::Z, Differential::constant(0.0));
            for index in 0..64 {
                let angle = index as f64 * 2.399_963_229_728_653;
                let y = 1.0 - 2.0 * (index as f64 + 0.5) / 64.0;
                let radial = (1.0 - y * y).sqrt();
                let direction = DVec3::new(radial * angle.cos(), y, radial * angle.sin());
                let (_, context) = live.parent.evaluate_with_context(direction).unwrap();
                if context.morphology.abs() > strongest.0 {
                    strongest = (
                        context.morphology.abs(),
                        direction,
                        Differential::new(context.morphology, context.gradient),
                    );
                }
            }
            assert!(
                strongest.0 > 0.01,
                "no parent process context for {algorithm:?}"
            );
            let directed = live
                .parent
                .evaluate_with_context(strongest.1)
                .unwrap()
                .1
                .directed;
            let with_parent = live
                .continuous_process(strongest.1, 2, strongest.2, &directed)
                .unwrap();
            let without_parent = live
                .continuous_process(strongest.1, 2, Differential::constant(0.0), &directed)
                .unwrap();
            assert_ne!(with_parent.value, without_parent.value);
        }
    }

    #[test]
    fn conservative_height_envelope_and_preserved_parent_replay_hold() {
        let seed = 0xa113_5eed;
        let samples = 64;
        let golden = std::f64::consts::PI * (3.0 - 5.0_f64.sqrt());
        for algorithm in [
            SurfaceAlgorithm::RockyV5,
            SurfaceAlgorithm::IcyV3,
            SurfaceAlgorithm::VolcanicV3,
        ] {
            let parameters =
                GeologicalParameters::generated(algorithm, super::super::TerrainSeed(seed));
            let field = HierarchicalField::new(algorithm, parameters, seed, 109_000.0).unwrap();
            let parent_algorithm = match algorithm {
                SurfaceAlgorithm::RockyV5 => SurfaceAlgorithm::RockyV4,
                SurfaceAlgorithm::IcyV3 => SurfaceAlgorithm::IcyV2,
                SurfaceAlgorithm::VolcanicV3 => SurfaceAlgorithm::VolcanicV2,
                _ => unreachable!(),
            };
            let parent = ProvinceField::new(parent_algorithm, parameters, seed, 109_000.0).unwrap();
            for index in 0..samples {
                let y = 1.0 - 2.0 * (index as f64 + 0.5) / samples as f64;
                let radial = (1.0 - y * y).sqrt();
                let angle = golden * index as f64;
                let direction = DVec3::new(radial * angle.cos(), y, radial * angle.sin());
                let before = parent.evaluate(direction).unwrap();
                let (after, context) = parent.evaluate_with_context(direction).unwrap();
                assert_eq!(before.height_m.to_bits(), after.height_m.to_bits());
                assert_eq!(before.gradient_m, after.gradient_m);
                assert_eq!(before.weights, after.weights);
                assert!(context.morphology.is_finite() && context.gradient.is_finite());
                let complete = field.evaluate(direction).unwrap();
                assert!(complete.height_m.abs() <= field.absolute_height_bound_m());
                let detail = field.diagnostics(direction).unwrap();
                assert_eq!(
                    detail.inherited_height_m.to_bits(),
                    before.height_m.to_bits()
                );
                assert_eq!(detail.inherited_gradient_m, before.gradient_m);
                let decomposed = detail.inherited_height_m
                    + detail.regional_height_m
                    + detail.local_height_m
                    + detail.fine_height_m;
                assert!((complete.height_m - decomposed).abs() < 1.0e-9);
            }
        }
    }
}
