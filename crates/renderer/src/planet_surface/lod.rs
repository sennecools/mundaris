//! Complete flat cover with hysteresis and atomic local balancing transactions.
use super::{
    MetadataCache, PatchMetadata, SurfaceErrorContributions, SurfaceExtent, SurfaceTopology,
    cover::AddressSet,
};
use crate::{CelestialProjection, PreparedRenderFrame, PreparedView, RenderPreparationError};
use mundaris_math::{
    FrameId, FramePosition, LocalPosition,
    surface::{CubeFace, CubePatchAddress, PatchEdge},
};
#[cfg(feature = "surface-profile")]
use std::time::{Duration, Instant};

pub struct SurfaceViewInput<'view, 'tree> {
    pub view: &'view PreparedView<'tree>,
    pub body_fixed_frame: FrameId,
    pub reference_radius_m: f64,
    pub projection: CelestialProjection,
}

/// App-supplied, domain-free terrain bounds, error certificates, and geometry readiness.
pub trait SurfaceGeometryPolicy {
    fn certificate(
        &mut self,
        address: CubePatchAddress,
        metadata: PatchMetadata,
        radius_m: f64,
    ) -> Result<(SurfaceExtent, SurfaceErrorContributions), RenderPreparationError>;
    /// Record/request geometry for `address`, returning whether it is ready.
    fn request_ready(&mut self, address: CubePatchAddress) -> bool;
    /// Freeze topology replacement while an external transition is in progress.
    fn allow_replacement(&self) -> bool {
        true
    }
    /// Called after one complete balanced replacement is published.
    fn replacement_committed(&mut self) {}
    /// Resource preflight for the complete proposed covering surface.
    fn admit_replacement(&mut self, _cover_patches: usize) -> bool {
        true
    }
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
    pub fn with_pixel_thresholds(
        mut self,
        split: f64,
        merge: f64,
    ) -> Result<Self, RenderPreparationError> {
        if !split.is_finite() || !merge.is_finite() || merge <= 0.0 || split <= merge {
            return Err(RenderPreparationError::InvalidBudget);
        }
        self.split_px = split;
        self.merge_px = merge;
        Ok(self)
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
    pub scratch_bytes: usize,
    pub quality_pending: bool,
    pub settled: bool,
    /// Opt-in coarse CPU timings and scratch-capacity observations for this update.
    #[cfg(feature = "surface-profile")]
    pub profile: LodProfile,
}

/// Coarse CPU-stage measurements collected only with the `surface-profile` feature.
/// Stage durations are disjoint portions of `total`; they exclude source preparation
/// and small setup/validation gaps between measured stages.
#[cfg(feature = "surface-profile")]
#[derive(Debug, Default, Clone, Copy)]
pub struct LodProfile {
    /// Merge candidate construction and sibling/balance eligibility decisions.
    pub merge_decisions: Duration,
    /// Requested splits, dependency metadata readiness, balancing and atomic commits.
    pub split_ready_transactions: Duration,
    /// Active visible-leaf selection and stitch-mask construction, including fallback.
    pub visible_and_masks: Duration,
    /// Root-based desired coverage traversal and its relevance/quality decisions.
    pub desired_traversal: Duration,
    /// Final report aggregation, culling counters and scratch accounting.
    pub final_reporting: Duration,
    /// Wall duration of the update body, from just after source preparation to return.
    pub total: Duration,
    /// Capacity beyond the current length for reusable address/request/visible buffers.
    pub scratch_capacity_slack: usize,
    /// Owned container capacity changes; not allocator calls.
    pub capacity_growths: usize,
    /// Explicit temporary dependency vectors created by refinement transactions.
    pub dependency_vectors: usize,
    pub dependency_bytes: usize,
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
    cover: AddressSet,
    previous_splits: AddressSet,
    visible: Vec<ActiveSurfacePatch>,
    stack: Vec<CubePatchAddress>,
    requests: Vec<(CubePatchAddress, f64, bool, f64)>,
    desired: AddressSet,
    proposal: AddressSet,
    pins: AddressSet,
    coarse: AddressSet,
    merge_candidates: AddressSet,
    pending: AddressSet,
    pending_parent: Option<CubePatchAddress>,
    visible_scratch: Vec<ActiveSurfacePatch>,
}
impl Default for SurfaceLodSession {
    fn default() -> Self {
        Self::new(4096).expect("default metadata quota")
    }
}
impl SurfaceLodSession {
    /// App assigns per-body quotas; this owned session is the cache key namespace.
    pub fn new(cache_records: usize) -> Result<Self, RenderPreparationError> {
        if !(6..=4096).contains(&cache_records) {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let topology = SurfaceTopology::new();
        let mut cache = MetadataCache::new(cache_records);
        if cache.bytes() > 1024 * 1024 {
            return Err(RenderPreparationError::InvalidBudget);
        }
        let cover: AddressSet = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect();
        for &p in &cover {
            cache.build(p, &topology, &cover)?;
        }
        Ok(Self {
            topology,
            cache,
            cover,
            previous_splits: AddressSet::new(),
            visible: Vec::new(),
            stack: Vec::new(),
            requests: Vec::new(),
            desired: AddressSet::new(),
            proposal: AddressSet::new(),
            pins: AddressSet::new(),
            coarse: AddressSet::new(),
            merge_candidates: AddressSet::new(),
            pending: AddressSet::new(),
            pending_parent: None,
            visible_scratch: Vec::new(),
        })
    }
    pub fn topology(&self) -> &SurfaceTopology {
        &self.topology
    }
    pub fn cache_usage(&self) -> (usize, usize) {
        (self.cache.len(), self.cache.bytes())
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
        self.update_inner(input, settings, None)
    }
    pub fn update_with_policy(
        &mut self,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
        policy: &mut dyn SurfaceGeometryPolicy,
    ) -> Result<LodReport, RenderPreparationError> {
        self.update_inner(input, settings, Some(policy))
    }
    fn update_inner(
        &mut self,
        input: &SurfaceViewInput<'_, '_>,
        settings: &LodSettings,
        mut policy: Option<&mut dyn SurfaceGeometryPolicy>,
    ) -> Result<LodReport, RenderPreparationError> {
        if !input.reference_radius_m.is_finite() || input.reference_radius_m <= 0.0 {
            return Err(RenderPreparationError::InvalidDebugGeometry);
        }
        let source = input.view.prepare_source(input.body_fixed_frame)?;
        let mut report = LodReport::default();
        #[cfg(feature = "surface-profile")]
        let total_started = Instant::now();
        #[cfg(feature = "surface-profile")]
        let capacities = self.profile_capacities();
        self.cache.reset_counters();
        self.pins.clear();
        self.pins.extend(self.cover.iter().copied());
        for &leaf in &self.cover {
            let mut p = leaf.parent();
            while let Some(a) = p {
                self.pins.insert(a);
                p = a.parent();
            }
        }
        // Discard obsolete pending dependencies by relevance, never by host time.
        if let Some(parent) = self.pending_parent {
            let relevant = self.cover.contains(&parent)
                && self.cache.get(parent)?.is_some_and(|m| {
                    relevance(parent, m, input, &source, &mut policy)
                        .is_ok_and(|(v, e, _)| v && e > settings.split_px)
                });
            if !relevant {
                self.pending.clear();
                self.pending_parent = None;
            }
        }
        for &p in &self.pending {
            self.pins.insert(p);
        }
        // Coarsening is also one complete sibling transaction. Deepest first frees
        // pins before approaching a different region of the same body.
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        self.merge_candidates.clear();
        self.merge_candidates
            .extend(self.cover.iter().filter_map(|p| p.parent()));
        for index in (0..self.merge_candidates.len()).rev() {
            let parent = *self
                .merge_candidates
                .iter()
                .nth(index)
                .expect("candidate index");
            let children = parent
                .children()
                .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
            if !children.iter().all(|p| self.cover.contains(p)) {
                continue;
            }
            let m = self.cache.get(parent)?.expect("active ancestors pinned");
            let (visible, error, _) = relevance(parent, m, input, &source, &mut policy)?;
            if visible && error >= settings.merge_px {
                continue;
            }
            if merge_neighbors_valid(parent, &self.cover)
                && policy.as_deref_mut().is_none_or(|p| {
                    p.allow_replacement()
                        && p.admit_replacement(self.cover.len() - 3)
                        && p.request_ready(parent)
                })
            {
                for child in children {
                    self.cover.remove(&child);
                }
                self.cover.insert(parent);
                self.previous_splits.remove(&parent);
                report.merges += 1;
                if let Some(p) = policy.as_deref_mut() {
                    p.replacement_committed();
                }
            }
        }
        self.pins.clear();
        self.pins.extend(self.cover.iter().copied());
        for &leaf in &self.cover {
            let mut p = leaf.parent();
            while let Some(a) = p {
                self.pins.insert(a);
                p = a.parent();
            }
        }
        self.pins.extend(self.pending.iter().copied());
        #[cfg(feature = "surface-profile")]
        {
            report.profile.merge_decisions = stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        self.requests.clear();
        for &p in &self.cover {
            let m = self.cache.get(p)?.expect("active metadata pinned");
            let (visible, error, _) = relevance(p, m, input, &source, &mut policy)?;
            if visible && error > settings.split_px {
                if p.level() < settings.max_level {
                    let observer = source.observer_in_source().metres();
                    let local = address_contains_observer_direction(p, observer);
                    let (center, _) = m.ball(
                        input.reference_radius_m,
                        SurfaceExtent::smooth(input.reference_radius_m),
                    )?;
                    self.requests
                        .push((p, error, local, (center - observer).length()));
                } else {
                    report.precision_floor = true;
                }
            }
        }
        self.requests.sort_by(compare_request_priority);
        // Preserve the previous finite-error dependency completion policy. Only
        // uncertifiably infinite work needs the camera-local urgency tie-break.
        if self.requests.first().is_none_or(|r| !r.1.is_infinite())
            && let Some(parent) = self.pending_parent
            && let Some(index) = self.requests.iter().position(|r| r.0 == parent)
        {
            let request = self.requests.remove(index);
            self.requests.insert(0, request);
        }
        // Do not let a retained dependency transaction leapfrog newly observed
        // infinite-error work; the ordinary deterministic priority order selects it.
        // Highest-error local closures allow progress without simultaneously pinning
        // two whole global covers. All balancing dependencies activate atomically.
        for index in 0..self.requests.len() {
            let parent = self.requests[index].0;
            if !self.cover.contains(&parent) {
                continue;
            }
            if policy.as_deref().is_some_and(|p| !p.allow_replacement()) {
                continue;
            }
            let children = parent
                .children()
                .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
            self.proposal.clear();
            self.proposal.extend(self.cover.iter().copied());
            self.proposal.remove(&parent);
            self.proposal.extend(children);
            self.pending.clear();
            self.pending_parent = Some(parent);
            let mut ready = true;
            let mut forced = 0;
            for child in children {
                self.pending.insert(child);
            }
            loop {
                balance_violations(&self.proposal, &mut self.coarse);
                if !self.coarse.is_empty() {
                    if self.proposal.len() + 3 * self.coarse.len() > settings.cover_limit {
                        ready = false;
                        report.budget_constrained = true;
                        break;
                    }
                    for &p in &self.coarse {
                        let children = p
                            .children()
                            .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
                        self.proposal.remove(&p);
                        self.proposal.extend(children);
                        self.pending.extend(children);
                        forced += 1;
                    }
                }
                let dependencies: Vec<_> = self.pending.iter().copied().collect();
                if self.proposal.len() > settings.cover_limit
                    || policy
                        .as_deref_mut()
                        .is_some_and(|p| !p.admit_replacement(self.proposal.len()))
                {
                    ready = false;
                    report.budget_constrained = true;
                    break;
                }
                #[cfg(feature = "surface-profile")]
                {
                    report.profile.dependency_vectors += 1;
                    report.profile.dependency_bytes +=
                        dependencies.capacity() * std::mem::size_of::<CubePatchAddress>();
                }
                for p in dependencies {
                    self.pins.insert(p);
                    if let Some(policy) = policy.as_deref_mut() {
                        ready &= policy.request_ready(p);
                    }
                    if self.cache.get(p)?.is_none() {
                        if report.metadata_built < settings.new_metadata {
                            if self.cache.build(p, &self.topology, &self.pins)? {
                                report.metadata_built += 1;
                            } else {
                                ready = false;
                                report.budget_constrained = true;
                            }
                        } else {
                            ready = false;
                        }
                    }
                }
                if self.coarse.is_empty() {
                    break;
                }
            }
            if ready {
                visible_for(
                    &mut self.cache,
                    &self.proposal,
                    input,
                    &source,
                    &mut self.visible_scratch,
                    &mut policy,
                )?;
                if self.proposal.len() <= settings.cover_limit
                    && self.visible_scratch.len() <= settings.visible_limit
                {
                    std::mem::swap(&mut self.cover, &mut self.proposal);
                    self.previous_splits.insert(parent);
                    report.splits += 1;
                    report.balance_splits += forced;
                    if let Some(p) = policy.as_deref_mut() {
                        p.replacement_committed();
                    }
                    self.pending.clear();
                    self.pending_parent = None;
                } else {
                    ready = false;
                    report.budget_constrained = true;
                    report.constrained_refinements += 1;
                }
            }
            if !ready {
                report.deferred_transactions += 1;
                if self.cache.len() + self.pending.len() > 4096 {
                    report.budget_constrained = true;
                }
                // A fixed highest-error dependency chain has priority next update.
                // Lower-priority cold work cannot evict its partial ready siblings.
                break;
            }
        }
        #[cfg(feature = "surface-profile")]
        {
            report.profile.split_ready_transactions = stage_started.elapsed();
        }
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        visible_for(
            &mut self.cache,
            &self.cover,
            input,
            &source,
            &mut self.visible,
            &mut policy,
        )?;
        if self.visible.len() > settings.visible_limit {
            // Displaced ownership cannot fall back to metadata-only roots.
            if policy.is_none() {
                self.cover.clear();
                self.cover
                    .extend(CubeFace::ALL.into_iter().map(CubePatchAddress::root));
                self.previous_splits.clear();
                self.pending.clear();
                self.pending_parent = None;
                visible_for(
                    &mut self.cache,
                    &self.cover,
                    input,
                    &source,
                    &mut self.visible,
                    &mut policy,
                )?;
            }
            report.budget_constrained = true;
            report.constrained_refinements += 1;
        }
        #[cfg(feature = "surface-profile")]
        {
            report.profile.visible_and_masks = stage_started.elapsed();
        }
        // Rebuild desired coverage from roots using previous quality splits and
        // readiness. Missing subdomains explicitly remain an incomplete estimate.
        self.desired.clear();
        self.stack.clear();
        self.stack
            .extend(CubeFace::ALL.into_iter().rev().map(CubePatchAddress::root));
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        while let Some(p) = self.stack.pop() {
            let m = self
                .cache
                .get(p)?
                .expect("desired traversal enters ready children");
            let (visible, error, _) = relevance(p, m, input, &source, &mut policy)?;
            let split = if self.previous_splits.contains(&p) {
                error >= settings.merge_px
            } else {
                error > settings.split_px
            };
            if visible && split && p.level() < settings.max_level {
                let children = p
                    .children()
                    .map_err(|_| RenderPreparationError::InvalidDebugGeometry)?;
                let mut ready = true;
                for child in children {
                    ready &= self.cache.get(child)?.is_some();
                }
                // Desirability remains distinct from geometry availability.
                if ready && self.desired.len() + self.stack.len() + 4 <= settings.cover_limit {
                    self.stack.extend(children.into_iter().rev());
                    continue;
                }
                report.desired_estimate_incomplete = true;
            }
            self.desired.insert(p);
        }
        #[cfg(feature = "surface-profile")]
        {
            report.profile.desired_traversal = stage_started.elapsed();
        }
        report.desired_patches = self.desired.len();
        report.balanced_patches = self.cover.len();
        report.active_patches = self.cover.len();
        report.visible_patches = self.visible.len();
        report.desired_estimate_incomplete |= self.pending_parent.is_some();
        for p in &self.visible {
            report.max_level = report.max_level.max(p.address.level());
            report.max_error_pixels = report.max_error_pixels.max(p.error_pixels);
        }
        report.quality_pending =
            report.max_error_pixels > settings.split_px || self.pending_parent.is_some();
        report.settled = !report.quality_pending
            && report.splits == 0
            && report.merges == 0
            && report.metadata_built == 0;
        report.cache_records = self.cache.len();
        report.cache_bytes = self.cache.bytes();
        report.cache_hits = self.cache.hits;
        report.cache_misses = self.cache.misses;
        report.cache_evictions = self.cache.evictions;
        #[cfg(feature = "surface-profile")]
        let stage_started = Instant::now();
        for &p in &self.cover {
            let m = self.cache.get(p)?.expect("active metadata pinned");
            let (visible, _, horizon) = relevance(p, m, input, &source, &mut policy)?;
            if !visible {
                if horizon {
                    report.horizon_culled += 1;
                } else {
                    report.frustum_culled += 1;
                }
            }
        }
        report.scratch_bytes = [
            &self.cover,
            &self.previous_splits,
            &self.desired,
            &self.proposal,
            &self.pins,
            &self.coarse,
            &self.merge_candidates,
            &self.pending,
        ]
        .into_iter()
        .map(|s| s.bytes())
        .sum::<usize>()
            + self.stack.capacity() * std::mem::size_of::<CubePatchAddress>()
            + self.requests.capacity() * std::mem::size_of::<(CubePatchAddress, f64, bool, f64)>()
            + (self.visible.capacity() + self.visible_scratch.capacity())
                * std::mem::size_of::<ActiveSurfacePatch>();
        if report.scratch_bytes > 8 * 1024 * 1024 {
            return Err(RenderPreparationError::InvalidBudget);
        }
        #[cfg(feature = "surface-profile")]
        {
            report.profile.final_reporting = stage_started.elapsed();
            report.profile.scratch_capacity_slack =
                self.stack.capacity().saturating_sub(self.stack.len())
                    + self.requests.capacity().saturating_sub(self.requests.len())
                    + self.visible.capacity().saturating_sub(self.visible.len())
                    + self
                        .visible_scratch
                        .capacity()
                        .saturating_sub(self.visible_scratch.len());
            report.profile.total = total_started.elapsed();
            report.profile.capacity_growths = capacities
                .into_iter()
                .zip(self.profile_capacities())
                .filter(|(before, after)| after > before)
                .count();
        }
        Ok(report)
    }
    #[cfg(feature = "surface-profile")]
    fn profile_capacities(&self) -> [usize; 13] {
        [
            self.cover.bytes(),
            self.previous_splits.bytes(),
            self.desired.bytes(),
            self.proposal.bytes(),
            self.pins.bytes(),
            self.coarse.bytes(),
            self.merge_candidates.bytes(),
            self.pending.bytes(),
            self.stack.capacity(),
            self.requests.capacity(),
            self.visible.capacity(),
            self.visible_scratch.capacity(),
            self.cache.bytes(),
        ]
    }
}
fn visible_for(
    cache: &mut MetadataCache,
    cover: &AddressSet,
    input: &SurfaceViewInput<'_, '_>,
    source: &PreparedRenderFrame<'_>,
    output: &mut Vec<ActiveSurfacePatch>,
    policy: &mut Option<&mut dyn SurfaceGeometryPolicy>,
) -> Result<(), RenderPreparationError> {
    output.clear();
    for &p in cover {
        let metadata = cache.get(p)?.expect("ready covering leaf");
        let (visible, error, _) = relevance(p, metadata, input, source, policy)?;
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
    Ok(())
}
fn balance_violations(cover: &AddressSet, coarse: &mut AddressSet) {
    coarse.clear();
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
}
/// Query only the four outside edges. In a complete cover, a touching immediate
/// neighbor child that is not a leaf necessarily has level ≥parent+2 descendants.
/// This avoids rescanning the complete cover for every hypothetical sibling merge.
fn merge_neighbors_valid(parent: CubePatchAddress, cover: &AddressSet) -> bool {
    for edge in PatchEdge::ALL {
        let neighbor = parent.neighbor(edge);
        let mut n = neighbor.address;
        let mut coarse = None;
        loop {
            if cover.contains(&n) {
                coarse = Some(n);
                break;
            }
            if let Some(p) = n.parent() {
                n = p;
            } else {
                break;
            }
        }
        if let Some(n) = coarse {
            if parent.level().abs_diff(n.level()) > 1 {
                return false;
            }
            continue;
        }
        let children = neighbor
            .address
            .children()
            .expect("merge parent below maximum level");
        let touching = match neighbor.edge {
            PatchEdge::UMin => [0, 2],
            PatchEdge::UMax => [1, 3],
            PatchEdge::VMin => [0, 1],
            PatchEdge::VMax => [2, 3],
        };
        if touching.into_iter().any(|i| !cover.contains(&children[i])) {
            return false;
        }
    }
    true
}
fn relevance(
    address: CubePatchAddress,
    m: PatchMetadata,
    input: &SurfaceViewInput<'_, '_>,
    source: &PreparedRenderFrame<'_>,
    policy: &mut Option<&mut dyn SurfaceGeometryPolicy>,
) -> Result<(bool, f64, bool), RenderPreparationError> {
    let radius = input.reference_radius_m;
    let (extent, error) = if let Some(policy) = policy.as_deref_mut() {
        policy.certificate(address, m, radius)?
    } else {
        (
            SurfaceExtent::smooth(radius),
            SurfaceErrorContributions::default(),
        )
    };
    error.total_m()?;
    if (policy.is_none() || extent.guaranteed_opaque_radius_m > 0.0)
        && m.horizon_reject(source.observer_in_source().metres(), radius, extent)
    {
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
        if policy.is_some() {
            m.projected_total_error(error, center, r, input.projection)?
        } else {
            m.projected_error(center, r, radius, input.projection)
        },
        false,
    ))
}

fn address_contains_observer_direction(address: CubePatchAddress, observer: glam::DVec3) -> bool {
    // A camera at the exact body centre has no radial leaf. Keep diagnostics
    // renderable and use the distance/address fallback rather than rejecting it.
    let Ok(direction) = mundaris_math::Direction3::try_new(observer) else {
        return false;
    };
    let (face, uv) = mundaris_math::surface::SurfaceLocation::new(direction).face_uv();
    address_contains_direction(address, face, uv)
}

fn address_contains_direction(
    address: CubePatchAddress,
    face: mundaris_math::surface::CubeFace,
    uv: [f64; 2],
) -> bool {
    if address.face() != face {
        return false;
    }
    address
        .patch_local(uv)
        .is_ok_and(|st| st[0] >= 0.0 && st[0] <= 1.0 && st[1] >= 0.0 && st[1] <= 1.0)
}

fn compare_request_priority(
    a: &(CubePatchAddress, f64, bool, f64),
    b: &(CubePatchAddress, f64, bool, f64),
) -> std::cmp::Ordering {
    b.1.total_cmp(&a.1).then_with(|| {
        if a.1.is_infinite() && b.1.is_infinite() {
            b.2.cmp(&a.2)
                .then_with(|| a.3.total_cmp(&b.3))
                .then(a.0.cmp(&b.0))
        } else {
            a.0.cmp(&b.0)
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn infinite_error_priority_prefers_observer_local_then_distance_then_address() {
        let local = CubePatchAddress::try_new(CubeFace::PositiveX, 2, 1, 1).unwrap();
        let other = CubePatchAddress::try_new(CubeFace::PositiveX, 2, 3, 1).unwrap();
        assert!(!address_contains_observer_direction(
            local,
            glam::DVec3::ZERO
        ));
        assert!(address_contains_direction(
            local,
            CubeFace::PositiveX,
            [0.0, 0.0]
        ));
        assert!(!address_contains_direction(
            other,
            CubeFace::PositiveX,
            [0.0, 0.0]
        ));
        let mut requests: [(CubePatchAddress, f64, bool, f64); 2] = [
            (other, f64::INFINITY, false, 2.0),
            (local, f64::INFINITY, true, 9.0),
        ];
        requests.sort_by(compare_request_priority);
        assert_eq!(requests[0].0, local);
        requests[0].2 = false;
        requests[0].3 = 1.0;
        requests[1].3 = 3.0;
        requests.sort_by(compare_request_priority);
        assert_eq!(requests[0].0, local);
        requests[0].3 = 4.0;
        requests[1].3 = 4.0;
        requests.sort_by(compare_request_priority);
        assert_eq!(requests[0].0, local.min(other));
    }
    #[test]
    fn adversarial_corner_closure_to_level_twenty_is_complete_and_finite() {
        for face in CubeFace::ALL {
            for child_index in 0..4 {
                let mut cover: AddressSet = CubeFace::ALL
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
                let mut coarse = AddressSet::new();
                loop {
                    balance_violations(&cover, &mut coarse);
                    if coarse.is_empty() {
                        break;
                    }
                    for &p in &coarse {
                        assert!(cover.remove(&p));
                        cover.extend(p.children().unwrap());
                        forced += 1;
                    }
                    assert!(forced < 1024);
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
            }
        }
    }
}
