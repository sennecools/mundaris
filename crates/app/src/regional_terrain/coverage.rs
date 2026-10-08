//! Bounded screen-space estimates of drawable terrain coverage.
//!
//! The sampler casts one perspective ray through each of 48 × 48 fixed screen
//! strata and intersects a reference sphere. It is deliberately a diagnostic
//! proxy: displacement, partial terrain occlusion, and rasterization are not
//! certified by this mask.

use std::collections::HashMap;
use std::sync::Arc;

use glam::{DMat3, DVec3};
use mundaris_math::surface::CubePatchAddress;
use mundaris_renderer::CelestialProjection;
use serde::Serialize;

const CELLS_PER_AXIS: usize = 3;
const SAMPLES_PER_CELL_AXIS: usize = 16;
const SCREEN_SAMPLE_COUNT: usize =
    CELLS_PER_AXIS * CELLS_PER_AXIS * SAMPLES_PER_CELL_AXIS * SAMPLES_PER_CELL_AXIS;
const USEFUL_PROXY_ERROR_PX: f64 = 4.0;
const MAX_CELL_CORRELATION_WITNESSES: usize = 4;
const MAX_COVERAGE_WITNESSES_PER_DIAGNOSTIC: usize =
    MAX_CELL_CORRELATION_WITNESSES * (CELLS_PER_AXIS * CELLS_PER_AXIS + 1);

/// Maximum logical heap payload for all ten serialized witness vectors in one
/// diagnostic, excluding allocator bookkeeping. Each vector reserves four slots.
pub(super) fn coverage_witness_heap_capacity_bytes() -> usize {
    MAX_COVERAGE_WITNESSES_PER_DIAGNOSTIC * std::mem::size_of::<CoverageAddressWitness>()
}

/// Whole-view and normalized 3 × 3 screen-space coverage estimate.
#[derive(Debug, Clone, Serialize)]
pub struct ScreenCoverageDiagnostic {
    /// Fixed diagnostic method identifier; the mask is approximate and uncertified.
    pub method: &'static str,
    /// Always false; this screen mask cannot certify rendered terrain visibility.
    pub certified: bool,
    /// Why reference-sphere sampling does not prove pixel-accurate coverage.
    pub limitation: &'static str,
    /// Whether observer, transform, radius, and proxy target inputs were valid.
    pub sampling_valid: bool,
    /// Stratified sample count along each axis of every 3 × 3 cell.
    pub samples_per_cell_axis: usize,
    /// Fixed useful-detail proxy threshold in pixels.
    pub useful_proxy_error_threshold_px: f64,
    /// Configured target proxy-error threshold in pixels.
    pub target_proxy_error_threshold_px: f64,
    /// Combined statistics over all visible reference-sphere samples.
    pub all_view: ScreenCoverageCell,
    /// Row-major: top-left through bottom-right, including empty cells.
    pub cells: [ScreenCoverageCell; 9],
    /// Per-drawable area estimated by this exact mask; omitted from serialized
    /// snapshots because cube addresses are runtime lookup keys, not schema data.
    #[serde(skip_serializing)]
    pub sampled_drawable_area_pixels: Arc<HashMap<CubePatchAddress, f64>>,
}

/// Statistics over reference-sphere terrain samples in one screen region.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ScreenCoverageCell {
    /// Reference-sphere terrain rays that intersect within this screen region.
    pub terrain_sample_count: usize,
    /// Estimated screen pixels occupied by the sampled reference-sphere terrain.
    pub estimated_terrain_area_pixels: f64,
    /// Fraction of terrain samples with a drawable leaf or drawable ancestor.
    pub fallback_covered_fraction: Option<f64>,
    /// Fraction of all terrain samples with a known proxy error at or below 4 px.
    pub useful_proxy_error_fraction: Option<f64>,
    /// Fraction of all terrain samples with a known proxy error at or below the
    /// configured target. Missing scores remain in the denominator.
    pub target_proxy_error_fraction: Option<f64>,
    /// Nearest-rank percentiles of known, finite, nonnegative proxy errors.
    pub proxy_error_p50_px: Option<f64>,
    pub proxy_error_p95_px: Option<f64>,
    pub proxy_error_max_px: Option<f64>,
    /// Terrain samples with no drawable fallback or with no valid error score.
    pub unresolved_proxy_sample_count: usize,
    /// Up to four deterministic worst drawable addresses represented by this
    /// reference-sphere mask. These correlate mask samples with selector work;
    /// they do not certify displaced-terrain pixel ownership.
    pub correlation_witnesses: Vec<CoverageAddressWitness>,
}

/// A bounded ray sample identifying one drawable address in a screen cell.
///
/// `normalized_screen_position` uses top-left viewport coordinates in [0, 1].
/// `reference_body_direction` is the unit body-fixed direction of the sampled
/// reference-sphere hit, not an authoritative displaced-terrain intersection.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CoverageAddressWitness {
    /// Stratified sample center in top-left-origin normalized viewport space.
    pub normalized_screen_position: [f64; 2],
    /// Unit direction from the body center through the reference-sphere hit.
    pub reference_body_direction: [f64; 3],
    /// Positive distance from the body-local observer along the sampled ray.
    pub reference_hit_distance_m: f64,
    /// Drawable leaf or ancestor selected by this reference-sphere address.
    pub drawable_address: CoverageDrawableAddress,
    /// None means the selected drawable had no finite, nonnegative proxy score.
    pub proxy_error_px: Option<f64>,
    /// Fixed strata assigned to this address in this cell, or across all cells
    /// for the all-view witness list.
    pub reference_sphere_sample_count: usize,
}

