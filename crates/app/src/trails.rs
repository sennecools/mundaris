//! Bounded app-owned actual committed f64 history, independent of FrameId lifetime.
use anyhow::{Result, ensure};
use glam::DVec3;
use mundaris_math::{FrameId, FramePosition, LocalPosition};
use mundaris_renderer::{CelestialProjection, DebugLine, PreparedView};
use mundaris_world::{BodyId, CelestialFrameProjection, CelestialSystem};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailMode {
    Inertial,
    SimultaneousBodyRelative(BodyId),
}
/// Reused screen-space simplification scratch. Retains only existing vertices;
/// endpoints and Cartesian extrema are mandatory, caps disclose missed tolerance.
#[derive(Default)]
pub struct TrailDisplayScratch {
    screens: Vec<Option<[f64; 2]>>,
    keep: Vec<bool>,
    stack: Vec<(usize, usize)>,
}
impl TrailDisplayScratch {
    pub fn simplify(
        &mut self,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        limit: usize,
        points: &mut Vec<FramePosition>,
        colors: &mut Vec<[f32; 4]>,
    ) -> Result<bool> {
        ensure!(
            points.len() == colors.len() && limit >= 8,
            "invalid trail display buffers/cap"
        );
        if points.len() < 3 {
            return Ok(false);
        }
        let source = view.prepare_source(points[0].frame())?;
        self.screens.clear();
        for &point in points.iter() {
            self.screens
                .push(projection.project_pixels(source.view_displacement(point)?.metres())?);
        }
        self.keep.clear();
        self.keep.resize(points.len(), false);
        self.keep[0] = true;
        self.keep[points.len() - 1] = true;
        for axis in 0..3 {
            let low = (0..points.len())
                .min_by(|&a, &b| {
                    points[a].local().metres()[axis].total_cmp(&points[b].local().metres()[axis])
                })
                .expect("nonempty");
            let high = (0..points.len())
                .max_by(|&a, &b| {
                    points[a].local().metres()[axis].total_cmp(&points[b].local().metres()[axis])
                })
                .expect("nonempty");
            self.keep[low] = true;
            self.keep[high] = true;
        }
        self.stack.clear();
        let mut previous = 0;
        for (i, &keep) in self.keep.iter().enumerate().skip(1) {
            if keep {
                self.stack.push((previous, i));
                previous = i;
            }
        }
        let mut count = self.keep.iter().filter(|&&k| k).count();
        let mut coarse = false;
        while let Some((a, b)) = self.stack.pop() {
            if b <= a + 1 {
                continue;
            }
            let mut worst = 0.5;
            let mut candidate = None;
            for i in a + 1..b {
                let error = match (self.screens[a], self.screens[b], self.screens[i]) {
                    (Some(a), Some(b), Some(p)) => crate::orbit_guides::screen_chord_error(a, b, p),
                    (None, None, None) => 0.0,
                    _ => f64::INFINITY,
                };
                if error > worst {
                    worst = error;
                    candidate = Some(i);
                }
            }
            if let Some(i) = candidate {
                if count >= limit {
                    coarse = true;
                    continue;
                }
                self.keep[i] = true;
                count += 1;
                self.stack.push((a, i));
                self.stack.push((i, b));
            }
        }
        let mut write = 0;
        for i in 0..points.len() {
            if self.keep[i] {
                points[write] = points[i];
                colors[write] = colors[i];
                write += 1;
            }
        }
        points.truncate(write);
        colors.truncate(write);
        Ok(coarse)
    }
}
#[derive(Clone, Copy, Default)]
struct Sample {
    tick: u64,
    time_s: f64,
}
pub struct TrailHistory {
    ids: Box<[BodyId]>,
    positions: Box<[DVec3]>,
    samples: Box<[Sample]>,
    head: usize,
    len: usize,
    stride: u64,
    branch: u64,
    last_commit: u64,
    direction: i8,
    mode: TrailMode,
}
impl TrailHistory {
    pub fn new(system: &CelestialSystem, stride: u64) -> Result<Self> {
        Self::with_limits(system, stride, 8192, 8 * 1024 * 1024)
    }
    pub fn with_limits(
        system: &CelestialSystem,
        stride: u64,
        max_samples: usize,
        max_bytes: usize,
    ) -> Result<Self> {
        ensure!(
            stride > 0 && max_samples > 0 && max_samples <= 8192 && max_bytes <= 8 * 1024 * 1024,
            "invalid trail policy"
        );
        let ids: Box<[_]> = system.bodies().map(|(id, _)| id).collect();
        let overhead = std::mem::size_of::<Self>()
            .checked_add(std::mem::size_of_val(&*ids))
            .ok_or_else(|| anyhow::anyhow!("trail payload overflow"))?;
        let sample_bytes = ids
            .len()
            .checked_mul(std::mem::size_of::<DVec3>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Sample>()))
            .ok_or_else(|| anyhow::anyhow!("trail payload overflow"))?;
        let capacity = max_samples.min(
            max_bytes
                .checked_sub(overhead)
                .ok_or_else(|| anyhow::anyhow!("trail byte cap too small"))?
                / sample_bytes,
        );
        ensure!(
            capacity > 0,
            "trail byte cap cannot hold one complete sample"
        );
        let mut result = Self {
            positions: vec![DVec3::ZERO; capacity * ids.len()].into_boxed_slice(),
            samples: vec![Sample::default(); capacity].into_boxed_slice(),
            ids,
            head: 0,
            len: 0,
            stride,
            branch: 0,
            last_commit: 0,
            direction: 0,
            mode: TrailMode::Inertial,
        };
        result.clear_and_seed(0, 0, system);
        Ok(result)
    }
    pub fn capacity(&self) -> usize {
        self.samples.len()
    }
    pub fn sample_count(&self) -> usize {
        self.len
    }
    pub fn stride(&self) -> u64 {
        self.stride
    }
    pub fn mode(&self) -> TrailMode {
        self.mode
    }
    pub fn payload_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + std::mem::size_of_val(&*self.ids)
            + std::mem::size_of_val(&*self.positions)
            + std::mem::size_of_val(&*self.samples)
    }
    pub fn retained_time_s(&self) -> Option<(f64, f64)> {
        (self.len > 0).then(|| {
            (
                self.samples[self.head].time_s,
                self.samples[(self.head + self.len - 1) % self.capacity()].time_s,
            )
        })
    }
    /// Exact timestamps retained in chronological publication order.
    pub fn retained_times_s(&self) -> Vec<f64> {
        (0..self.len)
            .map(|i| self.samples[(self.head + i) % self.capacity()].time_s)
            .collect()
    }
    pub fn retained_ticks(&self) -> Vec<u64> {
        (0..self.len)
            .map(|i| self.samples[(self.head + i) % self.capacity()].tick)
            .collect()
    }
    pub fn clear_and_seed(&mut self, branch: u64, tick: u64, system: &CelestialSystem) {
        self.head = 0;
        self.len = 0;
        self.branch = branch;
        self.last_commit = tick;
        self.direction = 0;
        self.push(tick, system);
    }
    /// Reset analytic history at an explicit discontinuity (seek/reset/load/rate change).
    pub fn clear_and_seed_analytic(&mut self, system: &CelestialSystem) {
        self.head = 0;
        self.len = 0;
        self.branch = 0;
        self.direction = 0;
        self.last_commit = system.revision();
        self.push(system.revision(), system);
    }
    /// Record one complete synchronized analytic publication, with no tick stride.
    pub fn record_analytic_publication(&mut self, system: &CelestialSystem) {
        let time = system.sample_time().seconds_since_epoch();
        let previous = self.samples[(self.head + self.len - 1) % self.capacity()].time_s;
        if time == previous {
            return;
        }
        let direction = if time > previous {
            1
        } else if time < previous {
            -1
        } else {
            0
        };
        if direction != 0 && self.direction != 0 && direction != self.direction {
            self.clear_and_seed_analytic(system);
            self.direction = direction;
            return;
        }
        if direction != 0 {
            self.direction = direction;
        }
        self.last_commit = system.revision();
        self.push(system.revision(), system);
    }
    pub fn set_mode(
        &mut self,
        mode: TrailMode,
        _branch: u64,
        _tick: u64,
        system: &CelestialSystem,
    ) -> Result<()> {
        if let TrailMode::SimultaneousBodyRelative(id) = mode {
            system.body(id)?;
            ensure!(self.ids.contains(&id), "reference body not recorded");
        }
        self.mode = mode;
        // Reference/mode are presentation only; synchronized absolute records survive.
        Ok(())
    }
    /// Called for every successful public commit, including substeps. Only whole
    /// synchronized samples are stored. Direction changes seed a separate strip.
    pub fn record_committed(&mut self, branch: u64, tick: u64, system: &CelestialSystem) {
        let direction = if tick > self.last_commit {
            1
        } else if tick < self.last_commit {
            -1
        } else {
            0
        };
        if branch != self.branch
            || (direction != 0 && self.direction != 0 && direction != self.direction)
            || (direction < 0 && self.direction == 0)
        {
            self.clear_and_seed(branch, tick, system);
            self.direction = direction;
            return;
        }
        if direction != 0 {
            self.direction = direction;
        }
        self.last_commit = tick;
        if tick.is_multiple_of(self.stride)
            && self.samples[(self.head + self.len - 1) % self.capacity()].tick != tick
        {
            self.push(tick, system);
        }
    }
    fn push(&mut self, tick: u64, system: &CelestialSystem) {
        assert_eq!(
            system.body_count(),
            self.ids.len(),
            "trail topology requires explicit replacement"
        );
        let slot = (self.head + self.len) % self.capacity();
        for (i, ((id, body), &recorded)) in system.bodies().zip(&self.ids).enumerate() {
            assert_eq!(id, recorded, "trail body identity belongs to session");
            self.positions[slot * self.ids.len() + i] = body.state().center_in_system().metres();
        }
        self.samples[slot] = Sample {
            tick,
            time_s: system.sample_time().seconds_since_epoch(),
        };
        if self.len == self.capacity() {
            self.head = (self.head + 1) % self.capacity();
        } else {
            self.len += 1;
        }
    }
    /// Current endpoint is derived separately and does not change recorded cadence.
    /// Relative history subtracts reference and body at the SAME historical sample.
    pub fn prepare_lines(
        &self,
        system: &CelestialSystem,
        projection: &CelestialFrameProjection,
        output: &mut Vec<DebugLine>,
    ) -> Result<FrameId> {
        output.clear();
        ensure!(
            system.body_count() == self.ids.len(),
            "trail topology mismatch"
        );
        let (source, reference) = match self.mode {
            TrailMode::Inertial => (projection.tree().root(), None),
            TrailMode::SimultaneousBodyRelative(id) => (
                projection.frames_for(id)?.translating,
                Some(
                    self.ids
                        .iter()
                        .position(|&body| body == id)
                        .ok_or_else(|| anyhow::anyhow!("missing trail reference"))?,
                ),
            ),
        };
        for (body_index, &id) in self.ids.iter().enumerate() {
            let mut previous = None;
            for sample in 0..self.len {
                let slot = (self.head + sample) % self.capacity();
                let positions = &self.positions[slot * self.ids.len()..(slot + 1) * self.ids.len()];
                let value = positions[body_index] - reference.map_or(DVec3::ZERO, |i| positions[i]);
                let point = FramePosition::new(source, LocalPosition::try_metres(value)?);
                if let Some(start) = previous {
                    output.push(DebugLine {
                        endpoints: [start, point],
                        color: crate::gravity_fixtures::body_color(body_index),
                    });
                }
                previous = Some(point);
            }
            let last = (self.head + self.len - 1) % self.capacity();
            if self.samples[last].time_s != system.sample_time().seconds_since_epoch() {
                let current = system.body(id)?.state().center_in_system().metres()
                    - if let Some(i) = reference {
                        system
                            .body(self.ids[i])?
                            .state()
                            .center_in_system()
                            .metres()
                    } else {
                        DVec3::ZERO
                    };
                if let Some(start) = previous {
                    output.push(DebugLine {
                        endpoints: [
                            start,
                            FramePosition::new(source, LocalPosition::try_metres(current)?),
                        ],
                        color: crate::gravity_fixtures::body_color(body_index),
                    });
                }
            }
        }
        Ok(source)
    }
    /// Bounded display of existing committed vertices and the current endpoint.
    /// Uniform subsampling is explicitly reported as capped, not a screen-error guarantee.
    pub fn display_points(
        &self,
        system: &CelestialSystem,
        projection: &CelestialFrameProjection,
        body: BodyId,
        limit: usize,
        points: &mut Vec<FramePosition>,
        colors: &mut Vec<[f32; 4]>,
    ) -> Result<bool> {
        ensure!(limit >= 2, "trail display needs two vertices");
        let index = self
            .ids
            .iter()
            .position(|&id| id == body)
            .ok_or_else(|| anyhow::anyhow!("body not recorded"))?;
        let (source, reference) = match self.mode {
            TrailMode::Inertial => (projection.tree().root(), None),
            TrailMode::SimultaneousBodyRelative(id) => (
                projection.frames_for(id)?.translating,
                self.ids.iter().position(|&b| b == id),
            ),
        };
        points.clear();
        colors.clear();
        let count = self.len.min(limit - 1);
        let first = self.samples[self.head].time_s;
        let now = system.sample_time().seconds_since_epoch();
        let span = (now - first).abs();
        let color = crate::gravity_fixtures::body_color(index);
        for sample in 0..count {
            let ordinal = if count <= 1 {
                0
            } else {
                sample * (self.len - 1) / (count - 1)
            };
            let slot = (self.head + ordinal) % self.capacity();
            let values = &self.positions[slot * self.ids.len()..(slot + 1) * self.ids.len()];
            let value = values[index] - reference.map_or(DVec3::ZERO, |r| values[r]);
            points.push(FramePosition::new(
                source,
                LocalPosition::try_metres(value)?,
            ));
            let age = if span > 0.0 {
                ((self.samples[slot].time_s - first).abs() / span).clamp(0.0, 1.0)
            } else {
                1.0
            };
            colors.push([color[0], color[1], color[2], (0.15 + 0.85 * age) as f32]);
        }
        let current = system.body(body)?.state().center_in_system().metres()
            - if let Some(r) = reference {
                system
                    .body(self.ids[r])?
                    .state()
                    .center_in_system()
                    .metres()
            } else {
                DVec3::ZERO
            };
        if self.samples[(self.head + self.len - 1) % self.capacity()].time_s != now {
            points.push(FramePosition::new(
                source,
                LocalPosition::try_metres(current)?,
            ));
            colors.push(color);
        }
        Ok(self.len > limit - 1)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gravity_fixtures::GravityFixture;
    use mundaris_simulation::*;
    use std::{num::NonZeroU64, time::Duration};
    #[test]
    fn display_keeps_committed_vertices_extrema_age_and_screen_tolerance() {
        let mut world = GravityFixture::Circular
            .create(NonZeroU64::new(1).unwrap())
            .unwrap();
        let id = world.bodies().nth(1).unwrap().0;
        let mut runner =
            FixedStepRunner::new(&world, SimulationConfig::try_new(10.0).unwrap()).unwrap();
        let mut history = TrailHistory::new(&world, 1).unwrap();
        runner.request_forward_to_tick(1000).unwrap();
        while runner.tick() < 1000 {
            runner
                .pump(&mut world, |tick, w| history.record_committed(0, tick, w))
                .unwrap();
        }
        let frames = CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
        let root = frames.tree().root();
        let view = PreparedView::new(
            &frames.tree().evaluate(),
            mundaris_math::FramePose::new(
                FramePosition::new(root, LocalPosition::try_metres(DVec3::Z * 1e8).unwrap()),
                mundaris_math::UnitRotation::identity(),
            ),
            mundaris_renderer::RenderPrecisionBudget::near_debug(),
        )
        .unwrap();
        let projection = CelestialProjection::try_new(1280, 800, 1.0, 0.1).unwrap();
        let mut points = Vec::new();
        let mut colors = Vec::new();
        history
            .display_points(&world, &frames, id, 8193, &mut points, &mut colors)
            .unwrap();
        let original = points.clone();
        assert!((colors[0][3] - 0.15).abs() < 1e-7);
        assert_eq!(colors.last().unwrap()[3], 1.0);
        let mut scratch = TrailDisplayScratch::default();
        assert!(
            !scratch
                .simplify(&view, projection, 1024, &mut points, &mut colors)
                .unwrap()
        );
        assert_eq!(points.first(), original.first());
        assert_eq!(points.last(), original.last());
        assert!(points.iter().all(|p| original.contains(p)));
        for axis in 0..3 {
            for maximum in [false, true] {
                let extreme = original
                    .iter()
                    .reduce(|a, b| {
                        if (b.local().metres()[axis] > a.local().metres()[axis]) == maximum {
                            b
                        } else {
                            a
                        }
                    })
                    .unwrap();
                assert!(points.contains(extreme));
            }
        }
        let source = view.prepare_source(root).unwrap();
        let project = |point: FramePosition| {
            projection
                .project_pixels(source.view_displacement(point).unwrap().metres())
                .unwrap()
                .unwrap()
        };
        for &point in &original {
            let p = project(point);
            let error = points
                .windows(2)
                .map(|pair| {
                    crate::orbit_guides::screen_chord_error(project(pair[0]), project(pair[1]), p)
                })
                .fold(f64::MAX, f64::min);
            assert!(error <= 0.5 + 1e-9);
        }
    }
    #[test]
    fn all_committed_substeps_same_stride_eviction_direction_and_rebuild() {
        let mut expected = None;
        for slices in [1, 30, 144] {
            let mut world = GravityFixture::Circular
                .create(NonZeroU64::new(1).unwrap())
                .unwrap();
            let mut runner =
                FixedStepRunner::new(&world, SimulationConfig::try_new(10.0).unwrap()).unwrap();
            let mut trails = TrailHistory::with_limits(&world, 8, 4, 8 * 1024 * 1024).unwrap();
            runner.set_rate(PlaybackRate::try_multiplier(1000.0).unwrap());
            runner.set_paused(false);
            let total = 1_000_000_000u64;
            let slice = total / slices;
            for i in 0..slices {
                runner
                    .admit_wall_elapsed(Duration::from_nanos(if i + 1 == slices {
                        total - slice * (slices - 1)
                    } else {
                        slice
                    }))
                    .unwrap();
                runner
                    .pump(&mut world, |tick, world| {
                        trails.record_committed(0, tick, world)
                    })
                    .unwrap();
            }
            let ticks = trails.retained_ticks();
            assert_eq!(ticks, [72, 80, 88, 96]);
            if let Some(previous) = &expected {
                assert_eq!(&ticks, previous);
            } else {
                expected = Some(ticks);
            }
            let mut lines = Vec::new();
            let projection =
                CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
            trails
                .prepare_lines(&world, &projection, &mut lines)
                .unwrap();
            assert_eq!(lines.len(), 8);
            let rebuilt =
                CelestialFrameProjection::build(&world, NonZeroU64::new(2).unwrap()).unwrap();
            let before = trails.retained_ticks();
            trails.prepare_lines(&world, &rebuilt, &mut lines).unwrap();
            assert_eq!(trails.retained_ticks(), before);
            runner.single_step(false).unwrap();
            runner
                .pump(&mut world, |tick, w| trails.record_committed(0, tick, w))
                .unwrap();
            assert_eq!(trails.retained_ticks(), [99]);
            runner.single_step(true).unwrap();
            runner
                .pump(&mut world, |tick, w| trails.record_committed(0, tick, w))
                .unwrap();
            assert_eq!(trails.retained_ticks(), [100]);
            assert!(trails.payload_bytes() <= 8 * 1024 * 1024);
        }
    }
    #[test]
    fn simultaneous_relative_history_and_seed_invalidation() {
        let mut world = GravityFixture::Hierarchy
            .create(NonZeroU64::new(1).unwrap())
            .unwrap();
        let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
        let mut history = TrailHistory::with_limits(&world, 1, 4, 8 * 1024 * 1024).unwrap();
        let mut runner =
            FixedStepRunner::new(&world, SimulationConfig::try_new(60.0).unwrap()).unwrap();
        history
            .set_mode(TrailMode::SimultaneousBodyRelative(ids[1]), 0, 0, &world)
            .unwrap();
        for _ in 0..2 {
            runner.single_step(true).unwrap();
            runner
                .pump(&mut world, |tick, w| history.record_committed(0, tick, w))
                .unwrap();
        }
        let projection =
            CelestialFrameProjection::build(&world, NonZeroU64::new(1).unwrap()).unwrap();
        let mut lines = Vec::new();
        let source = history
            .prepare_lines(&world, &projection, &mut lines)
            .unwrap();
        assert_eq!(source, projection.frames_for(ids[1]).unwrap().translating);
        // Moon's first endpoint is its true epoch relative separation, not today's planet.
        assert!((lines[4].endpoints[0].local().metres() - DVec3::X * 1e8).length() < 1e-4);
        history.clear_and_seed(1, 0, &world);
        assert_eq!(history.sample_count(), 1);
    }
}
