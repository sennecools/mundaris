//! Version-two impact history. Complete samples have no footprint or camera input.
use super::*;
use crate::terrain::{PreparedPageKey, SurfaceQueryContext};
use std::ops::{Add, Mul, Neg, Sub};
use std::sync::Arc;
use std::time::Instant;

const SUPPORT: f64 = 0.72;
const SHELL: f64 = 0.20;
const JITTER: f64 = 0.06;
// SUPPORT + SHELL = .92 < .94 = 1 - JITTER. Even after projecting
// a raw shell centre, an influencing feature stays in the one-cell halo.
const EPOCH_COUNT: usize = 10;
const PREPARED_PAGE_FAMILY: u8 = 3;
const PREPARED_PAGE_BUILD_RESERVATION: usize = 16 * 1024;

#[derive(Debug, Clone, Copy)]
struct Layout {
    rotation: DMat3,
    offset: DVec3,
    salt: u64,
}
#[derive(Debug, Clone, Copy)]
struct Epoch {
    edge_m: f64,
    relief_m: f64,
    density: f64,
    layouts: [Layout; 2],
}

#[derive(Debug, Clone)]
pub(super) struct AncientField {
    seed: u64,
    radius_m: f64,
    config: MoonTerrainConfig,
    epochs: [Epoch; EPOCH_COUNT],
    bound_m: f64,
}
impl AncientField {
    pub(super) fn new(d: &MoonTerrainDefinition, radius_m: f64) -> Result<Self, TerrainError> {
        let c = d.config;
        // Each epoch is a convex composition of individually bounded profiles.
        // Large history spends eight times its authoring relief; regional
        // history spends 2.25 times. Highlands spend twice theirs across octaves.
        let fraction = c.basin_relief_fraction
            + 2.0 * c.highland_relief_fraction
            + 8.0 * c.impact_relief_fractions[0]
            + 2.25 * c.impact_relief_fractions[1]
            + c.impact_relief_fractions[2];
        if !fraction.is_finite() || fraction >= 0.1 {
            return Err(TerrainError::InvalidConfig);
        }
        let seed = mix(d.seed.0 ^ d.identity.0.rotate_left(19) ^ 0x414e_4349_454e_5402);
        let scale = c.impact_scale_fractions;
        let relief = c.impact_relief_fractions;
        let edges = [
            scale[0],
            scale[0] * 0.5,
            scale[0] * 0.25,
            scale[0] * 0.125,
            scale[1],
            scale[1] * 0.5,
            scale[1] * 0.25,
            scale[1] * 0.125,
            scale[2] * 4.0,
            scale[2],
        ];
        let budgets = [
            relief[0] * 3.40,
            relief[0] * 2.20,
            relief[0] * 1.50,
            relief[0] * 0.90,
            relief[1] * 1.30,
            relief[1] * 0.60,
            relief[1] * 0.25,
            relief[1] * 0.10,
            relief[2] * 0.65,
            relief[2] * 0.35,
        ];
        let mut epochs: [Epoch; EPOCH_COUNT] = std::array::from_fn(|epoch| {
            let edge_m = (edges[epoch] * radius_m).max(MIN_CELL_EDGE_M);
            let layouts = std::array::from_fn(|layout| {
                let salt =
                    mix(seed ^ ((epoch * 2 + layout) as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15));
                let angle = |tag| unit(salt ^ tag) * std::f64::consts::TAU;
                Layout {
                    rotation: DMat3::from_rotation_z(angle(1))
                        * DMat3::from_rotation_y(angle(2))
                        * DMat3::from_rotation_x(angle(3)),
                    offset: DVec3::new(unit(salt ^ 4), unit(salt ^ 5), unit(salt ^ 6)) * edge_m,
                    salt,
                }
            });
            Epoch {
                edge_m,
                relief_m: budgets[epoch] * radius_m,
                density: match epoch {
                    0 => 0.45,
                    1 => 0.70,
                    _ => 1.0,
                },
                layouts,
            }
        });
        // Authoring groups can overlap in scale; chronology follows actual
        // physical scale even for non-default configurations and small radii.
        epochs.sort_by(|a, b| b.edge_m.total_cmp(&a.edge_m));
        Ok(Self {
            seed,
            radius_m,
            config: c,
            epochs,
            bound_m: (outward_product(fraction, radius_m) + REGOLITH_RELIEF_BOUND_M).next_up(),
        })
    }
    pub(super) fn absolute_height_bound_m(&self) -> f64 {
        self.bound_m
    }
    pub(super) fn evaluate(&self, location: SurfaceLocation) -> Result<FieldValue, TerrainError> {
        self.evaluate_halo(location, 1)
    }

