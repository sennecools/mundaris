//! Versioned province-directed geological processes. Complete queries contain no
//! rendering scale. All differential values carry analytic body-direction
//! derivatives through burial, support windows and bounded composition.
use super::{
    GeologicalControls, GeologicalParameters, MoonTerrainConfig, MoonTerrainDefinition,
    MoonTerrainGenerator, MoonTerrainVersion, SurfaceAlgorithm, SurfaceQueryWork, TerrainError,
    TerrainIdentity, TerrainSeed,
    director::{DirectedSample, DirectorField},
    mix,
    query_context::{CachedFeature, CellKey, PROVINCE_FEATURE_FAMILY, SurfaceQueryContext},
    unit,
};
use glam::{DMat3, DVec3};
use mundaris_math::{Direction3, noise::gradient_noise, surface::SurfaceLocation};
use std::ops::{Add, Div, Mul, Neg, Sub};
use std::time::Instant;

const JITTER: f64 = 0.16;
const SHELL: f64 = 0.30;
const SUPPORT: f64 = 0.52;
// A centre outside the one-cell halo is at least 1-JITTER-SHELL cell
// edges away after shell projection. All compact profiles share this support.
const _: () = assert!(1.0 - JITTER - SHELL > SUPPORT);
const LAYOUTS: usize = 2;
const LEVELS: usize = 5;
const LOCAL_RELIEF_ALLOCATIONS: [f64; LEVELS] = [1.2, 0.6, 0.25, 0.08, 0.08];

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
    fn exp(self) -> Self {
        let value = self.value.exp();
        Self::new(value, self.gradient * value)
    }
    fn sin(self) -> Self {
        Self::new(self.value.sin(), self.gradient * self.value.cos())
    }
    fn sqrt(self) -> Self {
        let value = self.value.sqrt();
        Self::new(
            value,
            if value > 1e-30 {
                self.gradient / (2.0 * value)
            } else {
                DVec3::ZERO
            },
        )
    }
    fn square(self) -> Self {
        self * self
    }
    fn cube(self) -> Self {
        self * self * self
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
            t.square() * (Self::constant(3.0) - t * 2.0)
        }
    }
}
impl Add for Differential {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self::new(self.value + b.value, self.gradient + b.gradient)
    }
}
impl Sub for Differential {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        Self::new(self.value - b.value, self.gradient - b.gradient)
    }
}
impl Mul for Differential {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self::new(
            self.value * b.value,
            self.gradient * b.value + b.gradient * self.value,
        )
    }
}
impl Div for Differential {
    type Output = Self;
    fn div(self, b: Self) -> Self {
        Self::new(
            self.value / b.value,
            (self.gradient * b.value - b.gradient * self.value) / (b.value * b.value),
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
    fn add(self, b: f64) -> Self {
        self + Self::constant(b)
    }
}
impl Sub<f64> for Differential {
    type Output = Self;
    fn sub(self, b: f64) -> Self {
        self - Self::constant(b)
    }
}
impl Mul<f64> for Differential {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self::new(self.value * b, self.gradient * b)
    }
}
impl Div<f64> for Differential {
    type Output = Self;
    fn div(self, b: f64) -> Self {
        self * (1.0 / b)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProvinceSample {
    pub height_m: f64,
    pub gradient_m: DVec3,
    pub weights: [f64; 4],
    pub work: SurfaceQueryWork,
}

/// Signed relief of the three larger compact process levels, normalized by
/// their individual physical budgets. This excludes the arbitrary broad
/// elevation datum and carries the derivative of the actual parent profiles.
#[derive(Debug, Clone, Copy)]
pub(super) struct ProvinceParentContext {
    pub morphology: f64,
    pub gradient: DVec3,
    pub directed: DirectedSample,
}

#[derive(Debug, Clone, Copy)]
struct Feature {
    center: DVec3,
    key: u64,
    edge_m: f64,
    level: usize,
}

fn cache_feature(feature: Feature) -> CachedFeature {
    CachedFeature {
        center: feature.center,
        key: feature.key,
        lineage_key: 0,
        edge_m: feature.edge_m,
        level: feature.level,
    }
}

fn uncache_feature(feature: CachedFeature) -> Feature {
    Feature {
        center: feature.center,
        key: feature.key,
        edge_m: feature.edge_m,
        level: feature.level,
    }
}

#[derive(Debug, Clone)]
pub(super) struct ProvinceField {
    algorithm: SurfaceAlgorithm,
    parameters: GeologicalParameters,
    radius_m: f64,
    seed: u64,
    director: DirectorField,
    rotation: DMat3,
    edges_m: [f64; LEVELS],
    rocky_history: Option<Box<MoonTerrainGenerator>>,
    bound_m: f64,
}

impl ProvinceField {
    pub fn new(
        algorithm: SurfaceAlgorithm,
        p: GeologicalParameters,
        seed: u64,
        radius_m: f64,
    ) -> Result<Self, TerrainError> {
        if !matches!(
            algorithm,
            SurfaceAlgorithm::RockyV4 | SurfaceAlgorithm::IcyV2 | SurfaceAlgorithm::VolcanicV2
        ) {
            return Err(TerrainError::InvalidConfig);
        }
        if !radius_m.is_finite() || radius_m <= 0.0 || radius_m > 1e8 {
            return Err(TerrainError::InvalidRadius);
        }
        p.validate()?;
        let feature_seed = mix(seed ^ algorithm.code() ^ 0x4645_4154_5552_0002);
        let angle = |tag| unit(feature_seed ^ tag) * std::f64::consts::TAU;
        let rotation = DMat3::from_rotation_z(angle(1))
            * DMat3::from_rotation_y(angle(2))
            * DMat3::from_rotation_x(angle(3));
        let edges_m = [
            radius_m * 0.18,
            radius_m * 0.045,
            radius_m * 0.009,
            180.0,
            24.0,
        ]
        .map(|e| e.max(8.0));
        let rocky_history = if algorithm == SurfaceAlgorithm::RockyV4 {
            let scale = p.relief_fraction / 0.006;
            let config = MoonTerrainConfig::new(
                0.0015 * scale,
                0.001 * scale * (1.1 - 0.5 * p.age),
                [
                    p.feature_scale_fraction,
                    p.feature_scale_fraction * 0.075,
                    0.00003,
                ],
                [
                    0.0025 * scale * (0.35 + 0.65 * p.impact_retention),
                    0.0008 * scale,
                    0.000018 * scale,
                ],
            )?;
            let definition = MoonTerrainDefinition::with_version(
                TerrainIdentity(0x524f_434b_5634_0001),
                TerrainSeed(mix(feature_seed ^ 0x494d_5041_4354_0001)),
                config,
                MoonTerrainVersion::MoonLikeV2,
            );
            Some(Box::new(MoonTerrainGenerator::new(&definition, radius_m)?))
        } else {
            None
        };
        // Algebraic envelope: the director relief is <=1.2*p.relief_fraction
        // (mutually exclusive normalized weights). All regional processes together
        // are <1.5 such relief units (<=1.8*p.relief_fraction*R). Five convex
        // local compositions bound profiles by 2.5; each is capped by both its
        // cell edge and its scale-specific relief allocation. The continuous
        // three-scale fine grammar uses <=.22*p.relief_fraction*R in total.
        // Signed profile bounds matter here: an ice trough plus positive
        // shoulders lies in [-1,.65], its second fault in [-1,.30], so their
        // combination has absolute value <=1.52. Activation <=1.30 and old
        // impact <=.35 give <=2.326. Volcanic normalized province weights
        // bound the weighted terms by .85; remaining activity/resurfacing
        // terms add <=1.118, hence <2.0. Rocky profiles are smaller still.
        let history_bound = rocky_history
            .as_ref()
            .map_or(0.0, |g| g.conservative_absolute_height_bound_m());
        let local_bound = edges_m
            .iter()
            .zip(LOCAL_RELIEF_ALLOCATIONS)
            .map(|(edge, allocation)| {
                (edge * 0.055).min(radius_m * p.relief_fraction * allocation) * 2.5
            })
            .sum::<f64>();
        let bound_m = (history_bound + radius_m * p.relief_fraction * 2.02 + local_bound).next_up();
        if !bound_m.is_finite() || bound_m >= radius_m * 0.1 {
            return Err(TerrainError::InvalidRadius);
        }
        Ok(Self {
            algorithm,
            parameters: p,
            radius_m,
            seed: feature_seed,
            director: DirectorField::new(algorithm, p, seed ^ 0x4449_5245_4354_0002),
            rotation,
            edges_m,
            rocky_history,
            bound_m,
        })
    }

    /// Heap payload retained by this field's fixed `Box<MoonTerrainGenerator>`
    /// history, if present. The remaining state is stored inline in this value;
    /// allocator bookkeeping is outside this byte count.
    pub(super) fn resident_heap_bytes(&self) -> usize {
        self.rocky_history
            .as_ref()
            .map_or(0, |_| std::mem::size_of::<MoonTerrainGenerator>())
    }
    pub fn absolute_height_bound_m(&self) -> f64 {
        self.bound_m
    }
    pub fn controls(&self, n: DVec3) -> Result<GeologicalControls, TerrainError> {
        Ok(self.director.evaluate(n)?.controls)
    }
    fn noise(&self, n: DVec3, frequency: f64, salt: u64) -> Result<Differential, TerrainError> {
        let sample = gradient_noise(self.seed ^ salt, self.rotation * n * frequency)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        Ok(Differential::new(
            sample.value,
            self.rotation.transpose() * sample.gradient * frequency,
        ))
    }
    fn stretched_noise(
        &self,
        n: DVec3,
        wavelength_m: f64,
        salt: u64,
        stretch: DVec3,
    ) -> Result<Differential, TerrainError> {
        let frequency = self.radius_m / wavelength_m;
        let map = DMat3::from_diagonal(stretch) * self.rotation;
        let sample = gradient_noise(self.seed ^ salt, map * n * frequency)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        Ok(Differential::new(
            sample.value,
            map.transpose() * sample.gradient * frequency,
        ))
    }
    fn controls_differential(d: DirectedSample) -> ([Differential; 4], [Differential; 5]) {
        (
            std::array::from_fn(|i| {
                Differential::new(d.controls.province_weights[i], d.province_gradients[i])
            }),
            std::array::from_fn(|i| {
                Differential::new(
                    [
                        d.controls.age,
                        d.controls.activity,
                        d.controls.resurfacing,
                        d.controls.impact_retention,
                        d.controls.relief_potential,
                    ][i],
                    d.control_gradients[i],
                )
            }),
        )
    }
    pub fn evaluate(&self, direction: DVec3) -> Result<ProvinceSample, TerrainError> {
        Ok(self.evaluate_with_context(direction)?.0)
    }

    pub fn evaluate_with_context(
        &self,
        direction: DVec3,
    ) -> Result<(ProvinceSample, ProvinceParentContext), TerrainError> {
        self.evaluate_with_query_context_inner(direction, None)
    }

    pub(super) fn evaluate_with_query_context(
        &self,
        direction: DVec3,
        context: &mut SurfaceQueryContext<'_>,
    ) -> Result<(ProvinceSample, ProvinceParentContext), TerrainError> {
        self.evaluate_with_query_context_inner(direction, Some(context))
    }

    fn evaluate_with_query_context_inner(
        &self,
        direction: DVec3,
        mut context: Option<&mut SurfaceQueryContext<'_>>,
    ) -> Result<(ProvinceSample, ProvinceParentContext), TerrainError> {
        let profile_enabled = context
            .as_deref()
            .is_some_and(SurfaceQueryContext::profiling);
        let profile_started = profile_enabled.then(Instant::now);
        let legacy_before = context
            .as_deref()
            .map_or(0, SurfaceQueryContext::legacy_history_elapsed_ns);
        if !direction.is_finite() || direction.length_squared() <= f64::MIN_POSITIVE {
            return Err(TerrainError::InvalidConfig);
        }
        let n = direction.normalize();
        let directed = self.director.evaluate(n)?;
        let (w, c) = Self::controls_differential(directed);
        let relief = c[4] * self.radius_m;
        let one = Differential::constant(1.0);
        let broad = self.noise(n, 2.4, 0x4252_4f41_4400_0002)?;
        let regional = self.noise(n, 9.0, 0x5245_4749_4f4e_0002)?;
        let mut work = SurfaceQueryWork {
            accepted_features: Some(0),
            ..SurfaceQueryWork::default()
        };
        let mut height = match self.algorithm {
            SurfaceAlgorithm::RockyV4 => {
                let history = self
                    .rocky_history
                    .as_ref()
                    .ok_or(TerrainError::InvalidConfig)?;
                let location = SurfaceLocation::new(
                    Direction3::try_new(n).map_err(|_| TerrainError::NonFiniteResult)?,
                );
                let sample = if let Some(context) = context.as_deref_mut() {
                    history.evaluate_point_with_context(location, context)?
                } else {
                    history.evaluate_point(location)?
                };
                let old = Differential::new(
                    sample.terrain().height_m(),
                    sample.terrain().tangent_gradient_m_per_unit_direction(),
                );
                work.cells_visited += 540;
                work.candidate_features += 540;
                work.accepted_features = None;
                let retained = (one - c[2] * 0.93) * (w[0] * 0.62 + c[3] * 0.25 + 0.13);
                old * retained
                    + relief * (broad * 0.18 + regional * (w[1] * 0.20 + w[3] * 0.14) - w[2] * 0.22)
            }
            SurfaceAlgorithm::IcyV2 => {
                // Older relief is softened and buried before fractures cut it.
                let old =
                    (broad * 0.24 + regional * 0.10) * (one - c[2] * 0.88) * (w[0] * 0.8 + 0.2);
                relief * (old - w[2] * 0.12 + regional * w[3] * 0.20)
            }
            SurfaceAlgorithm::VolcanicV2 => {
                let old =
                    (broad * 0.25 + regional * 0.16) * (one - c[2] * 0.94) * (w[0] * 0.80 + 0.20);
                relief * (old + w[3] * 0.16 - w[2] * 0.12)
            }
            _ => return Err(TerrainError::InvalidConfig),
        };
        // Regional feature candidates carry immutable centre history and stress;
        // query-side burial modifies old impacts before construction is applied.
        let mut parent_morphology = Differential::constant(0.0);
        for (level, allocation) in LOCAL_RELIEF_ALLOCATIONS.into_iter().enumerate() {
            let contribution =
                self.local_level(n, level, w, c, &mut work, context.as_deref_mut())?;
            height = height + contribution;
            if level < 3 {
                let budget = (self.edges_m[level] * 0.055)
                    .min(self.radius_m * self.parameters.relief_fraction * allocation);
                if budget > 0.0 {
                    parent_morphology = parent_morphology + contribution / (budget * 3.0);
                }
            }
        }
        // Continuous process networks fill space between local landmarks. Fixed
        // physical wavelengths retain family grammar at human-scale distances.
        for (i, wavelength) in [600.0_f64, 90.0, 14.0].into_iter().enumerate() {
            let amplitude = (wavelength * 0.08)
                .min(self.radius_m * self.parameters.relief_fraction * [0.035, 0.015, 0.006][i]);
            let salt = 0x4c4f_4341_4c00_0002_u64.wrapping_add((i as u64) * 0x9e37);
            let field = self.stretched_noise(n, wavelength, salt, DVec3::new(0.65, 1.35, 0.85))?;
            let warped = field
                + self.noise(n, self.radius_m / (wavelength * 3.7), salt ^ 0x5741_5250)? * 0.16;
            let fine = match self.algorithm {
                SurfaceAlgorithm::RockyV4 => {
                    let ridge = (one - (warped.square() + 0.0025).sqrt()).cube();
                    let grit =
                        self.noise(n, self.radius_m / (wavelength * 0.41), salt ^ 0x4752_4954)?;
                    (ridge * (w[3] * 0.55 + w[1] * 0.24) + grit * (w[0] * 0.42 + 0.10))
                        * (one - c[2] * 0.90)
                }
                SurfaceAlgorithm::IcyV2 => {
                    let crossed = self.stretched_noise(
                        n,
                        wavelength * 1.61,
                        salt ^ 0x5354_5245_5353,
                        DVec3::new(1.6, 0.55, 0.90),
                    )?;
                    let fault =
                        Self::ice_fault(warped) + Self::ice_fault(crossed + warped * 0.13) * 0.57;
                    let blocks = warped.smoothstep(-0.11, 0.11) - 0.5;
                    fault * (c[1] * 0.7 + w[1] * 0.65 + 0.18) * (one - c[2] * 0.55)
                        + blocks * w[3] * 0.52
                }
                SurfaceAlgorithm::VolcanicV2 => {
                    let emplacement = warped.smoothstep(-0.04, 0.065) - 0.5;
                    let pressure_front = warped.gaussian(0.06, 0.035) * 0.40;
                    let old_rough =
                        self.noise(n, self.radius_m / (wavelength * 0.48), salt ^ 0x4f4c_445f)?;
                    (emplacement + pressure_front) * (w[2] * 0.65 + c[2] * 0.70 + 0.12)
                        + old_rough * w[0] * (one - c[2]) * 0.16
                }
                _ => return Err(TerrainError::InvalidConfig),
            };
            height = height + fine * amplitude;
        }
        let gradient = height.gradient - n * n.dot(height.gradient);
        if !height.value.is_finite() || !gradient.is_finite() || height.value.abs() > self.bound_m {
            return Err(TerrainError::NonFiniteResult);
        }
        let disrupted = ((gradient.length() / self.radius_m) * 2.0).clamp(0.0, 1.0);
        let raw = match self.algorithm {
            SurfaceAlgorithm::RockyV4 => [
                0.1 + 0.65 * w[0].value,
                0.05 + disrupted * 0.5,
                0.05 + c[2].value * 0.8,
                0.04 + disrupted * 0.3,
            ],
            SurfaceAlgorithm::IcyV2 => [
                0.05 + w[2].value * 0.8,
                0.05 + c[0].value * 0.6,
                0.05 + disrupted * 0.65 + w[3].value * 0.3,
                0.05 + w[0].value * 0.3,
            ],
            SurfaceAlgorithm::VolcanicV2 => [
                0.06 + c[2].value * 0.75,
                0.06 + c[0].value * 0.5,
                0.06 + w[0].value * 0.65,
                0.04 + disrupted * 0.4,
            ],
            _ => return Err(TerrainError::InvalidConfig),
        };
        let total = raw.iter().sum::<f64>();
        if let (Some(started), Some(context)) = (profile_started, context) {
            let legacy_after = context.legacy_history_elapsed_ns();
            context.record_province_exclusive(
                elapsed_ns(started).saturating_sub(legacy_after.saturating_sub(legacy_before)),
            );
        }
        Ok((
            ProvinceSample {
                height_m: height.value,
                gradient_m: gradient,
                weights: raw.map(|v| v / total),
                work,
            },
            ProvinceParentContext {
                morphology: parent_morphology.value,
                gradient: parent_morphology.gradient - n * n.dot(parent_morphology.gradient),
                directed,
            },
        ))
    }
    fn ice_fault(field: Differential) -> Differential {
        // Unequal shoulder ridges around a trough, rather than periodic stripes.
        -field.gaussian(0.0, 0.050)
            + field.gaussian(-0.105, 0.040) * 0.38
            + field.gaussian(0.13, 0.055) * 0.27
    }
    fn feature(&self, x: i64, y: i64, z: i64, level: usize, layout: usize) -> Option<Feature> {
        let edge = self.edges_m[level];
        let key = hash(
            self.seed
                ^ (level as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15)
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
        Some(Feature {
            center: raw.normalize() * self.radius_m,
            key,
            edge_m: edge,
            level,
        })
    }
    fn local_level(
        &self,
        n: DVec3,
        level: usize,
        w: [Differential; 4],
        c: [Differential; 5],
        work: &mut SurfaceQueryWork,
        mut context: Option<&mut SurfaceQueryContext<'_>>,
    ) -> Result<Differential, TerrainError> {
        let point = n * self.radius_m;
        let edge = self.edges_m[level];
        let mut numerator = Differential::constant(0.0);
        let mut denominator = Differential::constant(1.0);
        for layout in 0..LAYOUTS {
            let translation = if layout == 0 {
                DVec3::ZERO
            } else {
                DVec3::new(0.31, -0.27, 0.19)
            };
            let cell = (point / edge - translation).floor();
            let [bx, by, bz] = cell.to_array().map(|v| v as i64);
            for x in bx - 1..=bx + 1 {
                for y in by - 1..=by + 1 {
                    for z in bz - 1..=bz + 1 {
                        work.cells_visited += 1;
                        let feature = if let Some(context) = context.as_deref_mut() {
                            context
                                .feature(
                                    CellKey::new(PROVINCE_FEATURE_FAMILY, level, layout, x, y, z),
                                    || self.feature(x, y, z, level, layout).map(cache_feature),
                                )
                                .map(uncache_feature)
                        } else {
                            self.feature(x, y, z, level, layout)
                        };
                        let Some(feature) = feature else {
                            continue;
                        };
                        work.candidate_features += 1;
                        let delta = point - feature.center;
                        let q2 = Differential::new(
                            delta.length_squared() / (edge * SUPPORT).powi(2),
                            delta * (2.0 * self.radius_m / (edge * SUPPORT).powi(2)),
                        );
                        if q2.value >= 1.0 {
                            continue;
                        }
                        if let Some(count) = work.accepted_features.as_mut() {
                            *count += 1;
                        }
                        let window = (Differential::constant(1.0) - q2).cube();
                        let centre_controls = if let Some(context) = context.as_deref_mut() {
                            context.controls(
                                CellKey::new(PROVINCE_FEATURE_FAMILY, level, layout, x, y, z),
                                || self.controls(feature.center),
                            )?
                        } else {
                            self.controls(feature.center)?
                        };
                        let profile = self.profile(n, feature, centre_controls, w, c)?;
                        let budget = (edge * 0.055).min(
                            self.radius_m
                                * self.parameters.relief_fraction
                                * LOCAL_RELIEF_ALLOCATIONS[level],
                        );
                        numerator = numerator + window * profile * budget;
                        denominator = denominator + window;
                    }
                }
            }
        }
        Ok(numerator / denominator)
    }
    fn profile(
        &self,
        n: DVec3,
        f: Feature,
        centre: GeologicalControls,
        w: [Differential; 4],
        c: [Differential; 5],
    ) -> Result<Differential, TerrainError> {
        let delta = n * self.radius_m - f.center;
        let axis = centre.structural_direction;
        let side = f.center.normalize().cross(axis).normalize();
        let x = Differential::new(
            delta.dot(axis) / f.edge_m,
            axis * (self.radius_m / f.edge_m),
        );
        let y = Differential::new(
            delta.dot(side) / f.edge_m,
            side * (self.radius_m / f.edge_m),
        );
        let distance = (x.square() + y.square() + 1e-12).sqrt();
        let phase = unit(f.key ^ 0x5048_4153) * std::f64::consts::TAU;
        let warp =
            (x * 13.0 + y * 7.0 + phase).sin() * 0.035 + (y * 21.0 + phase * 0.7).sin() * 0.018;
        let warped_radius = distance * (warp + 1.0);
        let radius = 0.16 + 0.12 * unit(f.key ^ 0x5241_4449);
        let r = warped_radius / radius;
        let bowl = -(r.square() * -1.5).exp();
        let rim = r.gaussian(1.0, 0.21) * (0.25 + centre.age * 0.30);
        let impact = bowl + rim;
        let one = Differential::constant(1.0);
        Ok(match self.algorithm {
            SurfaceAlgorithm::RockyV4 => {
                // Young impacts survive plain burial; ancient impact populations
                // reside in the preserved multi-epoch field composed earlier.
                let young = if unit(f.key ^ 0x594f_554e) < 0.22 {
                    impact * 0.50
                } else {
                    Differential::constant(0.0)
                };
                let ridge = (y + x * 0.2 + (x * 8.0 + phase).sin() * 0.025).gaussian(0.0, 0.055);
                let old_local = if f.level >= 3 {
                    impact
                        * (w[0] * 1.20 + 0.15)
                        * (one - c[2] * 0.65)
                        * (0.40 + centre.impact_retention * 0.60)
                } else {
                    Differential::constant(0.0)
                };
                young + old_local + ridge * (w[3] * 0.60 + w[1] * 0.35) * (one - c[2] * 0.65)
            }
            SurfaceAlgorithm::IcyV2 => {
                let curve = y + (x * 7.0 + phase).sin() * 0.035;
                let fault = -curve.gaussian(0.0, 0.025)
                    + curve.gaussian(0.065, 0.023) * 0.38
                    + curve.gaussian(-0.07, 0.03) * 0.26;
                let crossing = x * 0.70 + y * 0.45 + (y * 11.0 + phase).sin() * 0.024;
                let second =
                    -crossing.gaussian(0.0, 0.019) + crossing.gaussian(0.058, 0.020) * 0.30;
                let old_impact =
                    impact * (one - c[2] * 0.88) * w[0] * centre.impact_retention * 0.35;
                old_impact
                    + (fault + second * 0.52)
                        * (w[1] * 0.85 + c[1] * 0.35 + 0.10)
                        * (one - c[2] * 0.55)
            }
            SurfaceAlgorithm::VolcanicV2 => {
                // Construction is lobed and directional. A centre depresses its
                // summit; the surrounding emplacement front buries old relief.
                let lobed = distance + warp + (x * 5.0 + phase).sin() * 0.04;
                let shield = (-(lobed / 0.34).square()).exp();
                let caldera = (distance / 0.10).square();
                let summit = -(caldera * -1.4).exp() * 0.70;
                let front_field = y + x * 0.22 + (x * 9.0 + phase).sin() * 0.045;
                let flow = front_field.smoothstep(-0.05, 0.06) - 0.5
                    + front_field.gaussian(0.06, 0.025) * 0.35;
                let old_impact = impact * w[0] * (one - c[2]) * 0.24;
                let construction = if f.level < 3 {
                    (shield + summit) * (w[1] * 0.85 + c[1] * 0.40 + 0.08)
                } else {
                    Differential::constant(0.0)
                };
                old_impact + construction + flow * (w[2] * 0.75 + c[2] * 0.65 + 0.10)
            }
            _ => return Err(TerrainError::InvalidConfig),
        })
    }
    fn nearby_features(&self, near: DVec3, level: usize) -> Vec<Feature> {
        let point = near.normalize() * self.radius_m;
        let edge = self.edges_m[level];
        let mut output = Vec::new();
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
            for x in bx - 2..=bx + 2 {
                for y in by - 2..=by + 2 {
                    for z in bz - 2..=bz + 2 {
                        if let Some(f) = self.feature(x, y, z, level, layout) {
                            output.push(f);
                        }
                    }
                }
            }
        }
        output.sort_by(|a, b| {
            (a.center - point)
                .length_squared()
                .total_cmp(&(b.center - point).length_squared())
        });
        output
    }
    pub fn diagnostic_landmark_probes(
        &self,
        near: DVec3,
    ) -> Result<Vec<(&'static str, DVec3)>, TerrainError> {
        let n = near.normalize();
        let mut output = Vec::new();
        // Bound the local search; retain the nearest candidate per scale. These
        // are actual supported features, not a globally scanned catalogue.
        for level in [2, 3, 4] {
            if let Some(f) = self.nearby_features(n, level).first().copied() {
                let centre = f.center.normalize();
                let axis = self.controls(centre)?.structural_direction;
                let side = centre.cross(axis).normalize();
                let radius = f.edge_m * (0.16 + 0.12 * unit(f.key ^ 0x5241_4449));
                let shift = match self.algorithm {
                    SurfaceAlgorithm::RockyV4 => axis * radius,
                    SurfaceAlgorithm::IcyV2 => side * (f.edge_m * 0.025),
                    SurfaceAlgorithm::VolcanicV2 => side * (f.edge_m * 0.06),
                    _ => DVec3::ZERO,
                };
                output.push((
                    match self.algorithm {
                        SurfaceAlgorithm::RockyV4 => "rocky-impact-rim",
                        SurfaceAlgorithm::IcyV2 => "icy-intersecting-fracture-shoulder",
                        _ => "volcanic-emplacement-front",
                    },
                    (f.center + shift).normalize(),
                ));
                output.push(("province-construction-centre", centre));
            }
        }
        // The continuous fine process network also has landmarks between cells.
        // Search a fixed 9x9 tangent lattice within 28m; this is a diagnostic
        // location search, never an input to generation.
        let helper = if n.y.abs() < 0.8 { DVec3::Y } else { DVec3::X };
        let u = n.cross(helper).normalize();
        let v = n.cross(u);
        let mut best = None;
        for x in -4..=4 {
            for y in -4..=4 {
                let direction =
                    (n * self.radius_m + (u * x as f64 + v * y as f64) * 7.0).normalize();
                let sample = self.evaluate(direction)?;
                if sample.gradient_m.length() / self.radius_m > 1.0 {
                    continue;
                }
                let step = 4.0 / self.radius_m;
                let mut curvature = 0.0;
                for axis in [u, v] {
                    let plus = self.evaluate((direction + axis * step).normalize())?;
                    let minus = self.evaluate((direction - axis * step).normalize())?;
                    curvature += (plus.gradient_m - minus.gradient_m).dot(axis).abs();
                }
                if best.is_none_or(|(s, _)| curvature > s) {
                    best = Some((curvature, direction));
                }
            }
        }
        if let Some((_, direction)) = best {
            output.push(("province-fine-process-curvature", direction));
        }
        Ok(output)
    }
    pub fn diagnostic_boundary_probes(
        &self,
        near: DVec3,
    ) -> Result<Vec<(&'static str, DVec3)>, TerrainError> {
        let n = near.normalize();
        let mut output = Vec::new();
        let helper = if n.y.abs() < 0.8 { DVec3::Y } else { DVec3::X };
        let u = n.cross(helper).normalize();
        let v = n.cross(u);
        'rings: for ring in 1..=8 {
            let theta = ring as f64 * 0.09;
            for step in 0..64 {
                let point = |k: usize| {
                    let a = k as f64 * std::f64::consts::TAU / 64.0;
                    (n * theta.cos() + (u * a.cos() + v * a.sin()) * theta.sin()).normalize()
                };
                let mut a = point(step);
                let mut b = point(step + 1);
                let dominant = winner(self.controls(a)?.province_weights);
                if dominant == winner(self.controls(b)?.province_weights) {
                    continue;
                }
                for _ in 0..36 {
                    let mid = (a + b).normalize();
                    if winner(self.controls(mid)?.province_weights) == dominant {
                        a = mid
                    } else {
                        b = mid
                    }
                }
                let middle = (a + b).normalize();
                let tangent = (point(step + 1) - point(step)).normalize();
                for offset in [-1e-7, 0.0, 1e-7] {
                    output.push((
                        "province-transition",
                        (middle + tangent * offset).normalize(),
                    ));
                }
                break 'rings;
            }
        }
        if let Some(f) = self.nearby_features(n, 4).first().copied() {
            let center = f.center.normalize();
            let tangent = self.controls(center)?.structural_direction;
            for multiplier in [1.0 - 1e-4, 1.0, 1.0 + 1e-4] {
                let angle = 2.0
                    * ((f.edge_m * SUPPORT * multiplier / (2.0 * self.radius_m)).min(1.0)).asin();
                output.push((
                    "province-feature-support",
                    center * angle.cos() + tangent * angle.sin(),
                ));
            }
        }
        Ok(output)
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}
fn winner(w: [f64; 4]) -> usize {
    (0..4).max_by(|&a, &b| w[a].total_cmp(&w[b])).unwrap_or(0)
}
fn hash(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    mix(seed ^ mix(x as u64) ^ mix((y as u64).rotate_left(21)) ^ mix((z as u64).rotate_left(42)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn differential_products_and_quotients_match_local_differences() {
        let f = |x: f64| {
            let a = Differential::new(x, DVec3::X);
            ((a.square() + 0.2).sqrt() * a.sin() + a.gaussian(0.4, 0.3)) / (a.square() + 1.0)
        };
        let x = 0.29;
        let eps = 1e-6;
        let numeric = (f(x + eps).value - f(x - eps).value) / (2.0 * eps);
        assert!((numeric - f(x).gradient.x).abs() < 1e-8);
    }
    #[test]
    fn support_halo_matches_a_larger_cell_search() {
        for algorithm in [SurfaceAlgorithm::IcyV2, SurfaceAlgorithm::VolcanicV2] {
            let field = ProvinceField::new(
                algorithm,
                GeologicalParameters::generated(algorithm, TerrainSeed(19)),
                17,
                109000.0,
            )
            .unwrap();
            for n in [
                DVec3::Z,
                DVec3::new(1.0, 1.0, 0.0).normalize(),
                DVec3::ONE.normalize(),
            ] {
                for level in 0..LEVELS {
                    let edge = field.edges_m[level];
                    let point = n * field.radius_m;
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
                                    if let Some(f) = field.feature(x, y, z, level, layout) {
                                        assert!((f.center - point).length() >= edge * SUPPORT);
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}
