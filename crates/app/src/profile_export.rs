//! Bounded Chrome Trace Event export. CPU tracks use process-relative microseconds;
//! GPU durations remain frame-associated metadata without clock calibration.
use crate::developer_snapshot::DeveloperSnapshot;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{self, Write},
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender},
    },
};

const MAX_BYTES: usize = 16 * 1024 * 1024;
const MAX_EVENTS: usize = 16_384;

#[derive(Default)]
pub struct TimelineExporter {
    sender: Option<SyncSender<Arc<DeveloperSnapshot>>>,
    result: Option<Receiver<String>>,
    sequence: u64,
}

impl TimelineExporter {
    /// One in-flight write plus one queued capture. No file work on the caller.
    pub fn request(&mut self, snapshot: Arc<DeveloperSnapshot>) -> Result<(), &'static str> {
        if self.sender.is_none() {
            let root = std::env::var_os("MUNDARIS_DEV_OUTPUT")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("target/mundaris-diagnostics"));
            let (tx, rx) = mpsc::sync_channel::<Arc<DeveloperSnapshot>>(1);
            let (result_tx, result_rx) = mpsc::sync_channel(2);
            let mut sequence = self.sequence;
            std::thread::Builder::new()
                .name("timeline-export".into())
                .spawn(move || {
                    while let Ok(snapshot) = rx.recv() {
                        sequence += 1;
                        let path =
                            root.join(format!("timeline-{}-{sequence}.json", std::process::id()));
                        let result = write_trace(&path, &snapshot)
                            .map(|()| format!("Saved {}", path.display()))
                            .unwrap_or_else(|error| format!("Export failed: {error}"));
                        let _ = result_tx.try_send(result);
                    }
                })
                .map_err(|_| "export_worker_unavailable")?;
            self.sender = Some(tx);
            self.result = Some(result_rx);
        }
        self.sequence += 1;
        self.sender
            .as_ref()
            .ok_or("export_worker_unavailable")?
            .try_send(snapshot)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => "export_queue_full",
                _ => "export_worker_unavailable",
            })
    }

    pub fn poll(&self) -> Option<String> {
        self.result.as_ref()?.try_recv().ok()
    }
}

pub fn chrome_trace(profile: &Value, snapshot: Option<&DeveloperSnapshot>) -> Value {
    let mut events = Vec::new();
    let mut omitted = 0usize;
    let mut threads = HashSet::new();
    if let Some(lanes) = profile["lanes"].as_array() {
        for lane in lanes {
            if let Some(spans) = lane["events"].as_array() {
                for span in spans {
                    if events.len() >= MAX_EVENTS {
                        omitted += 1;
                        continue;
                    }
                    let (Some(start), Some(end), Some(thread)) = (
                        span["start_ns"].as_u64(),
                        span["end_ns"].as_u64(),
                        span["thread_sequence"].as_u64(),
                    ) else {
                        omitted += 1;
                        continue;
                    };
                    if end < start {
                        omitted += 1;
                        continue;
                    }
                    // Reused logical lanes can have different real threads. Export each
                    // actual thread independently so overlapping spans remain valid.
                    if events.len() + 1 + usize::from(!threads.contains(&thread)) > MAX_EVENTS {
                        omitted += 1;
                        continue;
                    }
                    if threads.insert(thread) {
                        events.push(json!({"ph":"M","name":"thread_name","pid":1,"tid":thread,
                            "args":{"name":format!("{} / thread {thread}",lane["worker"])}}));
                    }
                    events.push(json!({"ph":"X","name":span["name"],"cat":if span["wait_reason"].is_null(){"cpu_elapsed"}else{"instrumented_wait"},
                        "pid":1,"tid":thread,"ts":start as f64 / 1000.0,"dur":(end-start) as f64 / 1000.0,
                        "args":{"event_id":span["event_id"],"parent_event_id":span["parent_event_id"],
                            "frame_id":span["frame_id"],"job_identity":span["job_identity"],
                            "wait_reason":span["wait_reason"],"thread_cpu_ns":span["thread_cpu_ns"],
                            "nesting_depth":span["nesting_depth"]}}));
                }
            }
        }
    }
    json!({"traceEvents":events,"displayTimeUnit":"ms","mundaris":{
        "schema_version":1,"cpu_clock":"process_relative_monotonic_us","gpu_clock_calibrated":false,
        "gpu_absolute_timestamps":"unavailable","completed_spans_only":true,
        "profile_capture_id":profile["capture_id"],"generated_at_ns":profile["generated_at_ns"],
        "ai_analysis":ai_analysis(profile, omitted),
        "export_events_omitted":omitted,"profile_metadata":profile,
        "observation_gpu_measurements":snapshot.map(|s| &s.performance),
        "observation_frame":snapshot.map(|s| s.general.frame_number),
        "terrain_atlas":snapshot.and_then(|s| s.terrain_atlas.as_ref()),
        "limits":{"bytes":MAX_BYTES,"events":MAX_EVENTS},
        "interpretation":"Blank CPU time is unknown. Job/dependency IDs are recorded associations, not proof of a complete critical path. GPU values are asynchronous durations; never aligned on the CPU axis."
    }})
}

