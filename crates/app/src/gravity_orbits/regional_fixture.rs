//! Opt-in regional residency and local publication over the complete world field.
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::Arc,
    time::Duration,
};

use crate::{regional_terrain::*, resident_terrain::TileBuildIdentity};
use anyhow::{Context, Result, ensure};
use glam::DVec3;
use mundaris_math::surface::CubePatchAddress;
use mundaris_renderer::{
    regional_edges::{TileBoundary, build_boundaries, subdivided_parent_boundary},
    *,
};
use mundaris_world::terrain::SurfaceGenerator;

#[derive(Clone, Copy)]
pub(super) struct RegionalSettings {
    pub enabled: bool,
    pub max_depth: u8,
    pub gpu_slots: usize,
    pub cpu_tiles: usize,
    pub worker_count: usize,
    pub worker_delay_ms: u64,
    pub upload_tiles_per_frame: usize,
    pub upload_bytes_per_frame: u64,
    pub publication_groups_per_frame: usize,
    pub transition_limit: usize,
    pub morph_duration_ms: u64,
    pub split_error_px: f64,
    pub merge_error_px: f64,
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use crate::resident_terrain::ResidentTileBuilder;
    use mundaris_math::surface::CubeFace;
    use mundaris_world::terrain::{
        SurfaceAlgorithm, SurfaceDefinition, TerrainIdentity, TerrainSeed,
    };

