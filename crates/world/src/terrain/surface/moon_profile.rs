//! Immutable bounded height profiles and their world-authoritative Moon sampler.
use super::TerrainError;
use glam::DVec3;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{sync::Arc, time::Instant};

const MAX_PROFILE_DIMENSION: u32 = 2048;
const MAX_DETAIL_LAYER_COUNT: usize = 2;
pub(super) const PROFILE_BANDS: [(f64, f64); 3] = [(2.0, 0.012), (8.0, 0.0015), (32.0, 0.0002)];

/// Opt-in service and source-work measurements for one complete MoonProfile point.
/// Durations are nanoseconds from this process; source counters describe actual
/// profile lookups and analytic-gradient operations in the evaluator.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MoonProfileEvaluationDiagnostics {
    pub total_surface_evaluation_ns: u128,
    pub macro_sampling_ns: u128,
    pub detail_sampling_ns: [u128; MAX_DETAIL_LAYER_COUNT],
    pub detail_layer_count: usize,
    /// Sum of triplanar-weight-gradient setup and final material/result composition.
    pub weight_setup_and_material_result_composition_ns: u128,
    pub triplanar_samples: u64,
    /// Calls to the underlying profile's value accessor, excluding constant-profile fast paths.
    pub source_value_taps: u64,
    /// Source taps whose values participate in analytic UV derivative accumulation.
    pub source_derivative_taps: u64,
    /// Per-axis analytic gradient blend operations in triplanar sampling.
    pub triplanar_gradient_axis_combinations: u64,
    /// Three normalized triplanar-weight gradient vectors computed once per point.
    pub triplanar_weight_gradient_vectors: u64,
    /// Gradient-vector source terms consumed by macro and detail height composition.
    pub profile_gradient_accumulation_terms: u64,
}

#[derive(Clone, Copy)]
pub(super) enum ProfilePhase {
    Macro,
    Detail(usize),
    WeightAndMaterial,
}

pub(super) trait ProfileObserver {
    type Timer;
    fn start(&mut self, phase: ProfilePhase) -> Self::Timer;
    fn finish(&mut self, phase: ProfilePhase, timer: Self::Timer);
    fn record_profile_taps(&mut self, value_taps: u64, derivative_taps: u64);
    fn record_triplanar_sample(&mut self);
    fn record_weight_gradient_vectors(&mut self, count: u64);
    fn record_gradient_axis_combination(&mut self);
    fn record_gradient_accumulation_terms(&mut self, count: u64);
    fn record_detail_layer(&mut self);
}

pub(super) struct NoopProfileObserver;
impl ProfileObserver for NoopProfileObserver {
    type Timer = ();
    #[inline(always)]
    fn start(&mut self, _: ProfilePhase) {}
    #[inline(always)]
    fn finish(&mut self, _: ProfilePhase, _: ()) {}
    #[inline(always)]
    fn record_profile_taps(&mut self, _: u64, _: u64) {}
    #[inline(always)]
    fn record_triplanar_sample(&mut self) {}
    #[inline(always)]
    fn record_weight_gradient_vectors(&mut self, _: u64) {}
    #[inline(always)]
    fn record_gradient_axis_combination(&mut self) {}
    #[inline(always)]
    fn record_gradient_accumulation_terms(&mut self, _: u64) {}
    #[inline(always)]
    fn record_detail_layer(&mut self) {}
}

pub(super) struct TimedProfileObserver<'a>(pub &'a mut MoonProfileEvaluationDiagnostics);
impl ProfileObserver for TimedProfileObserver<'_> {
    type Timer = Instant;
    #[inline]
    fn start(&mut self, _: ProfilePhase) -> Instant {
        Instant::now()
    }
    #[inline]
    fn finish(&mut self, phase: ProfilePhase, started: Instant) {
        let elapsed = started.elapsed().as_nanos();
        match phase {
            ProfilePhase::Macro => self.0.macro_sampling_ns += elapsed,
            ProfilePhase::Detail(index) => self.0.detail_sampling_ns[index] += elapsed,
            ProfilePhase::WeightAndMaterial => {
                self.0.weight_setup_and_material_result_composition_ns += elapsed
            }
        }
    }
    #[inline]
    fn record_profile_taps(&mut self, value_taps: u64, derivative_taps: u64) {
        self.0.source_value_taps += value_taps;
        self.0.source_derivative_taps += derivative_taps;
    }
    #[inline]
    fn record_triplanar_sample(&mut self) {
        self.0.triplanar_samples += 1;
    }
    #[inline]
    fn record_weight_gradient_vectors(&mut self, count: u64) {
        self.0.triplanar_weight_gradient_vectors += count;
    }
    #[inline]
    fn record_gradient_axis_combination(&mut self) {
        self.0.triplanar_gradient_axis_combinations += 1;
    }
    #[inline]
    fn record_gradient_accumulation_terms(&mut self, count: u64) {
        self.0.profile_gradient_accumulation_terms += count;
    }
    #[inline]
    fn record_detail_layer(&mut self) {
        self.0.detail_layer_count += 1;
    }
}
pub(super) const MAX_PROFILE_WORKING_HEAP_BYTES: usize =
    24 * 1024 * 1024 + MAX_DETAIL_LAYER_COUNT * std::mem::size_of::<TerrainHeightDetailLayer>();

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ProfileKernel {
    Smoothstep,
    CubicBSpline,
}

/// Immutable unsigned 16-bit height image with canonical little-endian identity.
/// MoonProfileV1 maps the retained sample range to its explicit amplitude bands;
/// constant maps evaluate to the neutral midpoint. No physical units are inferred
/// from an external image's integer encoding.
#[derive(Debug, Clone)]
pub struct TerrainHeightProfile {
    width: u32,
    height: u32,
    values: Arc<[u16]>,
    digest: [u8; 32],
    sample_range: [u16; 2],
    detail_layers: Arc<[TerrainHeightDetailLayer]>,
    kernel: ProfileKernel,
    terrain_scale: Option<(f64, f64)>,
}

/// One immutable physical-scale detail map attached to a root height profile.
#[derive(Debug, Clone)]
pub struct TerrainHeightDetailLayer {
    profile: TerrainHeightProfile,
    footprint_m: f64,
    amplitude_m: f64,
}