    pub(super) fn evaluate_with_context(
        &self,
        location: SurfaceLocation,
        context: &mut SurfaceQueryContext<'_>,
    ) -> Result<FieldValue, TerrainError> {
        if context.is_prepared()
            && let Some(value) = self.evaluate_prepared(location, context)?
        {
            return Ok(value);
        }
        self.evaluate_profiled_reference(location, Some(context))
    }

    fn evaluate_profiled_reference(
        &self,
        location: SurfaceLocation,
        context: Option<&mut SurfaceQueryContext<'_>>,
    ) -> Result<FieldValue, TerrainError> {
        if !context
            .as_deref()
            .is_some_and(SurfaceQueryContext::profiling)
        {
            return self.evaluate(location);
        }
        let started = Instant::now();
        let result = self.evaluate_halo_profiled(location)?;
        if let Some(context) = context {
            context.record_legacy_profile(elapsed_ns(started), result.1, result.2);
        }
        Ok(result.0)
    }

    fn evaluate_halo_profiled(
        &self,
        location: SurfaceLocation,
    ) -> Result<(FieldValue, u64, u64), TerrainError> {
        let n = location.direction().unit();
        let (mut height, highland) = self.background(n)?;
        let mut rock = 0.0;
        let mut discovery_and_profile_ns = 0u64;
        let mut chronology_ns = 0u64;
        for epoch in self.epochs {
            let discovery_started = Instant::now();
            let mut candidates = [Candidate::ZERO; 54];
            let mut count = 0;
            for (layout_index, layout) in epoch.layouts.into_iter().enumerate() {
                let p = layout.rotation * (n * self.radius_m);
                let base = checked_cell((p + layout.offset) / epoch.edge_m)?;
                for z in -1..=1 {
                    for y in -1..=1 {
                        for x in -1..=1 {
                            let cell = [base[0] + x, base[1] + y, base[2] + z];
                            let key = cell_key(layout.salt, cell);
                            let mut feature =
                                feature(p, epoch, layout.offset, cell, key, self.radius_m);
                            if feature.influence.value <= 0.0 {
                                continue;
                            }
                            if count == candidates.len() {
                                return Err(TerrainError::NonFiniteResult);
                            }
                            feature.profile.gradient = layout.rotation.transpose()
                                * feature.profile.gradient
                                * self.radius_m;
                            feature.influence.gradient = layout.rotation.transpose()
                                * feature.influence.gradient
                                * self.radius_m;
                            candidates[count] = Candidate {
                                feature,
                                cell,
                                layout: layout_index,
                            };
                            count += 1;
                        }
                    }
                }
            }
            discovery_and_profile_ns =
                discovery_and_profile_ns.saturating_add(elapsed_ns(discovery_started));
            let chronology_started = Instant::now();
            candidates[..count].sort_unstable_by_key(|c| (c.feature.age, c.layout, c.cell));
            let mut residual = Differential::constant(0.0);
            for candidate in &candidates[..count] {
                let f = candidate.feature;
                residual = residual * (Differential::constant(1.0) - f.influence)
                    + f.profile * f.influence;
                rock = rock * (1.0 - f.influence.value) + f.rock * f.influence.value;
            }
            chronology_ns = chronology_ns.saturating_add(elapsed_ns(chronology_started));
            height = height + residual * (highland * 0.88 + Differential::constant(0.12));
        }
        Ok((
            FieldValue {
                height_m: height.value,
                gradient: tangent_project(n, height.gradient),
                highland: highland.value,
                basalt: 1.0 - highland.value,
                rock,
            },
            discovery_and_profile_ns,
            chronology_ns,
        ))
    }