    fn setup(revision: u64) -> (SurfaceGenerator, TileBuildIdentity, Arc<TileData>) {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(23),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
        let identity = TileBuildIdentity {
            body_identity: 23,
            surface_revision: revision,
            material_revision: 1,
        };
        let tile = ResidentTileBuilder::build(
            &generator,
            identity,
            CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 157, 39).unwrap(),
            2,
        )
        .unwrap()
        .0;
        (generator, identity, Arc::new(tile))
    }

    fn settings(slots: usize) -> RegionalSettings {
        RegionalSettings {
            enabled: true,
            max_depth: 1,
            gpu_slots: slots,
            cpu_tiles: 16,
            worker_count: 1,
            worker_delay_ms: 0,
            upload_tiles_per_frame: 1,
            upload_bytes_per_frame: 1_048_576,
            publication_groups_per_frame: 1,
            transition_limit: 1,
            morph_duration_ms: 150,
            split_error_px: 2.0,
            merge_error_px: 1.0,
        }
    }

    fn slot(tile: Arc<TileData>, generation: usize) -> Slot {
        let mut state = TileSlotState::default();
        let mut key = tile.key.clone();
        if generation > 1 {
            key.material_revision += 1;
        }
        let mut token = state.request(&key).unwrap();
        for index in 1..generation {
            key = tile.key.clone();
            if index + 1 != generation {
                key.material_revision += index as u64 + 1;
            }
            token = state.request(&key).unwrap();
        }
        Slot {
            state,
            tile,
            token,
            last_use: 0,
        }
    }

    #[test]
    fn shrink_regrow_preserves_retired_slot_generation() {
        let (generator, identity, tile) = setup(1);
        let mut fixture = RegionalFixture::default();
        fixture
            .configure(
                settings(8),
                Some(Arc::clone(&tile)),
                Some(generator.clone()),
                Some(identity),
            )
            .unwrap();
        fixture.slots[7] = Some(slot(Arc::clone(&tile), 3));
        fixture
            .configure(
                settings(5),
                Some(Arc::clone(&tile)),
                Some(generator.clone()),
                Some(identity),
            )
            .unwrap();
        assert!(fixture.slot_for(tile.key.address).is_none());
        fixture
            .configure(settings(8), Some(tile), Some(generator), Some(identity))
            .unwrap();
        assert_eq!(fixture.slots[7].as_ref().unwrap().state.generation(), 3);
    }

    #[test]
    fn reconfiguration_retains_monotonic_gpu_cache_age_and_frontier_pins() {
        let (generator, identity, tile) = setup(1);
        let mut fixture = RegionalFixture::default();
        fixture
            .configure(
                settings(8),
                Some(Arc::clone(&tile)),
                Some(generator.clone()),
                Some(identity),
            )
            .unwrap();
        fixture.frame = 500;
        fixture.gpu_use_clock = 10_000;
        let mut retained = slot(Arc::clone(&tile), 1);
        retained.last_use = 9_999;
        fixture.slots[7] = Some(retained);
        fixture
            .configure(
                settings(8),
                Some(tile.clone()),
                Some(generator),
                Some(identity),
            )
            .unwrap();
        assert_eq!(fixture.frame, 0);
        assert_eq!(fixture.gpu_use_clock, 10_000);
        assert_eq!(fixture.slots[7].as_ref().unwrap().last_use, 9_999);
        // Every admitted sibling remains pinned even when the complete current
        // frontier fits and no single constrained quartet is reserved.
        fixture
            .gpu_frontier_pins
            .extend(tile.key.address.children().unwrap());
        assert!(fixture.gpu_split_reservation.is_none());
        assert!(
            tile.key
                .address
                .children()
                .unwrap()
                .iter()
                .all(|a| fixture.pins().contains(a))
        );
    }

    #[test]
    fn obsolete_same_address_key_cannot_clear_current_residency() {
        let (_, _, old) = setup(1);
        let (generator, identity, current) = setup(2);
        let mut fixture = RegionalFixture::default();
        fixture
            .configure(
                settings(5),
                Some(Arc::clone(&current)),
                Some(generator),
                Some(identity),
            )
            .unwrap();
        fixture.slots[0] = Some(slot(old, 1));
        fixture.slots[1] = Some(slot(Arc::clone(&current), 2));
        let mut report = RegionalResidentReport::default();
        report.slots.resize(5, RegionalSlotReport::default());
        report.slots[1].key = Some(current.key.clone());
        fixture.observe(report);
        assert_eq!(fixture.slot_for(current.key.address), Some(1));
        assert_eq!(fixture.core.as_ref().unwrap().snapshot().resident_count, 1);
    }

    #[test]
    fn full_pinned_pool_rejects_authority_replacement_before_mutation() {
        let (generator, identity, old) = setup(1);
        let mut fixture = RegionalFixture::default();
        fixture
            .configure(
                settings(5),
                Some(Arc::clone(&old)),
                Some(generator),
                Some(identity),
            )
            .unwrap();
        for slot_record in &mut fixture.slots {
            *slot_record = Some(slot(Arc::clone(&old), 1));
        }
        fixture.last_report.slots = (0..5)
            .map(|_| RegionalSlotReport {
                key: Some(old.key.clone()),
                pinned: true,
                ..RegionalSlotReport::default()
            })
            .collect();
        let (generator, identity, current) = setup(2);
        assert!(
            fixture
                .configure(
                    settings(5),
                    Some(Arc::clone(&current)),
                    Some(generator.clone()),
                    Some(identity)
                )
                .is_err()
        );
        assert!(fixture.enabled);
        assert_eq!(
            fixture
                .core
                .as_ref()
                .unwrap()
                .tile(old.key.address)
                .unwrap()
                .key,
            old.key
        );
        fixture
            .configure(settings(6), Some(current), Some(generator), Some(identity))
            .unwrap();
    }

    #[test]
    fn gpu_reservation_keeps_a_complete_quartet_when_priorities_change() {
        let a = CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 0, 0).unwrap();
        let b = CubePatchAddress::try_new(CubeFace::PositiveZ, 1, 1, 0).unwrap();
        let base = BTreeSet::from([a, b]);
        let mut groups = [
            RegionalSplitFrontier {
                parent: a,
                children: a.children().unwrap(),
                aggregate_priority: 20.0,
                ready_children: 2,
                parent_resident: true,
            },
            RegionalSplitFrontier {
                parent: b,
                children: b.children().unwrap(),
                aggregate_priority: 10.0,
                ready_children: 2,
                parent_resident: true,
            },
        ];
        let admission = select_gpu_frontier(&base, &groups, None, 6);
        assert!(admission.constrained);
        assert!(!admission.pressure);
        assert_eq!(admission.reserved_parent, Some(a));
        groups[1].aggregate_priority = 100.0;
        groups[1].ready_children = 4;
        assert_eq!(
            select_gpu_frontier(&base, &groups, Some(a), 6).reserved_parent,
            Some(a)
        );
        assert!(!select_gpu_frontier(&base, &groups, None, 10).constrained);
    }

    #[test]
    fn mixed_split_merge_admission_restores_one_parent_within_peak_capacity() {
        let a = CubePatchAddress::try_new(CubeFace::PositiveZ, 2, 0, 0).unwrap();
        let b = CubePatchAddress::try_new(CubeFace::PositiveZ, 2, 1, 0).unwrap();
        let split = CubePatchAddress::try_new(CubeFace::PositiveZ, 2, 2, 0).unwrap();
        let mut base = BTreeSet::from([split]);
        base.extend(a.children().unwrap());
        base.extend(b.children().unwrap());
        let merges = BTreeSet::from([a, b]);
        // All missing merge parents plus a quartet need 15 slots. Admit only
        // one restoring parent, complete its merge, then restore the other.
        let (restore, pressure) = select_gpu_merge_restore(&base, &merges, 10);
        assert_eq!(restore, Some(a));
        assert!(!pressure);
        assert_eq!(base.union(&BTreeSet::from([a])).count(), 10);
        for child in a.children().unwrap() {
            base.remove(&child);
        }
        base.insert(a);
        let (restore, pressure) = select_gpu_merge_restore(&base, &BTreeSet::from([b]), 10);
        assert_eq!(restore, Some(b));
        assert!(!pressure);
        for child in b.children().unwrap() {
            base.remove(&child);
        }
        base.insert(b);
        let frontier = RegionalSplitFrontier {
            parent: split,
            children: split.children().unwrap(),
            aggregate_priority: 10.0,
            ready_children: 0,
            parent_resident: true,
        };
        assert!(!select_gpu_frontier(&base, &[frontier], None, 10).pressure);
        assert_eq!(select_gpu_merge_restore(&base, &merges, 2), (None, true));
    }

    #[test]
    fn native_ten_patch_cover_requires_fifteen_peak_slots_for_another_split() {
        let root = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 157, 39).unwrap();
        let children = root.children().unwrap();
        let mut base = BTreeSet::from([root, children[1], children[3]]);
        base.extend(children[0].children().unwrap());
        base.extend(children[2].children().unwrap());
        assert_eq!(base.len(), 11);
        let groups: Vec<_> = [children[1], children[3]]
            .into_iter()
            .map(|parent| RegionalSplitFrontier {
                parent,
                children: parent.children().unwrap(),
                aggregate_priority: 1.0,
                ready_children: 0,
                parent_resident: true,
            })
            .collect();
        let admission = select_gpu_frontier(&base, &groups, None, 12);
        assert!(admission.pressure);
        assert_eq!(admission.minimum_peak_slots, Some(15));
        assert_eq!(admission.reserved_parent, None);
        assert!(!select_gpu_frontier(&BTreeSet::from([root]), &[], None, 12).pressure);
    }

    #[test]
    fn small_gpu_pool_must_filter_uploads_even_when_current_frontier_fits() {
        let root = CubePatchAddress::try_new(CubeFace::PositiveZ, 9, 157, 39).unwrap();
        let config = RegionalConfig {
            roots: vec![root],
            max_level: 13,
            ..RegionalConfig::default()
        };
        let frontier = RegionalSplitFrontier {
            parent: root,
            children: root.children().unwrap(),
            aggregate_priority: 1.0,
            ready_children: 0,
            parent_resident: true,
        };
        let base = BTreeSet::from([root]);
        assert!(!select_gpu_frontier(&base, &[frontier], None, 12).constrained);
        assert_eq!(finite_region_tile_capacity(&config), 341);
        assert!(12 < finite_region_tile_capacity(&config));
        assert!(384 >= finite_region_tile_capacity(&config));
    }
}

impl RegionalSettings {
    pub fn validate(self) -> Result<()> {
        ensure!(
            (1..=4).contains(&self.max_depth),
            "regional depth outside prototype range"
        );
        ensure!(
            (5..=512).contains(&self.gpu_slots),
            "regional GPU slots outside prototype range"
        );
        ensure!(
            (5..=1024).contains(&self.cpu_tiles),
            "regional CPU tiles outside prototype range"
        );
        ensure!(
            (1..=8).contains(&self.worker_count),
            "regional worker count outside prototype range"
        );
        ensure!(
            self.worker_delay_ms <= 5_000 && self.morph_duration_ms <= 10_000,
            "regional delay outside prototype range"
        );
        ensure!(
            (1..=16).contains(&self.upload_tiles_per_frame),
            "regional upload count outside prototype range"
        );
        ensure!(
            self.upload_bytes_per_frame > 0 && self.upload_bytes_per_frame <= 64 * 1024 * 1024,
            "regional upload bytes outside prototype range"
        );
        ensure!(
            (1..=8).contains(&self.publication_groups_per_frame)
                && (1..=16).contains(&self.transition_limit),
            "regional publication budget outside prototype range"
        );
        ensure!(
            self.split_error_px.is_finite()
                && self.merge_error_px.is_finite()
                && self.merge_error_px > 0.0
                && self.split_error_px > self.merge_error_px,
            "invalid regional hysteresis thresholds"
        );
        Ok(())
    }
}