impl PartialEq for TerrainHeightDetailLayer {
    fn eq(&self, other: &Self) -> bool {
        self.footprint_m.to_bits() == other.footprint_m.to_bits()
            && self.amplitude_m.to_bits() == other.amplitude_m.to_bits()
            && self.profile == other.profile
    }
}

impl PartialEq for TerrainHeightProfile {
    fn eq(&self, other: &Self) -> bool {
        self.width == other.width
            && self.height == other.height
            && self.values == other.values
            && self.digest == other.digest
            && self.sample_range == other.sample_range
            && self.detail_layers == other.detail_layers
            && self.kernel == other.kernel
            && match (self.terrain_scale, other.terrain_scale) {
                (Some((footprint_a, amplitude_a)), Some((footprint_b, amplitude_b))) => {
                    footprint_a.to_bits() == footprint_b.to_bits()
                        && amplitude_a.to_bits() == amplitude_b.to_bits()
                }
                (None, None) => true,
                _ => false,
            }
    }
}

impl TerrainHeightProfile {
    /// Loads a bounded row-major little-endian u16 profile and hashes its exact bytes.
    pub fn from_u16_le(width: u32, height: u32, bytes: &[u8]) -> Result<Self, TerrainError> {
        if !(2..=MAX_PROFILE_DIMENSION).contains(&width)
            || !(2..=MAX_PROFILE_DIMENSION).contains(&height)
        {
            return Err(TerrainError::InvalidHeightProfile);
        }
        let count = (width as usize)
            .checked_mul(height as usize)
            .ok_or(TerrainError::InvalidHeightProfile)?;
        let expected = count
            .checked_mul(2)
            .ok_or(TerrainError::InvalidHeightProfile)?;
        if bytes.len() != expected {
            return Err(TerrainError::InvalidHeightProfile);
        }
        let values: Vec<u16> = bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        let digest: [u8; 32] = Sha256::digest(bytes).into();
        let sample_range = values
            .iter()
            .copied()
            .fold([u16::MAX, u16::MIN], |range, value| {
                [range[0].min(value), range[1].max(value)]
            });
        Ok(Self {
            width,
            height,
            values: values.into(),
            digest,
            sample_range,
            detail_layers: Arc::from([]),
            kernel: ProfileKernel::Smoothstep,
            terrain_scale: None,
        })
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    pub fn sample_range(&self) -> [u16; 2] {
        self.sample_range
    }

    /// Select the periodic C2 cubic B-spline reconstruction for this profile.
    /// Existing profiles keep the original smoothstep reconstruction by default.
    pub fn with_cubic_bspline(mut self) -> Self {
        self.kernel = ProfileKernel::CubicBSpline;
        self
    }

    pub fn kernel_name(&self) -> &'static str {
        match self.kernel {
            ProfileKernel::Smoothstep => "smoothstep-bilinear",
            ProfileKernel::CubicBSpline => "cubic-periodic-bspline",
        }
    }

    /// Set the root field's physical footprint and centered displacement amplitude.
    /// This experimental scale uses one profile band with a tangent footprint that
    /// stays fixed in metres as the body's radius changes. The default radius-scaled
    /// three-band reconstruction remains unchanged when this is not set.
    pub fn with_terrain_scale(
        mut self,
        footprint_m: f64,
        amplitude_m: f64,
    ) -> Result<Self, TerrainError> {
        if !footprint_m.is_finite()
            || !(0.25..=1.0e8).contains(&footprint_m)
            || !amplitude_m.is_finite()
            || !(0.0..=2000.0).contains(&amplitude_m)
        {
            return Err(TerrainError::InvalidHeightProfile);
        }
        self.terrain_scale = Some((footprint_m, amplitude_m));
        Ok(self)
    }

    pub fn terrain_scale(&self) -> Option<(f64, f64)> {
        self.terrain_scale
    }

    /// Attach one of at most two periodic maps with a physical tangent footprint.
    /// Detail maps are immutable and may share their retained sample allocation.
    pub fn with_detail_layer(
        mut self,
        profile: TerrainHeightProfile,
        footprint_m: f64,
        amplitude_m: f64,
    ) -> Result<Self, TerrainError> {
        if !footprint_m.is_finite()
            || footprint_m < 0.25
            || !amplitude_m.is_finite()
            || amplitude_m < 0.0
            || amplitude_m > 2.0
            || !profile.detail_layers.is_empty()
            || profile.terrain_scale.is_some()
            || self.detail_layers.len() >= MAX_DETAIL_LAYER_COUNT
        {
            return Err(TerrainError::InvalidHeightProfile);
        }
        let mut layers = Vec::with_capacity(self.detail_layers.len() + 1);
        layers.extend(self.detail_layers.iter().cloned());
        layers.push(TerrainHeightDetailLayer {
            profile,
            footprint_m,
            amplitude_m,
        });
        self.detail_layers = layers.into();
        Ok(self)
    }

    pub fn detail_layer_count(&self) -> usize {
        self.detail_layers.len()
    }

    pub fn detail_layer(&self, index: usize) -> Option<&TerrainHeightDetailLayer> {
        self.detail_layers.get(index)
    }

    /// Dimensions followed by the SHA256 digest interpreted as four little-endian words.
    pub fn identity_words(&self) -> Vec<u64> {
        let mut words = Vec::with_capacity(9);
        words.push(self.width as u64);
        words.push(self.height as u64);
        for chunk in self.digest.as_chunks::<8>().0 {
            words.push(u64::from_le_bytes(*chunk));
        }
        if self.kernel == ProfileKernel::CubicBSpline {
            words.push(0x4b45_524e_454c_0001);
        }
        if let Some((footprint_m, amplitude_m)) = self.terrain_scale {
            words.push(0x5445_5252_5343_0001);
            words.push(footprint_m.to_bits());
            words.push(amplitude_m.to_bits());
        }
        if !self.detail_layers.is_empty() {
            words.push(0x4445_5441_494c_0001);
            words.push(self.detail_layers.len() as u64);
            for layer in self.detail_layers.iter() {
                words.push(layer.footprint_m.to_bits());
                words.push(layer.amplitude_m.to_bits());
                words.extend(layer.profile.identity_words());
            }
        }
        words
    }