    fn evaluate_prepared(
        &self,
        location: SurfaceLocation,
        context: &mut SurfaceQueryContext<'_>,
    ) -> Result<Option<FieldValue>, TerrainError> {
        let n = location.direction().unit();
        let (mut height, highland) = self.background(n)?;
        let mut rock = 0.0;
        for (epoch_index, epoch) in self.epochs.iter().copied().enumerate() {
            let mut pages: [Option<Arc<PreparedEpochLayoutPage>>; 2] = [None, None];
            let mut rotations = [DMat3::IDENTITY; 2];
            let mut points = [DVec3::ZERO; 2];
            for (layout_index, layout) in epoch.layouts.into_iter().enumerate() {
                let p = layout.rotation * (n * self.radius_m);
                let base = checked_cell((p + layout.offset) / epoch.edge_m)?;
                let key = PreparedPageKey {
                    family: PREPARED_PAGE_FAMILY,
                    scale: epoch_index as u8,
                    layout: layout_index as u8,
                    cell: base,
                };
                let Some(page) = context.prepared_page(
                    epoch_index * 2 + layout_index,
                    key,
                    PREPARED_PAGE_BUILD_RESERVATION,
                    || {
                        let page = build_prepared_page(epoch, layout, base, self.radius_m);
                        (
                            page,
                            std::mem::size_of::<PreparedEpochLayoutPage>() + 32,
                            27,
                        )
                    },
                ) else {
                    return Ok(None);
                };
                rotations[layout_index] = layout.rotation;
                points[layout_index] = p;
                pages[layout_index] = Some(page);
            }
            let [Some(page_a), Some(page_b)] = pages else {
                return Ok(None);
            };
            let pages = [page_a, page_b];
            let mut indices = [0usize; 2];
            let mut residual = Differential::constant(0.0);
            while indices[0] < pages[0].count || indices[1] < pages[1].count {
                let layout_index = match (indices[0] < pages[0].count, indices[1] < pages[1].count)
                {
                    (true, true) => {
                        let left = pages[0].candidates[indices[0]];
                        let right = pages[1].candidates[indices[1]];
                        if (left.age, 0usize, left.cell) <= (right.age, 1usize, right.cell) {
                            0
                        } else {
                            1
                        }
                    }
                    (true, false) => 0,
                    (false, true) => 1,
                    (false, false) => break,
                };
                let descriptor = pages[layout_index].candidates[indices[layout_index]];
                indices[layout_index] += 1;
                let mut candidate =
                    prepared_feature(points[layout_index], epoch, descriptor, self.radius_m);
                context.record_candidate_discoveries(1);
                if candidate.influence.value <= 0.0 {
                    continue;
                }
                let rotation = rotations[layout_index];
                candidate.profile.gradient =
                    rotation.transpose() * candidate.profile.gradient * self.radius_m;
                candidate.influence.gradient =
                    rotation.transpose() * candidate.influence.gradient * self.radius_m;
                residual = residual * (Differential::constant(1.0) - candidate.influence)
                    + candidate.profile * candidate.influence;
                rock = rock * (1.0 - candidate.influence.value)
                    + candidate.rock * candidate.influence.value;
            }
            height = height + residual * (highland * 0.88 + Differential::constant(0.12));
        }
        Ok(Some(FieldValue {
            height_m: height.value,
            gradient: tangent_project(n, height.gradient),
            highland: highland.value,
            basalt: 1.0 - highland.value,
            rock,
        }))
    }
    fn evaluate_halo(
        &self,
        location: SurfaceLocation,
        halo: i64,
    ) -> Result<FieldValue, TerrainError> {
        let n = location.direction().unit();
        let (mut height, highland) = self.background(n)?;
        let mut rock = 0.0;
        for epoch in self.epochs {
            let mut candidates = [Candidate::ZERO; 54];
            let mut count = 0;
            for (layout_index, layout) in epoch.layouts.into_iter().enumerate() {
                let p = layout.rotation * (n * self.radius_m);
                let base = checked_cell((p + layout.offset) / epoch.edge_m)?;
                for z in -halo..=halo {
                    for y in -halo..=halo {
                        for x in -halo..=halo {
                            let cell = [base[0] + x, base[1] + y, base[2] + z];
                            let key = cell_key(layout.salt, cell);
                            let mut feature =
                                feature(p, epoch, layout.offset, cell, key, self.radius_m);
                            if feature.influence.value <= 0.0 {
                                continue;
                            }
                            if count == candidates.len() {
                                return Err(TerrainError::NonFiniteResult);
                            }
                            // Features are evaluated in metres; transform gradients into
                            // body axes and unit-direction units before composition.
                            feature.profile.gradient = layout.rotation.transpose()
                                * feature.profile.gradient
                                * self.radius_m;
                            feature.influence.gradient = layout.rotation.transpose()
                                * feature.influence.gradient
                                * self.radius_m;
                            candidates[count] = Candidate {
                                feature,
                                cell,
                                layout: layout_index,
                            };
                            count += 1;
                        }
                    }
                }
            }
            candidates[..count].sort_unstable_by_key(|c| (c.feature.age, c.layout, c.cell));
            let mut residual = Differential::constant(0.0);
            for candidate in &candidates[..count] {
                let f = candidate.feature;
                residual = residual * (Differential::constant(1.0) - f.influence)
                    + f.profile * f.influence;
                rock = rock * (1.0 - f.influence.value) + f.rock * f.influence.value;
            }
            // Ancient plains retain a few subdued impacts rather than erasing
            // the history outright. The mask's derivative is authoritative too.
            height = height + residual * (highland * 0.88 + Differential::constant(0.12));
        }
        Ok(FieldValue {
            height_m: height.value,
            gradient: tangent_project(n, height.gradient),
            highland: highland.value,
            basalt: 1.0 - highland.value,
            rock,
        })
    }
    fn background(&self, n: DVec3) -> Result<(Differential, Differential), TerrainError> {
        let a = self.noise(n, 2.7, 1)?;
        let b = self.noise(n, 5.1, 2)?;
        let plains = (a * 0.80 + b * 0.20).smoothstep(-0.06, 0.09);
        let highland = Differential::constant(1.0) - plains;
        let mut height =
            (a * 0.68 + b * 0.32) * (self.config.basin_relief_fraction * self.radius_m);
        // Correlated ridged highlands extend through the spaces between impacts.
        // Normalized weights sum to one; each smoothed ridge has |value| < 1.
        for (index, (frequency, weight)) in [
            (24.0, 0.38),
            (51.0, 0.28),
            (109.0, 0.20),
            (233.0, 0.10),
            (497.0, 0.03),
            (1061.0, 0.01),
        ]
        .into_iter()
        .enumerate()
        {
            let v = self.noise(n, frequency, 0x100 + index as u64)?;
            let ridge = (Differential::constant(1.0)
                - (v * v + Differential::constant(0.0004)).sqrt() * 2.0)
                * 0.999;
            height = height
                + ridge
                    * highland
                    * (2.0 * self.config.highland_relief_fraction * self.radius_m * weight);
        }
        let wavelength = (self.radius_m * REGOLITH_WAVELENGTH_FRACTION).max(MIN_CELL_EDGE_M);
        let grain = self.noise(n, self.radius_m / wavelength, 0x5245_474f)?;
        height = height
            + grain * (highland * 0.65 + Differential::constant(0.35)) * REGOLITH_RELIEF_BOUND_M;
        Ok((height, highland))
    }
    fn noise(&self, n: DVec3, frequency: f64, tag: u64) -> Result<Differential, TerrainError> {
        let sample = gradient_noise(mix(self.seed ^ tag), n * frequency)
            .map_err(|_| TerrainError::NonFiniteResult)?;
        Ok(Differential {
            value: sample.value,
            gradient: sample.gradient * frequency,
        })
    }
}

