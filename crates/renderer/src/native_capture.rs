//! Opt-in asynchronous readback of the native presentation surface.

use std::sync::mpsc::{self, Receiver};

const MAX_READBACK_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug)]
pub struct NativeCaptureFrame {
    pub capture_id: u64,
    pub submission_id: u64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub adapter: String,
    pub backend: String,
}

struct ReadbackSlot {
    capture_id: u64,
    submission_id: Option<u64>,
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    format: wgpu::TextureFormat,
    buffer: wgpu::Buffer,
    receiver: Option<Receiver<Result<(), wgpu::BufferAsyncError>>>,
}

pub(crate) struct NativeCapture {
    enabled: bool,
    pending_capture: Option<u64>,
    slot: Option<ReadbackSlot>,
    completed: Option<Result<NativeCaptureFrame, String>>,
    adapter: String,
    backend: wgpu::Backend,
}

impl NativeCapture {
    pub(crate) fn new(adapter: String, backend: wgpu::Backend) -> Self {
        Self {
            enabled: false,
            pending_capture: None,
            slot: None,
            completed: None,
            adapter,
            backend,
        }
    }

    pub(crate) fn enable(
        &mut self,
        surface_copy_src_supported: bool,
        format: wgpu::TextureFormat,
    ) -> Result<(), String> {
        if !surface_copy_src_supported {
            return Err("unsupported: presentation surface does not allow COPY_SRC".into());
        }
        if !is_supported_color_format(format) {
            return Err(format!(
                "unsupported: native capture requires RGBA8/BGRA8 UNORM, got {format:?}"
            ));
        }
        self.enabled = true;
        Ok(())
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn request(&mut self, capture_id: u64) -> Result<(), String> {
        if !self.enabled {
            return Err("unsupported: native capture has not been enabled".into());
        }
        if self.pending_capture.is_some() || self.slot.is_some() || self.completed.is_some() {
            return Err("busy: a native capture is pending or awaiting collection".into());
        }
        self.pending_capture = Some(capture_id);
        Ok(())
    }

    /// Encodes a copy only when requested. Any bounded-size failure is reported
    /// through `poll` and does not prevent the frame from being presented.
    pub(crate) fn encode_copy(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        texture: &wgpu::Texture,
        format: wgpu::TextureFormat,
        width: u32,
        height: u32,
    ) {
        let Some(capture_id) = self.pending_capture else {
            return;
        };
        let layout = match readback_layout(width, height) {
            Ok(layout) => layout,
            Err(error) => {
                self.pending_capture = None;
                self.completed = Some(Err(error));
                return;
            }
        };
        let (padded_bytes_per_row, buffer_size, _) = layout;
        if buffer_size > device.limits().max_buffer_size {
            self.pending_capture = None;
            self.completed = Some(Err(format!(
                "resize/size failure: {width}x{height} capture exceeds the device buffer limit"
            )));
            return;
        }
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Mundaris native capture readback"),
            size: buffer_size,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_bytes_per_row),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        self.pending_capture = None;
        self.slot = Some(ReadbackSlot {
            capture_id,
            submission_id: None,
            width,
            height,
            padded_bytes_per_row,
            format,
            buffer,
            receiver: None,
        });
    }

    pub(crate) fn submitted(&mut self, submission_id: u64) {
        let Some(slot) = self.slot.as_mut() else {
            return;
        };
        // Subsequent frames may submit while this asynchronous slot is in flight.
        // Its source submission and mapping belong to the original copy only.
        if slot.submission_id.is_some() {
            return;
        }
        slot.submission_id = Some(submission_id);
        let (sender, receiver) = mpsc::channel();
        slot.buffer
            .map_async(wgpu::MapMode::Read, .., move |result| {
                let _ = sender.send(result);
            });
        slot.receiver = Some(receiver);
    }

    pub(crate) fn poll(&mut self) -> Result<Option<NativeCaptureFrame>, String> {
        if let Some(completed) = self.completed.take() {
            return completed.map(Some);
        }
        if !self.enabled {
            return Err("unsupported: native capture has not been enabled".into());
        }
        let Some(slot) = self.slot.as_ref() else {
            return Ok(None);
        };
        let Some(receiver) = slot.receiver.as_ref() else {
            return Ok(None);
        };
        match receiver.try_recv() {
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                self.slot = None;
                Err("native capture readback callback disconnected".into())
            }
            Ok(Err(error)) => {
                self.slot = None;
                Err(format!("native capture readback failed: {error}"))
            }
            Ok(Ok(())) => {
                let slot = self.slot.take().expect("readback slot was present");
                let mapped = slot
                    .buffer
                    .get_mapped_range(..)
                    .map_err(|error| format!("native capture mapped range failed: {error}"))?;
                let rgba = convert_rows(
                    &mapped,
                    slot.width,
                    slot.height,
                    slot.padded_bytes_per_row,
                    slot.format,
                )?;
                drop(mapped);
                slot.buffer.unmap();
                Ok(Some(NativeCaptureFrame {
                    capture_id: slot.capture_id,
                    submission_id: slot
                        .submission_id
                        .expect("mapped capture has a submitted frame identity"),
                    width: slot.width,
                    height: slot.height,
                    rgba,
                    adapter: self.adapter.clone(),
                    backend: format!("{:?}", self.backend),
                }))
            }
        }
    }

    pub(crate) fn cancel(&mut self) {
        let active = self.pending_capture.take().is_some()
            || self.slot.is_some()
            || self.completed.is_some();
        if let Some(slot) = self.slot.take() {
            slot.buffer.unmap();
        }
        self.completed = active.then(|| Err("cancelled: native capture was cancelled".into()));
    }

    pub(crate) fn resized(&mut self) {
        if self.pending_capture.take().is_some() {
            self.completed = Some(Err(
                "resize: native capture request was cancelled before submission".into(),
            ));
        }
    }
}