    fn resident_heap_bytes(&self) -> usize {
        let mut bytes = self.detail_layers.len() * std::mem::size_of::<TerrainHeightDetailLayer>();
        bytes += self.values.len() * std::mem::size_of::<u16>();
        if let Some(first) = self.detail_layers.first()
            && !Arc::ptr_eq(&self.values, &first.profile.values)
        {
            bytes += first.profile.values.len() * std::mem::size_of::<u16>();
        }
        if let Some(second) = self.detail_layers.get(1) {
            let differs_from_root = !Arc::ptr_eq(&self.values, &second.profile.values);
            let differs_from_first = self
                .detail_layers
                .first()
                .is_some_and(|first| !Arc::ptr_eq(&first.profile.values, &second.profile.values));
            if differs_from_root && differs_from_first {
                bytes += second.profile.values.len() * std::mem::size_of::<u16>();
            }
        }
        bytes
    }
}

impl TerrainHeightDetailLayer {
    pub fn footprint_m(&self) -> f64 {
        self.footprint_m
    }

    pub fn amplitude_m(&self) -> f64 {
        self.amplitude_m
    }

    pub fn profile(&self) -> &TerrainHeightProfile {
        &self.profile
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct MoonProfileSample {
    pub height_m: f64,
    pub gradient_m: DVec3,
    pub weights: [f64; 4],
}

#[derive(Debug, Clone)]
pub(super) struct MoonProfileField {
    pub(super) profile: TerrainHeightProfile,
    pub(super) radius_m: f64,
    bound_m: f64,
}

impl MoonProfileField {
    pub(super) fn new(profile: TerrainHeightProfile, radius_m: f64) -> Result<Self, TerrainError> {
        if !radius_m.is_finite() || radius_m <= 0.0 || radius_m > 1.0e8 {
            return Err(TerrainError::InvalidRadius);
        }
        // Centered profile bands and detail maps each contribute at most half
        // of their configured amplitude. Detail modulation is bounded by one.
        let root_bound = profile.terrain_scale.map_or_else(
            || {
                radius_m
                    * PROFILE_BANDS
                        .iter()
                        .map(|(_, amplitude)| amplitude)
                        .sum::<f64>()
                    * 0.5
            },
            |(_, amplitude_m)| amplitude_m * 0.5,
        );
        let detail_bound: f64 = profile
            .detail_layers
            .iter()
            .map(|layer| layer.amplitude_m * 0.5)
            .sum();
        let bound_m = (root_bound + detail_bound)
            .mul_add(1.0 + 32.0 * f64::EPSILON, 0.0)
            .next_up();
        if !bound_m.is_finite() || bound_m >= 0.1 * radius_m {
            return Err(TerrainError::InvalidRadius);
        }
        Ok(Self {
            profile,
            radius_m,
            bound_m,
        })
    }

    pub(super) fn absolute_height_bound_m(&self) -> f64 {
        self.bound_m
    }

    pub(super) fn resident_heap_bytes(&self) -> usize {
        self.profile.resident_heap_bytes()
    }

    #[cfg(test)]
    pub(super) fn evaluate(&self, direction: DVec3) -> Result<MoonProfileSample, TerrainError> {
        self.evaluate_observed(direction, &mut NoopProfileObserver)
    }

    pub(super) fn evaluate_observed<O: ProfileObserver>(
        &self,
        direction: DVec3,
        observer: &mut O,
    ) -> Result<MoonProfileSample, TerrainError> {
        if !direction.is_finite() || direction.length_squared() < 1.0e-24 {
            return Err(TerrainError::NonFiniteResult);
        }
        let n = direction.normalize();
        let setup_timer = observer.start(ProfilePhase::WeightAndMaterial);
        let (weights, weight_gradients) = triplanar_weights(n)?;
        observer.finish(ProfilePhase::WeightAndMaterial, setup_timer);
        observer.record_weight_gradient_vectors(3);
        let mut macro_height = 0.0;
        let mut macro_gradient = DVec3::ZERO;
        let mut coarse = 0.0;
        let mut coarse_gradient = DVec3::ZERO;
        let macro_timer = observer.start(ProfilePhase::Macro);
        if let Some((footprint_m, amplitude_m)) = self.profile.terrain_scale {
            let frequency = 2.0 * self.radius_m / footprint_m;
            let (sampled, sampled_gradient) = sample_triplanar(
                self.profile.grid(),
                self.profile.kernel,
                n,
                weights,
                weight_gradients,
                frequency,
                observer,
            );
            let centered = sampled - 0.5;
            macro_height = centered * amplitude_m;
            observer.record_gradient_accumulation_terms(1);
            macro_gradient = sampled_gradient * amplitude_m;
            coarse = sampled;
            coarse_gradient = sampled_gradient;
        } else {
            for (band_index, (frequency, amplitude)) in PROFILE_BANDS.iter().copied().enumerate() {
                let (sampled, sampled_gradient) = sample_triplanar(
                    self.profile.grid(),
                    self.profile.kernel,
                    n,
                    weights,
                    weight_gradients,
                    frequency,
                    observer,
                );
                let centered = sampled - 0.5;
                let scale = self.radius_m * amplitude;
                macro_height += centered * scale;
                observer.record_gradient_accumulation_terms(1);
                macro_gradient += sampled_gradient * scale;
                if band_index == 0 {
                    coarse = sampled;
                    coarse_gradient = sampled_gradient;
                }
            }
        }
        observer.finish(ProfilePhase::Macro, macro_timer);

        for (layer_index, layer) in self.profile.detail_layers.iter().enumerate() {
            let detail_timer = observer.start(ProfilePhase::Detail(layer_index));
            observer.record_detail_layer();
            let frequency = 2.0 * self.radius_m / layer.footprint_m;
            let (sampled, sampled_gradient) = sample_triplanar(
                layer.profile.grid(),
                layer.profile.kernel,
                n,
                weights,
                weight_gradients,
                frequency,
                observer,
            );
            let centered = sampled - 0.5;
            let geology = 0.35 + 0.65 * coarse;
            macro_height += centered * layer.amplitude_m * geology;
            observer.record_gradient_accumulation_terms(2);
            macro_gradient += (sampled_gradient * geology + coarse_gradient * (centered * 0.65))
                * layer.amplitude_m;
            observer.finish(ProfilePhase::Detail(layer_index), detail_timer);
        }

        // Regional material expression is sampled from the coarse source band;
        // it never contributes to geometric height or its derivative.
        // Low basin floors receive the dark resurfaced channel; higher rims and
        // highlands retain regolith/substrate. This is fixture presentation data.
        let composition_timer = observer.start(ProfilePhase::WeightAndMaterial);
        let plains_t = ((0.55 - coarse) / 0.28).clamp(0.0, 1.0);
        let plains = plains_t * plains_t * (3.0 - 2.0 * plains_t);
        let ejecta = (0.12 * (1.0 - plains)).clamp(0.0, 1.0);
        let weights = [
            0.56 * (1.0 - plains),
            0.28 * (1.0 - plains) - ejecta * 0.12,
            0.20 + 0.72 * plains,
            ejecta,
        ];
        let total: f64 = weights.iter().sum();
        let weights = weights.map(|weight| weight / total);
        if !macro_height.is_finite() || !macro_gradient.is_finite() {
            observer.finish(ProfilePhase::WeightAndMaterial, composition_timer);
            return Err(TerrainError::NonFiniteResult);
        }
        let result = MoonProfileSample {
            height_m: macro_height,
            gradient_m: macro_gradient,
            weights,
        };
        observer.finish(ProfilePhase::WeightAndMaterial, composition_timer);
        Ok(result)
    }
}

#[derive(Debug, Clone, Copy)]
pub(super) struct ProfileValue {
    pub(super) value: f64,
    pub(super) gradient_uv: [f64; 2],
}

pub(super) fn projection(n: DVec3, axis: usize) -> (f64, f64) {
    match axis {
        0 => (n.y, n.z),
        1 => (n.x, n.z),
        _ => (n.x, n.y),
    }
}

pub(super) fn sample_triplanar<O: ProfileObserver>(
    grid: ProfileGrid<'_>,
    kernel: ProfileKernel,
    n: DVec3,
    weights: [f64; 3],
    weight_gradients: [DVec3; 3],
    frequency: f64,
    observer: &mut O,
) -> (f64, DVec3) {
    observer.record_triplanar_sample();
    let mut sampled = 0.0;
    let mut sampled_gradient = DVec3::ZERO;
    for axis in 0..3 {
        let (a, b) = projection(n, axis);
        let s = grid.sample(kernel, a, b, frequency, observer);
        sampled += weights[axis] * s.value;
        let chart_gradient = match axis {
            0 => DVec3::new(0.0, s.gradient_uv[0], s.gradient_uv[1]),
            1 => DVec3::new(s.gradient_uv[0], 0.0, s.gradient_uv[1]),
            _ => DVec3::new(s.gradient_uv[0], s.gradient_uv[1], 0.0),
        };
        sampled_gradient += weight_gradients[axis] * s.value + chart_gradient * weights[axis];
        observer.record_gradient_axis_combination();
    }
    (sampled, sampled_gradient)
}

pub(super) fn triplanar_weights(n: DVec3) -> Result<([f64; 3], [DVec3; 3]), TerrainError> {
    let raw = [n.x.powi(4), n.y.powi(4), n.z.powi(4)];
    let total = raw.iter().sum::<f64>();
    if !total.is_finite() || total <= 0.0 {
        return Err(TerrainError::NonFiniteResult);
    }
    let raw_gradients = [
        DVec3::new(4.0 * n.x.powi(3), 0.0, 0.0),
        DVec3::new(0.0, 4.0 * n.y.powi(3), 0.0),
        DVec3::new(0.0, 0.0, 4.0 * n.z.powi(3)),
    ];
    let total_gradient = raw_gradients.iter().copied().sum::<DVec3>();
    let weights = raw.map(|value| value / total);
    let gradients = std::array::from_fn(|i| {
        (raw_gradients[i] * total - total_gradient * raw[i]) / total.powi(2)
    });
    Ok((weights, gradients))
}

impl TerrainHeightProfile {
    #[cfg(test)]
    fn sample_periodic(&self, u: f64, v: f64, frequency: f64) -> ProfileValue {
        self.sample_periodic_observed(u, v, frequency, &mut NoopProfileObserver)
    }

    #[cfg(test)]
    fn sample_periodic_observed<O: ProfileObserver>(
        &self,
        u: f64,
        v: f64,
        frequency: f64,
        observer: &mut O,
    ) -> ProfileValue {
        self.grid().sample(self.kernel, u, v, frequency, observer)
    }

    pub(super) fn grid(&self) -> ProfileGrid<'_> {
        ProfileGrid {
            width: self.width,
            height: self.height,
            values: &self.values,
            sample_range: self.sample_range,
        }
    }

    #[cfg(test)]
    fn value(&self, x: u32, y: u32) -> f64 {
        self.grid().value(x, y)
    }

    pub(super) fn shared_values(&self) -> &Arc<[u16]> {
        &self.values
    }

    pub(super) fn is_cubic_bspline(&self) -> bool {
        self.kernel == ProfileKernel::CubicBSpline
    }
}

/// Borrowed periodic u16 sample grid. The complete profile and its derived
/// pre-filtered mip levels share this exact reconstruction arithmetic.
#[derive(Clone, Copy)]
pub(super) struct ProfileGrid<'a> {
    pub width: u32,
    pub height: u32,
    pub values: &'a [u16],
    pub sample_range: [u16; 2],
}