struct Slot {
    state: TileSlotState,
    tile: Arc<TileData>,
    token: TilePublicationToken,
    last_use: u64,
}

struct Patch {
    parent: CubePatchAddress,
    quadrant: Option<[u32; 2]>,
    endpoints: Arc<RegionalBoundaryEndpoints>,
    group: Option<u64>,
    merging: bool,
}

struct Group {
    id: u64,
    fraction: f32,
    affected: BTreeSet<CubePatchAddress>,
    merge: Option<(CubePatchAddress, [CubePatchAddress; 4])>,
}

#[derive(serde::Serialize)]
struct NativeFrameSample {
    frame: u64,
    interval_ms: f64,
    regional_advance_ms: f64,
    publication_ms: f64,
    preparation_ms: Option<f64>,
    gpu_terrain_ms: Option<f64>,
    gpu_source_frame: Option<u64>,
    desired: usize,
    drawable: usize,
    transitions: usize,
    tile_upload_bytes: u64,
    boundary_upload_bytes: u64,
}

struct GpuFrontierAdmission {
    reserved_parent: Option<CubePatchAddress>,
    constrained: bool,
    pressure: bool,
    minimum_peak_slots: Option<usize>,
}

fn select_gpu_frontier(
    base: &BTreeSet<CubePatchAddress>,
    frontiers: &[RegionalSplitFrontier],
    previous: Option<CubePatchAddress>,
    capacity: usize,
) -> GpuFrontierAdmission {
    let peak =
        |group: &RegionalSplitFrontier| base.union(&group.children.into_iter().collect()).count();
    let minimum_peak_slots = frontiers.iter().map(peak).min();
    let mut all = base.clone();
    all.extend(frontiers.iter().flat_map(|group| group.children));
    let constrained = all.len() > capacity;
    if !constrained {
        return GpuFrontierAdmission {
            reserved_parent: None,
            constrained: false,
            pressure: false,
            minimum_peak_slots,
        };
    }
    let retained = previous.and_then(|parent| {
        frontiers
            .iter()
            .find(|group| group.parent == parent && peak(group) <= capacity)
    });
    let mut ranked: Vec<_> = frontiers
        .iter()
        .filter(|group| peak(group) <= capacity)
        .collect();
    ranked.sort_by(|a, b| {
        b.ready_children
            .cmp(&a.ready_children)
            .then_with(|| b.aggregate_priority.total_cmp(&a.aggregate_priority))
            .then_with(|| a.parent.cmp(&b.parent))
    });
    let reserved_parent = retained
        .or_else(|| ranked.first().copied())
        .map(|group| group.parent);
    GpuFrontierAdmission {
        reserved_parent,
        constrained: true,
        pressure: reserved_parent.is_none() && !frontiers.is_empty(),
        minimum_peak_slots,
    }
}

fn select_gpu_merge_restore(
    base: &BTreeSet<CubePatchAddress>,
    merge_parents: &BTreeSet<CubePatchAddress>,
    capacity: usize,
) -> (Option<CubePatchAddress>, bool) {
    let missing = merge_parents.difference(base).next().copied();
    let peak = base.len() + usize::from(missing.is_some());
    (missing.filter(|_| peak <= capacity), peak > capacity)
}

fn finite_region_tile_capacity(config: &RegionalConfig) -> usize {
    config.roots.iter().fold(0usize, |total, root| {
        let mut level_tiles = 1usize;
        let mut region_tiles = 1usize;
        for _ in root.level()..config.max_level {
            level_tiles = level_tiles.saturating_mul(4);
            region_tiles = region_tiles.saturating_add(level_tiles);
        }
        total.saturating_add(region_tiles)
    })
}

#[derive(Default)]
pub(super) struct RegionalFixture {
    pub enabled: bool,
    core: Option<RegionalTerrain>,
    settings: Option<RegionalSettings>,
    slots: Vec<Option<Slot>>,
    boundaries: BTreeMap<CubePatchAddress, TileBoundary>,
    patches: BTreeMap<CubePatchAddress, Patch>,
    groups: Vec<Group>,
    next_group: u64,
    frame: u64,
    gpu_use_clock: u64,
    last_camera: Option<DVec3>,
    frame_intervals_ms: Vec<f64>,
    native_frame_samples: VecDeque<NativeFrameSample>,
    regional_advance_ms: f64,
    events: Vec<serde_json::Value>,
    topology_deferred: u64,
    slot_pressure: bool,
    gpu_admission_pressure: bool,
    gpu_split_reservation: Option<CubePatchAddress>,
    gpu_frontier_pins: BTreeSet<CubePatchAddress>,
    gpu_merge_parents: BTreeSet<CubePatchAddress>,
    gpu_policy_initialized: bool,
    gpu_policy_desired: Vec<CubePatchAddress>,
    gpu_policy_drawable: Vec<CubePatchAddress>,
    gpu_policy_base_pins: BTreeSet<CubePatchAddress>,
    gpu_minimum_peak_split_slots: Option<usize>,
    gpu_split_frontier_count: usize,
    cpu_publication_ms: f64,
    cache_hits_gpu: u64,
    evictions_gpu: u64,
    reuploads_gpu: u64,
    uploaded_keys: BTreeSet<CubePatchAddress>,
    last_report: RegionalResidentReport,
}