/// Serializable cube address for correlation with desired descendants.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CoverageDrawableAddress {
    /// Stable snake-case cube face.
    pub face: CoverageCubeFace,
    /// Cube quadtree level.
    pub level: u8,
    /// Horizontal cube-patch coordinate at `level`.
    pub x: u32,
    /// Vertical cube-patch coordinate at `level`.
    pub y: u32,
}

/// Cube-face name serialized in a stable snake-case form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CoverageCubeFace {
    PositiveX,
    NegativeX,
    PositiveY,
    NegativeY,
    PositiveZ,
    NegativeZ,
}

impl From<CubePatchAddress> for CoverageDrawableAddress {
    fn from(address: CubePatchAddress) -> Self {
        let face = match address.face() {
            mundaris_math::surface::CubeFace::PositiveX => CoverageCubeFace::PositiveX,
            mundaris_math::surface::CubeFace::NegativeX => CoverageCubeFace::NegativeX,
            mundaris_math::surface::CubeFace::PositiveY => CoverageCubeFace::PositiveY,
            mundaris_math::surface::CubeFace::NegativeY => CoverageCubeFace::NegativeY,
            mundaris_math::surface::CubeFace::PositiveZ => CoverageCubeFace::PositiveZ,
            mundaris_math::surface::CubeFace::NegativeZ => CoverageCubeFace::NegativeZ,
        };
        let [x, y] = address.coordinates();
        Self {
            face,
            level: address.level(),
            x,
            y,
        }
    }
}

/// Estimate visible drawable coverage without querying world surface authority.
///
/// Each map key is one drawable leaf. Its value is `None` when the drawable
/// leaf exists but its approximate error score is unavailable. The caller owns
/// construction and lifetime of this compact per-view lookup.
pub(super) fn screen_coverage_diagnostic(
    body_local_origin_m: DVec3,
    body_to_view: DMat3,
    projection: CelestialProjection,
    radius_m: f64,
    max_level: u8,
    drawable_leaf_errors_px: &HashMap<CubePatchAddress, Option<f64>>,
    target_error_px: f64,
) -> ScreenCoverageDiagnostic {
    let valid = body_local_origin_m.is_finite()
        && body_to_view.is_finite()
        && radius_m.is_finite()
        && radius_m > 0.0
        && target_error_px.is_finite()
        && target_error_px >= 0.0;

    let mut samples = [ScreenSample::default(); SCREEN_SAMPLE_COUNT];
    let [width, height] = projection.viewport();
    let sample_area_pixels = f64::from(width) * f64::from(height) / SCREEN_SAMPLE_COUNT as f64;
    let mut sampled_drawable_area_pixels = HashMap::new();
    if valid {
        let [origin_x, origin_y] = projection.origin();
        let level = max_level.min(30);

        for row in 0..CELLS_PER_AXIS {
            for column in 0..CELLS_PER_AXIS {
                let cell_index = row * CELLS_PER_AXIS + column;
                for sample_y in 0..SAMPLES_PER_CELL_AXIS {
                    for sample_x in 0..SAMPLES_PER_CELL_AXIS {
                        let sample_index =
                            cell_index * SAMPLES_PER_CELL_AXIS * SAMPLES_PER_CELL_AXIS
                                + sample_y * SAMPLES_PER_CELL_AXIS
                                + sample_x;
                        let normalized_x = (column as f64
                            + (sample_x as f64 + 0.5) / SAMPLES_PER_CELL_AXIS as f64)
                            / CELLS_PER_AXIS as f64;
                        let normalized_y = (row as f64
                            + (sample_y as f64 + 0.5) / SAMPLES_PER_CELL_AXIS as f64)
                            / CELLS_PER_AXIS as f64;
                        let pointer = [
                            f64::from(origin_x) + normalized_x * f64::from(width),
                            f64::from(origin_y) + normalized_y * f64::from(height),
                        ];

                        let hit = projection.unproject_ray(pointer).ok().and_then(|view_ray| {
                            let body_ray =
                                (body_to_view.transpose() * view_ray).normalize_or_zero();
                            let distance = nearest_positive_sphere_intersection(
                                body_local_origin_m,
                                body_ray,
                                radius_m,
                            )?;
                            if !hit_is_past_near_plane(view_ray, distance, projection.near_m()) {
                                return None;
                            }
                            Some(ReferenceSphereHit {
                                body_direction: (body_local_origin_m + body_ray * distance)
                                    .normalize_or_zero(),
                                distance_m: distance,
                            })
                        });
                        let (drawable_address, error_px) = hit
                            .and_then(|surface_hit| {
                                direction_to_patch(surface_hit.body_direction, level)
                            })
                            .and_then(|address| {
                                find_drawable_ancestor(address, drawable_leaf_errors_px)
                            })
                            .unwrap_or((None, None));
                        samples[sample_index] = ScreenSample {
                            cell_index: cell_index as u8,
                            terrain_hit: hit.is_some(),
                            fallback_covered: drawable_address.is_some(),
                            error_px,
                            normalized_screen_position: [normalized_x, normalized_y],
                            reference_body_direction: hit
                                .map_or(DVec3::ZERO, |surface_hit| surface_hit.body_direction),
                            reference_hit_distance_m: hit
                                .map_or(0.0, |surface_hit| surface_hit.distance_m),
                            drawable_address,
                        };
                        if hit.is_some()
                            && let Some(address) = drawable_address
                        {
                            *sampled_drawable_area_pixels.entry(address).or_default() +=
                                sample_area_pixels;
                        }
                    }
                }
            }
        }
    }

    let cell_witness_candidates = build_cell_witness_candidates(&samples);
    let (mut cells, mut all_view) =
        summarize_samples(&mut samples, target_error_px, sample_area_pixels);
    for (cell, candidates) in cells.iter_mut().zip(&cell_witness_candidates) {
        cell.correlation_witnesses = candidates.to_witness_vec();
    }
    all_view.correlation_witnesses = build_all_view_witnesses(&samples, &cell_witness_candidates);
    debug_assert!(
        all_view.correlation_witnesses.len()
            + cells
                .iter()
                .map(|cell| cell.correlation_witnesses.len())
                .sum::<usize>()
            <= MAX_COVERAGE_WITNESSES_PER_DIAGNOSTIC
    );

    ScreenCoverageDiagnostic {
        method: "reference_sphere_mask_48x48",
        certified: false,
        limitation: "Reference-sphere silhouette only; displacement, terrain occlusion, and raster coverage are approximate.",
        sampling_valid: valid,
        samples_per_cell_axis: SAMPLES_PER_CELL_AXIS,
        useful_proxy_error_threshold_px: USEFUL_PROXY_ERROR_PX,
        target_proxy_error_threshold_px: target_error_px,
        all_view,
        cells,
        sampled_drawable_area_pixels: Arc::new(sampled_drawable_area_pixels),
    }
}