#[derive(Debug, Clone, Copy)]
struct Feature {
    age: u64,
    profile: Differential,
    influence: Differential,
    rock: f64,
}
impl Feature {
    const ZERO: Self = Self {
        age: 0,
        profile: Differential::ZERO,
        influence: Differential::ZERO,
        rock: 0.0,
    };
}
#[derive(Debug, Clone, Copy)]
struct Candidate {
    feature: Feature,
    cell: [i64; 3],
    layout: usize,
}
impl Candidate {
    const ZERO: Self = Self {
        feature: Feature::ZERO,
        cell: [0; 3],
        layout: 0,
    };
}

#[derive(Debug, Clone, Copy)]
struct PreparedCraterDescriptor {
    age: u64,
    cell: [i64; 3],
    center: DVec3,
    radius: f64,
    axis: DVec3,
    axis_b: DVec3,
    ellipticity: f64,
    fresh: f64,
    phase_a: f64,
    phase_b: f64,
    width: f64,
}

impl PreparedCraterDescriptor {
    const ZERO: Self = Self {
        age: 0,
        cell: [0; 3],
        center: DVec3::ZERO,
        radius: 0.0,
        axis: DVec3::ZERO,
        axis_b: DVec3::ZERO,
        ellipticity: 0.0,
        fresh: 0.0,
        phase_a: 0.0,
        phase_b: 0.0,
        width: 0.0,
    };
}

