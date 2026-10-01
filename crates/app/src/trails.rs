//! Bounded app-owned actual committed f64 history, independent of FrameId lifetime.
use anyhow::{Result, ensure};
use glam::DVec3;
use mundaris_math::{FrameId, FramePosition, LocalPosition};
use mundaris_renderer::DebugLine;
use mundaris_world::{BodyId, CelestialFrameProjection, CelestialSystem};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailMode {
    Inertial,
    SimultaneousBodyRelative(BodyId),
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
    pub fn set_mode(
        &mut self,
        mode: TrailMode,
        branch: u64,
        tick: u64,
        system: &CelestialSystem,
    ) -> Result<()> {
        if let TrailMode::SimultaneousBodyRelative(id) = mode {
            system.body(id)?;
            ensure!(self.ids.contains(&id), "reference body not recorded");
        }
        self.mode = mode;
        self.clear_and_seed(branch, tick, system);
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
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::gravity_fixtures::GravityFixture;
    use mundaris_simulation::*;
    use std::{num::NonZeroU64, time::Duration};
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