#[derive(Clone, Copy, Default)]
struct ScreenSample {
    cell_index: u8,
    terrain_hit: bool,
    fallback_covered: bool,
    error_px: Option<f64>,
    normalized_screen_position: [f64; 2],
    reference_body_direction: DVec3,
    reference_hit_distance_m: f64,
    drawable_address: Option<CubePatchAddress>,
}

#[derive(Clone, Copy)]
struct ReferenceSphereHit {
    body_direction: DVec3,
    distance_m: f64,
}

#[derive(Clone, Copy)]
struct WitnessCandidate {
    address: CubePatchAddress,
    sample: ScreenSample,
    sample_count: usize,
}

#[derive(Clone, Copy)]
struct RankedWitnessCandidates {
    entries: [Option<WitnessCandidate>; MAX_CELL_CORRELATION_WITNESSES],
    len: usize,
}

impl RankedWitnessCandidates {
    fn from_entries(entries: [Option<WitnessCandidate>; MAX_CELL_CORRELATION_WITNESSES]) -> Self {
        let len = entries.iter().take_while(|entry| entry.is_some()).count();
        Self { entries, len }
    }

    fn iter(&self) -> impl Iterator<Item = &WitnessCandidate> {
        self.entries[..self.len].iter().flatten()
    }

    fn to_witness_vec(self) -> Vec<CoverageAddressWitness> {
        let mut witnesses = Vec::with_capacity(MAX_CELL_CORRELATION_WITNESSES);
        for candidate in self.iter() {
            witnesses.push(candidate.to_witness());
        }
        witnesses
    }
}

fn build_cell_witness_candidates(
    samples: &[ScreenSample; SCREEN_SAMPLE_COUNT],
) -> [RankedWitnessCandidates; 9] {
    std::array::from_fn(|cell_index| {
        let first = cell_index * SAMPLES_PER_CELL_AXIS * SAMPLES_PER_CELL_AXIS;
        let cell_samples = &samples[first..first + SAMPLES_PER_CELL_AXIS * SAMPLES_PER_CELL_AXIS];
        let mut top: [Option<WitnessCandidate>; MAX_CELL_CORRELATION_WITNESSES] =
            std::array::from_fn(|_| None);

        // The fixed 256-sample cell bounds this scan. Re-scan duplicate
        // addresses to retain the worst representative and full support count,
        // while keeping working storage to the four final candidates.
        for (sample_index, sample) in cell_samples.iter().enumerate() {
            if !sample.terrain_hit || !sample.fallback_covered {
                continue;
            }
            let Some(address) = sample.drawable_address else {
                continue;
            };
            if cell_samples[..sample_index]
                .iter()
                .any(|earlier| earlier.drawable_address == Some(address))
            {
                continue;
            }

            let mut candidate = WitnessCandidate {
                address,
                sample: *sample,
                sample_count: 0,
            };
            for same_address in cell_samples
                .iter()
                .filter(|other| other.drawable_address == Some(address))
            {
                candidate.sample_count += 1;
                if compare_witness_sample(same_address, address, &candidate.sample, address)
                    == std::cmp::Ordering::Less
                {
                    candidate.sample = *same_address;
                }
            }
            insert_worst_candidate(&mut top, candidate);
        }

        RankedWitnessCandidates::from_entries(top)
    })
}

fn insert_worst_candidate(
    top: &mut [Option<WitnessCandidate>; MAX_CELL_CORRELATION_WITNESSES],
    candidate: WitnessCandidate,
) {
    let Some(insert_at) = top.iter().position(|existing| {
        existing.is_none_or(|existing| {
            compare_witness_sample(
                &candidate.sample,
                candidate.address,
                &existing.sample,
                existing.address,
            ) == std::cmp::Ordering::Less
        })
    }) else {
        return;
    };
    for index in (insert_at + 1..top.len()).rev() {
        top[index] = top[index - 1];
    }
    top[insert_at] = Some(candidate);
}