impl RegionalFixture {
    pub fn configure(
        &mut self,
        settings: RegionalSettings,
        parent: Option<Arc<TileData>>,
        generator: Option<SurfaceGenerator>,
        identity: Option<TileBuildIdentity>,
    ) -> Result<()> {
        settings.validate()?;
        if !settings.enabled {
            self.disable();
            return Ok(());
        }
        self.pump_shutdown();
        ensure!(
            self.core.as_ref().is_none_or(RegionalTerrain::is_idle),
            "regional reconfiguration waits for cancelled workers to drain"
        );
        let parent = parent.context("regional fixture needs a prebuilt root")?;
        let root_slot_available = (0..settings.gpu_slots).any(|index| {
            self.slots
                .get(index)
                .and_then(Option::as_ref)
                .is_some_and(|slot| slot.tile.key == parent.key)
                || self
                    .last_report
                    .slots
                    .get(index)
                    .is_none_or(|report| report.key.is_none() || report.reuse_safe)
        });
        ensure!(
            root_slot_available,
            "regional authority replacement needs a free or submission-safe slot"
        );
        let bytes = parent.texels.len() as u64 * 32;
        ensure!(
            settings.upload_bytes_per_frame >= bytes,
            "regional upload budget cannot admit one tile"
        );
        let root = parent.key.address;
        ensure!(
            root.level().saturating_add(settings.max_depth) <= 28,
            "regional boundary keys exceed supported depth"
        );
        let config = RegionalConfig {
            roots: vec![root],
            max_level: root.level() + settings.max_depth,
            cells: parent.key.cells,
            cpu_tile_cap: settings.cpu_tiles,
            cpu_byte_cap: settings.cpu_tiles * parent.retained_payload_bytes(),
            worker_count: settings.worker_count,
            worker_delay: Duration::from_millis(settings.worker_delay_ms),
            queue_cap: 128,
            completion_cap: settings.worker_count * 2,
            admission_cap_per_tick: settings.worker_count,
            max_desired_patches: 256,
            upload_tile_cap: settings.upload_tiles_per_frame,
            upload_byte_cap: settings.upload_bytes_per_frame as usize,
            publication_cap_per_tick: settings.publication_groups_per_frame,
            transition_cap: settings.transition_limit,
            split_threshold_px: settings.split_error_px,
            merge_threshold_px: settings.merge_error_px,
            prediction_seconds: 0.15,
            high_speed_mps: 500.0,
        };
        let mut core = RegionalTerrain::new(
            generator.context("regional generator unavailable")?,
            identity.context("regional identity unavailable")?,
            config,
        )?;
        core.seed_tile(parent)?;
        // Preserve slot generations across reconfiguration. Content lookup below
        // requires the new authority's complete key, even at the same address.
        let mut slots = std::mem::take(&mut self.slots);
        if slots.len() < settings.gpu_slots {
            slots.resize_with(settings.gpu_slots, || None);
        }
        let report = std::mem::take(&mut self.last_report);
        let next_group = self.next_group;
        let gpu_use_clock = self.gpu_use_clock;
        *self = Self {
            enabled: true,
            core: Some(core),
            settings: Some(settings),
            slots,
            last_report: report,
            next_group,
            gpu_use_clock,
            ..Self::default()
        };
        Ok(())
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        if let Some(core) = &mut self.core {
            core.request_cancel_all();
        }
    }

    pub fn pump_shutdown(&mut self) {
        if let Some(core) = &mut self.core {
            core.drain_cancelled();
        }
    }

    fn record(&mut self, event: serde_json::Value) {
        if self.events.len() == 4096 {
            self.events.remove(0);
        }
        self.events.push(event);
    }

    pub fn observe(&mut self, report: RegionalResidentReport) {
        if let Some(core) = &mut self.core {
            for slot in self.slots.iter().flatten() {
                if core
                    .tile(slot.tile.key.address)
                    .is_some_and(|tile| tile.key == slot.tile.key)
                    && !report
                        .slots
                        .iter()
                        .any(|r| r.key.as_ref() == Some(&slot.tile.key))
                {
                    core.mark_not_resident(slot.tile.key.address);
                }
            }
            for (index, slot) in report.slots.iter().enumerate() {
                if index >= self.settings.map_or(0, |settings| settings.gpu_slots) {
                    continue;
                }
                if let Some(key) = &slot.key
                    && core.tile(key.address).is_some_and(|tile| tile.key == *key)
                    && self.slots[index]
                        .as_ref()
                        .is_some_and(|local| local.tile.key == *key)
                {
                    core.ack_resident(key.address);
                }
            }
        }
        self.last_report = report;
    }

    fn slot_for(&self, address: CubePatchAddress) -> Option<usize> {
        let key = &self.core.as_ref()?.tile(address)?.key;
        self.slots
            .iter()
            .take(self.settings?.gpu_slots)
            .position(|s| s.as_ref().is_some_and(|s| s.tile.key == *key))
    }

    fn base_pins(&self) -> BTreeSet<CubePatchAddress> {
        let mut pins = BTreeSet::new();
        for (address, patch) in &self.patches {
            pins.insert(*address);
            pins.insert(patch.parent);
        }
        if let Some(core) = &self.core {
            for root in &core.config().roots {
                pins.insert(*root);
            }
        }
        for group in &self.groups {
            if let Some((parent, _)) = group.merge {
                pins.insert(parent);
            }
        }
        pins
    }

    fn pins(&self) -> BTreeSet<CubePatchAddress> {
        let mut pins = self.base_pins();
        pins.extend(self.gpu_frontier_pins.iter().copied());
        if let Some(parent) = self.gpu_split_reservation
            && let Ok(children) = parent.children()
        {
            pins.extend(children);
        }
        pins.extend(
            self.gpu_merge_parents
                .iter()
                .filter(|parent| self.slot_for(**parent).is_some())
                .copied(),
        );
        pins
    }

