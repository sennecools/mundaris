//! Coarsest-incident ownership on the complete, balanced, ready grid16 cover.
use super::{
    ActiveSurfacePatch, GRID_SAMPLES, GeneratedSurfacePatch, PatchMetadata, SurfaceGeometrySample,
    SurfaceTopology,
};
use crate::RenderPreparationError;
use mundaris_math::surface::{CubePatchAddress, PatchEdge};
use std::mem::size_of;

const MAX_STITCHED_PATCHES: usize = 4096;
type BoundaryReference = (u128, usize, usize);

/// Disposable constrained geometry. Terrain truth and unmodified cache entries
/// remain external; invisible incident leaves participate in ownership as well.
pub struct StitchedSurface {
    patches: Vec<GeneratedSurfacePatch>,
    masks: Vec<u8>,
}
impl StitchedSurface {
    /// Conservative allocated-capacity peak for construction, excluding inputs.
    pub fn construction_bytes(patches: usize) -> usize {
        size_of::<Self>()
            + patches
                * (size_of::<GeneratedSurfacePatch>()
                    + GRID_SAMPLES * size_of::<SurfaceGeometrySample>()
                    + 64 * size_of::<BoundaryReference>()
                    + size_of::<CubePatchAddress>()
                    + size_of::<u8>())
    }
    pub fn build(
        patches: &[ActiveSurfacePatch],
        geometry: &[&GeneratedSurfacePatch],
        _topology: &SurfaceTopology,
    ) -> Result<Self, RenderPreparationError> {
        if patches.len() != geometry.len() || patches.len() > MAX_STITCHED_PATCHES {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let addresses: Vec<_> = patches.iter().map(|p| p.address).collect();
        validate_cover(&addresses)?;
        if patches.iter().zip(geometry).any(|(p, g)| {
            p.address != g.address()
                || g.reference_radius_m() != geometry[0].reference_radius_m()
                || !expected_mask(p.address, &addresses).is_ok_and(|m| m == p.stitch_mask)
        }) {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        // Compact references, not a heap tree of duplicated physical samples.
        let mut owners = Vec::<BoundaryReference>::with_capacity(64 * patches.len());
        for (at, patch) in patches.iter().enumerate() {
            for j in 0..=16 {
                for i in 0..=16 {
                    if i == 0 || i == 16 || j == 0 || j == 16 {
                        let key = patch
                            .address
                            .sample_key(i, j, 16)
                            .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?
                            .compact_key();
                        owners.push((key, at, (j * 17 + i) as usize));
                    }
                }
            }
        }
        owners.sort_unstable_by(|a, b| {
            a.0.cmp(&b.0)
                .then(
                    patches[a.1]
                        .address
                        .level()
                        .cmp(&patches[b.1].address.level()),
                )
                .then(patches[a.1].address.cmp(&patches[b.1].address))
        });
        owners.dedup_by_key(|r| r.0);
        let mut output = Vec::with_capacity(patches.len());
        for (at, patch) in patches.iter().enumerate() {
            let source = geometry[at];
            let raw = source.samples();
            let mut samples = raw.to_vec();
            for j in 0..=16 {
                for i in 0..=16 {
                    if i == 0 || i == 16 || j == 0 || j == 16 {
                        let key = patch
                            .address
                            .sample_key(i, j, 16)
                            .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?
                            .compact_key();
                        let owner = owners[owners
                            .binary_search_by_key(&key, |r| r.0)
                            .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?];
                        samples[(j * 17 + i) as usize] = geometry[owner.1].samples()[owner.2];
                    }
                }
            }
            // Only referenced even boundary vertices matter on a collapsed edge.
            // Extend their profile correction smoothly over the first two rows.
            for j in 1..16 {
                for i in 1..16 {
                    let index = (j * 17 + i) as usize;
                    let mut height_delta = 0.0;
                    let mut normal_delta = glam::DVec3::ZERO;
                    let mut weight_sum: f64 = 0.0;
                    for (edge, distance, along) in [
                        (PatchEdge::UMin, i, j),
                        (PatchEdge::UMax, 16 - i, j),
                        (PatchEdge::VMin, j, i),
                        (PatchEdge::VMax, 16 - j, i),
                    ] {
                        if distance > 2 {
                            continue;
                        }
                        let step = if patch.stitch_mask & edge.bit() != 0 {
                            2
                        } else {
                            1
                        };
                        let a = along / step * step;
                        let b = (a + step).min(16);
                        let fraction = (along - a) as f64 / step as f64;
                        let correction = |t| {
                            let [u, v] = edge.grid(t, 16);
                            let k = (v * 17 + u) as usize;
                            (
                                samples[k].position_body_m.length()
                                    - raw[k].position_body_m.length(),
                                samples[k].normal_body - raw[k].normal_body,
                            )
                        };
                        let (ha, na) = correction(a);
                        let (hb, nb) = correction(b);
                        let weight = (3 - distance) as f64 / 3.0;
                        height_delta += weight * (ha + (hb - ha) * fraction);
                        normal_delta += weight * na.lerp(nb, fraction);
                        weight_sum += weight;
                    }
                    let divisor = weight_sum.max(1.0);
                    if height_delta != 0.0 {
                        let n = patch
                            .address
                            .sample_key(i, j, 16)
                            .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?
                            .direction()
                            .unit();
                        samples[index].position_body_m =
                            n * (raw[index].position_body_m.length() + height_delta / divisor);
                    }
                    if normal_delta != glam::DVec3::ZERO {
                        samples[index].normal_body =
                            (raw[index].normal_body + normal_delta / divisor).normalize();
                    }
                }
            }
            let perturbation = samples
                .iter()
                .zip(raw)
                .map(|(a, b)| (a.position_body_m - b.position_body_m).length())
                .fold(0.0, f64::max)
                .next_up();
            let mut extent = source.extent();
            // The terrain interval remains valid for truth. Union it with the
            // actual constrained vertex envelope, rather than expanding an
            // already-global interval by an unrelated maximum correction.
            for sample in &samples {
                let height = sample.position_body_m.length() - source.reference_radius_m();
                extent.min_height_m = extent.min_height_m.min(height.next_down());
                extent.max_height_m = extent.max_height_m.max(height.next_up());
            }
            let mut error = source.error();
            error.boundary_constraint_m = (error.boundary_constraint_m + perturbation).next_up();
            output.push(GeneratedSurfacePatch::new(
                source.address(),
                source.reference_radius_m(),
                source.footprint_m(),
                samples,
                extent,
                error,
            )?);
        }
        Ok(Self {
            patches: output,
            masks: patches.iter().map(|p| p.stitch_mask).collect(),
        })
    }
    pub fn patches(&self) -> &[GeneratedSurfacePatch] {
        &self.patches
    }
    pub fn stitch_mask(&self, index: usize) -> Option<u8> {
        self.masks.get(index).copied()
    }
    pub fn resident_bytes(&self) -> usize {
        size_of::<Self>()
            + self.patches.capacity() * size_of::<GeneratedSurfacePatch>()
            + self.masks.capacity()
            + self
                .patches
                .iter()
                .map(GeneratedSurfacePatch::resident_heap_capacity_bytes)
                .sum::<usize>()
    }
}

fn validate_cover(addresses: &[CubePatchAddress]) -> Result<(), RenderPreparationError> {
    if addresses.len() < 6 || addresses.len() > MAX_STITCHED_PATCHES {
        return Err(RenderPreparationError::InvalidBudget);
    }
    if addresses.windows(2).any(|w| w[0] >= w[1]) {
        return Err(RenderPreparationError::InvalidDebugGeometry);
    }
    for face in mundaris_math::surface::CubeFace::ALL {
        let area: u128 = addresses
            .iter()
            .filter(|a| a.face() == face)
            .map(|a| 1u128 << (2 * (30 - a.level())))
            .sum();
        if area != 1u128 << 60 {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
    }
    for address in addresses {
        let mut ancestor = address.parent();
        while let Some(parent) = ancestor {
            if addresses.binary_search(&parent).is_ok() {
                return Err(RenderPreparationError::InvalidDebugGeometry);
            }
            ancestor = parent.parent();
        }
        expected_mask(*address, addresses)?;
    }
    Ok(())
}
fn expected_mask(
    address: CubePatchAddress,
    addresses: &[CubePatchAddress],
) -> Result<u8, RenderPreparationError> {
    let mut mask = 0;
    for edge in PatchEdge::ALL {
        let mut neighbor = Some(address.neighbor(edge).address);
        while let Some(n) = neighbor {
            if addresses.binary_search(&n).is_ok() {
                if address.level().abs_diff(n.level()) > 1 {
                    return Err(RenderPreparationError::InvalidDebugGeometry);
                }
                if n.level() < address.level() {
                    mask |= edge.bit();
                }
                break;
            }
            neighbor = n.parent();
        }
    }
    Ok(mask)
}
/// Construct metadata and masks, rejecting incomplete or unsupported covers.
pub fn active_surface_cover(
    addresses: &[CubePatchAddress],
    topology: &SurfaceTopology,
) -> Result<Vec<ActiveSurfacePatch>, RenderPreparationError> {
    let mut addresses = addresses.to_vec();
    addresses.sort_unstable();
    validate_cover(&addresses)?;
    addresses
        .iter()
        .map(|&address| {
            Ok(ActiveSurfacePatch {
                address,
                stitch_mask: expected_mask(address, &addresses)?,
                metadata: PatchMetadata::build(address, topology)
                    .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?,
                error_pixels: 0.0,
            })
        })
        .collect()
}