fn ai_analysis(profile: &Value, export_omitted: usize) -> Value {
    let mut spans = Vec::<(u64, Value)>::new();
    let mut frames = Vec::<(u64, Value)>::new();
    let mut stages = BTreeMap::<(String, String, u64), Vec<u64>>::new();
    let mut waits = BTreeMap::<String, Vec<u64>>::new();
    let mut span_count = 0usize;
    let mut typed_wait_count = 0usize;

    if let Some(lanes) = profile["lanes"].as_array() {
        for lane in lanes {
            let worker = lane.get("worker").cloned().unwrap_or(Value::Null);
            if let Some(events) = lane["events"].as_array() {
                for event in events {
                    let (Some(start), Some(end), Some(thread)) = (
                        event["start_ns"].as_u64(),
                        event["end_ns"].as_u64(),
                        event["thread_sequence"].as_u64(),
                    ) else {
                        continue;
                    };
                    if end < start {
                        continue;
                    }
                    let duration = event["duration_ns"].as_u64().unwrap_or(end - start);
                    let name = event["name"].as_str().unwrap_or("unknown");
                    let frame_id = event["frame_id"].as_u64();
                    let wait_reason = event.get("wait_reason").filter(|value| !value.is_null());
                    let summary = json!({
                        "name":name,
                        "duration_ns":duration,
                        "start_ns":start,
                        "end_ns":end,
                        "frame_id":frame_id,
                        "event_id":event["event_id"],
                        "parent_event_id":event["parent_event_id"],
                        "thread_sequence":thread,
                        "worker":worker,
                        "job_identity":event["job_identity"],
                        "wait_reason":wait_reason,
                    });
                    spans.push((duration, summary.clone()));
                    if name == "Frame" {
                        frames.push((duration, summary));
                    }
                    let stage = stages
                        .entry((name.to_owned(), worker.to_string(), thread))
                        .or_default();
                    stage.push(duration);
                    if let Some(reason) = wait_reason {
                        typed_wait_count += 1;
                        let key = reason
                            .as_str()
                            .map(str::to_owned)
                            .unwrap_or_else(|| reason.to_string());
                        waits.entry(key).or_default().push(duration);
                    }
                    span_count += 1;
                }
            }
        }
    }

    spans.sort_by_key(|(duration, _)| std::cmp::Reverse(*duration));
    frames.sort_by_key(|(duration, _)| std::cmp::Reverse(*duration));
    let top_spans = spans
        .iter()
        .take(64)
        .map(|(_, span)| span.clone())
        .collect::<Vec<_>>();
    let slowest_frames = frames
        .iter()
        .take(32)
        .map(|(_, frame)| frame.clone())
        .collect::<Vec<_>>();
    let stage_aggregates = stages
        .into_iter()
        .map(|((name, worker, thread), mut durations)| {
            durations.sort_unstable();
            let count = durations.len();
            let total: u128 = durations.iter().map(|duration| *duration as u128).sum();
            json!({
                "name":name,
                "worker":worker,
                "thread_sequence":thread,
                "count":count,
                "inclusive_total_ns":total,
                "mean_ns":if count == 0 { 0 } else { (total / count as u128) as u64 },
                "p95_ns":quantile(&durations, 0.95),
                "max_ns":durations.last().copied().unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();
    let wait_aggregates = waits
        .into_iter()
        .map(|(reason, mut durations)| {
            durations.sort_unstable();
            let total: u128 = durations.iter().map(|duration| *duration as u128).sum();
            json!({
                "reason":reason,
                "count":durations.len(),
                "inclusive_total_ns":total,
                "p95_ns":quantile(&durations, 0.95),
                "max_ns":durations.last().copied().unwrap_or(0),
            })
        })
        .collect::<Vec<_>>();

    json!({
        "schema":"mundaris.performance.timeline-analysis.v1",
        "purpose":"Rank observed spans and typed waits for quick diagnostic review.",
        "capture_id":profile["capture_id"],
        "generated_at_ns":profile["generated_at_ns"],
        "span_count":span_count,
        "typed_wait_count":typed_wait_count,
        "top_spans_by_inclusive_duration":top_spans,
        "slowest_frame_spans":slowest_frames,
        "stage_aggregates":stage_aggregates,
        "wait_aggregates":wait_aggregates,
        "data_quality":{
            "exported_trace_events_omitted":export_omitted,
            "snapshot_events_omitted":profile["snapshot_events_omitted"],
            "dropped_nesting":profile["dropped_nesting"],
            "completed_spans_only":true,
        },
        "interpretation":{
            "duration":"Inclusive elapsed duration; nested spans can overlap and totals must not be treated as CPU utilization.",
            "waits":"Only explicitly instrumented wait reasons are counted; absence of a wait does not prove useful work.",
            "causality":"Parent and job IDs are associations, not proof of a complete critical path.",
            "gpu":"GPU time has no calibrated CPU-clock placement in this export.",
        }
    })
}

fn quantile(sorted: &[u64], fraction: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * fraction.clamp(0.0, 1.0)).ceil() as usize;
    sorted[index.min(sorted.len() - 1)]
}

struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        if self.0.len().saturating_add(data.len()) > MAX_BYTES {
            return Err(io::Error::other(
                "export_byte_limit_exceeded; no partial trace published",
            ));
        }
        self.0.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub fn write_trace(path: &std::path::Path, snapshot: &DeveloperSnapshot) -> io::Result<()> {
    let profile = snapshot
        .engine_profile
        .as_deref()
        .ok_or_else(|| io::Error::other("profile_unavailable"))?;
    write_profile_trace(path, profile, Some(snapshot))
}

pub fn write_profile_trace(
    path: &std::path::Path,
    profile: &Value,
    snapshot: Option<&DeveloperSnapshot>,
) -> io::Result<()> {
    let mut bytes = BoundedBytes(Vec::new());
    serde_json::to_writer(&mut bytes, &chrome_trace(profile, snapshot))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(&bytes.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn export_keeps_microseconds_thread_identity_and_missing_gpu_explicit() {
        let trace = chrome_trace(
            &json!({"lanes":[{"worker":{"kind":"named","value":"test"},"events":[{
                "name":"synthetic","start_ns":1250,"end_ns":2750,"thread_sequence":9,
                "frame_id":4,"event_id":5,"job_identity":{"job_id":7},"thread_cpu_ns":null
            }]}]}),
            None,
        );
        let span = &trace["traceEvents"][1];
        assert_eq!(span["ts"], 1.25);
        assert_eq!(span["dur"], 1.5);
        assert_eq!(span["tid"], 9);
        assert_eq!(span["args"]["job_identity"]["job_id"], 7);
        assert!(span["args"]["thread_cpu_ns"].is_null());
        assert!(trace["mundaris"]["observation_gpu_measurements"].is_null());
        assert_eq!(trace["mundaris"]["gpu_clock_calibrated"], false);
    }
    #[test]
    fn oversized_export_does_not_exceed_byte_bound() {
        let mut bytes = BoundedBytes(Vec::new());
        assert!(bytes.write_all(&vec![0; MAX_BYTES + 1]).is_err());
        assert!(bytes.0.is_empty());
    }

    #[test]
    fn ai_analysis_ranks_spans_and_labels_inclusive_totals() {
        let profile = json!({
            "capture_id": "capture-1",
            "generated_at_ns": 20,
            "lanes": [{
                "worker": {"kind":"named","value":"terrain"},
                "events": [
                    {"name":"Frame","start_ns":0,"end_ns":20,"duration_ns":20,"frame_id":1,"event_id":1,"thread_sequence":3},
                    {"name":"worker wait","start_ns":2,"end_ns":12,"duration_ns":10,"frame_id":1,"event_id":2,"parent_event_id":1,"thread_sequence":3,"wait_reason":"worker_queue_full"},
                    {"name":"prepare","start_ns":4,"end_ns":9,"duration_ns":5,"frame_id":1,"event_id":3,"parent_event_id":1,"thread_sequence":3}
                ]
            }],
            "snapshot_events_omitted": 2,
            "dropped_nesting": 1
        });
        let analysis = ai_analysis(&profile, 4);
        assert_eq!(
            analysis["schema"],
            "mundaris.performance.timeline-analysis.v1"
        );
        assert_eq!(analysis["span_count"], 3);
        assert_eq!(
            analysis["top_spans_by_inclusive_duration"][0]["name"],
            "Frame"
        );
        assert_eq!(
            analysis["wait_aggregates"][0]["reason"],
            "worker_queue_full"
        );
        assert_eq!(analysis["data_quality"]["exported_trace_events_omitted"], 4);
        assert!(
            analysis["interpretation"]["duration"]
                .as_str()
                .unwrap()
                .contains("overlap")
        );
    }
}