    fn update_gpu_admission(&mut self) -> Result<()> {
        let core = self.core.as_ref().context("regional core unavailable")?;
        let desired_changed = self.gpu_policy_desired != core.desired();
        let drawable_changed = self.gpu_policy_drawable != core.drawable();
        if !self.gpu_policy_initialized || desired_changed || drawable_changed {
            let drawable: BTreeSet<_> = core.drawable().iter().copied().collect();
            self.gpu_merge_parents = drawable
                .iter()
                .filter_map(|child| child.parent())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .filter(|parent| {
                    core.desired()
                        .iter()
                        .any(|desired| desired.contains(*parent))
                })
                .filter(|parent| {
                    let Ok(children) = parent.children() else {
                        return false;
                    };
                    if !children.iter().all(|child| drawable.contains(child)) {
                        return false;
                    }
                    let mut trial: Vec<_> = drawable
                        .iter()
                        .filter(|address| !children.contains(address))
                        .copied()
                        .collect();
                    trial.push(*parent);
                    core.cover_is_valid(&trial)
                })
                .collect();
        }
        let mut base = self.base_pins();
        base.extend(
            self.gpu_merge_parents
                .iter()
                .filter(|parent| self.slot_for(**parent).is_some())
                .copied(),
        );
        if self.gpu_policy_initialized
            && !desired_changed
            && !drawable_changed
            && base == self.gpu_policy_base_pins
        {
            return Ok(());
        }
        let mut frontiers = core.split_frontiers();
        for group in &mut frontiers {
            group.ready_children = group
                .children
                .iter()
                .filter(|child| self.slot_for(**child).is_some())
                .count();
        }
        let capacity = self
            .settings
            .context("regional settings missing")?
            .gpu_slots;
        // Restore one balanced coarsening parent before competing split uploads.
        // Missing parents consume peak capacity too, even before they have slots.
        let (merge_restore, merge_pressure) =
            select_gpu_merge_restore(&base, &self.gpu_merge_parents, capacity);
        let decision = if self.gpu_merge_parents.is_empty() {
            select_gpu_frontier(&base, &frontiers, self.gpu_split_reservation, capacity)
        } else {
            GpuFrontierAdmission {
                reserved_parent: None,
                constrained: true,
                pressure: merge_pressure,
                minimum_peak_slots: frontiers
                    .iter()
                    .map(|group| base.union(&group.children.into_iter().collect()).count())
                    .min(),
            }
        };
        self.gpu_split_reservation = decision.reserved_parent;
        self.gpu_admission_pressure = decision.pressure;
        self.gpu_minimum_peak_split_slots = decision.minimum_peak_slots;
        self.gpu_split_frontier_count = frontiers.len();
        self.gpu_policy_desired = core.desired().to_vec();
        self.gpu_policy_drawable = core.drawable().to_vec();
        self.gpu_policy_base_pins = base.clone();
        self.gpu_policy_initialized = true;
        self.gpu_frontier_pins.clear();
        let allowed = if decision.constrained {
            base.extend(merge_restore);
            if let Some(parent) = decision.reserved_parent {
                self.gpu_frontier_pins.extend(parent.children()?);
                base.extend(self.gpu_frontier_pins.iter().copied());
            }
            Some(base.into_iter().collect::<Vec<_>>())
        } else if capacity < finite_region_tile_capacity(core.config()) {
            // An unconstrained current frontier does not imply that all future
            // descendants fit. Keep proactive CPU results cached; only upload
            // children that can participate in the current drawable publication.
            self.gpu_frontier_pins
                .extend(frontiers.iter().flat_map(|group| group.children));
            base.extend(self.gpu_frontier_pins.iter().copied());
            Some(base.into_iter().collect::<Vec<_>>())
        } else {
            None
        };
        self.core
            .as_mut()
            .context("regional core unavailable")?
            .set_upload_allowlist(allowed.as_deref());
        Ok(())
    }