impl ProfileGrid<'_> {
    pub(super) fn sample<O: ProfileObserver>(
        self,
        kernel: ProfileKernel,
        u: f64,
        v: f64,
        frequency: f64,
        observer: &mut O,
    ) -> ProfileValue {
        if self.sample_range[0] == self.sample_range[1] {
            return ProfileValue {
                value: 0.5,
                gradient_uv: [0.0, 0.0],
            };
        }
        if kernel == ProfileKernel::CubicBSpline {
            return self.sample_periodic_cubic_bspline(u, v, frequency, observer);
        }
        observer.record_profile_taps(4, 4);
        let x = (u * 0.5 + 0.5) * frequency * self.width as f64;
        let y = (v * 0.5 + 0.5) * frequency * self.height as f64;
        let x_wrapped = x.rem_euclid(self.width as f64);
        let y_wrapped = y.rem_euclid(self.height as f64);
        let x0 = x_wrapped.floor() as u32;
        let y0 = y_wrapped.floor() as u32;
        let x1 = (x0 + 1) % self.width;
        let y1 = (y0 + 1) % self.height;
        let tx = x_wrapped - x0 as f64;
        let ty = y_wrapped - y0 as f64;
        let sx = smoothstep(tx);
        let sy = smoothstep(ty);
        let dsx = 6.0 * tx * (1.0 - tx);
        let dsy = 6.0 * ty * (1.0 - ty);
        let a = self.value(x0, y0);
        let b = self.value(x1, y0);
        let c = self.value(x0, y1);
        let d = self.value(x1, y1);
        let low = a + (b - a) * sx;
        let high = c + (d - c) * sx;
        let value = low + (high - low) * sy;
        let dx =
            ((b - a) * (1.0 - sy) + (d - c) * sy) * dsx * (0.5 * frequency * self.width as f64);
        let dy = (high - low) * dsy * (0.5 * frequency * self.height as f64);
        ProfileValue {
            value,
            gradient_uv: [dx, dy],
        }
    }

    fn sample_periodic_cubic_bspline<O: ProfileObserver>(
        self,
        u: f64,
        v: f64,
        frequency: f64,
        observer: &mut O,
    ) -> ProfileValue {
        observer.record_profile_taps(16, 16);
        let x = (u * 0.5 + 0.5) * frequency * self.width as f64;
        let y = (v * 0.5 + 0.5) * frequency * self.height as f64;
        let x_wrapped = x.rem_euclid(self.width as f64);
        let y_wrapped = y.rem_euclid(self.height as f64);
        let x_floor = x_wrapped.floor() as i64;
        let y_floor = y_wrapped.floor() as i64;
        let tx = x_wrapped - x_floor as f64;
        let ty = y_wrapped - y_floor as f64;
        let (wx, dwx) = cubic_bspline_weights(tx);
        let (wy, dwy) = cubic_bspline_weights(ty);
        let mut value = 0.0;
        let mut dx = 0.0;
        let mut dy = 0.0;
        for row in 0..4 {
            let py = (y_floor + row as i64 - 1).rem_euclid(self.height as i64) as u32;
            for column in 0..4 {
                let px = (x_floor + column as i64 - 1).rem_euclid(self.width as i64) as u32;
                let sample = self.value(px, py);
                value += wx[column] * wy[row] * sample;
                dx += dwx[column] * wy[row] * sample;
                dy += wx[column] * dwy[row] * sample;
            }
        }
        ProfileValue {
            value,
            gradient_uv: [
                dx * (0.5 * frequency * self.width as f64),
                dy * (0.5 * frequency * self.height as f64),
            ],
        }
    }

    fn value(self, x: u32, y: u32) -> f64 {
        let [low, high] = self.sample_range;
        if low == high {
            return 0.5;
        }
        f64::from(self.values[(y * self.width + x) as usize] - low) / f64::from(high - low)
    }
}