fn readback_layout(width: u32, height: u32) -> Result<(u32, u64, usize), String> {
    if width == 0 || height == 0 {
        return Err("resize failure: native capture dimensions are zero".into());
    }
    let row_bytes = width
        .checked_mul(4)
        .ok_or_else(|| "native capture row size overflowed".to_owned())?;
    let alignment = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded_bytes_per_row = row_bytes
        .checked_add(alignment - 1)
        .map(|bytes| bytes / alignment * alignment)
        .ok_or_else(|| "native capture row alignment overflowed".to_owned())?;
    let buffer_size = u64::from(padded_bytes_per_row)
        .checked_mul(u64::from(height))
        .ok_or_else(|| "native capture buffer size overflowed".to_owned())?;
    let rgba_len = usize::try_from(u64::from(row_bytes) * u64::from(height))
        .map_err(|_| "native capture image size exceeds host address space".to_owned())?;
    if buffer_size > MAX_READBACK_BYTES {
        return Err(format!(
            "resize/size failure: native capture readback exceeds {} MiB",
            MAX_READBACK_BYTES / (1024 * 1024)
        ));
    }
    Ok((padded_bytes_per_row, buffer_size, rgba_len))
}

fn convert_rows(
    mapped: &[u8],
    width: u32,
    height: u32,
    padded_bytes_per_row: u32,
    format: wgpu::TextureFormat,
) -> Result<Vec<u8>, String> {
    let (expected_padded, buffer_size, rgba_len) = readback_layout(width, height)?;
    if expected_padded != padded_bytes_per_row || mapped.len() < buffer_size as usize {
        return Err("native capture readback layout did not match the submitted surface".into());
    }
    let row_bytes = width as usize * 4;
    let padded_bytes_per_row = padded_bytes_per_row as usize;
    let mut rgba = Vec::with_capacity(rgba_len);
    for row in mapped
        .chunks_exact(padded_bytes_per_row)
        .take(height as usize)
    {
        let pixels = &row[..row_bytes];
        match format {
            wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Rgba8UnormSrgb => {
                rgba.extend_from_slice(pixels)
            }
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Bgra8UnormSrgb => {
                for pixel in pixels.as_chunks::<4>().0 {
                    rgba.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
            _ => return Err(format!("unsupported native capture format {format:?}")),
        }
    }
    Ok(rgba)
}

fn is_supported_color_format(format: wgpu::TextureFormat) -> bool {
    matches!(
        format,
        wgpu::TextureFormat::Rgba8Unorm
            | wgpu::TextureFormat::Rgba8UnormSrgb
            | wgpu::TextureFormat::Bgra8Unorm
            | wgpu::TextureFormat::Bgra8UnormSrgb
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readback_layout_pads_rows_and_bounds_total_bytes() {
        assert_eq!(readback_layout(3, 2).unwrap(), (256, 512, 24));
        assert!(readback_layout(4096, 4096).is_ok());
        assert!(
            readback_layout(8192, 8192)
                .unwrap_err()
                .contains("exceeds 64 MiB")
        );
        assert!(
            readback_layout(0, 1)
                .unwrap_err()
                .contains("dimensions are zero")
        );
    }

    #[test]
    fn converts_srgb_rgba_and_bgra_rows_to_rgba() {
        let mut padded = vec![0; 256];
        padded[..8].copy_from_slice(&[1, 2, 3, 4, 10, 20, 30, 40]);
        assert_eq!(
            convert_rows(&padded, 2, 1, 256, wgpu::TextureFormat::Rgba8UnormSrgb).unwrap(),
            [1, 2, 3, 4, 10, 20, 30, 40]
        );
        assert_eq!(
            convert_rows(&padded, 2, 1, 256, wgpu::TextureFormat::Bgra8UnormSrgb).unwrap(),
            [3, 2, 1, 4, 30, 20, 10, 40]
        );
        assert_eq!(
            convert_rows(&padded, 2, 1, 256, wgpu::TextureFormat::Rgba8Unorm).unwrap(),
            [1, 2, 3, 4, 10, 20, 30, 40]
        );
        assert_eq!(
            convert_rows(&padded, 2, 1, 256, wgpu::TextureFormat::Bgra8Unorm).unwrap(),
            [3, 2, 1, 4, 30, 20, 10, 40]
        );
    }

    #[test]
    fn enable_and_request_report_unsupported_busy_and_cancelled_states() {
        let mut capture = NativeCapture::new("adapter".into(), wgpu::Backend::Vulkan);
        assert!(
            capture
                .enable(false, wgpu::TextureFormat::Rgba8UnormSrgb)
                .unwrap_err()
                .contains("unsupported")
        );
        assert!(
            capture
                .enable(true, wgpu::TextureFormat::Rgba16Float)
                .unwrap_err()
                .contains("unsupported")
        );
        assert!(capture.request(1).unwrap_err().contains("not been enabled"));
        assert!(capture.poll().unwrap_err().contains("not been enabled"));

        capture
            .enable(true, wgpu::TextureFormat::Rgba8UnormSrgb)
            .unwrap();
        capture.request(1).unwrap();
        assert!(capture.request(2).unwrap_err().contains("busy"));
        capture.cancel();
        assert!(capture.poll().unwrap_err().contains("cancelled"));
        capture.request(2).unwrap();
        capture.resized();
        assert!(capture.poll().unwrap_err().contains("resize"));
        capture.request(3).unwrap();
    }
}