#[derive(Debug)]
struct PreparedEpochLayoutPage {
    candidates: [PreparedCraterDescriptor; 27],
    count: usize,
}

fn build_prepared_page(
    epoch: Epoch,
    layout: Layout,
    base: [i64; 3],
    radius_m: f64,
) -> PreparedEpochLayoutPage {
    let mut page = PreparedEpochLayoutPage {
        candidates: [PreparedCraterDescriptor::ZERO; 27],
        count: 0,
    };
    if epoch.relief_m == 0.0 {
        return page;
    }
    for z in -1..=1 {
        for y in -1..=1 {
            for x in -1..=1 {
                let cell = [base[0] + x, base[1] + y, base[2] + z];
                let key = cell_key(layout.salt, cell);
                if unit(key ^ 0x4445_4e53) > epoch.density {
                    continue;
                }
                let jitter = DVec3::new(unit(key ^ 1), unit(key ^ 2), unit(key ^ 3))
                    * (2.0 * JITTER)
                    - DVec3::splat(JITTER);
                let raw = (DVec3::new(cell[0] as f64, cell[1] as f64, cell[2] as f64) + jitter)
                    * epoch.edge_m
                    - layout.offset;
                let raw_radius = raw.length();
                if raw_radius <= 0.0 || (raw_radius - radius_m).abs() > epoch.edge_m * SHELL {
                    continue;
                }
                let center = raw * (radius_m / raw_radius);
                let fresh = unit(key ^ 0x0041_4745);
                let descriptor = PreparedCraterDescriptor {
                    age: mix(key ^ 0x0041_4745),
                    cell,
                    center,
                    radius: epoch.edge_m * SUPPORT * (0.42 + 0.40 * unit(key ^ 0x10)),
                    axis: unit_vector(key ^ 0x11),
                    axis_b: unit_vector(key ^ 0x12),
                    ellipticity: 0.04 + 0.16 * unit(key ^ 0x13),
                    fresh,
                    phase_a: unit(key ^ 0x20) * std::f64::consts::TAU,
                    phase_b: unit(key ^ 0x21) * std::f64::consts::TAU,
                    width: 0.10 + 0.14 * (1.0 - fresh),
                };
                page.candidates[page.count] = descriptor;
                page.count += 1;
            }
        }
    }
    page.candidates[..page.count].sort_unstable_by_key(|candidate| (candidate.age, candidate.cell));
    page
}

