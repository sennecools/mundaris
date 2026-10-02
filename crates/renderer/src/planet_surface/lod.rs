//! Flat complete cover, deterministic hysteresis and atomic balanced replacement.
use super::{MetadataCache, PatchMetadata, SurfaceExtent, SurfaceTopology};
use crate::{CelestialProjection, PreparedRenderFrame, PreparedView, RenderPreparationError};
use mundaris_math::{
    FrameId, FramePosition, LocalPosition,
    surface::{CubeFace, CubePatchAddress, PatchEdge},
};
use std::collections::BTreeSet;

pub struct SurfaceViewInput<'view, 'tree> {
    pub view: &'view PreparedView<'tree>,
    pub body_fixed_frame: FrameId,
    pub reference_radius_m: f64,
    pub projection: CelestialProjection,
}
#[derive(Debug, Clone, Copy)]
pub struct LodSettings {
    split_px: f64,
    merge_px: f64,
    max_level: u8,
    new_metadata: usize,
    visible_limit: usize,
    cover_limit: usize,
}
impl Default for LodSettings {
    fn default() -> Self {
        Self {
            split_px: 0.125,
            merge_px: 0.0625,
            max_level: 30,
            new_metadata: 32,
            visible_limit: 4096,
            cover_limit: 65536,
        }
    }
}
impl LodSettings {
    pub fn with_work_limit(mut self, records: usize) -> Result<Self, RenderPreparationError> {
        if records > 4096 {
            return Err(RenderPreparationError::InvalidBudget);
        }
        self.new_metadata = records;
        Ok(self)
    }
    pub fn with_limits(
        mut self,
        visible: usize,
        cover: usize,
        level: u8,
    ) -> Result<Self, RenderPreparationError> {
        if !(6..=4096).contains(&visible) || !(6..=65536).contains(&cover) || level > 30 {
            return Err(RenderPreparationError::InvalidBudget);
        }
        self.visible_limit = visible;
        self.cover_limit = cover;
        self.max_level = level;
        Ok(self)
    }
    pub fn split_pixels(self) -> f64 {
        self.split_px
    }
    pub fn merge_pixels(self) -> f64 {
        self.merge_px
    }
}
#[derive(Debug, Default, Clone, Copy)]
pub struct LodReport {
    pub desired_patches: usize,
    pub balanced_patches: usize,
    pub active_patches: usize,
    pub visible_patches: usize,
    pub desired_estimate_incomplete: bool,
    pub max_level: u8,
    pub splits: usize,
    pub merges: usize,
    pub balance_splits: usize,
    pub constrained_refinements: usize,
    pub deferred_transactions: usize,
    pub frustum_culled: usize,
    pub horizon_culled: usize,
    pub max_error_pixels: f64,
    pub metadata_built: usize,
    pub cache_records: usize,
    pub cache_bytes: usize,
    pub cache_hits: usize,
    pub cache_misses: usize,
    pub cache_evictions: usize,
    pub precision_floor: bool,
    pub budget_constrained: bool,
}
#[derive(Debug, Clone, Copy)]
pub struct ActiveSurfacePatch {
    pub address: CubePatchAddress,
    pub stitch_mask: u8,
    pub metadata: PatchMetadata,
    pub error_pixels: f64,
}
pub struct SurfaceLodSession {
    topology: SurfaceTopology,
    cache: MetadataCache,
    cover: BTreeSet<CubePatchAddress>,
    previous_splits: BTreeSet<CubePatchAddress>,
    visible: Vec<ActiveSurfacePatch>,
    stack: Vec<CubePatchAddress>,
    requests: Vec<(CubePatchAddress, f64)>,
}
impl Default for SurfaceLodSession {
    fn default() -> Self {
        Self::new(4096).expect("default metadata quota")
    }
}
impl SurfaceLodSession {
    /// App assigns per-body quotas; cache key namespace is this owned session.
    pub fn new(cache_records: usize) -> Result<Self, RenderPreparationError> {
        if !(6..=4096).contains(&cache_records) {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let topology = SurfaceTopology::new();
        let mut cache = MetadataCache::new(cache_records);
        if cache.bytes() > 1024 * 1024 {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let cover: BTreeSet<_> = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        for &address in &cover {
            cache.build(address, &topology, &cover)?;
        }
        Ok(Self {
            topology,
            cache,
            cover,
            previous_splits: BTreeSet::new(),
            visible: Vec::new(),
            stack: Vec::new(),
            requests: Vec::new(),
        })
    }
    pub fn topology(&self) -> &SurfaceTopology {
        &self.topology
    }
    pub fn active_visible(&self) -> &[ActiveSurfacePatch] {
        &self.visible
    }
    pub fn covering_leaves(&self) -> impl Iterator<Item = CubePatchAddress> + '_ {
        self.cover.iter().copied()
    }
    pub fn update(
        &mut self,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
    ) -> Result<LodReport, RenderPreparationError> {
        if !input.reference_radius_m.is_finite() || input.reference_radius_m <= 0.0 {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let source = input.view.prepare_source(input.body_fixed_frame)?;
        self.cache.reset_counters();
        self.requests.clear();
        let mut pins = self.cover.clone();
        for &leaf in &self.cover {
            let mut p = leaf.parent();
            while let Some(a) = p {
                pins.insert(a);
                p = a.parent();
            }
        }
        let mut report = LodReport::default();
        let (mut desired, mut splits) =
            self.traverse(input, settings, &source, &mut report, &mut pins)?;
        self.requests
            .sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        self.requests.dedup_by_key(|r| r.0);
        for &(address, _) in &self.requests {
            if report.metadata_built >= settings.new_metadata {
                break;
            }
            if self.cache.build(address, &self.topology, &pins)? {
                report.metadata_built += 1;
                pins.insert(address);
            } else {
                report.budget_constrained = true;
            }
        }
        if report.metadata_built > 0 {
            self.requests.clear();
            (desired, splits) = self.traverse(input, settings, &source, &mut report, &mut pins)?;
        }
        report.desired_patches = desired.len();
        let mut closure = true;
        loop {
            let coarse = balance_violations(&desired);
            if coarse.is_empty() {
                break;
            }
            for parent in coarse {
                if desired.len() + 3 > settings.cover_limit {
                    closure = false;
                    report.budget_constrained = true;
                    break;
                }
                let children = parent
                    .children()
                    .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
                let mut ready = true;
                for child in children {
                    if self.cache.get(child)?.is_none() {
                        if report.metadata_built < settings.new_metadata
                            && self.cache.build(child, &self.topology, &pins)?
                        {
                            report.metadata_built += 1;
                            pins.insert(child);
                        } else {
                            ready = false;
                        }
                    }
                }
                if !ready {
                    closure = false;
                    report.deferred_transactions += 1;
                    break;
                }
                desired.remove(&parent);
                desired.extend(children);
                report.balance_splits += 1;
            }
            if !closure {
                break;
            }
        }
        report.balanced_patches = desired.len();
        if closure {
            let visible = self.visible_for(&desired, input, &source)?;
            if visible.len() <= settings.visible_limit {
                report.splits = desired.iter().filter(|a| !self.cover.contains(a)).count() / 4;
                report.merges = self.cover.iter().filter(|a| !desired.contains(a)).count() / 4;
                std::mem::swap(&mut self.cover, &mut desired);
                self.previous_splits.clear();
                self.previous_splits.append(&mut splits);
                self.visible = visible;
            } else {
                report.budget_constrained = true;
                report.constrained_refinements += 1;
            }
        }
        if !closure || report.budget_constrained {
            self.visible = self.visible_for(&self.cover.clone(), input, &source)?;
        }
        if self.visible.len() > settings.visible_limit {
            self.cover = CubeFace::ALL
                .into_iter()
                .map(CubePatchAddress::root)
                .collect();
            self.previous_splits.clear();
            self.visible = self.visible_for(&self.cover.clone(), input, &source)?;
            report.budget_constrained = true;
            report.constrained_refinements += 1;
        }
        report.active_patches = self.cover.len();
        report.visible_patches = self.visible.len();
        report.desired_estimate_incomplete = !self.requests.is_empty() || !closure;
        report.deferred_transactions += self.requests.len() / 4;
        for p in &self.visible {
            report.max_level = report.max_level.max(p.address.level());
            report.max_error_pixels = report.max_error_pixels.max(p.error_pixels);
        }
        report.cache_records = self.cache.len();
        report.cache_bytes = self.cache.bytes();
        report.cache_hits = self.cache.hits;
        report.cache_misses = self.cache.misses;
        report.cache_evictions = self.cache.evictions;
        // Count coarse logical leaves separately from selection candidates.
        for &p in &self.cover {
            let m = self.cache.get(p)?.expect("active metadata pinned");
            let (visible, _, horizon) = relevance(m, input, &source)?;
            if !visible {
                if horizon {
                    report.horizon_culled += 1;
                } else {
                    report.frustum_culled += 1;
                }
            }
        }
        Ok(report)
    }
    fn traverse(
        &mut self,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
        source: &PreparedRenderFrame<'_>,
        report: &mut LodReport,
        pins: &mut BTreeSet<CubePatchAddress>,
    ) -> Result<(BTreeSet<CubePatchAddress>, BTreeSet<CubePatchAddress>), RenderPreparationError>
    {
        let mut cover = BTreeSet::new();
        let mut splits = BTreeSet::new();
        self.stack.clear();
        self.stack
            .extend(CubeFace::ALL.into_iter().rev().map(CubePatchAddress::root));
        while let Some(p) = self.stack.pop() {
            let m = self
                .cache
                .get(p)?
                .expect("traversal only enters ready children");
            pins.insert(p);
            let (visible, error, _) = relevance(m, input, source)?;
            let threshold = if self.previous_splits.contains(&p) {
                settings.merge_px
            } else {
                settings.split_px
            };
            let needs_split = if self.previous_splits.contains(&p) {
                error >= threshold
            } else {
                error > threshold
            };
            if visible && needs_split && p.level() < settings.max_level {
                let children = p
                    .children()
                    .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
                let mut ready = true;
                for child in children {
                    if self.cache.get(child)?.is_none() {
                        ready = false;
                        self.requests.push((child, error));
                    } else {
                        pins.insert(child);
                    }
                }
                if ready && cover.len() + self.stack.len() + 4 <= settings.cover_limit {
                    splits.insert(p);
                    self.stack.extend(children.into_iter().rev());
                    continue;
                }
                report.desired_estimate_incomplete = true;
            } else if visible && needs_split {
                report.precision_floor = true;
            }
            cover.insert(p);
        }
        Ok((cover, splits))
    }
    fn visible_for(
        &mut self,
        cover: &BTreeSet<CubePatchAddress>,
        input: &SurfaceViewInput<'_, '_>,
        source: &PreparedRenderFrame<'_>,
    ) -> Result<Vec<ActiveSurfacePatch>, RenderPreparationError> {
        let mut output = Vec::new();
        for &p in cover {
            let metadata = self.cache.get(p)?.expect("ready proposed leaf");
            let (visible, error, _) = relevance(metadata, input, source)?;
            if visible {
                let mut mask = 0;
                for edge in PatchEdge::ALL {
                    let mut n = p.neighbor(edge).address;
                    while !cover.contains(&n) {
                        if let Some(parent) = n.parent() {
                            n = parent;
                        } else {
                            break;
                        }
                    }
                    if cover.contains(&n) && n.level() + 1 == p.level() {
                        mask |= edge.bit();
                    }
                }
                output.push(ActiveSurfacePatch {
                    address: p,
                    stitch_mask: mask,
                    metadata,
                    error_pixels: error,
                });
            }
        }
        Ok(output)
    }
}
fn balance_violations(cover: &BTreeSet<CubePatchAddress>) -> BTreeSet<CubePatchAddress> {
    let mut coarse = BTreeSet::new();
    for &leaf in cover {
        for edge in PatchEdge::ALL {
            let mut n = leaf.neighbor(edge).address;
            while !cover.contains(&n) {
                if let Some(parent) = n.parent() {
                    n = parent;
                } else {
                    break;
                }
            }
            if cover.contains(&n) && leaf.level() > n.level() + 1 {
                coarse.insert(n);
            }
        }
    }
    coarse
}
fn relevance(
    m: PatchMetadata,
    input: &SurfaceViewInput<'_, '_>,
    source: &PreparedRenderFrame<'_>,
) -> Result<(bool, f64, bool), RenderPreparationError> {
    let radius = input.reference_radius_m;
    let extent = SurfaceExtent::smooth(radius);
    if m.horizon_reject(source.observer_in_source().metres(), radius, extent) {
        return Ok((false, 0.0, true));
    }
    let (center, r) = m.ball(radius, extent)?;
    let center = source
        .view_displacement(FramePosition::new(
            input.body_fixed_frame,
            LocalPosition::try_metres(center)?,
        ))?
        .metres();
    if input.projection.rejects_ball(center, r)? {
        return Ok((false, 0.0, false));
    }
    Ok((
        true,
        m.projected_error(center, r, radius, input.projection),
        false,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adversarial_corner_closure_to_level_twenty_is_complete_and_finite() {
        for face in CubeFace::ALL {
            for child_index in 0..4 {
                let mut cover: BTreeSet<_> = CubeFace::ALL
                    .into_iter()
                    .map(CubePatchAddress::root)
                    .collect();
                let mut p = CubePatchAddress::root(face);
                for _ in 0..20 {
                    cover.remove(&p);
                    let children = p.children().unwrap();
                    cover.extend(children);
                    p = children[child_index];
                }
                let unbalanced = cover.len();
                let mut forced = 0;
                loop {
                    let coarse = balance_violations(&cover);
                    if coarse.is_empty() {
                        break;
                    }
                    for p in coarse {
                        assert!(cover.remove(&p));
                        cover.extend(p.children().unwrap());
                        forced += 1;
                    }
                    assert!(forced < 1024, "unexpected adversarial propagation");
                }
                assert_eq!(cover.len() - unbalanced, 3 * forced);
                for face in CubeFace::ALL {
                    let area: u128 = cover
                        .iter()
                        .filter(|p| p.face() == face)
                        .map(|p| 1u128 << (2 * (30 - p.level())))
                        .sum();
                    assert_eq!(area, 1u128 << 60);
                }
                assert!(balance_violations(&cover).is_empty());
            }
        }
    }
}
