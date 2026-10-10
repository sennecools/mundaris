//! PROTOTYPE (M3 Water): runs the macro hydrology on a worker thread when a
//! world bake's elevation and moisture arrive from the GPU (pipeline §7.1:
//! a CPU worker on the read-back), and returns the packed rivers for the
//! producer. Constants come from `HydrologyParams::prototype`.
use astrum_renderer::AtlasWorldFields;
use astrum_world::terrain::{
    hydrology::{self, HydrologyInput, HydrologyParams, gpu},
    world_map::CubeMap,
};
use std::sync::mpsc::{Receiver, channel};

#[derive(Default)]
pub struct HydrologyJobs {
    jobs: Vec<(u64, Receiver<Vec<u32>>)>,
}

impl HydrologyJobs {
    /// Start hydrology for `fields` of a body with `radius_m`; without a
    /// radius (the body was released) the source gets disabled rivers.
    pub fn start(&mut self, fields: AtlasWorldFields, radius_m: Option<f64>) {
        let (sender, receiver) = channel();
        let source = fields.source;
        let Some(radius_m) = radius_m else {
            let _ = sender.send(gpu::disabled());
            self.jobs.push((source, receiver));
            return;
        };
        let spawned = std::thread::Builder::new()
            .name("atlas-hydrology".into())
            .spawn(move || {
                let started = std::time::Instant::now();
                let n = fields.face_cells as usize;
                let to_map = |values: &[f32]| {
                    let mut map = CubeMap::new(n, 0.0f32);
                    map.data_mut().copy_from_slice(values);
                    map
                };
                let (elevation, moisture) = (to_map(&fields.elevation), to_map(&fields.moisture));
                let result = hydrology::run(&HydrologyInput {
                    elevation: &elevation,
                    moisture: &moisture,
                    radius_m,
                    params: HydrologyParams::prototype(),
                });
                let words = gpu::pack(&result);
                eprintln!(
                    "hydrology: source {source} {n}^2, {} lakes, {} river segments, {} mouths, {:.0} km, carve bound {:.0} m, {:.0} ms, {} KiB",
                    result.fill.lakes.len(),
                    result.rivers.segment_count(),
                    result.rivers.mouth_count(),
                    result.rivers.length_km(radius_m),
                    result.carve_depth_bound_m(),
                    started.elapsed().as_secs_f64() * 1e3,
                    words.len() * 4 / 1024
                );
                // Dev aid: where to look (largest river upstream of its
                // mouth, deepest valley), as unit body directions.
                let vertices = &result.rivers.vertices;
                let pick = |key: &dyn Fn(&hydrology::RiverVertex) -> f64| {
                    vertices
                        .iter()
                        .filter(|v| v.mouth == hydrology::rivers::Mouth::None)
                        .max_by(|a, b| key(a).total_cmp(&key(b)))
                        .map(|v| v.pos.to_array())
                };
                eprintln!(
                    "hydrology: largest river at {:?}, deepest valley at {:?}",
                    pick(&|v| v.discharge_km2),
                    pick(&|v| v.incision_m)
                );
                let _ = sender.send(words);
            });
        if spawned.is_err() {
            // No worker: the source still becomes ready, uncarved.
            let (sender, receiver) = channel();
            let _ = sender.send(gpu::disabled());
            self.jobs.push((source, receiver));
            return;
        }
        self.jobs.push((source, receiver));
    }

    /// Finished jobs: (source key, packed rivers).
    pub fn poll(&mut self) -> Vec<(u64, Vec<u32>)> {
        let mut done = Vec::new();
        self.jobs
            .retain(|(source, receiver)| match receiver.try_recv() {
                Ok(words) => {
                    done.push((*source, words));
                    false
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => true,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    done.push((*source, gpu::disabled()));
                    false
                }
            });
        done
    }
}