    pub fn advance(
        &mut self,
        position: DVec3,
        projection_scale_px: f64,
        dt: Duration,
        deterministic: bool,
    ) -> Result<()> {
        let regional_started = std::time::Instant::now();
        if !self.enabled {
            return Ok(());
        }
        self.frame += 1;
        self.gpu_use_clock = self.gpu_use_clock.saturating_add(1);
        if !deterministic {
            if self.frame_intervals_ms.len() == 8192 {
                self.frame_intervals_ms.remove(0);
            }
            self.frame_intervals_ms.push(dt.as_secs_f64() * 1000.0);
        }
        let velocity = self.last_camera.map_or(DVec3::ZERO, |old| {
            (position - old) / dt.as_secs_f64().max(1.0e-6)
        });
        self.last_camera = Some(position);
        let pins: Vec<_> = self.pins().into_iter().collect();
        let core = self.core.as_mut().context("regional core unavailable")?;
        core.set_external_pins(&pins);
        core.tick(
            RegionalView {
                body_position_m: position,
                body_velocity_mps: velocity,
                projection_scale_px,
            },
            dt,
        )?;
        let started = std::time::Instant::now();
        let completed_merges = self.advance_groups(dt)?;
        self.bootstrap()?;
        let settings = self.settings.context("regional settings unavailable")?;
        for _ in completed_merges..settings.publication_groups_per_frame {
            if self.groups.len() >= settings.transition_limit {
                break;
            }
            let candidates = self
                .core
                .as_ref()
                .context("regional core unavailable")?
                .publication_candidates();
            let mut admitted = false;
            for candidate in candidates {
                if self.publish(candidate)? {
                    admitted = true;
                    break;
                }
            }
            if !admitted {
                break;
            }
        }
        self.update_gpu_admission()?;
        self.cpu_publication_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.regional_advance_ms = regional_started.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    pub fn record_native_frame(
        &mut self,
        performance: &crate::developer_snapshot::PerformanceSnapshot,
    ) {
        if self.native_frame_samples.len() == 8192 {
            self.native_frame_samples.pop_front();
        }
        self.native_frame_samples.push_back(NativeFrameSample {
            frame: self.frame,
            interval_ms: self.frame_intervals_ms.last().copied().unwrap_or(0.0),
            regional_advance_ms: self.regional_advance_ms,
            publication_ms: self.cpu_publication_ms,
            preparation_ms: performance.preparation_ms,
            gpu_terrain_ms: performance.gpu_terrain_ms,
            gpu_source_frame: performance.gpu_source_frame,
            desired: self.core.as_ref().map_or(0, |core| core.desired().len()),
            drawable: self.core.as_ref().map_or(0, |core| core.drawable().len()),
            transitions: self.groups.len(),
            tile_upload_bytes: self.last_report.tile_upload_bytes,
            boundary_upload_bytes: self.last_report.boundary_upload_bytes,
        });
    }

    fn bootstrap(&mut self) -> Result<()> {
        if !self.patches.is_empty() {
            return Ok(());
        }
        let core = self.core.as_ref().context("regional core unavailable")?;
        let cover = if core.drawable().is_empty() {
            core.config().roots.clone()
        } else {
            core.drawable().to_vec()
        };
        if cover.is_empty() {
            return Ok(());
        }
        let tiles = cover
            .iter()
            .map(|a| {
                core.tile(*a)
                    .map(|t| (*a, t))
                    .context("root payload missing")
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        self.boundaries = build_boundaries(&tiles)?;
        for address in cover {
            let boundary = self
                .boundaries
                .get(&address)
                .context("root boundary missing")?
                .clone();
            self.next_group += 1;
            self.patches.insert(
                address,
                Patch {
                    parent: address,
                    quadrant: None,
                    endpoints: Arc::new(RegionalBoundaryEndpoints {
                        version: self.next_group,
                        own_coarse: boundary.clone(),
                        own_fine: boundary.clone(),
                        parent: boundary,
                    }),
                    group: None,
                    merging: false,
                },
            );
        }
        Ok(())
    }

    fn advance_groups(&mut self, dt: Duration) -> Result<usize> {
        let duration = self.settings.map_or(150, |s| s.morph_duration_ms);
        let step = if duration == 0 {
            1.0
        } else {
            dt.as_secs_f32() / (duration as f32 * 0.001)
        };
        // Every newly admitted group is drawn once at its exact outgoing endpoint.
        for group in &mut self.groups {
            group.fraction = (group.fraction + step).min(1.0);
        }
        let completed: Vec<_> = self
            .groups
            .iter()
            .filter(|g| g.fraction == 1.0)
            .map(|g| g.id)
            .collect();
        let mut merged = 0;
        for id in completed {
            let index = self
                .groups
                .iter()
                .position(|g| g.id == id)
                .context("completed group missing")?;
            if self.groups[index].merge.is_some()
                && merged >= self.settings.map_or(1, |s| s.publication_groups_per_frame)
            {
                continue;
            }
            let group = self.groups.remove(index);
            if let Some((parent, children)) = group.merge {
                for child in children {
                    self.patches.remove(&child);
                    self.boundaries.remove(&child);
                }
                let boundary = self
                    .boundaries
                    .get(&parent)
                    .context("merge parent boundary missing")?
                    .clone();
                self.patches.insert(
                    parent,
                    Patch {
                        parent,
                        quadrant: None,
                        endpoints: Arc::new(RegionalBoundaryEndpoints {
                            version: id,
                            own_coarse: boundary.clone(),
                            own_fine: boundary.clone(),
                            parent: boundary,
                        }),
                        group: None,
                        merging: false,
                    },
                );
                let cover: Vec<_> = self.patches.keys().copied().collect();
                self.core
                    .as_mut()
                    .context("regional core missing")?
                    .ack_drawable(&cover)?;
                merged += 1;
            }
            for address in group.affected {
                if let Some(patch) = self.patches.get_mut(&address) {
                    patch.group = None;
                    patch.merging = false;
                    // Stable interiors no longer depend on the parent; boundary
                    // endpoints remain resident and only their fraction is one.
                    patch.parent = address;
                    patch.quadrant = None;
                }
            }
            self.record(
                serde_json::json!({"event":"transition_completed","group":id,"frame":self.frame}),
            );
        }
        Ok(merged)
    }

    fn publish(&mut self, candidate: RegionalPublication) -> Result<bool> {
        let (parent, children, cover, merging) = match candidate {
            RegionalPublication::Split {
                parent,
                children,
                cover,
            } => (parent, children, cover, false),
            RegionalPublication::Merge {
                parent,
                children,
                cover,
            } => (parent, children, cover, true),
        };
        let replaced: BTreeSet<_> = if merging {
            children.into_iter().collect()
        } else {
            BTreeSet::from([parent])
        };
        if replaced
            .iter()
            .any(|a| self.patches.get(a).is_some_and(|p| p.group.is_some()))
        {
            return Ok(false);
        }
        let core = self.core.as_ref().context("regional core missing")?;
        if std::iter::once(parent)
            .chain(children)
            .any(|a| self.slot_for(a).is_none())
        {
            return Ok(false);
        }
        let tiles = cover
            .iter()
            .map(|a| {
                core.tile(*a)
                    .map(|t| (*a, t))
                    .context("publication tile missing")
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let target = build_boundaries(&tiles)?;
        let mut affected = BTreeSet::new();
        for address in &cover {
            if self.boundaries.get(address) != target.get(address) {
                affected.insert(*address);
            }
        }
        affected.extend(replaced.iter().copied());
        if affected
            .iter()
            .any(|a| self.patches.get(a).is_some_and(|p| p.group.is_some()))
        {
            self.topology_deferred += 1;
            return Ok(false);
        }
        self.next_group += 1;
        let id = self.next_group;
        let parent_tile = core.tile(parent).context("publication parent missing")?;
        let parent_boundary = if merging {
            target.get(&parent)
        } else {
            self.boundaries.get(&parent)
        }
        .context("publication parent boundary missing")?
        .clone();
        for child in children {
            let child_tile = core.tile(child).context("publication child missing")?;
            let coarse = subdivided_parent_boundary(&parent_tile, &parent_boundary, &child_tile)?;
            let (start, end) = if merging {
                (
                    self.boundaries
                        .get(&child)
                        .context("merge child boundary missing")?
                        .clone(),
                    coarse,
                )
            } else {
                (
                    coarse,
                    target
                        .get(&child)
                        .context("split child boundary missing")?
                        .clone(),
                )
            };
            let [x, y] = child.coordinates();
            self.patches.insert(
                child,
                Patch {
                    parent,
                    quadrant: Some([x % 2, y % 2]),
                    endpoints: Arc::new(RegionalBoundaryEndpoints {
                        version: id,
                        own_coarse: start,
                        own_fine: end,
                        parent: parent_boundary.clone(),
                    }),
                    group: Some(id),
                    merging,
                },
            );
            affected.insert(child);
        }
        for address in &cover {
            if *address == parent || children.contains(address) || !affected.contains(address) {
                continue;
            }
            let old = self
                .boundaries
                .get(address)
                .context("neighbor outgoing boundary missing")?
                .clone();
            let fine = target
                .get(address)
                .context("neighbor target boundary missing")?
                .clone();
            let patch = self
                .patches
                .get_mut(address)
                .context("neighbor patch missing")?;
            patch.parent = *address;
            patch.quadrant = None;
            patch.group = Some(id);
            patch.merging = false;
            patch.endpoints = Arc::new(RegionalBoundaryEndpoints {
                version: id,
                own_coarse: old.clone(),
                own_fine: fine,
                parent: old,
            });
        }
        if !merging {
            self.patches.remove(&parent);
            self.core
                .as_mut()
                .context("regional core missing")?
                .ack_drawable(&cover)?;
        }
        // Target edges are immutable; active groups own their old endpoints too.
        for (a, b) in target {
            self.boundaries.insert(a, b);
        }
        if !merging {
            self.boundaries.remove(&parent);
        }
        self.groups.push(Group {
            id,
            fraction: 0.0,
            affected,
            merge: merging.then_some((parent, children)),
        });
        self.record(serde_json::json!({"event":if merging{"merge"}else{"split"},"group":id,"parent":format!("{parent:?}"),"frame":self.frame}));
        Ok(true)
    }

    fn tile_draw(&self, address: CubePatchAddress, root_draw: &TileDraw) -> Result<TileDraw> {
        let slot = self
            .slot_for(address)
            .context("regional drawable tile has no slot")?;
        let resident = self.slots[slot].as_ref().context("regional slot missing")?;
        let delta =
            resident.tile.anchor_position_body()? - root_draw.tile.anchor_position_body()?;
        Ok(TileDraw {
            tile: Arc::clone(&resident.tile),
            publication: resident.token.clone(),
            anchor_view_m: root_draw.anchor_view_m + root_draw.body_to_view * delta,
            body_to_view: root_draw.body_to_view,
            mode: root_draw.mode,
            sun_body: root_draw.sun_body,
        })
    }

    pub fn draw(&mut self, root_draw: &TileDraw) -> Result<RegionalResidentDraw> {
        let settings = self.settings.context("regional settings missing")?;
        let mut pins = self.pins();
        self.slot_pressure = self.gpu_admission_pressure;
        let pending: Vec<_> = self
            .core
            .as_ref()
            .context("regional core missing")?
            .queued_uploads()
            .iter()
            .map(|u| Arc::clone(&u.tile))
            .collect();
        let mut uploads = Vec::new();
        for tile in pending {
            if let Some(index) = self
                .slots
                .iter()
                .take(settings.gpu_slots)
                .position(|s| s.as_ref().is_some_and(|s| s.tile.key == tile.key))
            {
                if self
                    .last_report
                    .slots
                    .get(index)
                    .and_then(|s| s.key.as_ref())
                    == Some(&tile.key)
                {
                    self.core
                        .as_mut()
                        .context("regional core missing")?
                        .ack_resident(tile.key.address);
                    self.cache_hits_gpu += 1;
                    continue;
                }
                uploads.push(RegionalTileUpload {
                    slot: index,
                    tile: self.tile_draw(tile.key.address, root_draw)?,
                });
                pins.insert(tile.key.address);
                continue;
            }
            let free = self
                .slots
                .iter()
                .take(settings.gpu_slots)
                .enumerate()
                .position(|(i, s)| {
                    s.is_none()
                        && self
                            .last_report
                            .slots
                            .get(i)
                            .is_none_or(|r| r.key.is_none() || r.reuse_safe)
                })
                .or_else(|| {
                    self.slots
                        .iter()
                        .take(settings.gpu_slots)
                        .enumerate()
                        .filter(|(i, s)| {
                            s.as_ref().is_some_and(|s| {
                                !pins.contains(&s.tile.key.address)
                                    || self.core.as_ref().is_none_or(|core| {
                                        core.tile(s.tile.key.address)
                                            .is_none_or(|tile| tile.key != s.tile.key)
                                    })
                            }) && !uploads.iter().any(|upload| upload.slot == *i)
                                && self.last_report.slots.get(*i).is_some_and(|r| r.reuse_safe)
                        })
                        .min_by_key(|(_, s)| s.as_ref().map_or(0, |s| s.last_use))
                        .map(|(i, _)| i)
                });
            let Some(index) = free else {
                self.slot_pressure = true;
                break;
            };
            let mut state = self.slots[index]
                .as_ref()
                .map_or_else(TileSlotState::default, |s| s.state.clone());
            let token = state.request(&tile.key)?;
            if let Some(old) = self.slots[index].as_ref() {
                let core = self.core.as_mut().context("regional core missing")?;
                if core
                    .tile(old.tile.key.address)
                    .is_some_and(|tile| tile.key == old.tile.key)
                {
                    core.mark_not_resident(old.tile.key.address);
                }
                self.evictions_gpu += 1;
            }
            if !self.uploaded_keys.insert(tile.key.address) {
                self.reuploads_gpu += 1;
            }
            self.slots[index] = Some(Slot {
                state,
                tile: Arc::clone(&tile),
                token,
                last_use: self.gpu_use_clock,
            });
            uploads.push(RegionalTileUpload {
                slot: index,
                tile: self.tile_draw(tile.key.address, root_draw)?,
            });
            pins.insert(tile.key.address);
        }
        let mut patches = Vec::new();
        for (address, patch) in &self.patches {
            let own_slot = self.slot_for(*address).context("draw slot missing")?;
            let parent_slot = self
                .slot_for(patch.parent)
                .context("parent reconstruction slot missing")?;
            let t = patch
                .group
                .and_then(|id| self.groups.iter().find(|g| g.id == id).map(|g| g.fraction))
                .unwrap_or(1.0);
            patches.push(RegionalPatchDraw {
                quality_fallback: self.core.as_ref().is_some_and(|core| {
                    core.desired()
                        .iter()
                        .any(|desired| address.contains(*desired) && desired != address)
                }),
                own_slot,
                parent_slot,
                own: self.tile_draw(*address, root_draw)?,
                parent: self.tile_draw(patch.parent, root_draw)?,
                morph_fraction: if patch.quadrant.is_some() {
                    if patch.merging { 1.0 - t } else { t }
                } else {
                    1.0
                },
                boundary_fraction: t,
                quadrant: patch.quadrant,
                boundary_endpoints: Arc::clone(&patch.endpoints),
            });
        }
        for a in pins {
            if let Some(i) = self.slot_for(a)
                && let Some(slot) = &mut self.slots[i]
            {
                slot.last_use = self.gpu_use_clock;
            }
        }
        Ok(RegionalResidentDraw {
            capacity: settings.gpu_slots,
            cells: root_draw.tile.key.cells,
            uploads,
            patches,
        })
    }

    pub fn snapshot(&self) -> Option<serde_json::Value> {
        if !self.enabled {
            return None;
        }
        let core = self.core.as_ref()?;
        let mut snapshot = serde_json::to_value(core.snapshot()).ok()?;
        let object = snapshot.as_object_mut()?;
        object.insert(
            "active_transitions".into(),
            serde_json::json!(self.groups.len()),
        );
        object.insert(
            "quality_pending".into(),
            serde_json::json!(
                !self.groups.is_empty()
                    || core.desired() != core.drawable()
                    || self.last_report.fallback_active
            ),
        );
        object.insert(
            "gpu_retained_cover_fallback".into(),
            serde_json::json!(self.last_report.fallback_active),
        );
        object.insert(
            "gpu_active_morph_patches".into(),
            serde_json::json!(self.last_report.active_morph_count),
        );
        object.insert(
            "slot_pressure".into(),
            serde_json::json!(self.slot_pressure),
        );
        object.insert(
            "gpu_frontier_reserved_parent".into(),
            serde_json::json!(
                self.gpu_split_reservation
                    .map(|parent| format!("{parent:?}"))
            ),
        );
        object.insert(
            "gpu_admission_pressure".into(),
            serde_json::json!(self.gpu_admission_pressure),
        );
        object.insert(
            "gpu_minimum_peak_split_slots".into(),
            serde_json::json!(self.gpu_minimum_peak_split_slots),
        );
        object.insert(
            "gpu_base_dependency_count".into(),
            serde_json::json!(self.gpu_policy_base_pins.len()),
        );
        object.insert(
            "gpu_balanced_split_frontier_count".into(),
            serde_json::json!(self.gpu_split_frontier_count),
        );
        object.insert(
            "topology_deferred".into(),
            serde_json::json!(self.topology_deferred),
        );
        object.insert(
            "publication_ms".into(),
            serde_json::json!(self.cpu_publication_ms),
        );
        object.insert(
            "frame_intervals_ms".into(),
            serde_json::json!(self.frame_intervals_ms),
        );
        object.insert(
            "frame_interval_scope".into(),
            serde_json::json!("native_wall_elapsed; offscreen excluded"),
        );
        object.insert(
            "native_frame_samples".into(),
            serde_json::json!(self.native_frame_samples),
        );
        object.insert("native_frame_sample_scope".into(), serde_json::json!("bounded last 8192 ordinary native frames per configuration; CPU elapsed stages, GPU latest_completed with source frame; capture-free scenario required for distributions"));
        object.insert("events".into(), serde_json::json!(self.events));
        object.insert(
            "gpu_cache_hits".into(),
            serde_json::json!(self.cache_hits_gpu),
        );
        object.insert(
            "gpu_evictions".into(),
            serde_json::json!(self.evictions_gpu),
        );
        object.insert(
            "gpu_reuploads".into(),
            serde_json::json!(self.reuploads_gpu),
        );
        object.insert("gpu_slots".into(),serde_json::json!(self.last_report.slots.iter().enumerate().map(|(i,s)|serde_json::json!({"slot":i,"key":s.key.as_ref().map(|k|format!("{k:?}")),"address":s.key.as_ref().map(|k|format!("{:?}",k.address)),"generation":s.generation,"reuse_safe":s.reuse_safe,"pinned":s.pinned,"in_flight":s.in_flight})).collect::<Vec<_>>()));
        object.insert("terrain_attributable_waits".into(), serde_json::json!(0));
        object.insert("wait_counter_scope".into(), serde_json::json!("explicit regional worker joins and GPU waits; excludes driver/present and offscreen capture readback"));
        object.insert("ready".into(), serde_json::json!(!self.patches.is_empty()));
        object.insert(
            "gpu_upload_bytes_per_frame".into(),
            serde_json::json!(self.last_report.tile_upload_bytes),
        );
        object.insert(
            "gpu_upload_tiles_per_frame".into(),
            serde_json::json!(self.last_report.tile_upload_count),
        );
        object.insert(
            "boundary_upload_bytes_per_frame".into(),
            serde_json::json!(self.last_report.boundary_upload_bytes),
        );
        object.insert(
            "metadata_upload_bytes_per_frame".into(),
            serde_json::json!(self.last_report.metadata_upload_bytes),
        );
        object.insert(
            "gpu_deferred_uploads".into(),
            serde_json::json!(self.last_report.deferred_upload_count),
        );
        object.insert("resources".into(),serde_json::json!({
            "cpu_cache_payload_bytes":core.snapshot().cpu_cached_bytes,
            "cpu_slot_retained_payload_bytes":self.slots.iter().flatten().map(|s|s.tile.retained_payload_bytes()).sum::<usize>(),
            "cpu_payload_accounting":"cache and slot references overlap; not process RSS",
            "cpu_boundary_endpoint_bytes":self.patches.values().map(|p|p.endpoints.own_coarse.edges.iter().chain(&p.endpoints.own_fine.edges).chain(&p.endpoints.parent.edges).map(|e|e.capacity()*std::mem::size_of::<mundaris_renderer::regional_edges::BoundaryVertex>()).sum::<usize>()).sum::<usize>(),
            "worker_stack_virtual_reservation_bytes":self.settings.map_or(0,|s|s.worker_count*4*1024*1024),
            "gpu_tile_capacity_bytes":self.last_report.tile_capacity_bytes,
            "gpu_allocated_slots":self.last_report.allocated_slot_count,
            "gpu_active_pool_tile_capacity_bytes":self.last_report.active_tile_capacity_bytes,
            "gpu_active_pool_boundary_capacity_bytes":self.last_report.active_boundary_capacity_bytes,
            "gpu_resident_payload_bytes":self.last_report.resident_count as u64*self.last_report.active_tile_capacity_bytes/self.last_report.capacity.max(1) as u64,
            "gpu_pinned_payload_bytes":self.last_report.pinned_count as u64*self.last_report.active_tile_capacity_bytes/self.last_report.capacity.max(1) as u64,
            "gpu_in_flight_payload_bytes":self.last_report.in_flight_count as u64*self.last_report.active_tile_capacity_bytes/self.last_report.capacity.max(1) as u64,
            "gpu_evictable_payload_bytes":self.last_report.evictable_count as u64*self.last_report.active_tile_capacity_bytes/self.last_report.capacity.max(1) as u64,
            "gpu_boundary_capacity_bytes":self.last_report.boundary_capacity_bytes,
            "gpu_metadata_capacity_bytes":self.last_report.metadata_capacity_bytes,
            "gpu_grid_capacity_bytes":self.last_report.grid_capacity_bytes,
            "gpu_validation_capacity_bytes":self.last_report.validation_capacity_bytes,
            "transfer_logical_staging_bytes":self.last_report.transfer_staging_bytes,
            "gpu_accounting":"requested wgpu buffers; not physical VRAM or driver staging",
        }));
        Some(snapshot)
    }
}
