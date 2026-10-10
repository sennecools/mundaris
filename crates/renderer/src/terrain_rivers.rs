//! PROTOTYPE (M3 Water): one-shot read-back of a finished Tier A bake's
//! elevation and moisture for the CPU hydrology worker, and the packed river
//! buffer the producer carves with (`shaders/river_carve.wgsl`).
use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

/// Level-0 elevation (m) and moisture of a finished world bake, in cube-map
/// storage order (`6·n²` each).
#[derive(Debug, Clone)]
pub struct AtlasWorldFields {
    pub source: u64,
    pub face_cells: u32,
    pub elevation: Vec<f32>,
    pub moisture: Vec<f32>,
}

/// A disabled river buffer: the header with the enabled flag clear.
pub(crate) const DISABLED_RIVERS: [u32; 16] = [0; 16];

pub(crate) struct WorldFieldsReadback {
    buffer: wgpu::Buffer,
    face_cells: u32,
    // 0 copy recorded, 1 mapping, 2 mapped, 3 failed
    state: Arc<AtomicU8>,
}

impl WorldFieldsReadback {
    /// Record copies of result runs 0 (elevation) and 2 (moisture).
    pub(crate) fn record(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        result: &wgpu::Buffer,
        face_cells: u32,
    ) -> Self {
        let run = 4 * 6 * u64::from(face_cells) * u64::from(face_cells);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Tier A hydrology read-back"),
            size: 2 * run,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let (elevation, _) = crate::tier_a::field_mip_offset(face_cells, 0, 0);
        let (moisture, _) = crate::tier_a::field_mip_offset(face_cells, 0, 2);
        encoder.copy_buffer_to_buffer(result, 4 * u64::from(elevation), &buffer, 0, run);
        encoder.copy_buffer_to_buffer(result, 4 * u64::from(moisture), &buffer, run, run);
        Self {
            buffer,
            face_cells,
            state: Arc::new(AtomicU8::new(0)),
        }
    }

    /// Start mapping once the copy has been submitted.
    pub(crate) fn on_submitted(&self) {
        if self.state.load(Ordering::Acquire) != 0 {
            return;
        }
        self.state.store(1, Ordering::Release);
        let state = Arc::clone(&self.state);
        self.buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                state.store(if result.is_ok() { 2 } else { 3 }, Ordering::Release);
            });
    }

    /// The fields once mapped (`Some(Err)` if mapping failed).
    pub(crate) fn take(&self, source: u64) -> Option<Result<AtlasWorldFields, String>> {
        match self.state.load(Ordering::Acquire) {
            2 => {
                let result = self
                    .buffer
                    .slice(..)
                    .get_mapped_range()
                    .map_err(|e| e.to_string())
                    .map(|view| {
                        let words: Vec<f32> = view
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .map(|w| f32::from_le_bytes(*w))
                            .collect();
                        let half = words.len() / 2;
                        AtlasWorldFields {
                            source,
                            face_cells: self.face_cells,
                            elevation: words[..half].to_vec(),
                            moisture: words[half..].to_vec(),
                        }
                    });
                self.buffer.unmap();
                Some(result)
            }
            3 => Some(Err("hydrology read-back mapping failed".into())),
            _ => None,
        }
    }
}

/// Storage buffer holding packed river words (at least the header).
pub(crate) fn river_buffer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    words: &[u32],
) -> wgpu::Buffer {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Terrain atlas world rivers"),
        size: bytes.len().max(64) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buffer, 0, &bytes);
    buffer
}