fn build_all_view_witnesses(
    samples: &[ScreenSample; SCREEN_SAMPLE_COUNT],
    cell_candidates: &[RankedWitnessCandidates; 9],
) -> Vec<CoverageAddressWitness> {
    // Any address in the global top four must be in its cell's top four, so the
    // nine bounded lists are sufficient candidates for the whole-view result.
    let mut unique_addresses: [Option<CubePatchAddress>; 9 * MAX_CELL_CORRELATION_WITNESSES] =
        std::array::from_fn(|_| None);
    let mut unique_address_count = 0;
    let mut top: [Option<WitnessCandidate>; MAX_CELL_CORRELATION_WITNESSES] =
        std::array::from_fn(|_| None);
    for cell_candidate in cell_candidates.iter().flat_map(|cell| cell.iter()) {
        let address = cell_candidate.address;
        if unique_addresses[..unique_address_count].contains(&Some(address)) {
            continue;
        }
        unique_addresses[unique_address_count] = Some(address);
        unique_address_count += 1;
        let mut candidate = WitnessCandidate {
            address,
            sample: cell_candidate.sample,
            sample_count: 0,
        };
        for sample in samples
            .iter()
            .filter(|sample| sample.terrain_hit && sample.drawable_address == Some(address))
        {
            candidate.sample_count += 1;
            if compare_witness_sample(sample, address, &candidate.sample, address)
                == std::cmp::Ordering::Less
            {
                candidate.sample = *sample;
            }
        }
        insert_worst_candidate(&mut top, candidate);
    }
    let mut witnesses = Vec::with_capacity(MAX_CELL_CORRELATION_WITNESSES);
    for candidate in top.into_iter().flatten() {
        witnesses.push(candidate.to_witness());
    }
    witnesses
}

impl WitnessCandidate {
    fn to_witness(self) -> CoverageAddressWitness {
        CoverageAddressWitness {
            normalized_screen_position: self.sample.normalized_screen_position,
            reference_body_direction: self.sample.reference_body_direction.to_array(),
            reference_hit_distance_m: self.sample.reference_hit_distance_m,
            drawable_address: self.address.into(),
            proxy_error_px: self.sample.error_px,
            reference_sphere_sample_count: self.sample_count,
        }
    }
}

fn compare_witness_sample(
    left: &ScreenSample,
    left_address: CubePatchAddress,
    right: &ScreenSample,
    right_address: CubePatchAddress,
) -> std::cmp::Ordering {
    // Missing scores are unresolved and sort before known scores; otherwise the
    // largest proxy error wins. Address and top-left sample position break ties.
    right
        .error_px
        .is_none()
        .cmp(&left.error_px.is_none())
        .then_with(|| match (left.error_px, right.error_px) {
            (Some(left), Some(right)) => right.total_cmp(&left),
            _ => std::cmp::Ordering::Equal,
        })
        .then_with(|| left_address.cmp(&right_address))
        .then_with(|| {
            left.normalized_screen_position[1].total_cmp(&right.normalized_screen_position[1])
        })
        .then_with(|| {
            left.normalized_screen_position[0].total_cmp(&right.normalized_screen_position[0])
        })
}

#[derive(Clone, Copy, Default)]
struct CoverageCounts {
    terrain: usize,
    fallback: usize,
    useful: usize,
    target: usize,
    unresolved: usize,
}

fn summarize_samples(
    samples: &mut [ScreenSample; SCREEN_SAMPLE_COUNT],
    target_error_px: f64,
    sample_area_pixels: f64,
) -> ([ScreenCoverageCell; 9], ScreenCoverageCell) {
    // First order by cell and then known score. Each cell's scored samples are
    // contiguous for its percentiles, all using the same bounded 2,304 records.
    samples.sort_unstable_by(|left, right| {
        left.cell_index
            .cmp(&right.cell_index)
            .then_with(|| right.error_px.is_some().cmp(&left.error_px.is_some()))
            .then_with(|| {
                left.error_px
                    .unwrap_or(0.0)
                    .total_cmp(&right.error_px.unwrap_or(0.0))
            })
    });

    let mut cells = std::array::from_fn(|_| ScreenCoverageCell::default());
    let mut counts = [CoverageCounts::default(); 9];
    for (cell_index, cell) in cells.iter_mut().enumerate() {
        let region = samples
            .iter()
            .filter(|sample| usize::from(sample.cell_index) == cell_index);
        let mut terrain_count = 0usize;
        let mut fallback_count = 0usize;
        let mut useful_count = 0usize;
        let mut target_count = 0usize;
        let mut unresolved_count = 0usize;
        let mut known_error_count = 0usize;
        for sample in region {
            if !sample.terrain_hit {
                continue;
            }
            terrain_count += 1;
            fallback_count += usize::from(sample.fallback_covered);
            if sample.fallback_covered {
                if let Some(error) = sample
                    .error_px
                    .filter(|error| error.is_finite() && *error >= 0.0)
                {
                    known_error_count += 1;
                    useful_count += usize::from(error <= USEFUL_PROXY_ERROR_PX);
                    target_count += usize::from(error <= target_error_px);
                } else {
                    unresolved_count += 1;
                }
            } else {
                unresolved_count += 1;
            }
        }
        let first_error = samples.iter().position(|sample| {
            usize::from(sample.cell_index) == cell_index && sample.error_px.is_some()
        });
        let error_slice =
            first_error.map_or(&[][..], |start| &samples[start..start + known_error_count]);
        counts[cell_index] = CoverageCounts {
            terrain: terrain_count,
            fallback: fallback_count,
            useful: useful_count,
            target: target_count,
            unresolved: unresolved_count,
        };
        *cell = ScreenCoverageCell {
            terrain_sample_count: terrain_count,
            estimated_terrain_area_pixels: terrain_count as f64 * sample_area_pixels,
            fallback_covered_fraction: (terrain_count > 0)
                .then_some(fallback_count as f64 / terrain_count as f64),
            useful_proxy_error_fraction: (terrain_count > 0)
                .then_some(useful_count as f64 / terrain_count as f64),
            target_proxy_error_fraction: (terrain_count > 0)
                .then_some(target_count as f64 / terrain_count as f64),
            proxy_error_p50_px: percentile_sample(error_slice, 50, 100),
            proxy_error_p95_px: percentile_sample(error_slice, 95, 100),
            proxy_error_max_px: error_slice.last().and_then(|sample| sample.error_px),
            unresolved_proxy_sample_count: unresolved_count,
            correlation_witnesses: Vec::new(),
        };
    }

    // The cell summaries hold all counts; only p50/p95/max need another ordering.
    samples.sort_unstable_by(|left, right| {
        left.error_px
            .is_some()
            .cmp(&right.error_px.is_some())
            .then_with(|| {
                left.error_px
                    .unwrap_or(0.0)
                    .total_cmp(&right.error_px.unwrap_or(0.0))
            })
    });
    let all_error_count = samples
        .iter()
        .filter(|sample| {
            sample.terrain_hit
                && sample.fallback_covered
                && sample
                    .error_px
                    .is_some_and(|error| error.is_finite() && error >= 0.0)
        })
        .count();
    let all_error_start = samples.iter().position(|sample| {
        sample.terrain_hit
            && sample.fallback_covered
            && sample
                .error_px
                .is_some_and(|error| error.is_finite() && error >= 0.0)
    });
    let all_error_slice =
        all_error_start.map_or(&[][..], |start| &samples[start..start + all_error_count]);

    let terrain_count: usize = counts.iter().map(|cell| cell.terrain).sum();
    let fallback_count: usize = counts.iter().map(|cell| cell.fallback).sum();
    let useful_count: usize = counts.iter().map(|cell| cell.useful).sum();
    let target_count: usize = counts.iter().map(|cell| cell.target).sum();
    let unresolved_count: usize = counts.iter().map(|cell| cell.unresolved).sum();
    let all_view = ScreenCoverageCell {
        terrain_sample_count: terrain_count,
        estimated_terrain_area_pixels: terrain_count as f64 * sample_area_pixels,
        fallback_covered_fraction: (terrain_count > 0)
            .then_some(fallback_count as f64 / terrain_count as f64),
        useful_proxy_error_fraction: (terrain_count > 0)
            .then_some(useful_count as f64 / terrain_count as f64),
        target_proxy_error_fraction: (terrain_count > 0)
            .then_some(target_count as f64 / terrain_count as f64),
        proxy_error_p50_px: percentile_sample(all_error_slice, 50, 100),
        proxy_error_p95_px: percentile_sample(all_error_slice, 95, 100),
        proxy_error_max_px: all_error_slice.last().and_then(|sample| sample.error_px),
        unresolved_proxy_sample_count: unresolved_count,
        correlation_witnesses: Vec::new(),
    };
    (cells, all_view)
}

