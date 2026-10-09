//! Bounded frame-performance history for the Studio profiler and the developer
//! protocol. It stores samples only; it performs no capture, benchmark, renderer
//! control or terrain query. Action flags are consumed by the frame owner.

use std::{collections::VecDeque, sync::Arc, time::Instant};

use serde_json::Value;

use crate::developer_snapshot::{DeveloperSnapshot, GpuScopeSnapshot};

/// Frames of history kept for charts and percentiles (10 s at 60 Hz).
pub const HISTORY_CAPACITY: usize = 600;

/// One completed frame's headline timings in milliseconds.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameSample {
    pub frame: u64,
    /// Wall-clock time since the previous observed frame (presentation pacing).
    pub interval_ms: Option<f64>,
    pub host_ms: Option<f64>,
    pub cpu_ms: Option<f64>,
    /// GPU frame time measured for this frame's own submission; `None` when the
    /// single timestamp readback slot did not sample it.
    pub gpu_ms: Option<f64>,
    /// Renderer submission of this frame, used to attribute later GPU samples.
    pub submission: Option<u64>,
    pub ui_render_ms: Option<f64>,
    pub terrain_jobs: Option<u64>,
}

impl FrameSample {
    fn from_snapshot(snapshot: &DeveloperSnapshot) -> Self {
        let jobs = snapshot
            .terrain_atlas
            .as_ref()
            .and_then(|atlas| atlas["bodies"].as_array())
            .map(|bodies| {
                bodies
                    .iter()
                    .filter_map(|body| body["stats"]["jobs_last_frame"].as_u64())
                    .sum()
            });
        Self {
            frame: snapshot.general.frame_number,
            interval_ms: None,
            host_ms: snapshot.performance.host_frame_ms,
            cpu_ms: snapshot.performance.frame_cpu_ms,
            gpu_ms: None,
            submission: snapshot.performance.native_submission_id,
            ui_render_ms: snapshot.performance.ui_render_ms,
            terrain_jobs: jobs,
        }
    }
}

/// GPU timestamp scopes of one sampled frame, relative to its GPU frame start.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuFrame {
    pub frame: u64,
    pub scopes: Vec<GpuScopeSnapshot>,
}

/// Controls shared by the Studio UI and the developer protocol.
#[derive(Debug, Default, Clone, Copy)]
pub struct ProfilerControls {
    pub enabled: Option<bool>,
    pub freeze: Option<bool>,
    pub export: bool,
}

pub struct Profiler {
    pub enabled: bool,
    pub paused: bool,
    pub capture_requested: bool,
    pub capture_stop_requested: bool,
    /// Frame shown in the timeline; `None` follows the latest profiled frame.
    pub selected_frame: Option<u64>,
    /// Index of the selected span in the shown timeline.
    pub selected_span: Option<usize>,
    /// Timeline spans the whole frame including UI drawing, instead of
    /// fitting the engine work.
    pub full_frame: bool,
    history: VecDeque<FrameSample>,
    gpu_frames: VecDeque<GpuFrame>,
    last_gpu_source: Option<u64>,
    last_observed: Option<Instant>,
    latest_profile: Option<Arc<Value>>,
    frozen_snapshot: Option<Arc<DeveloperSnapshot>>,
    exporter: crate::profile_export::TimelineExporter,
    export_status: Option<String>,
}

impl Default for Profiler {
    fn default() -> Self {
        Self {
            enabled: false,
            paused: false,
            capture_requested: false,
            capture_stop_requested: false,
            selected_frame: None,
            selected_span: None,
            full_frame: false,
            history: VecDeque::with_capacity(HISTORY_CAPACITY),
            gpu_frames: VecDeque::new(),
            last_gpu_source: None,
            last_observed: None,
            latest_profile: None,
            frozen_snapshot: None,
            exporter: crate::profile_export::TimelineExporter::default(),
            export_status: None,
        }
    }
}

impl Profiler {
    /// Retain one frame sample. Duplicate frame identifiers are ignored.
    pub fn observe_frame(&mut self, snapshot: &DeveloperSnapshot) {
        let now = Instant::now();
        let interval_ms = self
            .last_observed
            .replace(now)
            .map(|previous| now.duration_since(previous).as_secs_f64() * 1000.0);
        if self.paused {
            return;
        }
        if self
            .history
            .back()
            .is_some_and(|last| last.frame == snapshot.general.frame_number)
        {
            return;
        }
        if self.history.len() == HISTORY_CAPACITY {
            self.history.pop_front();
        }
        self.history.push_back(FrameSample {
            interval_ms,
            ..FrameSample::from_snapshot(snapshot)
        });
        self.attribute_gpu(snapshot);
    }

    /// GPU timings complete a few frames after submission; attach each new
    /// sample to the retained frame that submitted it.
    fn attribute_gpu(&mut self, snapshot: &DeveloperSnapshot) {
        let performance = &snapshot.performance;
        if let Some(source) = performance.gpu_source_frame {
            self.attribute_gpu_sample(source, performance.gpu_frame_ms, &performance.gpu_scopes);
        }
    }

    fn attribute_gpu_sample(
        &mut self,
        source: u64,
        gpu_ms: Option<f64>,
        scopes: &[GpuScopeSnapshot],
    ) {
        if self.last_gpu_source.replace(source) == Some(source) {
            return;
        }
        let Some(sample) = self
            .history
            .iter_mut()
            .rev()
            .find(|sample| sample.submission == Some(source))
        else {
            return;
        };
        sample.gpu_ms = gpu_ms;
        if self.gpu_frames.len() == HISTORY_CAPACITY {
            self.gpu_frames.pop_front();
        }
        self.gpu_frames.push_back(GpuFrame {
            frame: sample.frame,
            scopes: scopes.to_vec(),
        });
    }