fn cubic_bspline_weights(t: f64) -> ([f64; 4], [f64; 4]) {
    let t2 = t * t;
    let t3 = t2 * t;
    let one_minus_t = 1.0 - t;
    let weights = [
        one_minus_t.powi(3) / 6.0,
        (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0,
        (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0,
        t3 / 6.0,
    ];
    let derivatives = [
        -0.5 * one_minus_t.powi(2),
        1.5 * t2 - 2.0 * t,
        -1.5 * t2 + t + 0.5,
        0.5 * t2,
    ];
    (weights, derivatives)
}

fn smoothstep(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::{
        SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
    };
    use astrum_math::{Direction3, surface::SurfaceLocation};

    fn profile() -> TerrainHeightProfile {
        let side = 32_u32;
        let mut bytes = Vec::with_capacity((side * side * 2) as usize);
        for y in 0..side {
            for x in 0..side {
                let fx = std::f64::consts::TAU * x as f64 / side as f64;
                let fy = std::f64::consts::TAU * y as f64 / side as f64;
                let sample = (0.5 + 0.45 * fx.sin() * fy.cos()) * 65535.0;
                bytes.extend_from_slice(&(sample.round() as u16).to_le_bytes());
            }
        }
        TerrainHeightProfile::from_u16_le(side, side, &bytes).unwrap()
    }

    fn sized_profile(width: u32, height: u32, salt: u16) -> TerrainHeightProfile {
        let mut bytes = Vec::with_capacity((width * height * 2) as usize);
        for y in 0..height {
            for x in 0..width {
                let value = ((x * 173 + y * 911) as u16).wrapping_add(salt);
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        TerrainHeightProfile::from_u16_le(width, height, &bytes).unwrap()
    }

    fn location(direction: DVec3) -> SurfaceLocation {
        SurfaceLocation::new(Direction3::try_new(direction).unwrap())
    }

    #[test]
    fn profile_loader_is_bounded_and_content_identity_tracks_samples() {
        let source = profile();
        assert_eq!(source.width(), 32);
        assert_eq!(source.height(), 32);
        assert_eq!(source.identity_words().len(), 6);
        let mut changed = Vec::new();
        for y in 0..32_u16 {
            for x in 0..32_u16 {
                let value = if x == 7 && y == 13 { u16::MAX } else { 0x4000 };
                changed.extend_from_slice(&value.to_le_bytes());
            }
        }
        let original = TerrainHeightProfile::from_u16_le(32, 32, &changed).unwrap();
        changed[2 * (13 * 32 + 7)] ^= 1;
        let edited = TerrainHeightProfile::from_u16_le(32, 32, &changed).unwrap();
        assert_ne!(original.identity_words(), edited.identity_words());
        assert_eq!(
            TerrainHeightProfile::from_u16_le(32, 32, &changed),
            TerrainHeightProfile::from_u16_le(32, 32, &changed)
        );
        assert_eq!(
            TerrainHeightProfile::from_u16_le(1, 32, &[0; 64]),
            Err(TerrainError::InvalidHeightProfile)
        );
        assert_eq!(
            TerrainHeightProfile::from_u16_le(32, 32, &[0; 3]),
            Err(TerrainError::InvalidHeightProfile)
        );
    }

    #[test]
    fn profile_range_normalization_has_explicit_bounded_amplitudes() {
        let values = [31000u16, 32000, 33000, 34000];
        let bytes: Vec<_> = values.into_iter().flat_map(u16::to_le_bytes).collect();
        let profile = TerrainHeightProfile::from_u16_le(2, 2, &bytes).unwrap();
        assert_eq!(profile.sample_range(), [31000, 34000]);
        assert_eq!(profile.value(0, 0), 0.0);
        assert_eq!(profile.value(1, 1), 1.0);
        let flat = TerrainHeightProfile::from_u16_le(2, 2, &[0; 8]).unwrap();
        assert_eq!(flat.value(0, 0), 0.5);
    }

    #[test]
    fn triplanar_and_periodic_charts_are_continuous_at_cube_faces() {
        let profile = profile();
        let layered = profile
            .clone()
            .with_detail_layer(sized_profile(8, 4, 20), 12.0, 1.5)
            .unwrap();
        let field = MoonProfileField::new(layered, 1_737_000.0).unwrap();
        for direction in [
            DVec3::new(1.0, 1.0, 0.31),
            DVec3::new(-1.0, 1.0, -0.22),
            DVec3::new(0.19, -1.0, 1.0),
            DVec3::new(1.0, 0.17, -1.0),
        ] {
            let center = direction.normalize();
            let tangent = center.cross(DVec3::Z).normalize();
            let epsilon = 1.0e-8;
            let before = field
                .evaluate((center - tangent * epsilon).normalize())
                .unwrap();
            let after = field
                .evaluate((center + tangent * epsilon).normalize())
                .unwrap();
            assert!((after.height_m - before.height_m).abs() < 0.1);
            assert!((after.gradient_m - before.gradient_m).length().is_finite());
        }
        let a = profile.sample_periodic(0.23, -0.41, 8.0);
        let b = profile.sample_periodic(0.23 + 2.0 / 8.0, -0.41, 8.0);
        assert!((a.value - b.value).abs() < 1.0e-12);
        assert!((a.gradient_uv[0] - b.gradient_uv[0]).abs() < 1.0e-9);
        assert!((a.gradient_uv[1] - b.gradient_uv[1]).abs() < 1.0e-9);
    }

    #[test]
    fn profile_analytic_gradient_matches_finite_difference_at_both_scales() {
        let profile = profile();
        for radius in [109_000.0, 1_737_000.0] {
            let field = MoonProfileField::new(profile.clone(), radius).unwrap();
            let n = DVec3::new(0.37, -0.51, 0.78).normalize();
            let tangent = n.cross(DVec3::Y).normalize();
            let epsilon = 2.0e-7;
            let center = field.evaluate(n).unwrap();
            let plus = field.evaluate((n + tangent * epsilon).normalize()).unwrap();
            let minus = field.evaluate((n - tangent * epsilon).normalize()).unwrap();
            let finite_difference = (plus.height_m - minus.height_m) / (2.0 * epsilon);
            let analytic = center.gradient_m.dot(tangent);
            assert!(
                (finite_difference - analytic).abs() < 0.08,
                "radius={radius}: numeric={finite_difference}, analytic={analytic}"
            );
            assert!(center.height_m.abs() <= 0.1 * radius);
            assert!(center.gradient_m.is_finite());
        }
    }

    #[test]
    fn profile_algorithm_requires_payload_and_preserves_other_algorithm_rules() {
        let identity = TerrainIdentity(0x1234);
        let seed = TerrainSeed(0x9876);
        let empty = SurfaceDefinition::generated(identity, seed, SurfaceAlgorithm::MoonProfileV1);
        assert!(SurfaceGenerator::new(&empty, 109_000.0).is_err());
        assert!(empty.clone().with_height_profile(profile()).is_ok());
        let old = SurfaceDefinition::generated(identity, seed, SurfaceAlgorithm::MoonFieldsV1);
        assert!(old.with_height_profile(profile()).is_err());
        let attached = empty.with_height_profile(profile()).unwrap();
        assert!(SurfaceGenerator::new(&attached, 109_000.0).is_ok());
        let sample = SurfaceGenerator::new(&attached, 109_000.0)
            .unwrap()
            .evaluate_point(location(DVec3::new(0.2, 0.6, -0.7)))
            .unwrap();
        assert!(sample.normal().is_finite());
        assert!(sample.material_weights()[2] > 0.0);
    }

    #[test]
    fn detail_layers_have_ordered_exact_identity_and_read_only_metadata() {
        let root = profile();
        assert_eq!(root.identity_words().len(), 6);
        let small = sized_profile(8, 4, 10);
        let large = sized_profile(16, 8, 10);
        let one = root
            .clone()
            .with_detail_layer(small.clone(), 14.0, 1.0)
            .unwrap();
        let changed_footprint = root
            .clone()
            .with_detail_layer(small.clone(), 14.5, 1.0)
            .unwrap();
        let changed_amplitude = root
            .clone()
            .with_detail_layer(small.clone(), 14.0, 1.5)
            .unwrap();
        let positive_zero = root
            .clone()
            .with_detail_layer(small.clone(), 14.0, 0.0)
            .unwrap();
        let negative_zero = root
            .clone()
            .with_detail_layer(small.clone(), 14.0, -0.0)
            .unwrap();
        let changed_map = root
            .clone()
            .with_detail_layer(sized_profile(8, 4, 11), 14.0, 1.0)
            .unwrap();
        let ordered = root
            .clone()
            .with_detail_layer(small.clone(), 14.0, 1.0)
            .unwrap()
            .with_detail_layer(large.clone(), 7.0, 0.5)
            .unwrap();
        let reversed = root
            .clone()
            .with_detail_layer(large, 7.0, 0.5)
            .unwrap()
            .with_detail_layer(small, 14.0, 1.0)
            .unwrap();
        for changed in [changed_footprint, changed_amplitude, changed_map] {
            assert_ne!(one.identity_words(), changed.identity_words());
            assert_ne!(one, changed);
        }
        assert_ne!(ordered.identity_words(), reversed.identity_words());
        assert_ne!(ordered, reversed);
        assert_ne!(
            positive_zero.identity_words(),
            negative_zero.identity_words()
        );
        assert_ne!(positive_zero, negative_zero);
        assert_eq!(one.detail_layer_count(), 1);
        let layer = one.detail_layer(0).unwrap();
        assert_eq!(layer.footprint_m(), 14.0);
        assert_eq!(layer.amplitude_m(), 1.0);
        assert_eq!(layer.profile().width(), 8);
        assert_eq!(layer.profile().height(), 4);
        assert!(one.detail_layer(1).is_none());
    }

    #[test]
    fn detail_layer_validation_rejects_nesting_limits_and_invalid_physical_scales() {
        let root = profile();
        let nested = sized_profile(8, 8, 2)
            .with_detail_layer(sized_profile(4, 4, 3), 3.0, 0.5)
            .unwrap();
        assert!(root.clone().with_detail_layer(nested, 8.0, 1.0).is_err());
        let once = root
            .clone()
            .with_detail_layer(sized_profile(8, 8, 4), 8.0, 1.0)
            .unwrap();
        let twice = once
            .clone()
            .with_detail_layer(sized_profile(8, 8, 5), 4.0, 2.0)
            .unwrap();
        assert_eq!(twice.detail_layer_count(), 2);
        assert!(
            twice
                .clone()
                .with_detail_layer(sized_profile(8, 8, 6), 2.0, 0.1)
                .is_err()
        );
        for (footprint, amplitude) in [
            (f64::NAN, 0.1),
            (f64::INFINITY, 0.1),
            (0.0, 0.1),
            (0.249, 0.1),
            (1.0, f64::NAN),
            (1.0, f64::INFINITY),
            (1.0, -0.01),
            (1.0, 2.01),
        ] {
            assert!(
                root.clone()
                    .with_detail_layer(sized_profile(4, 4, 0), footprint, amplitude)
                    .is_err()
            );
        }
        // The specification permits zero amplitude; its map remains identified.
        assert!(
            root.with_detail_layer(sized_profile(4, 4, 0), 0.25, 0.0)
                .is_ok()
        );
    }

    #[test]
    fn physical_detail_gradients_bounds_and_shared_map_accounting_hold_at_both_scales() {
        let base = profile();
        let layered = base
            .clone()
            .with_detail_layer(base.clone(), 17.0, 1.6)
            .unwrap()
            .with_detail_layer(sized_profile(16, 8, 0x1234), 3.5, 1.2)
            .unwrap();
        let identity = TerrainIdentity(0x1234);
        let seed = TerrainSeed(0x9876);
        let definition =
            SurfaceDefinition::generated(identity, seed, SurfaceAlgorithm::MoonProfileV1)
                .with_height_profile(layered.clone())
                .unwrap();
        for radius in [109_000.0, 1_737_000.0] {
            let generator = SurfaceGenerator::new(&definition, radius).unwrap();
            let n = DVec3::new(0.37, -0.51, 0.78).normalize();
            let tangent = n.cross(DVec3::Y).normalize();
            let physical_step_m = 0.001 * 3.5;
            let epsilon = physical_step_m / radius;
            let center = generator.evaluate_point(location(n)).unwrap();
            let plus = generator
                .evaluate_point(location((n + tangent * epsilon).normalize()))
                .unwrap();
            let minus = generator
                .evaluate_point(location((n - tangent * epsilon).normalize()))
                .unwrap();
            let finite_difference =
                (plus.terrain().height_m() - minus.terrain().height_m()) / (2.0 * epsilon);
            let analytic = center
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .dot(tangent);
            assert!(
                (finite_difference - analytic).abs() < analytic.abs() * 0.005 + 0.03,
                "radius={radius}: numeric={finite_difference}, analytic={analytic}"
            );
            assert!(center.terrain().height_m().abs() <= 0.1 * radius);
            assert!(center.normal().is_finite());
            let profile_bound = radius * PROFILE_BANDS.iter().map(|(_, a)| a).sum::<f64>() * 0.5
                + (1.6 + 1.2) * 0.5;
            assert!(generator.conservative_absolute_height_bound_m() >= profile_bound);
            assert!(generator.conservative_absolute_height_bound_m() < 0.1 * radius);
        }

        let one_map_with_layer = base.clone().with_detail_layer(base, 17.0, 1.6).unwrap();
        let definition =
            SurfaceDefinition::generated(identity, seed, SurfaceAlgorithm::MoonProfileV1)
                .with_height_profile(one_map_with_layer)
                .unwrap();
        let generator = SurfaceGenerator::new(&definition, 109_000.0).unwrap();
        assert_eq!(
            generator.resident_heap_bytes(),
            32 * 32 * std::mem::size_of::<u16>()
                + std::mem::size_of::<TerrainHeightDetailLayer>()
                + super::super::PREPARATION_STORE_OWNER_BYTES
        );
        assert!(SurfaceGenerator::working_heap_bound_bytes() >= MAX_PROFILE_WORKING_HEAP_BYTES);
    }

    #[test]
    fn cubic_bspline_kernel_identity_and_default_kernel_compatibility_are_explicit() {
        let default = profile();
        let cubic = default.clone().with_cubic_bspline();
        assert_eq!(default.kernel_name(), "smoothstep-bilinear");
        assert_eq!(cubic.kernel_name(), "cubic-periodic-bspline");
        assert_eq!(default.identity_words().len(), 6);
        assert_eq!(default, profile());
        assert_ne!(default, cubic);
        assert_ne!(default.identity_words(), cubic.identity_words());

        let cubic_detail = sized_profile(8, 4, 0x21).with_cubic_bspline();
        let with_cubic_detail = default
            .clone()
            .with_detail_layer(cubic_detail, 8.0, 1.0)
            .unwrap();
        assert_ne!(default.identity_words(), with_cubic_detail.identity_words());
        assert_eq!(
            with_cubic_detail
                .detail_layer(0)
                .unwrap()
                .profile()
                .kernel_name(),
            "cubic-periodic-bspline"
        );
    }

    #[test]
    fn cubic_bspline_weights_partition_and_samples_remain_inside_source_range() {
        for step in 0..=100 {
            let t = step as f64 / 100.0;
            let (weights, derivatives) = cubic_bspline_weights(t);
            assert!(weights.iter().all(|weight| *weight >= 0.0));
            assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1.0e-14);
            assert!(derivatives.iter().all(|derivative| derivative.is_finite()));
            assert!(derivatives.iter().sum::<f64>().abs() < 1.0e-14);
        }
        let mut flat_bytes = Vec::new();
        for _ in 0..16 {
            flat_bytes.extend_from_slice(&0x1234u16.to_le_bytes());
        }
        let flat = TerrainHeightProfile::from_u16_le(4, 4, &flat_bytes)
            .unwrap()
            .with_cubic_bspline();
        for u_step in -20..=20 {
            for v_step in -20..=20 {
                let sample =
                    flat.sample_periodic(u_step as f64 * 0.071, v_step as f64 * 0.063, 7.25);
                assert_eq!(sample.value, 0.5);
                assert_eq!(sample.gradient_uv, [0.0, 0.0]);
            }
        }

        let varying = profile().with_cubic_bspline();
        for u_step in -50..=50 {
            for v_step in -50..=50 {
                let sample =
                    varying.sample_periodic(u_step as f64 * 0.017, v_step as f64 * 0.019, 11.0);
                assert!((0.0..=1.0).contains(&sample.value));
                assert!(
                    sample
                        .gradient_uv
                        .iter()
                        .all(|component| component.is_finite())
                );
            }
        }
    }

    #[test]
    fn cubic_bspline_periodic_seams_and_analytic_gradients_are_smooth_at_both_scales() {
        let root = profile().with_cubic_bspline();
        let detail = sized_profile(16, 8, 0x3456).with_cubic_bspline();
        let profile = root.with_detail_layer(detail, 5.0, 1.4).unwrap();
        let epsilon_u = 1.0e-7;
        let left = profile.sample_periodic(-1.0 - epsilon_u, -0.28, 9.0);
        let right = profile.sample_periodic(-1.0 + epsilon_u, -0.28, 9.0);
        assert!((left.value - right.value).abs() < 1.0e-5);
        assert!((left.gradient_uv[0] - right.gradient_uv[0]).abs() < 1.0e-3);
        assert!((left.gradient_uv[1] - right.gradient_uv[1]).abs() < 1.0e-3);

        let identity = TerrainIdentity(0x1234);
        let seed = TerrainSeed(0x9876);
        let definition =
            SurfaceDefinition::generated(identity, seed, SurfaceAlgorithm::MoonProfileV1)
                .with_height_profile(profile)
                .unwrap();
        for radius in [109_000.0, 1_737_000.0] {
            let generator = SurfaceGenerator::new(&definition, radius).unwrap();
            let n = DVec3::new(0.37, -0.51, 0.78).normalize();
            let tangent = n.cross(DVec3::Y).normalize();
            let epsilon = 0.01 * 5.0 / radius;
            let center = generator.evaluate_point(location(n)).unwrap();
            let plus = generator
                .evaluate_point(location((n + tangent * epsilon).normalize()))
                .unwrap();
            let minus = generator
                .evaluate_point(location((n - tangent * epsilon).normalize()))
                .unwrap();
            let finite_difference =
                (plus.terrain().height_m() - minus.terrain().height_m()) / (2.0 * epsilon);
            let analytic = center
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .dot(tangent);
            assert!(
                (finite_difference - analytic).abs() < analytic.abs() * 0.002 + 0.03,
                "radius={radius}: numeric={finite_difference}, analytic={analytic}"
            );
        }
    }

    #[test]
    fn physical_root_scale_has_exact_identity_validation_bounds_and_radius_invariant_local_scale() {
        let default = profile();
        let scaled = default
            .clone()
            .with_cubic_bspline()
            .with_terrain_scale(1.0, 800.0)
            .unwrap();
        assert_eq!(default.identity_words().len(), 6);
        assert_eq!(default.terrain_scale(), None);
        assert_eq!(scaled.terrain_scale(), Some((1.0, 800.0)));
        assert_ne!(default, scaled);
        assert_ne!(default.identity_words(), scaled.identity_words());
        assert_eq!(
            scaled,
            profile()
                .with_cubic_bspline()
                .with_terrain_scale(1.0, 800.0)
                .unwrap()
        );
        assert_ne!(
            scaled.identity_words(),
            default
                .clone()
                .with_cubic_bspline()
                .with_terrain_scale(1.5, 800.0)
                .unwrap()
                .identity_words()
        );
        assert_ne!(
            scaled.identity_words(),
            default
                .clone()
                .with_cubic_bspline()
                .with_terrain_scale(1.0, 801.0)
                .unwrap()
                .identity_words()
        );
        assert_ne!(
            default.clone().with_terrain_scale(24.0, 0.0).unwrap(),
            default.with_terrain_scale(24.0, -0.0).unwrap()
        );

        for (footprint, amplitude) in [
            (f64::NAN, 1.0),
            (f64::INFINITY, 1.0),
            (0.249, 1.0),
            (1.0e8 + 1.0, 1.0),
            (1.0, f64::NAN),
            (1.0, f64::INFINITY),
            (1.0, -0.01),
            (1.0, 2000.01),
        ] {
            assert!(profile().with_terrain_scale(footprint, amplitude).is_err());
        }
        assert!(profile().with_terrain_scale(0.25, 0.0).is_ok());
        assert!(profile().with_terrain_scale(1.0e8, 2000.0).is_ok());

        let detail_profile = profile().with_terrain_scale(20.0, 400.0).unwrap();
        assert!(
            profile()
                .with_detail_layer(detail_profile, 12.0, 1.0)
                .is_err()
        );

        let identity = TerrainIdentity(0x1234);
        let seed = TerrainSeed(0x9876);
        let definition =
            SurfaceDefinition::generated(identity, seed, SurfaceAlgorithm::MoonProfileV1)
                .with_height_profile(scaled)
                .unwrap();
        // At the projection pole the dominant chart starts at profile texel (0,0).
        // Integer radii align that sample phase here, so the metre-scale deltas can
        // be compared directly without asserting global phase equality elsewhere.
        let n = DVec3::Z;
        let tangent = DVec3::X;
        let physical_step_m = 0.005;
        let mut height_deltas = Vec::new();
        for radius in [109_000.0, 1_737_000.0] {
            let generator = SurfaceGenerator::new(&definition, radius).unwrap();
            let center = generator.evaluate_point(location(n)).unwrap();
            let plus = generator
                .evaluate_point(location(
                    (n + tangent * (physical_step_m / radius)).normalize(),
                ))
                .unwrap();
            let minus = generator
                .evaluate_point(location(
                    (n - tangent * (physical_step_m / radius)).normalize(),
                ))
                .unwrap();
            let delta = plus.terrain().height_m() - minus.terrain().height_m();
            height_deltas.push(delta);
            let finite_difference = delta / (2.0 * physical_step_m / radius);
            let analytic = center
                .terrain()
                .tangent_gradient_m_per_unit_direction()
                .dot(tangent);
            assert!(
                (finite_difference - analytic).abs() < analytic.abs() * 0.01 + 0.03,
                "radius={radius}: numeric={finite_difference}, analytic={analytic}"
            );
            assert!(center.terrain().height_m().abs() <= 400.0);
            assert!(generator.conservative_absolute_height_bound_m() >= 400.0);
            assert!(generator.conservative_absolute_height_bound_m() < 0.1 * radius);
        }
        assert!(
            (height_deltas[0] - height_deltas[1]).abs() < 0.02,
            "local height deltas differ across radii: {height_deltas:?}"
        );
    }
}