fn percentile_sample(values: &[ScreenSample], numerator: usize, denominator: usize) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    let index = (numerator * values.len())
        .div_ceil(denominator)
        .saturating_sub(1);
    values.get(index.min(values.len() - 1))?.error_px
}

fn nearest_positive_sphere_intersection(
    origin: DVec3,
    direction: DVec3,
    radius_m: f64,
) -> Option<f64> {
    if !origin.is_finite() || !direction.is_finite() || !radius_m.is_finite() || radius_m <= 0.0 {
        return None;
    }
    let direction = direction.normalize_or_zero();
    if direction == DVec3::ZERO {
        return None;
    }

    // Solve t² + 2*b*t + c = 0 using q/c-over-q roots. This avoids losing the
    // near root when a close observer looks almost directly at the sphere.
    let b = origin.dot(direction);
    let c = origin.length_squared() - radius_m * radius_m;
    let discriminant = b.mul_add(b, -c);
    if !discriminant.is_finite() || discriminant < 0.0 {
        return None;
    }
    let root = discriminant.sqrt();
    let q = -b - root.copysign(b);
    let (first, second) = if q == 0.0 {
        (-b - root, -b + root)
    } else {
        (q, c / q)
    };
    [first, second]
        .into_iter()
        .filter(|distance| distance.is_finite() && *distance > 0.0)
        .min_by(f64::total_cmp)
}

fn hit_is_past_near_plane(view_ray: DVec3, distance: f64, near_m: f64) -> bool {
    view_ray.is_finite()
        && distance.is_finite()
        && near_m.is_finite()
        && near_m > 0.0
        && -view_ray.z * distance >= near_m
}

fn direction_to_patch(direction: DVec3, level: u8) -> Option<CubePatchAddress> {
    if !direction.is_finite() || direction == DVec3::ZERO {
        return None;
    }
    let direction = direction.normalize();
    let face = super::cube_face_for_direction(direction);
    let [normal, u, v] = face.basis();
    let denominator = direction.dot(normal);
    if !denominator.is_finite() || denominator <= 0.0 {
        return None;
    }
    let uv = [
        direction.dot(u) / denominator,
        direction.dot(v) / denominator,
    ];
    if !uv.iter().all(|coordinate| coordinate.is_finite()) {
        return None;
    }
    let level = level.min(30);
    let scale = 1u64 << level;
    let coordinate = |value: f64| {
        (((value + 1.0) * 0.5 * scale as f64).floor() as i64).clamp(0, scale as i64 - 1) as u32
    };
    CubePatchAddress::try_new(face, level, coordinate(uv[0]), coordinate(uv[1])).ok()
}