    pub fn latest_gpu_frame(&self) -> Option<&GpuFrame> {
        self.gpu_frames.back()
    }

    /// GPU scopes measured for `frame`, if that frame was sampled.
    pub fn gpu_frame(&self, frame: u64) -> Option<&GpuFrame> {
        self.gpu_frames.iter().rev().find(|gpu| gpu.frame == frame)
    }

    /// Retain the latest CPU profile payload at the caller's bounded cadence.
    pub fn ingest(&mut self, snapshot: &DeveloperSnapshot) {
        self.observe_frame(snapshot);
        if !self.enabled || self.paused {
            return;
        }
        self.latest_profile = snapshot.engine_profile.clone();
    }

    pub fn history(&self) -> impl ExactSizeIterator<Item = &FrameSample> {
        self.history.iter()
    }

    /// Percentile of host frame time over the retained history.
    pub fn host_percentile(&self, fraction: f64) -> Option<f64> {
        percentile(self.history.iter().filter_map(|s| s.host_ms), fraction)
    }

    /// Percentile of sampled GPU frame time over the retained history.
    pub fn gpu_percentile(&self, fraction: f64) -> Option<f64> {
        percentile(self.history.iter().filter_map(|s| s.gpu_ms), fraction)
    }

    /// Percentile of the wall-clock interval between frames.
    pub fn interval_percentile(&self, fraction: f64) -> Option<f64> {
        percentile(self.history.iter().filter_map(|s| s.interval_ms), fraction)
    }

    pub fn export_status(&mut self) -> Option<&str> {
        if let Some(status) = self.exporter.poll() {
            self.export_status = Some(status);
        }
        self.export_status.as_deref()
    }

    /// CPU profile shown by the timeline: the frozen one, else the latest.
    pub fn profile(&self) -> Option<Arc<Value>> {
        self.frozen_snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.engine_profile.clone())
            .or_else(|| self.latest_profile.clone())
    }

    pub fn frozen(&self) -> Option<&DeveloperSnapshot> {
        self.frozen_snapshot.as_deref()
    }

    fn freeze(&self, snapshot: Option<&DeveloperSnapshot>) -> Option<Arc<DeveloperSnapshot>> {
        snapshot.cloned().map(|mut snapshot| {
            if snapshot.engine_profile.is_none() {
                snapshot.engine_profile = self.latest_profile.clone();
            }
            Arc::new(snapshot)
        })
    }

    /// Diagnostic controls used by the Studio UI and the developer protocol.
    pub fn configure(
        &mut self,
        snapshot: Option<&DeveloperSnapshot>,
        controls: ProfilerControls,
    ) -> Result<(), &'static str> {
        if let Some(enabled) = controls.enabled {
            self.enabled = enabled;
        }
        if let Some(freeze) = controls.freeze {
            self.frozen_snapshot = if freeze { self.freeze(snapshot) } else { None };
            self.paused = freeze;
        }
        if controls.export {
            let capture = if self.paused {
                self.frozen_snapshot.clone()
            } else {
                self.freeze(snapshot)
            };
            self.exporter
                .request(capture.ok_or("snapshot_unavailable")?)?;
            self.export_status = Some("Export queued; up to 16 MiB, background write".into());
        }
        Ok(())
    }
}

/// Nearest-rank percentile; `None` for an empty sample.
fn percentile(values: impl Iterator<Item = f64>, fraction: f64) -> Option<f64> {
    let mut values: Vec<f64> = values.collect();
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let index = ((fraction * values.len() as f64).ceil() as usize).clamp(1, values.len()) - 1;
    Some(values[index])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_percentile_uses_nearest_rank_and_ignores_missing_samples() {
        let mut profiler = Profiler::default();
        for (frame, host) in [(1, Some(10.0)), (2, None), (3, Some(30.0)), (4, Some(20.0))] {
            profiler.history.push_back(FrameSample {
                frame,
                host_ms: host,
                ..Default::default()
            });
        }
        assert_eq!(profiler.host_percentile(0.5), Some(20.0));
        assert_eq!(profiler.host_percentile(1.0), Some(30.0));
        assert_eq!(Profiler::default().host_percentile(0.5), None);
    }

    #[test]
    fn gpu_samples_attach_to_the_submitting_frame_once() {
        let mut profiler = Profiler::default();
        for frame in 10..14 {
            profiler.history.push_back(FrameSample {
                frame,
                submission: Some(frame + 100),
                ..Default::default()
            });
        }
        let scopes = [GpuScopeSnapshot {
            name: "GPU frame".into(),
            depth: 0,
            start_ms: 0.0,
            end_ms: 0.4,
        }];
        profiler.attribute_gpu_sample(111, Some(0.4), &scopes);
        // The same completed sample is reported again on later frames.
        profiler.attribute_gpu_sample(111, Some(0.4), &scopes);
        let gpu: Vec<_> = profiler.history().map(|s| s.gpu_ms).collect();
        assert_eq!(gpu, [None, Some(0.4), None, None]);
        assert_eq!(profiler.gpu_frame(11).map(|g| g.scopes.len()), Some(1));
        assert!(profiler.gpu_frame(12).is_none());
        // A submission no longer retained is dropped, not misattributed.
        profiler.attribute_gpu_sample(42, Some(9.0), &scopes);
        assert_eq!(profiler.gpu_percentile(1.0), Some(0.4));
    }
}
