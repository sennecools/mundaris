//! Anti-aliasing support (docs/RENDER_PIPELINE_HDR.md, amendment 2026-10-10
//! anti-aliasing): sample counts the adapter supports, and pipelines that
//! exist once per sample count, compiled on demand.
//!
//! Main-pass pipelines (sky, spheres, terrain, scatter, grass, flora) draw
//! into the multisampled HDR targets when MSAA is on. Each wraps its builder
//! in an [`MsaaPipeline`]; the first switch to a new sample count compiles
//! that variant (a one-time hitch), later switches are free.

use std::fmt;

use crate::post::{DEPTH_FORMAT, SCENE_TARGETS};

/// Sample counts the main pass may use, smallest first.
pub const SAMPLE_COUNTS: [u32; 4] = [1, 2, 4, 8];

fn slot(samples: u32) -> usize {
    SAMPLE_COUNTS
        .iter()
        .position(|&n| n == samples)
        .unwrap_or(0)
}

/// Multisample state for `samples` (1 = off).
pub(crate) fn multisample(samples: u32) -> wgpu::MultisampleState {
    wgpu::MultisampleState {
        count: samples,
        mask: !0,
        alpha_to_coverage_enabled: false,
    }
}

/// Sample counts every main-pass target format supports on this adapter.
/// Counts other than 1 and 4 need `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES`.
pub(crate) fn supported_sample_counts(adapter: &wgpu::Adapter, features: wgpu::Features) -> u32 {
    let specific = features.contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES);
    let mut mask = 0;
    for samples in SAMPLE_COUNTS {
        let ok = samples == 1
            || ((specific || samples == 4)
                && SCENE_TARGETS.iter().chain([&DEPTH_FORMAT]).all(|&format| {
                    adapter
                        .get_texture_format_features(format)
                        .flags
                        .sample_count_supported(samples)
                }));
        if ok {
            mask |= samples;
        }
    }
    mask
}

/// The largest supported sample count not above `wanted`.
pub(crate) fn clamp_samples(wanted: u32, supported: u32) -> u32 {
    SAMPLE_COUNTS
        .iter()
        .rev()
        .copied()
        .find(|&n| n <= wanted && supported & n != 0)
        .unwrap_or(1)
}

type Builder = dyn Fn(&wgpu::Device, u32) -> wgpu::RenderPipeline;

/// One render pipeline per sample count, built on first use.
pub(crate) struct MsaaPipeline {
    build: Box<Builder>,
    variants: [Option<wgpu::RenderPipeline>; SAMPLE_COUNTS.len()],
    samples: u32,
}

impl fmt::Debug for MsaaPipeline {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MsaaPipeline")
            .field("samples", &self.samples)
            .finish_non_exhaustive()
    }
}

impl MsaaPipeline {
    /// Builds the single-sample variant now.
    pub(crate) fn new(
        device: &wgpu::Device,
        build: impl Fn(&wgpu::Device, u32) -> wgpu::RenderPipeline + 'static,
    ) -> Self {
        let mut pipeline = Self {
            build: Box::new(build),
            variants: Default::default(),
            samples: 1,
        };
        pipeline.set_samples(device, 1);
        pipeline
    }

    /// Selects `samples`, compiling that variant if it is new. Returns true
    /// when a variant was compiled.
    pub(crate) fn set_samples(&mut self, device: &wgpu::Device, samples: u32) -> bool {
        self.samples = samples;
        let variant = &mut self.variants[slot(samples)];
        if variant.is_some() {
            return false;
        }
        *variant = Some((self.build)(device, samples));
        true
    }

    /// The pipeline for the selected sample count.
    pub(crate) fn get(&self) -> &wgpu::RenderPipeline {
        self.variants[slot(self.samples)]
            .as_ref()
            .or(self.variants[0].as_ref())
            .expect("the single-sample variant is built at creation")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_picks_the_largest_supported_count_below() {
        let mask = 1 | 4;
        assert_eq!(clamp_samples(8, mask), 4);
        assert_eq!(clamp_samples(2, mask), 1);
        assert_eq!(clamp_samples(4, 1 | 2 | 4 | 8), 4);
        assert_eq!(clamp_samples(1, 0), 1);
    }
}