fn prepared_feature(p: DVec3, epoch: Epoch, d: PreparedCraterDescriptor, radius_m: f64) -> Feature {
    let delta = p - d.center;
    let support = epoch.edge_m * SUPPORT;
    let distance = delta.length();
    if distance >= support {
        return Feature::ZERO;
    }
    let q = Differential {
        value: delta.dot(d.axis) / d.radius,
        gradient: d.axis / d.radius,
    };
    let q_b = Differential {
        value: delta.dot(d.axis_b) / d.radius,
        gradient: d.axis_b / d.radius,
    };
    let r2 = Differential {
        value: delta.length_squared() / (d.radius * d.radius),
        gradient: delta * (2.0 / (d.radius * d.radius)),
    };
    let radial = (r2 + q * q * d.ellipticity).sqrt();
    let pattern = ((q * 5.0 + Differential::constant(d.phase_a)).sin()
        + (q_b * 9.0 + Differential::constant(d.phase_b)).sin() * 0.5)
        * (2.0 / 3.0);
    let gate = radial.smoothstep(0.15, 0.65);
    let s = radial + pattern * gate * (0.025 + 0.065 * (1.0 - d.fresh));
    let floor = -(Differential::constant(1.0) - s.smoothstep(0.30, 0.95 + 0.10 * (1.0 - d.fresh)))
        * (0.32 + 0.30 * d.fresh);
    let sector = pattern.smoothstep(-0.3, 0.5) * 0.55 + Differential::constant(0.45);
    let ring = ((s - Differential::constant(1.0)) * (1.0 / d.width)).gaussian()
        * gate
        * sector
        * (0.08 + 0.12 * d.fresh);
    let ejecta = ((s - Differential::constant(1.25)) * (1.0 / 0.38)).gaussian()
        * gate
        * (pattern * 0.45 + Differential::constant(0.55))
        * (0.04 + 0.04 * d.fresh);
    let corrugation = ((q * 13.0).sin() + (q_b * 19.0).sin() * 0.5) * (2.0 / 3.0);
    let wall =
        ((s - Differential::constant(0.72)) * (1.0 / 0.33)).gaussian() * corrugation * gate * 0.05;
    let peak = (s * (1.0 / 0.23)).gaussian() * smoothstep(0.01, 0.03, d.radius / radius_m) * 0.08;
    let asym = Differential::constant(1.0) + q * (d.radius / support * 0.08);
    let chord = Differential {
        value: distance / support,
        gradient: if distance > 0.0 {
            delta / (distance * support)
        } else {
            DVec3::ZERO
        },
    };
    let cutoff = Differential::constant(1.0) - chord.smoothstep(0.82, 1.0);
    let profile = (floor + ring + ejecta + wall + peak) * asym * cutoff * (0.88 * epoch.relief_m);
    Feature {
        age: d.age,
        profile,
        influence: cutoff * (Differential::constant(1.0) - s.smoothstep(1.02, 1.45)),
        rock: (sector.value * (1.0 - ((s.value - 1.0) / 0.22).abs()).max(0.0)).clamp(0.0, 1.0),
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    started.elapsed().as_nanos().min(u64::MAX as u128) as u64
}

fn feature(
    p: DVec3,
    epoch: Epoch,
    offset: DVec3,
    cell: [i64; 3],
    key: u64,
    radius_m: f64,
) -> Feature {
    if unit(key ^ 0x4445_4e53) > epoch.density {
        return Feature::ZERO;
    }
    let jitter = DVec3::new(unit(key ^ 1), unit(key ^ 2), unit(key ^ 3)) * (2.0 * JITTER)
        - DVec3::splat(JITTER);
    let raw = (DVec3::new(cell[0] as f64, cell[1] as f64, cell[2] as f64) + jitter) * epoch.edge_m
        - offset;
    let raw_radius = raw.length();
    if raw_radius <= 0.0 || (raw_radius - radius_m).abs() > epoch.edge_m * SHELL {
        return Feature::ZERO;
    }
    let delta = p - raw * (radius_m / raw_radius);
    let support = epoch.edge_m * SUPPORT;
    let distance = delta.length();
    if distance >= support || epoch.relief_m == 0.0 {
        return Feature::ZERO;
    }
    let r = support * (0.42 + 0.40 * unit(key ^ 0x10));
    let axis = unit_vector(key ^ 0x11);
    let axis_b = unit_vector(key ^ 0x12);
    let q = Differential {
        value: delta.dot(axis) / r,
        gradient: axis / r,
    };
    let q_b = Differential {
        value: delta.dot(axis_b) / r,
        gradient: axis_b / r,
    };
    let r2 = Differential {
        value: delta.length_squared() / (r * r),
        gradient: delta * (2.0 / (r * r)),
    };
    let ellipticity = 0.04 + 0.16 * unit(key ^ 0x13);
    let radial = (r2 + q * q * ellipticity).sqrt();
    let fresh = unit(key ^ 0x0041_4745);
    let pattern = ((q * 5.0 + Differential::constant(unit(key ^ 0x20) * std::f64::consts::TAU))
        .sin()
        + (q_b * 9.0 + Differential::constant(unit(key ^ 0x21) * std::f64::consts::TAU)).sin()
            * 0.5)
        * (2.0 / 3.0);
    let gate = radial.smoothstep(0.15, 0.65);
    let s = radial + pattern * gate * (0.025 + 0.065 * (1.0 - fresh));
    let floor = -(Differential::constant(1.0) - s.smoothstep(0.30, 0.95 + 0.10 * (1.0 - fresh)))
        * (0.32 + 0.30 * fresh);
    let sector = pattern.smoothstep(-0.3, 0.5) * 0.55 + Differential::constant(0.45);
    let width = 0.10 + 0.14 * (1.0 - fresh);
    let ring = ((s - Differential::constant(1.0)) * (1.0 / width)).gaussian()
        * gate
        * sector
        * (0.08 + 0.12 * fresh);
    let ejecta = ((s - Differential::constant(1.25)) * (1.0 / 0.38)).gaussian()
        * gate
        * (pattern * 0.45 + Differential::constant(0.55))
        * (0.04 + 0.04 * fresh);
    let corrugation = ((q * 13.0).sin() + (q_b * 19.0).sin() * 0.5) * (2.0 / 3.0);
    let wall =
        ((s - Differential::constant(0.72)) * (1.0 / 0.33)).gaussian() * corrugation * gate * 0.05;
    let peak = (s * (1.0 / 0.23)).gaussian() * smoothstep(0.01, 0.03, r / radius_m) * 0.08;
    let asym = Differential::constant(1.0) + q * (r / support * 0.08);
    let chord = Differential {
        value: distance / support,
        gradient: if distance > 0.0 {
            delta / (distance * support)
        } else {
            DVec3::ZERO
        },
    };
    let cutoff = Differential::constant(1.0) - chord.smoothstep(0.82, 1.0);
    // |floor|+.20+.08+.05+.08 <= 1.03; |asym| <= 1.08.
    // Multiplying by .88 keeps the profile below .979 of the epoch budget.
    let profile = (floor + ring + ejecta + wall + peak) * asym * cutoff * (0.88 * epoch.relief_m);
    Feature {
        age: mix(key ^ 0x0041_4745),
        profile,
        // Excavation replaces the older floor and wall, while the ejecta fringe
        // blends into the older surface instead of clearing the whole support.
        influence: cutoff * (Differential::constant(1.0) - s.smoothstep(1.02, 1.45)),
        rock: (sector.value * (1.0 - ((s.value - 1.0) / 0.22).abs()).max(0.0)).clamp(0.0, 1.0),
    }
}
fn unit(key: u64) -> f64 {
    (mix(key) >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
}
fn cell_key(salt: u64, cell: [i64; 3]) -> u64 {
    let mut value = salt;
    for (coordinate, tag) in cell.into_iter().zip([
        0xd6e8_feb8_6659_fd93,
        0xa5a3_56e4_27f8_862f,
        0x9e37_79b1_85eb_ca87,
    ]) {
        value = mix(value ^ (coordinate as u64).wrapping_mul(tag));
    }
    value
}

/// Value and analytic Cartesian derivative in the current input coordinates.
/// Metre-space feature derivatives convert to unit-direction derivatives once,
/// at the cell-layout boundary. Background derivatives already use directions.
#[derive(Debug, Clone, Copy)]
struct Differential {
    value: f64,
    gradient: DVec3,
}
impl Differential {
    const ZERO: Self = Self {
        value: 0.0,
        gradient: DVec3::ZERO,
    };
    fn constant(value: f64) -> Self {
        Self {
            value,
            gradient: DVec3::ZERO,
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
    fn sin(self) -> Self {
        Self {
            value: self.value.sin(),
            gradient: self.gradient * self.value.cos(),
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
        Self {
            value: smoothstep(lo, hi, self.value),
            gradient: self.gradient * smoothstep_derivative(lo, hi, self.value),
        }
    }
}
impl Add for Differential {
    type Output = Self;
    fn add(self, b: Self) -> Self {
        Self {
            value: self.value + b.value,
            gradient: self.gradient + b.gradient,
        }
    }
}
impl Sub for Differential {
    type Output = Self;
    fn sub(self, b: Self) -> Self {
        self + (-b)
    }
}
impl Neg for Differential {
    type Output = Self;
    fn neg(self) -> Self {
        Self {
            value: -self.value,
            gradient: -self.gradient,
        }
    }
}
impl Mul for Differential {
    type Output = Self;
    fn mul(self, b: Self) -> Self {
        Self {
            value: self.value * b.value,
            gradient: self.gradient * b.value + b.gradient * self.value,
        }
    }
}
impl Mul<f64> for Differential {
    type Output = Self;
    fn mul(self, b: f64) -> Self {
        Self {
            value: self.value * b,
            gradient: self.gradient * b,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn field(seed: u64, radius: f64) -> AncientField {
        AncientField::new(
            &MoonTerrainDefinition::with_version(
                TerrainIdentity(77),
                TerrainSeed(seed),
                MoonTerrainConfig::default(),
                MoonTerrainVersion::MoonLikeV2,
            ),
            radius,
        )
        .unwrap()
    }
    #[test]
    fn compact_profiles_have_independent_cartesian_derivatives() {
        let f = field(2, 109_000.0);
        let epoch = f.epochs[4];
        let layout = epoch.layouts[0];
        let mut active = None;
        let base = checked_cell((DVec3::Z * f.radius_m + layout.offset) / epoch.edge_m).unwrap();
        for z in -3..=3 {
            for y in -3..=3 {
                for x in -3..=3 {
                    let cell = [base[0] + x, base[1] + y, base[2] + z];
                    let key = cell_key(layout.salt, cell);
                    let jitter = DVec3::new(unit(key ^ 1), unit(key ^ 2), unit(key ^ 3))
                        * (2.0 * JITTER)
                        - DVec3::splat(JITTER);
                    let raw = (DVec3::new(cell[0] as f64, cell[1] as f64, cell[2] as f64) + jitter)
                        * epoch.edge_m
                        - layout.offset;
                    if (raw.length() - f.radius_m).abs() <= epoch.edge_m * SHELL {
                        active = Some((cell, key, raw.normalize() * f.radius_m));
                    }
                }
            }
        }
        let (cell, key, center) = active.expect("fixture contains shell features");
        let direction = DVec3::new(0.31, -0.44, 0.79).normalize();
        for ratio in [
            0.0, 0.1, 0.3, 0.55, 0.7, 0.81999, 0.82001, 0.95, 0.99999, 1.00001,
        ] {
            let p = center + direction * (epoch.edge_m * SUPPORT * ratio);
            let sample = |p| feature(p, epoch, layout.offset, cell, key, f.radius_m);
            let v = sample(p);
            for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                let epsilon = 1e-4;
                let plus = sample(p + axis * epsilon);
                let minus = sample(p - axis * epsilon);
                let profile_fd = (plus.profile.value - minus.profile.value) / (2.0 * epsilon);
                let influence_fd = (plus.influence.value - minus.influence.value) / (2.0 * epsilon);
                assert!(
                    (profile_fd - v.profile.gradient.dot(axis)).abs() < 1e-5,
                    "profile at {ratio}: fd {profile_fd}, analytic {:?}",
                    v.profile.gradient
                );
                assert!((influence_fd - v.influence.gradient.dot(axis)).abs() < 1e-7);
            }
            assert!(v.profile.value.abs() <= epoch.relief_m);
        }
    }
    #[test]
    fn translated_rotated_halos_match_three_cell_oracle() {
        for radius in [20.0, 109_000.0, 1_737_000.0] {
            let f = field(19, radius);
            for index in 0..24 {
                let z = 1.0 - 2.0 * (index as f64 + 0.5) / 24.0;
                let angle = index as f64 * 2.399963229728653;
                let r = (1.0 - z * z).sqrt();
                let location = SurfaceLocation::new(
                    Direction3::try_new(DVec3::new(r * angle.cos(), r * angle.sin(), z)).unwrap(),
                );
                let a = f.evaluate_halo(location, 1).unwrap();
                let b = f.evaluate_halo(location, 3).unwrap();
                assert_eq!(a.height_m.to_bits(), b.height_m.to_bits());
                assert_eq!(a.gradient, b.gradient);
                assert_eq!(a.rock, b.rock);
            }
        }
    }
    #[test]
    fn overlap_and_grid_plane_gradients_include_context_and_composition() {
        let f = field(7, 109_000.0);
        let sample = |n| {
            f.evaluate(SurfaceLocation::new(Direction3::try_new(n).unwrap()))
                .unwrap()
        };
        for epoch in f.epochs {
            for layout in epoch.layouts {
                let cell = (f.radius_m * 0.4 / epoch.edge_m).floor();
                let x = cell * epoch.edge_m - layout.offset.x;
                let local =
                    DVec3::new(x, (f.radius_m * f.radius_m - x * x).sqrt(), 0.0) / f.radius_m;
                let n = layout.rotation.transpose() * local;
                let tangent =
                    tangent_project(n, layout.rotation.transpose() * DVec3::X).normalize();
                let epsilon = 1e-8;
                let analytic = sample(n).gradient.dot(tangent);
                let fd = (sample((n + tangent * epsilon).normalize()).height_m
                    - sample((n - tangent * epsilon).normalize()).height_m)
                    / (2.0 * epsilon);
                assert!(
                    (fd - analytic).abs() < 0.3 + analytic.abs() * 0.001,
                    "fd={fd}, analytic={analytic}"
                );
            }
        }
    }
}
