//! A terrain-only pool: one admitted calculation per slot, no hidden worker queue.
//! The app retains requests, reservations, topology ownership and publication.
use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::thread::{self, JoinHandle};

const WORKER_STACK_BYTES: usize = 512 * 1024;
const WORKER_FIXED_BYTES: usize = WORKER_STACK_BYTES + 128 * 1024;
pub(super) const PATCH_RESERVATION: usize =
    GRID_SAMPLES * size_of::<SurfaceGeometrySample>() + size_of::<GeneratedSurfacePatch>() + 64;

pub(super) struct CoverJob {
    pub old: Option<Arc<StitchedSurface>>,
    pub old_cover: Vec<ActiveSurfacePatch>,
    pub old_raw: Vec<Arc<GeneratedSurfacePatch>>,
    pub cover: Vec<ActiveSurfacePatch>,
    pub raw: Vec<Arc<GeneratedSurfacePatch>>,
    pub transition_budget: usize,
    pub morph: bool,
}
pub(super) struct CoverOutput {
    pub surface: Arc<StitchedSurface>,
    pub cover: Vec<ActiveSurfacePatch>,
    pub transition: Option<SurfaceTransition>,
    pub stitch_cpu: Duration,
    pub morph_cpu: Duration,
}
enum Calculation {
    Patch(CubePatchAddress),
    Cover(CoverJob),
}
struct Job {
    identity: TerrainGeometryIdentity,
    cancelled: Arc<AtomicBool>,
    calculation: Calculation,
}
pub(super) enum Output {
    Patch(GeneratedSurfacePatch, PatchMetadata),
    Cover(CoverOutput),
    Cancelled,
}
pub(super) struct Completion {
    pub id: u64,
    pub identity: TerrainGeometryIdentity,
    pub address: Option<CubePatchAddress>,
    pub pin_when_ready: bool,
    pub reserved_bytes: usize,
    pub cpu: Duration,
    pub result: Result<Output>,
}
struct Pending {
    id: u64,
    identity: TerrainGeometryIdentity,
    address: Option<CubePatchAddress>,
    pin_when_ready: bool,
    reserved_bytes: usize,
    cancelled: Arc<AtomicBool>,
    abandoned: bool,
}
struct Slot {
    sender: Option<SyncSender<Job>>,
    receiver: Receiver<(Duration, Result<Output>)>,
    thread: Option<JoinHandle<()>>,
    pending: Option<Pending>,
    completed: Option<(Duration, Result<Output>)>,
}
pub(super) struct TerrainWorkers {
    slots: Vec<Slot>,
    next_id: u64,
}
impl TerrainWorkers {
    pub fn required_bytes(count: usize) -> usize {
        size_of::<Self>() + count * (size_of::<Slot>() + WORKER_FIXED_BYTES)
    }
    pub fn new(count: usize) -> Result<Self> {
        anyhow::ensure!(
            (1..=4).contains(&count),
            "terrain worker count must be 1..=4"
        );
        let mut pool = Self {
            slots: Vec::with_capacity(count),
            next_id: 0,
        };
        for index in 0..count {
            let (sender, jobs) = mpsc::sync_channel::<Job>(1);
            let (results, receiver) = mpsc::sync_channel(1);
            let thread = thread::Builder::new()
                .name(format!("terrain-{index}"))
                .stack_size(WORKER_STACK_BYTES)
                .spawn(move || {
                    let topology = SurfaceTopology::new();
                    while let Ok(job) = jobs.recv() {
                        let start = Instant::now();
                        let output = calculate(job, &topology);
                        if results.send((start.elapsed(), output)).is_err() {
                            break;
                        }
                    }
                })?;
            pool.slots.push(Slot {
                sender: Some(sender),
                receiver,
                thread: Some(thread),
                pending: None,
                completed: None,
            });
        }
        Ok(pool)
    }
    pub fn count(&self) -> usize {
        self.slots.len()
    }
    pub fn idle(&self) -> bool {
        self.slots.iter().any(|s| s.pending.is_none())
    }
    pub fn in_flight(&self) -> usize {
        self.slots.iter().filter(|s| s.pending.is_some()).count()
    }
    pub fn pending_covers(&self) -> usize {
        self.slots
            .iter()
            .filter_map(|s| s.pending.as_ref())
            .filter(|p| p.address.is_none())
            .count()
    }
    pub fn abandon_cover(&mut self, id: u64) -> bool {
        for pending in self.slots.iter_mut().filter_map(|s| s.pending.as_mut()) {
            if pending.id == id && pending.address.is_none() {
                pending.abandoned = true;
                return !pending.cancelled.swap(true, Ordering::Relaxed);
            }
        }
        false
    }
    pub fn bytes(&self) -> usize {
        size_of::<Self>()
            + self.slots.capacity() * size_of::<Slot>()
            + self.count() * WORKER_FIXED_BYTES
            + self
                .slots
                .iter()
                .filter_map(|s| s.pending.as_ref())
                .map(|p| p.reserved_bytes)
                .sum::<usize>()
    }
    pub fn reservations(&self) -> usize {
        self.slots
            .iter()
            .filter_map(|s| s.pending.as_ref())
            .map(|p| p.reserved_bytes)
            .sum()
    }
    pub fn fixed_bytes(&self) -> usize {
        self.count() * WORKER_FIXED_BYTES
    }
    pub fn patch_pending(
        &self,
        identity: &TerrainGeometryIdentity,
        address: CubePatchAddress,
    ) -> bool {
        self.slots
            .iter()
            .filter_map(|s| s.pending.as_ref())
            .any(|p| {
                !p.cancelled.load(Ordering::Relaxed)
                    && p.identity == *identity
                    && p.address == Some(address)
            })
    }
    pub fn pending_for_body(&self, body: BodyId) -> usize {
        self.slots
            .iter()
            .filter_map(|s| s.pending.as_ref())
            .filter(|p| p.identity.body == body)
            .count()
    }
    pub fn pin(&mut self, identity: &TerrainGeometryIdentity, address: CubePatchAddress) {
        for p in self.slots.iter_mut().filter_map(|s| s.pending.as_mut()) {
            if p.identity == *identity && p.address == Some(address) {
                p.pin_when_ready = true;
            }
        }
    }
    pub fn cancel_where(
        &mut self,
        predicate: impl Fn(&TerrainGeometryIdentity, Option<CubePatchAddress>) -> bool,
    ) -> usize {
        let mut count = 0;
        for p in self.slots.iter_mut().filter_map(|s| s.pending.as_mut()) {
            if predicate(&p.identity, p.address) && !p.cancelled.swap(true, Ordering::Relaxed) {
                count += 1;
            }
        }
        count
    }
    pub fn submit_patch(&mut self, request: &Request) -> Result<u64> {
        self.submit(
            &request.identity,
            Some(request.address),
            request.pin_when_ready,
            PATCH_RESERVATION,
            Calculation::Patch(request.address),
        )
    }
    pub fn submit_cover(
        &mut self,
        identity: &TerrainGeometryIdentity,
        job: CoverJob,
        bytes: usize,
    ) -> Result<u64> {
        self.submit(identity, None, false, bytes, Calculation::Cover(job))
    }
    fn submit(
        &mut self,
        identity: &TerrainGeometryIdentity,
        address: Option<CubePatchAddress>,
        pin_when_ready: bool,
        reserved_bytes: usize,
        calculation: Calculation,
    ) -> Result<u64> {
        let slot = self
            .slots
            .iter_mut()
            .find(|s| s.pending.is_none())
            .ok_or_else(|| anyhow::anyhow!("no admitted terrain worker slot"))?;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("terrain job sequence overflow"))?;
        let cancelled = Arc::new(AtomicBool::new(false));
        slot.sender
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("terrain worker shut down"))?
            .try_send(Job {
                identity: identity.clone(),
                cancelled: cancelled.clone(),
                calculation,
            })
            .map_err(|_| anyhow::anyhow!("terrain worker submission failed"))?;
        slot.pending = Some(Pending {
            id: self.next_id,
            identity: identity.clone(),
            address,
            pin_when_ready,
            reserved_bytes,
            cancelled,
            abandoned: false,
        });
        Ok(self.next_id)
    }
    /// Ordered app-thread publication, independent of which worker finished first.
    pub fn take_next(&mut self) -> Result<Option<Completion>> {
        // Cancelled reconstructible calculations may finish in the background,
        // but cannot head-of-line block a newer body's useful publication.
        for slot in &mut self.slots {
            if slot
                .pending
                .as_ref()
                .is_some_and(|p| p.cancelled.load(Ordering::Relaxed))
                && slot.completed.is_none()
            {
                match slot.receiver.try_recv() {
                    Ok(value) => slot.completed = Some(value),
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => {
                        bail!("cancelled terrain worker stopped before acknowledgement")
                    }
                }
            }
        }
        let Some(index) = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(i, s)| {
                s.pending.as_ref().and_then(|p| {
                    let cancelled = p.cancelled.load(Ordering::Relaxed);
                    (!cancelled || s.completed.is_some()).then_some((i, (!cancelled, p.id)))
                })
            })
            .min_by_key(|(_, order)| *order)
            .map(|(i, _)| i)
        else {
            return Ok(None);
        };
        let slot = &mut self.slots[index];
        if slot.completed.is_none() {
            match slot.receiver.try_recv() {
                Ok(value) => slot.completed = Some(value),
                Err(TryRecvError::Empty) => return Ok(None),
                Err(TryRecvError::Disconnected) => {
                    bail!("terrain worker stopped before completion")
                }
            }
        }
        let pending = slot
            .pending
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing terrain job"))?;
        let (cpu, result) = slot
            .completed
            .take()
            .ok_or_else(|| anyhow::anyhow!("missing terrain result"))?;
        if pending.abandoned {
            // No coordinator retains this source now. All calculation handles
            // have dropped, so the acknowledgement may release its charge.
            return self.take_next();
        }
        Ok(Some(Completion {
            id: pending.id,
            identity: pending.identity,
            address: pending.address,
            pin_when_ready: pending.pin_when_ready,
            reserved_bytes: pending.reserved_bytes,
            cpu,
            result: if pending.cancelled.load(Ordering::Relaxed) {
                Ok(Output::Cancelled)
            } else {
                result
            },
        }))
    }
}
impl Drop for TerrainWorkers {
    fn drop(&mut self) {
        self.cancel_where(|_, _| true);
        for slot in &mut self.slots {
            slot.sender.take();
        }
        // There is only one result per slot, so worker send cannot block shutdown.
        for slot in &mut self.slots {
            if let Some(thread) = slot.thread.take() {
                let _ = thread.join();
            }
        }
    }
}
fn calculate(job: Job, topology: &SurfaceTopology) -> Result<Output> {
    if job.cancelled.load(Ordering::Relaxed) {
        return Ok(Output::Cancelled);
    }
    match job.calculation {
        Calculation::Patch(address) => {
            let generator = TerrainGenerator::new(&job.identity.definition, job.identity.radius_m)?;
            let radius = job.identity.radius_m;
            let footprint =
                TerrainFootprint::new(radius * 2.0 / (16.0 * (1u64 << address.level()) as f64))?;
            let mut samples = Vec::with_capacity(GRID_SAMPLES);
            while samples.len() < GRID_SAMPLES {
                if job.cancelled.load(Ordering::Relaxed) {
                    return Ok(Output::Cancelled);
                }
                let count = GENERATION_MICROBATCH.min(GRID_SAMPLES - samples.len());
                let mut locations = [SurfaceLocation::new(Direction3::try_new(glam::DVec3::X)?);
                    GENERATION_MICROBATCH];
                let mut output = [TerrainSample::default(); GENERATION_MICROBATCH];
                for (offset, location) in locations[..count].iter_mut().enumerate() {
                    let index = samples.len() + offset;
                    *location = SurfaceLocation::new(
                        address
                            .sample_key(index as u32 % 17, index as u32 / 17, 16)?
                            .direction(),
                    );
                }
                generator.evaluate_batch(&locations[..count], footprint, &mut output[..count])?;
                for (location, sample) in locations[..count].iter().zip(&output[..count]) {
                    samples.push(SurfaceGeometrySample {
                        position_body_m: location.direction().unit() * (radius + sample.height_m()),
                        normal_body: sample.normal_body(*location, radius)?.unit(),
                    });
                }
            }
            let metadata = PatchMetadata::build(address, topology)?;
            let (extent, error) =
                certificate::certificate_for_samples(&generator, address, metadata, &samples)?;
            Ok(Output::Patch(
                GeneratedSurfacePatch::new(
                    address,
                    radius,
                    footprint.metres(),
                    samples,
                    extent,
                    error,
                )?,
                metadata,
            ))
        }
        Calculation::Cover(input) => {
            let start = Instant::now();
            let raw: Vec<_> = input.raw.iter().map(Arc::as_ref).collect();
            let old_raw: Vec<_> = input.old_raw.iter().map(Arc::as_ref).collect();
            let surface = Arc::new(StitchedSurface::build_reusing(
                &input.cover,
                &raw,
                topology,
                input
                    .old
                    .as_ref()
                    .map(|old| (old.as_ref(), old_raw.as_slice())),
            )?);
            let stitch_cpu = start.elapsed();
            if job.cancelled.load(Ordering::Relaxed) {
                return Ok(Output::Cancelled);
            }
            let start = Instant::now();
            let transition = if input.morph {
                input
                    .old
                    .as_ref()
                    .map(|old| {
                        SurfaceTransition::build_cancellable(
                            &input.old_cover,
                            old,
                            &input.cover,
                            &surface,
                            topology,
                            input.transition_budget,
                            || job.cancelled.load(Ordering::Relaxed),
                        )
                    })
                    .transpose()?
                    .flatten()
            } else {
                None
            };
            if job.cancelled.load(Ordering::Relaxed) {
                return Ok(Output::Cancelled);
            }
            Ok(Output::Cover(CoverOutput {
                surface,
                cover: input.cover,
                transition,
                stitch_cpu,
                morph_cpu: if input.morph {
                    start.elapsed()
                } else {
                    Duration::ZERO
                },
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::solar_system::{SolarBody, SolarSystemPreset};
    use std::num::NonZeroU64;

    #[test]
    fn worker_stitch_and_transition_endpoints_match_serial_and_cancel_atomically() {
        let world = SolarSystemPreset::gameplay()
            .create(NonZeroU64::new(5107).unwrap())
            .unwrap();
        let (body, state) = world.bodies().nth(SolarBody::Earth as usize).unwrap();
        let identity = TerrainGeometryIdentity::new(
            body,
            state.terrain().unwrap().clone(),
            state.terrain_revision(),
            state.properties().reference_radius_m(),
        )
        .unwrap();
        let roots = CubeFace::ALL.map(CubePatchAddress::root);
        let mut destination = roots.to_vec();
        destination.retain(|p| *p != roots[4]);
        destination.extend(roots[4].children().unwrap());
        let topology = SurfaceTopology::new();
        let old_cover = active_surface_cover(&roots, &topology).unwrap();
        let new_cover = active_surface_cover(&destination, &topology).unwrap();
        let mut cache = TerrainPatchCache::new(TERRAIN_CPU_CAP_BYTES, 16).unwrap();
        for p in old_cover.iter().chain(&new_cover) {
            cache.request(&identity, p.address);
        }
        cache
            .generate(16 * GRID_SAMPLES, GENERATION_MICROBATCH, None)
            .unwrap();
        let raw_for = |cover: &[ActiveSurfacePatch]| -> Vec<Arc<GeneratedSurfacePatch>> {
            cover
                .iter()
                .map(|p| {
                    cache.entries[cache.entry_index(&identity, p.address).unwrap()]
                        .patch
                        .clone()
                })
                .collect()
        };
        let old_raw = raw_for(&old_cover);
        let new_raw = raw_for(&new_cover);
        let old_refs = old_raw.iter().map(Arc::as_ref).collect::<Vec<_>>();
        let new_refs = new_raw.iter().map(Arc::as_ref).collect::<Vec<_>>();
        let source = Arc::new(StitchedSurface::build(&old_cover, &old_refs, &topology).unwrap());
        let target = StitchedSurface::build_reusing(
            &new_cover,
            &new_refs,
            &topology,
            Some((&source, &old_refs)),
        )
        .unwrap();
        let serial = SurfaceTransition::build(
            &old_cover,
            &source,
            &new_cover,
            &target,
            &topology,
            16 * 1024 * 1024,
        )
        .unwrap();
        for count in [1, 2, 4] {
            let mut pool = TerrainWorkers::new(count).unwrap();
            let make_job = || CoverJob {
                old: Some(source.clone()),
                old_cover: old_cover.clone(),
                old_raw: old_raw.clone(),
                cover: new_cover.clone(),
                raw: new_raw.clone(),
                morph: true,
                transition_budget: 16 * 1024 * 1024,
            };
            let id = pool
                .submit_cover(
                    &identity,
                    make_job(),
                    StitchedSurface::construction_bytes(new_cover.len())
                        + 16 * 1024 * 1024
                        + source.resident_bytes(),
                )
                .unwrap();
            let deadline = Instant::now() + Duration::from_secs(10);
            let completion = loop {
                if let Some(c) = pool.take_next().unwrap() {
                    break c;
                }
                assert!(Instant::now() < deadline, "worker cover timeout");
                thread::sleep(Duration::from_millis(1));
            };
            assert_eq!(completion.id, id);
            assert_eq!(completion.identity, identity);
            let Output::Cover(output) = completion.result.unwrap() else {
                panic!("missing cover output")
            };
            for (a, b) in target.patches().iter().zip(output.surface.patches()) {
                assert_eq!(a.samples(), b.samples());
                assert_eq!(
                    a.extent().min_height_m.to_bits(),
                    b.extent().min_height_m.to_bits()
                );
                assert_eq!(
                    a.extent().max_height_m.to_bits(),
                    b.extent().max_height_m.to_bits()
                );
            }
            let parallel = output.transition.unwrap();
            assert_eq!(serial.affected_old(), parallel.affected_old());
            assert_eq!(serial.affected_new(), parallel.affected_new());
            assert_eq!(serial.triangles().len(), parallel.triangles().len());
            for (a, b) in serial
                .triangles()
                .iter()
                .flatten()
                .zip(parallel.triangles().iter().flatten())
            {
                for t in [0.0, 0.25, 0.5, 1.0] {
                    let a = a.sample(t).unwrap();
                    let b = b.sample(t).unwrap();
                    assert_eq!(
                        a.position_body_m.to_array().map(f64::to_bits),
                        b.position_body_m.to_array().map(f64::to_bits)
                    );
                    assert_eq!(
                        a.normal_body.to_array().map(f64::to_bits),
                        b.normal_body.to_array().map(f64::to_bits)
                    );
                }
                for (a, b) in [
                    (a.old_reference, b.old_reference),
                    (a.new_reference, b.new_reference),
                ] {
                    assert_eq!(a.address, b.address);
                    assert_eq!(a.indices, b.indices);
                    assert_eq!(a.weights.map(f64::to_bits), b.weights.map(f64::to_bits));
                }
                assert_eq!(a.old_elevation.to_bits(), b.old_elevation.to_bits());
                assert_eq!(a.new_elevation.to_bits(), b.new_elevation.to_bits());
            }
            pool.submit_cover(&identity, make_job(), 16 * 1024 * 1024)
                .unwrap();
            assert_eq!(pool.cancel_where(|i, _| i.body == body), 1);
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                if let Some(c) = pool.take_next().unwrap() {
                    assert!(matches!(c.result.unwrap(), Output::Cancelled));
                    break;
                }
                assert!(Instant::now() < deadline);
                thread::sleep(Duration::from_millis(1));
            }
            assert_eq!(pool.in_flight(), 0);
        }
    }
}