fn find_drawable_ancestor(
    mut address: CubePatchAddress,
    drawable_leaf_errors_px: &HashMap<CubePatchAddress, Option<f64>>,
) -> Option<(Option<CubePatchAddress>, Option<f64>)> {
    loop {
        if let Some(error_px) = drawable_leaf_errors_px.get(&address) {
            return Some((
                Some(address),
                error_px.filter(|error| error.is_finite() && *error >= 0.0),
            ));
        }
        address = address.parent()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mundaris_math::surface::CubeFace;

    fn projection(size: u32, fov_degrees: f64) -> CelestialProjection {
        CelestialProjection::try_new(size, size, fov_degrees.to_radians(), 0.01).unwrap()
    }

    fn six_root_cover(error_px: Option<f64>) -> HashMap<CubePatchAddress, Option<f64>> {
        CubeFace::ALL
            .into_iter()
            .map(|face| (CubePatchAddress::root(face), error_px))
            .collect()
    }

    #[test]
    fn sky_is_excluded_and_empty_fractions_are_none() {
        let diagnostic = screen_coverage_diagnostic(
            DVec3::Z * 2.0,
            DMat3::from_rotation_x(std::f64::consts::PI),
            projection(96, 60.0),
            1.0,
            5,
            &six_root_cover(Some(1.0)),
            0.15,
        );

        assert_eq!(diagnostic.all_view.terrain_sample_count, 0);
        assert_eq!(diagnostic.all_view.estimated_terrain_area_pixels, 0.0);
        assert_eq!(diagnostic.all_view.fallback_covered_fraction, None);
        assert_eq!(diagnostic.all_view.proxy_error_p95_px, None);
        assert!(diagnostic.all_view.correlation_witnesses.is_empty());
        assert!(
            diagnostic
                .cells
                .iter()
                .all(|cell| cell.terrain_sample_count == 0 && cell.correlation_witnesses.is_empty())
        );
    }

    #[test]
    fn stable_sphere_roots_cover_outside_close_and_inside_observers() {
        let outside = nearest_positive_sphere_intersection(DVec3::Z * 2.0, -DVec3::Z, 1.0).unwrap();
        let close = nearest_positive_sphere_intersection(DVec3::Z * (1.0 + 1.0e-9), -DVec3::Z, 1.0)
            .unwrap();
        let inside = nearest_positive_sphere_intersection(DVec3::Z * 0.5, DVec3::Z, 1.0).unwrap();

        assert_eq!(outside, 1.0);
        assert!((close - 1.0e-9).abs() < 1.0e-15);
        assert_eq!(inside, 0.5);
    }

    #[test]
    fn near_plane_uses_view_depth_instead_of_ray_distance() {
        let near_m = 0.19;
        let oblique_view_ray = DVec3::new(0.6, 0.0, -0.8).normalize();

        assert!(!hit_is_past_near_plane(oblique_view_ray, 0.2, near_m));
        assert!(hit_is_past_near_plane(-DVec3::Z, 0.2, near_m));

        let cover = six_root_cover(Some(1.0));
        let clipped = screen_coverage_diagnostic(
            DVec3::Z * 1.1,
            DMat3::IDENTITY,
            CelestialProjection::try_new(96, 96, 10.0_f64.to_radians(), 0.2).unwrap(),
            1.0,
            5,
            &cover,
            0.15,
        );
        let visible = screen_coverage_diagnostic(
            DVec3::Z * 1.1,
            DMat3::IDENTITY,
            CelestialProjection::try_new(96, 96, 10.0_f64.to_radians(), 0.05).unwrap(),
            1.0,
            5,
            &cover,
            0.15,
        );

        assert_eq!(clipped.all_view.terrain_sample_count, 0);
        assert!(visible.all_view.terrain_sample_count > 0);
    }

    #[test]
    fn mixed_level_cover_and_cube_face_seams_find_drawable_ancestors() {
        let mut cover = six_root_cover(Some(7.0));
        cover.remove(&CubePatchAddress::root(CubeFace::PositiveZ));
        for child in CubePatchAddress::root(CubeFace::PositiveZ)
            .children()
            .unwrap()
        {
            cover.insert(child, Some(2.0));
        }

        for direction in [DVec3::new(1.0, 0.0, 1.0), DVec3::new(0.0, 1.0, 1.0)] {
            let address = direction_to_patch(direction, 8).unwrap();
            assert!(find_drawable_ancestor(address, &cover).is_some());
        }
        let body_local_origin_m = DVec3::Z * 3.0;
        let side_direction = DVec3::new(
            55.0_f64.to_radians().sin(),
            0.0,
            55.0_f64.to_radians().cos(),
        );
        let body_ray = (side_direction - body_local_origin_m).normalize();
        let body_to_view = DMat3::from_rotation_y(body_ray.x.atan2(-body_ray.z));
        let seam_sample_offset_px = 22.0;
        let sight_angle = body_ray.x.atan2(-body_ray.z);
        let focal_pixels = seam_sample_offset_px / sight_angle.tan();
        let vertical_fov_rad = 2.0 * (192.0 / (2.0 * focal_pixels)).atan();
        let diagnostic = screen_coverage_diagnostic(
            body_local_origin_m,
            body_to_view,
            CelestialProjection::try_new(192, 192, vertical_fov_rad, 0.01).unwrap(),
            1.0,
            8,
            &cover,
            0.15,
        );

        assert!(diagnostic.all_view.terrain_sample_count > 0);
        assert_eq!(diagnostic.all_view.fallback_covered_fraction, Some(1.0));
        assert!(
            diagnostic
                .sampled_drawable_area_pixels
                .contains_key(&CubePatchAddress::root(CubeFace::PositiveX))
        );
        let mapped_area: f64 = diagnostic.sampled_drawable_area_pixels.values().sum();
        assert!((mapped_area - diagnostic.all_view.estimated_terrain_area_pixels).abs() < 1.0e-9);
        assert!(
            diagnostic
                .all_view
                .proxy_error_max_px
                .is_some_and(|error| error >= 7.0)
        );
        assert_eq!(
            diagnostic.all_view.terrain_sample_count,
            diagnostic
                .cells
                .iter()
                .map(|cell| cell.terrain_sample_count)
                .sum::<usize>()
        );
    }

    #[test]
    fn coverage_area_sums_once_and_missing_scores_remain_unresolved() {
        let diagnostic = screen_coverage_diagnostic(
            DVec3::Z * 2.0,
            DMat3::IDENTITY,
            projection(120, 90.0),
            1.0,
            4,
            &six_root_cover(None),
            0.15,
        );

        assert!(diagnostic.all_view.terrain_sample_count > 0);
        assert!(diagnostic.all_view.terrain_sample_count <= 9 * 256);
        assert_eq!(
            diagnostic.all_view.terrain_sample_count,
            diagnostic
                .cells
                .iter()
                .map(|cell| cell.terrain_sample_count)
                .sum::<usize>()
        );
        let expected_area_pixels =
            diagnostic.all_view.terrain_sample_count as f64 * 120.0 * 120.0 / 2304.0;
        assert!(
            (diagnostic.all_view.estimated_terrain_area_pixels - expected_area_pixels).abs()
                < 1.0e-9
        );
        let area_sum: f64 = diagnostic
            .cells
            .iter()
            .map(|cell| cell.estimated_terrain_area_pixels)
            .sum();
        assert!((area_sum - diagnostic.all_view.estimated_terrain_area_pixels).abs() < 1.0e-9);
        assert_eq!(diagnostic.all_view.fallback_covered_fraction, Some(1.0));
        assert_eq!(diagnostic.all_view.useful_proxy_error_fraction, Some(0.0));
        let mapped_area: f64 = diagnostic.sampled_drawable_area_pixels.values().sum();
        assert!((mapped_area - diagnostic.all_view.estimated_terrain_area_pixels).abs() < 1.0e-9);
        assert!(
            diagnostic.sampled_drawable_area_pixels.len()
                <= diagnostic.all_view.terrain_sample_count
        );
        assert_eq!(
            diagnostic.all_view.unresolved_proxy_sample_count,
            diagnostic.all_view.terrain_sample_count
        );
    }

    #[test]
    fn perspective_projection_changes_mask_without_counting_sky() {
        let narrow = screen_coverage_diagnostic(
            DVec3::Z * 2.0,
            DMat3::IDENTITY,
            projection(192, 90.0),
            1.0,
            6,
            &six_root_cover(Some(1.0)),
            0.15,
        );
        let wide = screen_coverage_diagnostic(
            DVec3::Z * 2.0,
            DMat3::IDENTITY,
            projection(192, 150.0),
            1.0,
            6,
            &six_root_cover(Some(1.0)),
            0.15,
        );

        assert!(narrow.all_view.terrain_sample_count > wide.all_view.terrain_sample_count);
        assert_eq!(narrow.all_view.fallback_covered_fraction, Some(1.0));
        assert_eq!(wide.all_view.fallback_covered_fraction, Some(1.0));
        assert_eq!(narrow.all_view.useful_proxy_error_fraction, Some(1.0));
        assert_eq!(wide.all_view.target_proxy_error_fraction, Some(0.0));
        assert_eq!(narrow.all_view.proxy_error_p95_px, Some(1.0));
        assert!(wide.all_view.terrain_sample_count < 9 * 256);
    }

    #[test]
    fn correlation_witnesses_leave_frozen_reference_sphere_summaries_unchanged() {
        let diagnostic = screen_coverage_diagnostic(
            DVec3::Z * 2.0,
            DMat3::IDENTITY,
            projection(96, 5.0),
            1.0,
            5,
            &six_root_cover(Some(1.0)),
            0.15,
        );

        assert_eq!(diagnostic.method, "reference_sphere_mask_48x48");
        assert_eq!(diagnostic.all_view.terrain_sample_count, 2304);
        assert_eq!(diagnostic.all_view.estimated_terrain_area_pixels, 9216.0);
        assert_eq!(diagnostic.all_view.fallback_covered_fraction, Some(1.0));
        assert_eq!(diagnostic.all_view.useful_proxy_error_fraction, Some(1.0));
        assert_eq!(diagnostic.all_view.target_proxy_error_fraction, Some(0.0));
        assert_eq!(diagnostic.all_view.proxy_error_p50_px, Some(1.0));
        assert_eq!(diagnostic.all_view.proxy_error_p95_px, Some(1.0));
        assert_eq!(diagnostic.all_view.proxy_error_max_px, Some(1.0));
        assert_eq!(diagnostic.all_view.unresolved_proxy_sample_count, 0);
        assert_eq!(diagnostic.all_view.correlation_witnesses.len(), 1);
        let serialized = serde_json::to_value(&diagnostic).unwrap();
        assert_eq!(
            serialized["all_view"]["correlation_witnesses"][0]["drawable_address"]["face"],
            "positive_z"
        );

        for (index, cell) in diagnostic.cells.iter().enumerate() {
            assert_eq!(cell.terrain_sample_count, 256);
            assert_eq!(cell.estimated_terrain_area_pixels, 1024.0);
            assert_eq!(cell.fallback_covered_fraction, Some(1.0));
            assert_eq!(cell.useful_proxy_error_fraction, Some(1.0));
            assert_eq!(cell.target_proxy_error_fraction, Some(0.0));
            assert_eq!(cell.proxy_error_p50_px, Some(1.0));
            assert_eq!(cell.proxy_error_p95_px, Some(1.0));
            assert_eq!(cell.proxy_error_max_px, Some(1.0));
            assert_eq!(cell.unresolved_proxy_sample_count, 0);
            assert_eq!(cell.correlation_witnesses.len(), 1);

            let witness = &cell.correlation_witnesses[0];
            assert_eq!(witness.drawable_address.face, CoverageCubeFace::PositiveZ);
            assert_eq!(witness.drawable_address.level, 0);
            assert_eq!(witness.drawable_address.x, 0);
            assert_eq!(witness.drawable_address.y, 0);
            assert_eq!(witness.proxy_error_px, Some(1.0));
            assert_eq!(witness.reference_sphere_sample_count, 256);
            assert_eq!(
                witness.normalized_screen_position[0],
                (index % 3) as f64 / 3.0 + 1.0 / 96.0
            );
            assert_eq!(
                witness.normalized_screen_position[1],
                (index / 3) as f64 / 3.0 + 1.0 / 96.0
            );
            assert!(witness.reference_hit_distance_m.is_finite());
            assert!(witness.reference_hit_distance_m > 0.0);
            assert!(
                (DVec3::from_array(witness.reference_body_direction).length() - 1.0).abs()
                    < 1.0e-12
            );
        }
    }

    #[test]
    fn correlation_witnesses_are_bounded_unique_and_deterministic() {
        let mut cover = CubeFace::ALL
            .into_iter()
            .filter(|face| *face != CubeFace::PositiveZ)
            .map(|face| (CubePatchAddress::root(face), Some(8.0)))
            .collect::<HashMap<_, _>>();
        for y in 0..32 {
            for x in 0..32 {
                let address = CubePatchAddress::try_new(CubeFace::PositiveZ, 5, x, y).unwrap();
                let error_px = f64::from((x * 17 + y * 31) % 101) / 10.0;
                cover.insert(address, Some(error_px));
            }
        }
        let make_diagnostic = || {
            screen_coverage_diagnostic(
                DVec3::Z * 1.1,
                DMat3::IDENTITY,
                projection(192, 110.0),
                1.0,
                5,
                &cover,
                0.15,
            )
        };

        let first = make_diagnostic();
        let second = make_diagnostic();
        assert_eq!(
            first.all_view.correlation_witnesses,
            second.all_view.correlation_witnesses
        );
        assert!(first.all_view.correlation_witnesses.len() <= MAX_CELL_CORRELATION_WITNESSES);
        assert!(
            first
                .cells
                .iter()
                .all(|cell| { cell.correlation_witnesses.len() <= MAX_CELL_CORRELATION_WITNESSES })
        );
        assert!(
            first.all_view.correlation_witnesses.len()
                + first
                    .cells
                    .iter()
                    .map(|cell| cell.correlation_witnesses.len())
                    .sum::<usize>()
                <= MAX_COVERAGE_WITNESSES_PER_DIAGNOSTIC
        );
        for (left, right) in first.cells.iter().zip(second.cells.iter()) {
            assert_eq!(left.correlation_witnesses, right.correlation_witnesses);
            let mut addresses = left
                .correlation_witnesses
                .iter()
                .map(|witness| witness.drawable_address.clone())
                .collect::<Vec<_>>();
            addresses.sort_by(|left, right| {
                left.face
                    .cmp(&right.face)
                    .then(left.level.cmp(&right.level))
                    .then(left.x.cmp(&right.x))
                    .then(left.y.cmp(&right.y))
            });
            addresses.dedup();
            assert_eq!(addresses.len(), left.correlation_witnesses.len());
            assert!(
                left.correlation_witnesses
                    .windows(2)
                    .all(|pair| pair[0].proxy_error_px >= pair[1].proxy_error_px)
            );
        }
    }

    #[test]
    fn witness_reducer_keeps_four_unique_worst_addresses_per_cell() {
        let addresses = CubePatchAddress::root(CubeFace::PositiveZ)
            .children()
            .unwrap()
            .into_iter()
            .flat_map(|parent| parent.children().unwrap())
            .collect::<Vec<_>>();
        let mut samples = [ScreenSample::default(); SCREEN_SAMPLE_COUNT];
        for cell_index in 0..CELLS_PER_AXIS * CELLS_PER_AXIS {
            for sample_in_cell in 0..SAMPLES_PER_CELL_AXIS * SAMPLES_PER_CELL_AXIS {
                let sample_index =
                    cell_index * SAMPLES_PER_CELL_AXIS * SAMPLES_PER_CELL_AXIS + sample_in_cell;
                let address_index = sample_in_cell % 8;
                samples[sample_index] = ScreenSample {
                    cell_index: cell_index as u8,
                    terrain_hit: true,
                    fallback_covered: true,
                    error_px: Some(address_index as f64),
                    normalized_screen_position: [
                        (sample_in_cell % SAMPLES_PER_CELL_AXIS) as f64
                            / SAMPLES_PER_CELL_AXIS as f64,
                        (sample_in_cell / SAMPLES_PER_CELL_AXIS) as f64
                            / SAMPLES_PER_CELL_AXIS as f64,
                    ],
                    reference_body_direction: DVec3::Z,
                    reference_hit_distance_m: 1.0,
                    drawable_address: Some(addresses[address_index]),
                };
            }
        }

        let candidates = build_cell_witness_candidates(&samples);
        assert!(candidates.iter().all(|cell| {
            let populated = cell.iter().collect::<Vec<_>>();
            cell.len == MAX_CELL_CORRELATION_WITNESSES
                && populated.windows(2).all(|pair| {
                    compare_witness_sample(
                        &pair[0].sample,
                        pair[0].address,
                        &pair[1].sample,
                        pair[1].address,
                    ) == std::cmp::Ordering::Less
                })
        }));
        for cell in &candidates {
            let mut unique = cell
                .iter()
                .map(|candidate| candidate.address)
                .collect::<Vec<_>>();
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), MAX_CELL_CORRELATION_WITNESSES);
            assert_eq!(
                cell.iter()
                    .map(|candidate| candidate.sample.error_px)
                    .collect::<Vec<_>>(),
                [Some(7.0), Some(6.0), Some(5.0), Some(4.0)]
            );
            assert!(cell.iter().all(|candidate| candidate.sample_count == 32));
        }
        assert_eq!(
            coverage_witness_heap_capacity_bytes(),
            MAX_COVERAGE_WITNESSES_PER_DIAGNOSTIC * std::mem::size_of::<CoverageAddressWitness>()
        );
    }
}
