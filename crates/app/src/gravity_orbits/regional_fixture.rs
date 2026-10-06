//! Opt-in regional residency and local publication over the complete world field.
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc::{self, Receiver, SyncSender, TryRecvError, TrySendError},
    },
    thread,
    time::Duration,
};

use crate::{
    developer_snapshot::ResidentDiagnosticSnapshot,
    engine_profile,
    regional_terrain::*,
    resident_terrain::{ResidentTileBuilder, TileBuildIdentity},
    terrain_trace::{BlockReason, Stage, TerrainTrace},
};
use anyhow::{Context, Result, ensure};
use glam::{DMat3, DVec3};
use mundaris_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, PatchEdge, SurfaceLocation},
};
#[cfg(test)]
use mundaris_renderer::regional_edges::build_boundaries;
use mundaris_renderer::{
    CelestialProjection,
    regional_edges::{TileBoundary, subdivided_parent_boundary},
    *,
};
use mundaris_world::terrain::SurfaceGenerator;

const PLANETARY_GPU_SLOTS: usize = 32768;
const PLANETARY_CPU_TILES: usize = 49152;
const PLANETARY_MAX_DESIRED_PATCHES: usize = 16384;

#[derive(Clone, Copy)]
#[cfg_attr(not(any(feature = "developer-tools", test)), allow(dead_code))]
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

    #[test]
    fn planetary_frame_diagnostics_export_bounded_history_and_explicit_detail_scope() {
        let (generator, identity, tile) = setup(1);
        let mut fixture = RegionalFixture::default();
        fixture
            .configure(settings(8), Some(tile), Some(generator), Some(identity))
            .unwrap();
        for id in 0..500 {
            fixture.record(serde_json::json!({"id": id}));
        }
        let detailed = fixture.snapshot().unwrap();
        assert!(detailed["events"].as_array().unwrap().len() >= 500);
        assert!(detailed.get("desired").is_some());
        fixture.planetary = true;
        let compact = fixture.snapshot().unwrap();
        assert_eq!(compact["events"].as_array().unwrap().len(), 256);
        assert_eq!(compact["events"][0]["id"], 244);
        assert_eq!(compact["events"][255]["id"], 499);
        assert!(compact["retained_event_count"].as_u64().unwrap() >= 500);
        assert!(
            compact["detail_scope"]
                .as_str()
                .unwrap()
                .contains("omitted")
        );
        for field in ["desired", "resident", "drawable"] {
            assert!(compact.get(field).is_none());
        }
        for field in [
            "desired_count",
            "resident_count",
            "drawable_count",
            "target_quality_reached",
            "refinement_debt",
        ] {
            assert_eq!(compact[field], detailed[field]);
        }

        for frame in 0..300 {
            fixture.frame_intervals_ms.push_back(frame as f64);
            fixture.record_native_frame(
                &crate::developer_snapshot::PerformanceSnapshot::default(),
                1_000 + frame,
            );
        }
        let summary = fixture.snapshot_frame().unwrap();
        assert!(summary.get("native_frame_samples").is_none());
        assert!(summary.get("frame_intervals_ms").is_none());
        assert!(summary.get("events").is_none());
        assert_eq!(summary["latest_native_frame"]["frame"], 1_299);

        let exported = serde_json::to_value(&summary).unwrap();
        assert_eq!(exported, fixture.snapshot().unwrap());
        assert_eq!(
            exported["native_frame_samples"].as_array().unwrap().len(),
            256
        );
        assert_eq!(exported["native_frame_samples"][0]["frame"], 1_044);
        assert_eq!(exported["native_frame_samples"][255]["frame"], 1_299);
        assert_eq!(exported["frame_intervals_ms"][0], 44.0);
        assert_eq!(exported["frame_intervals_ms"][255], 299.0);
        let decoded: ResidentDiagnosticSnapshot = serde_json::from_value(exported).unwrap();
        assert_eq!(decoded, summary);
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
    fn bounded_slot_scan_defers_unsafe_entries_and_advances_to_available_capacity() {
        let (_, _, tile) = setup(1);
        let mut fixture = RegionalFixture::default();
        fixture.slots.resize_with(512, || None);
        fixture
            .last_report
            .slots
            .resize(512, RegionalSlotReport::default());
        for report in &mut fixture.last_report.slots[..300] {
            report.key = Some(tile.key.clone());
            report.reuse_safe = false;
        }
        assert_eq!(
            fixture.scan_available_slot(&BTreeSet::new(), &[], 512),
            None
        );
        assert_eq!(fixture.slot_scan_cursor, 256);
        assert_eq!(
            fixture.scan_available_slot(&BTreeSet::new(), &[], 512),
            Some(300)
        );
        assert_eq!(fixture.slot_scan_cursor, 301);
        assert!(fixture.slots.iter().all(Option::is_none));
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
        fixture.rebuild_slot_index();
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
    fn base_pin_reuse_invalidates_on_topology_and_split_completion() {
        let parent = CubePatchAddress::root(CubeFace::PositiveZ);
        let child = parent.children().unwrap()[0];
        let boundary = TileBoundary {
            edges: std::array::from_fn(|_| Vec::new()),
        };
        let mut fixture = RegionalFixture::default();
        fixture.patches.insert(
            child,
            Patch {
                parent,
                quadrant: Some([0, 0]),
                endpoints: Arc::new(RegionalBoundaryEndpoints {
                    version: 1,
                    own_coarse: boundary.clone(),
                    own_fine: boundary.clone(),
                    parent: boundary,
                }),
                group: Some(1),
                merging: false,
            },
        );
        fixture.groups.push(Group {
            id: 1,
            fraction: 0.0,
            affected: BTreeSet::from([child]),
            merge: None,
            completion_parent: None,
        });
        let first = fixture.shared_base_pins();
        assert_eq!(*first, BTreeSet::from([parent, child]));
        assert!(Arc::ptr_eq(&first, &fixture.shared_base_pins()));
        // Split completion changes reconstruction dependencies without changing
        // the cover revision. The group signature must invalidate that case.
        fixture.advance_groups(Duration::from_secs(1)).unwrap();
        assert_eq!(fixture.topology_revision, 0);
        let completed = fixture.shared_base_pins();
        assert_eq!(*completed, BTreeSet::from([child]));
        assert!(!Arc::ptr_eq(&first, &completed));
        fixture.patches.clear();
        fixture.topology_revision += 1;
        assert!(fixture.shared_base_pins().is_empty());
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
    fn gpu_frontier_bounded_selection_matches_union_and_stable_sort_oracle() {
        fn legacy(
            base: &BTreeSet<CubePatchAddress>,
            frontiers: &[RegionalSplitFrontier],
            previous: Option<CubePatchAddress>,
            capacity: usize,
        ) -> GpuFrontierAdmission {
            let peak = |group: &RegionalSplitFrontier| {
                base.len()
                    + group
                        .children
                        .iter()
                        .filter(|child| !base.contains(child))
                        .count()
            };
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

        fn assert_matches(
            base: &BTreeSet<CubePatchAddress>,
            frontiers: &[RegionalSplitFrontier],
            previous: Option<CubePatchAddress>,
            capacity: usize,
        ) {
            let expected = legacy(base, frontiers, previous, capacity);
            let actual = select_gpu_frontier(base, frontiers, previous, capacity);
            assert_eq!(actual.reserved_parent, expected.reserved_parent);
            assert_eq!(actual.constrained, expected.constrained);
            assert_eq!(actual.pressure, expected.pressure);
            assert_eq!(actual.minimum_peak_slots, expected.minimum_peak_slots);
        }

        let addresses: Vec<_> = (0..=2)
            .flat_map(|level| {
                let edge = 1_u32 << level;
                (0..edge).flat_map(move |y| {
                    (0..edge).map(move |x| {
                        CubePatchAddress::try_new(CubeFace::PositiveZ, level, x, y).unwrap()
                    })
                })
            })
            .collect();
        let base = BTreeSet::from([addresses[0], addresses[1]]);

        // The child arrays intentionally overlap and repeat entries. The old
        // union counts each address once globally, while each group's peak
        // counts every non-base child occurrence. Keep both semantics exact.
        let frontiers = [
            RegionalSplitFrontier {
                parent: addresses[2],
                children: [addresses[4], addresses[4], addresses[5], addresses[5]],
                aggregate_priority: f64::NAN,
                ready_children: 3,
                parent_resident: false,
            },
            RegionalSplitFrontier {
                parent: addresses[3],
                children: [addresses[4], addresses[6], addresses[6], addresses[0]],
                aggregate_priority: 10.0,
                ready_children: 3,
                parent_resident: true,
            },
            RegionalSplitFrontier {
                parent: addresses[2],
                children: [addresses[7], addresses[8], addresses[9], addresses[10]],
                aggregate_priority: 10.0,
                ready_children: 3,
                parent_resident: true,
            },
            RegionalSplitFrontier {
                parent: addresses[11],
                children: [addresses[12], addresses[13], addresses[14], addresses[15]],
                aggregate_priority: -0.0,
                ready_children: 4,
                parent_resident: false,
            },
        ];

        for capacity in [0, 2, 3, 4, 5, 6, 8, 10, 16] {
            assert_matches(&base, &frontiers, None, capacity);
            assert_matches(&base, &frontiers, Some(addresses[2]), capacity);
            assert_matches(&base, &frontiers, Some(addresses[3]), capacity);
        }

        // Differentially cover empty, unconstrained, exact-capacity, and
        // constrained cases with repeat/overlap patterns and varied ranks.
        for case in 0..96 {
            let base = BTreeSet::from([addresses[case % addresses.len()]]);
            let groups: Vec<_> = (0..(case % 7))
                .map(|index| {
                    let start = (case * 3 + index * 5) % addresses.len();
                    let c0 = addresses[start];
                    let c1 = addresses[(start + index + 1) % addresses.len()];
                    RegionalSplitFrontier {
                        parent: addresses[(start + 2) % addresses.len()],
                        children: [c0, c1, c0, addresses[(start + 3) % addresses.len()]],
                        aggregate_priority: match (case + index) % 9 {
                            0 => f64::NAN,
                            1 => -0.0,
                            _ => ((case * 17 + index * 11) % 23) as f64,
                        },
                        ready_children: (case + index * 3) % 5,
                        parent_resident: index % 2 == 0,
                    }
                })
                .collect();
            let previous = (case % 3 == 0).then(|| addresses[(case + 4) % addresses.len()]);
            for capacity in [0, 1, 2, 4, 7, 13] {
                assert_matches(&base, &groups, previous, capacity);
            }
        }
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

    #[test]
    fn planetary_configuration_queues_six_roots_without_synchronous_tiles() {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(23),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
        let identity = TileBuildIdentity {
            body_identity: 23,
            surface_revision: 1,
            material_revision: 1,
        };
        let mut fixture = RegionalFixture::default();
        assert!(
            fixture
                .configure_planetary(generator, identity, 32)
                .unwrap()
        );
        let core = fixture.core.as_ref().unwrap();
        assert_eq!(core.config().roots.len(), 6);
        assert_eq!(core.config().max_level, 24);
        assert_eq!(
            core.config().max_desired_patches,
            PLANETARY_MAX_DESIRED_PATCHES
        );
        assert_eq!(core.config().cpu_tile_cap, PLANETARY_CPU_TILES);
        assert_eq!(
            core.config().cpu_byte_cap,
            PLANETARY_CPU_TILES * 35 * 35 * 32
        );
        assert_eq!(
            fixture.settings.as_ref().unwrap().gpu_slots,
            PLANETARY_GPU_SLOTS
        );
        assert_eq!(core.config().worker_count, 4);
        assert_eq!(core.config().upload_tile_cap, 4);
        assert_eq!(core.config().upload_byte_cap, 8 * 1024 * 1024);
        assert!(fixture.template_tile().is_none());
        assert!(!fixture.has_coverage());
    }

    #[test]
    fn planetary_bootstrap_publishes_only_after_six_async_root_uploads() {
        use mundaris_renderer::{RegionalResidentReport, RegionalSlotReport, TileDraw};

        let definition = SurfaceDefinition::generated(
            TerrainIdentity(23),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
        let identity = TileBuildIdentity {
            body_identity: 23,
            surface_revision: 1,
            material_revision: 1,
        };
        let mut fixture = RegionalFixture::default();
        fixture
            .configure_planetary(generator.clone(), identity, 2)
            .unwrap();
        for face in CubeFace::ALL {
            let address = CubePatchAddress::root(face);
            let (tile, _) = ResidentTileBuilder::build(&generator, identity, address, 2).unwrap();
            fixture
                .core
                .as_mut()
                .unwrap()
                .seed_tile(Arc::new(tile))
                .unwrap();
        }
        let mut report = RegionalResidentReport {
            capacity: 1024,
            ..RegionalResidentReport::default()
        };
        report.slots.resize(1024, RegionalSlotReport::default());
        let mut saw_upload_only = false;
        for _ in 0..256 {
            fixture
                .advance(DVec3::Z * 100_000.0, 0.0, Duration::from_millis(16), true)
                .unwrap();
            if let Some(tile) = fixture.template_tile() {
                let mut state = TileSlotState::default();
                let publication = state.request(&tile.key).unwrap();
                let root_draw = TileDraw {
                    tile,
                    publication,
                    anchor_view_m: DVec3::ZERO,
                    body_to_view: DMat3::IDENTITY,
                    mode: 0,
                    sun_body: DVec3::Z,
                };
                let draw = fixture.draw(&root_draw).unwrap();
                saw_upload_only |= draw.patches.is_empty() && !draw.uploads.is_empty();
                for upload in draw.uploads {
                    report.slots[upload.slot] = RegionalSlotReport {
                        key: Some(upload.tile.tile.key.clone()),
                        reuse_safe: true,
                        ..RegionalSlotReport::default()
                    };
                }
                report.resident_count = report
                    .slots
                    .iter()
                    .filter(|slot| slot.key.is_some())
                    .count();
                fixture.observe(report.clone());
            }
            if fixture.has_coverage() {
                break;
            }
            std::thread::yield_now();
        }
        assert!(saw_upload_only, "snapshot: {:?}", fixture.snapshot());
        assert!(fixture.has_coverage());
        assert_eq!(fixture.patches.len(), 6);
    }

    #[test]
    fn independent_batch_matches_serial_products_and_logical_merge_overlay() {
        let (generator, identity, first) = setup(1);
        let parents = [
            first.key.address,
            CubePatchAddress::try_new(CubeFace::NegativeZ, 9, 350, 370).unwrap(),
        ];
        let mut retained = BTreeMap::new();
        let mut transitions = Vec::new();
        for (index, parent) in parents.into_iter().enumerate() {
            let parent_tile = Arc::new(
                ResidentTileBuilder::build(&generator, identity, parent, 2)
                    .unwrap()
                    .0,
            );
            retained.insert(parent, Arc::clone(&parent_tile));
            let children = parent.children().unwrap();
            let child_tiles = children
                .into_iter()
                .map(|child| {
                    (
                        child,
                        Arc::new(
                            ResidentTileBuilder::build(&generator, identity, child, 2)
                                .unwrap()
                                .0,
                        ),
                    )
                })
                .collect::<BTreeMap<_, _>>();
            transitions.push(TransitionPublicationWork {
                candidate: RegionalPublication::Split {
                    parent,
                    children,
                    cover: Vec::new(),
                },
                tiles: BTreeMap::new(),
                parent_tile,
                child_tiles,
                outgoing_boundaries: Arc::new(BTreeMap::new()),
                group_id: index as u64 + 1,
            });
        }
        let base = Arc::new(build_boundaries(&retained).unwrap());
        let mut batch = BatchPublicationWork {
            base: Arc::clone(&base),
            overlays: BTreeMap::new(),
            transitions,
            keep_alive: Vec::new(),
        };
        let mut cache = mundaris_renderer::regional_edges::RegionalBoundaryCache::default();
        let PreparedPublication::Batch {
            base: outgoing,
            transitions: products,
        } = prepare_publication_batch(&mut batch, &mut retained, &mut cache).unwrap()
        else {
            panic!("batch expected")
        };
        assert_eq!(outgoing, base);
        assert_eq!(products.len(), 2);
        assert!(products[0].1.affected.is_disjoint(&products[1].1.affected));
        for (candidate, product) in &products {
            assert!(candidate.cover().is_empty());
            assert!(product.cover.is_empty());
            let parent = publication_parent(candidate);
            let work = batch
                .transitions
                .iter()
                .find(|work| work.parent_tile.key.address == parent)
                .unwrap();
            let mut serial = TransitionPublicationWork {
                candidate: work.candidate.clone(),
                tiles: BTreeMap::new(),
                parent_tile: Arc::clone(&work.parent_tile),
                child_tiles: work.child_tiles.clone(),
                outgoing_boundaries: Arc::clone(&base),
                group_id: work.group_id,
            };
            let mut serial_cover = base.keys().copied().collect::<BTreeSet<_>>();
            serial_cover.remove(&parent);
            serial_cover.extend(parent.children().unwrap());
            if let RegionalPublication::Split { cover, .. } = &mut serial.candidate {
                *cover = serial_cover.iter().copied().collect();
            }
            serial.tiles = serial_cover
                .iter()
                .map(|address| (*address, Arc::clone(&retained[address])))
                .collect();
            let reference = prepare_transition_publication(&serial).unwrap();
            assert_eq!(product.affected, reference.affected);
            for (address, patch) in &product.staged {
                assert_eq!(patch.endpoints, reference.staged[address].endpoints);
            }
        }
        // A merge's logical target is installed while its drawable children
        // still morph. An independent next batch must use that target cover.
        let split_target = Arc::clone(&products[0].1.target);
        let first_children = parents[0].children().unwrap();
        let second_children = parents[1].children().unwrap();
        let overlays = std::iter::once((parents[0], Some(base[&parents[0]].clone())))
            .chain(first_children.into_iter().map(|child| (child, None)))
            .collect();
        let mut merge_other = BatchPublicationWork {
            base: split_target,
            overlays,
            keep_alive: Vec::new(),
            transitions: vec![TransitionPublicationWork {
                candidate: RegionalPublication::Merge {
                    parent: parents[1],
                    children: second_children,
                    cover: Vec::new(),
                },
                tiles: BTreeMap::new(),
                parent_tile: Arc::clone(&batch.transitions[1].parent_tile),
                child_tiles: batch.transitions[1].child_tiles.clone(),
                outgoing_boundaries: Arc::clone(&base),
                group_id: 3,
            }],
        };
        let PreparedPublication::Batch {
            base: logical,
            transitions: merge,
        } = prepare_publication_batch(&mut merge_other, &mut retained, &mut cache).unwrap()
        else {
            panic!("batch expected")
        };
        assert!(logical.contains_key(&parents[0]));
        assert!(
            first_children
                .iter()
                .all(|child| !logical.contains_key(child))
        );
        assert_eq!(merge[0].1.target, base);
        assert!(
            first_children
                .iter()
                .all(|child| !merge[0].1.affected.contains(child))
        );
    }

    #[test]
    fn prepared_split_and_merge_match_boundary_endpoint_reference() {
        let definition = SurfaceDefinition::generated(
            TerrainIdentity(91),
            TerrainSeed(0),
            SurfaceAlgorithm::RockyV5,
        );
        let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
        let identity = TileBuildIdentity {
            body_identity: 91,
            surface_revision: 1,
            material_revision: 1,
        };
        let roots = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect::<Vec<_>>();
        let mut root_tiles = BTreeMap::new();
        for address in &roots {
            let (tile, _) = ResidentTileBuilder::build(&generator, identity, *address, 2).unwrap();
            root_tiles.insert(*address, Arc::new(tile));
        }
        let parent = CubePatchAddress::root(CubeFace::PositiveZ);
        let children = parent.children().unwrap();
        let parent_tile = Arc::clone(root_tiles.get(&parent).unwrap());
        let mut child_tiles = BTreeMap::new();
        for child in children {
            let (tile, _) = ResidentTileBuilder::build(&generator, identity, child, 2).unwrap();
            child_tiles.insert(child, Arc::new(tile));
        }
        let outgoing = build_boundaries(&root_tiles).unwrap();
        let mut split_tiles = root_tiles.clone();
        split_tiles.remove(&parent);
        split_tiles.extend(child_tiles.clone());
        let split_candidate = RegionalPublication::Split {
            parent,
            children,
            cover: split_tiles.keys().copied().collect(),
        };
        let split_expected = build_boundaries(&split_tiles).unwrap();
        let mut split_affected = split_expected
            .iter()
            .filter(|(address, boundary)| outgoing.get(address) != Some(*boundary))
            .map(|(address, _)| *address)
            .collect::<BTreeSet<_>>();
        split_affected.insert(parent);
        let mut retained_tiles = root_tiles.clone();
        let mut split_work = TransitionPublicationWork {
            candidate: split_candidate,
            tiles: BTreeMap::new(),
            parent_tile: Arc::clone(&parent_tile),
            child_tiles: child_tiles.clone(),
            outgoing_boundaries: Arc::new(outgoing.clone()),
            group_id: 10,
        };
        let mut missing_outgoing = BTreeMap::new();
        assert!(collect_transition_tiles(&mut split_work, &mut missing_outgoing).is_err());
        collect_transition_tiles(&mut split_work, &mut retained_tiles).unwrap();
        assert_eq!(
            split_work.tiles.keys().copied().collect::<Vec<_>>(),
            split_tiles.keys().copied().collect::<Vec<_>>()
        );
        assert_eq!(retained_tiles.len(), split_tiles.len() + 1);
        let split = prepare_transition_publication(&split_work).unwrap();
        // A discarded result leaves the outgoing cover available for a retry.
        split_work.tiles.clear();
        collect_transition_tiles(&mut split_work, &mut retained_tiles).unwrap();
        assert_eq!(
            prepare_transition_publication(&split_work).unwrap().target,
            split.target
        );
        assert_eq!(split.target.as_ref(), &split_expected);
        assert_eq!(split.affected, split_affected);
        assert!(split.completion_parent.is_none());
        for child in children {
            let coarse = subdivided_parent_boundary(
                &parent_tile,
                outgoing.get(&parent).unwrap(),
                child_tiles.get(&child).unwrap(),
            )
            .unwrap();
            let endpoints = &split.staged[&child].endpoints;
            assert_eq!(endpoints.own_coarse, coarse);
            assert_eq!(endpoints.own_fine, split_expected[&child]);
            assert_eq!(endpoints.parent, outgoing[&parent]);
            assert!(split.staged[&child].group.is_some());
        }
        for address in split
            .affected
            .iter()
            .filter(|address| **address != parent && !children.contains(*address))
        {
            let endpoints = &split.staged[address].endpoints;
            assert_eq!(endpoints.own_coarse, outgoing[address]);
            assert_eq!(endpoints.own_fine, split_expected[address]);
            assert_eq!(endpoints.parent, outgoing[address]);
        }

        let merge_candidate = RegionalPublication::Merge {
            parent,
            children,
            cover: roots.clone(),
        };
        let merge_expected = build_boundaries(&root_tiles).unwrap();
        let mut merge_affected = merge_expected
            .iter()
            .filter(|(address, boundary)| split_expected.get(address) != Some(*boundary))
            .map(|(address, _)| *address)
            .collect::<BTreeSet<_>>();
        merge_affected.extend(children);
        let mut merge_work = TransitionPublicationWork {
            candidate: merge_candidate,
            tiles: BTreeMap::new(),
            parent_tile: Arc::clone(&parent_tile),
            child_tiles: child_tiles.clone(),
            outgoing_boundaries: Arc::new(split_expected.clone()),
            group_id: 11,
        };
        collect_transition_tiles(&mut merge_work, &mut retained_tiles).unwrap();
        assert_eq!(retained_tiles.len(), roots.len() + children.len());
        let merge = prepare_transition_publication(&merge_work).unwrap();
        assert_eq!(merge.target.as_ref(), &merge_expected);
        assert_eq!(merge.affected, merge_affected);
        let completion_parent = merge.completion_parent.unwrap();
        assert_eq!(completion_parent.own_coarse, merge_expected[&parent]);
        assert_eq!(completion_parent.own_fine, merge_expected[&parent]);
        assert_eq!(completion_parent.parent, merge_expected[&parent]);
        for child in children {
            let coarse = subdivided_parent_boundary(
                parent_tile.as_ref(),
                merge_expected.get(&parent).unwrap(),
                child_tiles.get(&child).unwrap(),
            )
            .unwrap();
            let endpoints = &merge.staged[&child].endpoints;
            assert_eq!(endpoints.own_coarse, split_expected[&child]);
            assert_eq!(endpoints.own_fine, coarse);
            assert_eq!(endpoints.parent, merge_expected[&parent]);
        }
    }
}

impl RegionalSettings {
    #[cfg_attr(not(any(feature = "developer-tools", test)), allow(dead_code))]
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
    completion_parent: Option<Arc<RegionalBoundaryEndpoints>>,
}

struct BootstrapPublicationWork {
    tiles: BTreeMap<CubePatchAddress, Arc<TileData>>,
}

struct TransitionPublicationWork {
    candidate: RegionalPublication,
    tiles: BTreeMap<CubePatchAddress, Arc<TileData>>,
    parent_tile: Arc<TileData>,
    child_tiles: BTreeMap<CubePatchAddress, Arc<TileData>>,
    outgoing_boundaries: Arc<BTreeMap<CubePatchAddress, TileBoundary>>,
    group_id: u64,
}

struct BatchPublicationWork {
    base: Arc<BTreeMap<CubePatchAddress, TileBoundary>>,
    overlays: BTreeMap<CubePatchAddress, Option<TileBoundary>>,
    transitions: Vec<TransitionPublicationWork>,
    keep_alive: Vec<Arc<BTreeMap<CubePatchAddress, TileBoundary>>>,
}

enum PublicationWork {
    Bootstrap(Box<BootstrapPublicationWork>),
    Transition(Box<TransitionPublicationWork>),
    Batch(Box<BatchPublicationWork>),
}

enum PublicationKind {
    Batch {
        overlays: Vec<CubePatchAddress>,
    },
    Bootstrap {
        cover: Vec<CubePatchAddress>,
    },
    Transition {
        parent: CubePatchAddress,
        children: [CubePatchAddress; 4],
        merging: bool,
    },
}

struct PublicationToken {
    trace: Option<Arc<TerrainTrace>>,
    revision: u64,
    kind: PublicationKind,
    keys: BTreeMap<CubePatchAddress, TileKey>,
    dependencies: Arc<[CubePatchAddress]>,
    retained_tile_bytes: usize,
}

struct PreparedTransition {
    cover: Vec<CubePatchAddress>,
    target: Arc<BTreeMap<CubePatchAddress, TileBoundary>>,
    affected: BTreeSet<CubePatchAddress>,
    staged: BTreeMap<CubePatchAddress, Patch>,
    group_id: u64,
    completion_parent: Option<Arc<RegionalBoundaryEndpoints>>,
}

enum PreparedPublication {
    Bootstrap(Arc<BTreeMap<CubePatchAddress, TileBoundary>>),
    Transition(PreparedTransition),
    Batch {
        base: Arc<BTreeMap<CubePatchAddress, TileBoundary>>,
        transitions: Vec<(RegionalPublication, PreparedTransition)>,
    },
}

struct PendingPublication {
    candidate: RegionalPublication,
    prepared: PreparedTransition,
    expected: Arc<BTreeMap<CubePatchAddress, TileBoundary>>,
    keys: BTreeMap<CubePatchAddress, TileKey>,
    ready_at: std::time::Instant,
}

struct PublicationCompletion {
    token: PublicationToken,
    prepared: Result<PreparedPublication, String>,
    elapsed: Duration,
}

struct PublicationJob {
    work: PublicationWork,
    token: PublicationToken,
}

struct PublicationInFlight {
    dependencies: Arc<[CubePatchAddress]>,
    retained_tile_bytes: usize,
    queued_at: std::time::Instant,
}

struct PublicationWorker {
    sender: SyncSender<PublicationJob>,
    completions: Receiver<PublicationCompletion>,
    in_flight: Option<PublicationInFlight>,
    retained_tile_bytes: Arc<AtomicUsize>,
    boundary_cache_bytes: Arc<AtomicUsize>,
    rebuilt_boundaries: Arc<AtomicUsize>,
    active: Arc<AtomicUsize>,
    completion_ready: Arc<AtomicUsize>,
    snapshot_bytes: Arc<AtomicUsize>,
}

fn collect_transition_tiles(
    work: &mut TransitionPublicationWork,
    retained: &mut BTreeMap<CubePatchAddress, Arc<TileData>>,
) -> std::result::Result<(), mundaris_renderer::resident_tile::TileGeometryError> {
    // The worker owns the unchanged outgoing payload references. Each frame
    // submits only the replacement quartet and parent; assembling a full cover
    // and retiring stale references happen here, away from the frame thread.
    retained.insert(work.parent_tile.key.address, Arc::clone(&work.parent_tile));
    retained.extend(work.child_tiles.clone());
    let tiles = work
        .candidate
        .cover()
        .iter()
        .map(|address| {
            retained
                .get(address)
                .map(|tile| (*address, Arc::clone(tile)))
                .ok_or(mundaris_renderer::resident_tile::TileGeometryError::InvalidTile)
        })
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
    retained.retain(|address, _| {
        work.outgoing_boundaries.contains_key(address)
            || tiles.contains_key(address)
            || *address == work.parent_tile.key.address
            || work.child_tiles.contains_key(address)
    });
    work.tiles = tiles;
    Ok(())
}

#[cfg(test)]
fn prepare_transition_publication(
    work: &TransitionPublicationWork,
) -> std::result::Result<PreparedTransition, mundaris_renderer::resident_tile::TileGeometryError> {
    let target = mundaris_renderer::regional_edges::build_boundaries(&work.tiles)?;
    prepare_transition_with_boundaries(work, target)
}

fn prepare_transition_with_boundaries(
    work: &TransitionPublicationWork,
    target: BTreeMap<CubePatchAddress, TileBoundary>,
) -> std::result::Result<PreparedTransition, mundaris_renderer::resident_tile::TileGeometryError> {
    prepare_transition_with_target(work, Arc::new(target), None)
}

fn publication_zone(parent: CubePatchAddress) -> BTreeSet<CubePatchAddress> {
    let mut zone = BTreeSet::from([parent]);
    let mut frontier = vec![parent];
    for _ in 0..2 {
        let mut next = Vec::new();
        for address in frontier {
            for edge in PatchEdge::ALL {
                let neighbor = address.neighbor(edge).address;
                if zone.insert(neighbor) {
                    next.push(neighbor);
                }
            }
        }
        frontier = next;
    }
    zone
}

fn zone_contains(zone: &BTreeSet<CubePatchAddress>, address: CubePatchAddress) -> bool {
    zone.iter()
        .any(|region| region.contains(address) || address.contains(*region))
}

fn publication_parent(candidate: &RegionalPublication) -> CubePatchAddress {
    match candidate {
        RegionalPublication::Split { parent, .. } | RegionalPublication::Merge { parent, .. } => {
            *parent
        }
    }
}

fn prepare_transition_with_target(
    work: &TransitionPublicationWork,
    target: Arc<BTreeMap<CubePatchAddress, TileBoundary>>,
    zone: Option<&BTreeSet<CubePatchAddress>>,
) -> std::result::Result<PreparedTransition, mundaris_renderer::resident_tile::TileGeometryError> {
    let (parent, children, merging) = match &work.candidate {
        RegionalPublication::Split {
            parent, children, ..
        } => (*parent, *children, false),
        RegionalPublication::Merge {
            parent, children, ..
        } => (*parent, *children, true),
    };
    let parent_boundary = if merging {
        target.get(&parent)
    } else {
        work.outgoing_boundaries.get(&parent)
    }
    .ok_or(mundaris_renderer::resident_tile::TileGeometryError::InvalidTile)?;
    let mut coarse = BTreeMap::new();
    for child in children {
        let child_tile = work
            .child_tiles
            .get(&child)
            .ok_or(mundaris_renderer::resident_tile::TileGeometryError::InvalidTile)?;
        coarse.insert(
            child,
            subdivided_parent_boundary(work.parent_tile.as_ref(), parent_boundary, child_tile)?,
        );
    }
    let mut affected = work
        .candidate
        .cover()
        .iter()
        .filter(|address| zone.is_none_or(|zone| zone_contains(zone, **address)))
        .filter(|address| work.outgoing_boundaries.get(address) != target.get(address))
        .copied()
        .collect::<BTreeSet<_>>();
    affected.extend(if merging {
        children.into_iter().collect::<BTreeSet<_>>()
    } else {
        BTreeSet::from([parent])
    });
    let parent_boundary = parent_boundary.clone();
    let mut staged = BTreeMap::new();
    for child in children {
        let start = if merging {
            work.outgoing_boundaries.get(&child)
        } else {
            coarse.get(&child)
        }
        .ok_or(mundaris_renderer::resident_tile::TileGeometryError::InvalidTile)?
        .clone();
        let end = if merging {
            coarse.get(&child)
        } else {
            target.get(&child)
        }
        .ok_or(mundaris_renderer::resident_tile::TileGeometryError::InvalidTile)?
        .clone();
        let [x, y] = child.coordinates();
        staged.insert(
            child,
            Patch {
                parent,
                quadrant: Some([x % 2, y % 2]),
                endpoints: Arc::new(RegionalBoundaryEndpoints {
                    version: work.group_id,
                    own_coarse: start,
                    own_fine: end,
                    parent: parent_boundary.clone(),
                }),
                group: Some(work.group_id),
                merging,
            },
        );
        affected.insert(child);
    }
    for address in work.candidate.cover() {
        if *address == parent || children.contains(address) || !affected.contains(address) {
            continue;
        }
        let old = work
            .outgoing_boundaries
            .get(address)
            .ok_or(mundaris_renderer::resident_tile::TileGeometryError::InvalidTile)?
            .clone();
        let fine = target
            .get(address)
            .ok_or(mundaris_renderer::resident_tile::TileGeometryError::InvalidTile)?
            .clone();
        staged.insert(
            *address,
            Patch {
                parent: *address,
                quadrant: None,
                endpoints: Arc::new(RegionalBoundaryEndpoints {
                    version: work.group_id,
                    own_coarse: old.clone(),
                    own_fine: fine,
                    parent: old,
                }),
                group: Some(work.group_id),
                merging: false,
            },
        );
    }
    let completion_parent = merging.then(|| {
        Arc::new(RegionalBoundaryEndpoints {
            version: work.group_id,
            own_coarse: parent_boundary.clone(),
            own_fine: parent_boundary.clone(),
            parent: parent_boundary.clone(),
        })
    });
    Ok(PreparedTransition {
        cover: work.candidate.cover().to_vec(),
        target,
        affected,
        staged,
        group_id: work.group_id,
        completion_parent,
    })
}

fn boundary_retained_bytes(boundary: &TileBoundary) -> usize {
    std::mem::size_of::<TileBoundary>()
        + boundary
            .edges
            .iter()
            .map(|edge| {
                edge.capacity()
                    * std::mem::size_of::<mundaris_renderer::regional_edges::BoundaryVertex>()
            })
            .sum::<usize>()
}

fn prepare_publication_batch(
    work: &mut BatchPublicationWork,
    retained_tiles: &mut BTreeMap<CubePatchAddress, Arc<TileData>>,
    cache: &mut mundaris_renderer::regional_edges::RegionalBoundaryCache,
) -> std::result::Result<PreparedPublication, mundaris_renderer::resident_tile::TileGeometryError> {
    use mundaris_renderer::resident_tile::TileGeometryError;
    let mut outgoing = work.base.as_ref().clone();
    for (address, boundary) in &work.overlays {
        if let Some(boundary) = boundary {
            outgoing.insert(*address, boundary.clone());
        } else {
            outgoing.remove(address);
        }
    }
    let outgoing = Arc::new(outgoing);
    let mut cover: BTreeSet<_> = outgoing.keys().copied().collect();
    for transition in &work.transitions {
        retained_tiles.insert(
            transition.parent_tile.key.address,
            Arc::clone(&transition.parent_tile),
        );
        retained_tiles.extend(transition.child_tiles.clone());
        match &transition.candidate {
            RegionalPublication::Split {
                parent, children, ..
            } => {
                if !cover.remove(parent) {
                    return Err(TileGeometryError::InvalidTile);
                }
                cover.extend(children);
            }
            RegionalPublication::Merge {
                parent, children, ..
            } => {
                if !children.iter().all(|child| cover.remove(child)) {
                    return Err(TileGeometryError::InvalidTile);
                }
                cover.insert(*parent);
            }
        }
    }
    let tiles = cover
        .iter()
        .map(|address| {
            retained_tiles
                .get(address)
                .map(|tile| (*address, Arc::clone(tile)))
                .ok_or(TileGeometryError::InvalidTile)
        })
        .collect::<std::result::Result<BTreeMap<_, _>, _>>()?;
    let target = {
        let _span = engine_profile::span("Canonical boundaries");
        Arc::new(cache.build_parallel(&tiles, 2)?)
    };
    for transition in &mut work.transitions {
        transition.outgoing_boundaries = Arc::clone(&outgoing);
        match &mut transition.candidate {
            RegionalPublication::Split {
                cover: candidate_cover,
                ..
            }
            | RegionalPublication::Merge {
                cover: candidate_cover,
                ..
            } => {
                *candidate_cover = cover.iter().copied().collect();
            }
        }
    }
    // At most two endpoint tasks borrow one immutable canonical target. The
    // coordinator alone owns cache mutation and deterministic result assembly.
    let chunk_size = work.transitions.len().div_ceil(2).max(1);
    let transitions = thread::scope(|scope| {
        let mut handles = Vec::new();
        for (index, chunk) in work.transitions.chunks(chunk_size).enumerate() {
            let target = &target;
            handles.push(
                thread::Builder::new()
                    .name("regional-endpoints".into())
                    .stack_size(2 * 1024 * 1024)
                    .spawn_scoped(scope, move || {
                        let _lane = engine_profile::worker_scope_named(if index == 0 {
                            "Boundary Worker 0"
                        } else {
                            "Boundary Worker 1"
                        });
                        let _span = engine_profile::span("Endpoint preparation");
                        chunk
                            .iter()
                            .map(|transition| {
                                let zone =
                                    publication_zone(publication_parent(&transition.candidate));
                                let mut prepared = prepare_transition_with_target(
                                    transition,
                                    Arc::clone(target),
                                    Some(&zone),
                                )?;
                                prepared.cover.clear();
                                let mut candidate = transition.candidate.clone();
                                match &mut candidate {
                                    RegionalPublication::Split { cover, .. }
                                    | RegionalPublication::Merge { cover, .. } => cover.clear(),
                                }
                                Ok((candidate, prepared))
                            })
                            .collect::<std::result::Result<Vec<_>, TileGeometryError>>()
                    })
                    .map_err(|_| TileGeometryError::InvalidTile)?,
            );
        }
        let mut entries = Vec::new();
        for handle in handles {
            entries.extend(
                handle
                    .join()
                    .map_err(|_| TileGeometryError::InvalidTile)??,
            );
        }
        Ok::<_, TileGeometryError>(entries)
    })?;
    let mut staged = BTreeSet::new();
    for (_, prepared) in &transitions {
        // Corner/canonical dependencies must be disjoint, even if candidate
        // parent regions passed the conservative dispatch preflight.
        if prepared
            .staged
            .keys()
            .any(|address| !staged.insert(*address))
        {
            return Err(TileGeometryError::InvalidTile);
        }
    }
    retained_tiles.retain(|address, _| {
        outgoing.contains_key(address)
            || tiles.contains_key(address)
            || work.transitions.iter().any(|transition| {
                transition.parent_tile.key.address == *address
                    || transition.child_tiles.contains_key(address)
            })
    });
    Ok(PreparedPublication::Batch {
        base: outgoing,
        transitions,
    })
}

fn boundary_table_retained_bytes(boundaries: &BTreeMap<CubePatchAddress, TileBoundary>) -> usize {
    boundaries.values().map(boundary_retained_bytes).sum()
}

fn endpoint_retained_bytes(endpoints: &RegionalBoundaryEndpoints) -> usize {
    std::mem::size_of::<RegionalBoundaryEndpoints>()
        + boundary_retained_bytes(&endpoints.own_coarse)
        + boundary_retained_bytes(&endpoints.own_fine)
        + boundary_retained_bytes(&endpoints.parent)
}

fn endpoint_tables_retained_bytes(patches: &BTreeMap<CubePatchAddress, Patch>) -> usize {
    let mut seen = HashSet::new();
    patches
        .values()
        .filter(|patch| seen.insert(Arc::as_ptr(&patch.endpoints)))
        .map(|patch| endpoint_retained_bytes(&patch.endpoints))
        .sum()
}

fn boundary_workspace_bound_bytes(max_cover: usize, cells: u32) -> usize {
    let one_boundary = std::mem::size_of::<TileBoundary>().saturating_add(
        4usize
            .saturating_mul(cells as usize + 1)
            .saturating_mul(std::mem::size_of::<
                mundaris_renderer::regional_edges::BoundaryVertex,
            >()),
    );
    max_cover
        .saturating_mul(2)
        .saturating_mul(one_boundary)
        .saturating_add(
            max_cover
                .saturating_add(4)
                .saturating_mul(3)
                .saturating_mul(one_boundary),
        )
        // Current-cover canonical sources, dependency indexes and cached
        // boundaries. This is a conservative workspace bound, not RSS.
        .saturating_add(
            max_cover
                .saturating_mul(4)
                .saturating_mul(cells as usize + 1)
                .saturating_mul(1024),
        )
}

impl PublicationWorker {
    fn new() -> Result<Self> {
        let (sender, jobs) = mpsc::sync_channel::<PublicationJob>(1);
        let (completion_sender, completions) = mpsc::sync_channel::<PublicationCompletion>(1);
        let retained_tile_bytes = Arc::new(AtomicUsize::new(0));
        let worker_retained_tile_bytes = Arc::clone(&retained_tile_bytes);
        let boundary_cache_bytes = Arc::new(AtomicUsize::new(0));
        let worker_boundary_cache_bytes = Arc::clone(&boundary_cache_bytes);
        let rebuilt_boundaries = Arc::new(AtomicUsize::new(0));
        let worker_rebuilt_boundaries = Arc::clone(&rebuilt_boundaries);
        let active = Arc::new(AtomicUsize::new(0));
        let worker_active = Arc::clone(&active);
        let completion_ready = Arc::new(AtomicUsize::new(0));
        let worker_completion_ready = Arc::clone(&completion_ready);
        let snapshot_bytes = Arc::new(AtomicUsize::new(0));
        let worker_snapshot_bytes = Arc::clone(&snapshot_bytes);
        thread::Builder::new()
            .name("regional-publication".into())
            .stack_size(2 * 1024 * 1024)
            .spawn(move || {
                let _lane = engine_profile::worker_scope_named("Boundary coordinator");
                let mut retained_outgoing: Option<Arc<BTreeMap<CubePatchAddress, TileBoundary>>> =
                    None;
                let mut retained_prepared: Option<Arc<BTreeMap<CubePatchAddress, TileBoundary>>> =
                    None;
                let mut retained_tiles = BTreeMap::new();
                let mut retired_base = None;
                let mut retained_snapshots = Vec::new();
                let mut boundary_cache =
                    mundaris_renderer::regional_edges::RegionalBoundaryCache::default();
                while let Ok(job) = jobs.recv() {
                    let _span = engine_profile::span("Boundary preparation");
                    worker_active.store(1, Ordering::Release);
                    let started = std::time::Instant::now();
                    let PublicationJob { work, token } = job;
                    if let Some(trace) = &token.trace {
                        for key in token.keys.values() {
                            trace.event_key(key, Stage::BoundaryStarted);
                        }
                    }
                    drop(retained_prepared.take());
                    let prepared = match work {
                        PublicationWork::Bootstrap(work) => {
                            drop(retained_outgoing.take());
                            retained_tiles = work.tiles.clone();
                            boundary_cache
                                .build(&work.tiles)
                                .map(Arc::new)
                                .map(|target| {
                                    retained_prepared = Some(Arc::clone(&target));
                                    PreparedPublication::Bootstrap(target)
                                })
                                .map_err(|error| error.to_string())
                        }
                        PublicationWork::Transition(mut work) => {
                            drop(retained_outgoing.take());
                            retained_outgoing = Some(Arc::clone(&work.outgoing_boundaries));
                            let prepared = collect_transition_tiles(&mut work, &mut retained_tiles)
                                .and_then(|()| boundary_cache.build(&work.tiles))
                                .and_then(|target| {
                                    prepare_transition_with_boundaries(&work, target)
                                })
                                .map(PreparedPublication::Transition)
                                .map_err(|error| error.to_string());
                            if let Ok(PreparedPublication::Transition(transition)) = &prepared {
                                retained_prepared = Some(Arc::clone(&transition.target));
                            }
                            prepared
                        }
                        PublicationWork::Batch(mut work) => {
                            retained_snapshots = std::mem::take(&mut work.keep_alive);
                            drop(retired_base.take());
                            retired_base = Some(Arc::clone(&work.base));
                            drop(retained_outgoing.take());
                            let result = prepare_publication_batch(
                                &mut work,
                                &mut retained_tiles,
                                &mut boundary_cache,
                            )
                            .map_err(|error| error.to_string());
                            if let Ok(PreparedPublication::Batch { base, transitions }) = &result {
                                retained_snapshots.push(Arc::clone(base));
                                if let Some((_, prepared)) = transitions.first() {
                                    retained_snapshots.push(Arc::clone(&prepared.target));
                                }
                                retained_outgoing = Some(Arc::clone(base));
                                retained_prepared = transitions
                                    .first()
                                    .map(|(_, prepared)| Arc::clone(&prepared.target));
                            }
                            result
                        }
                    };
                    // Keep snapshots until the following job identifies those
                    // still owned by pending products. Final release is here.
                    let mut unique_maps = HashSet::new();
                    let snapshot_bytes = retained_snapshots
                        .iter()
                        .chain(retained_outgoing.iter())
                        .chain(retained_prepared.iter())
                        .chain(retired_base.iter())
                        .filter(|map| unique_maps.insert(Arc::as_ptr(map)))
                        .map(|map| boundary_table_retained_bytes(map))
                        .sum();
                    worker_snapshot_bytes.store(snapshot_bytes, Ordering::Release);
                    worker_retained_tile_bytes.store(
                        retained_tiles
                            .values()
                            .map(|tile| tile.retained_payload_bytes())
                            .sum(),
                        Ordering::Release,
                    );
                    worker_boundary_cache_bytes
                        .store(boundary_cache.accounted_owned_bytes(), Ordering::Release);
                    worker_rebuilt_boundaries
                        .store(boundary_cache.last_rebuilt_count(), Ordering::Release);
                    if let Some(trace) = &token.trace {
                        for key in token.keys.values() {
                            if prepared.is_ok() {
                                trace.event_key(key, Stage::BoundaryFinished);
                                trace.event_key(key, Stage::FullyPrepared);
                            } else {
                                trace.event_key(key, Stage::BoundaryFailed);
                            }
                        }
                    }
                    let completion = PublicationCompletion {
                        token,
                        prepared,
                        elapsed: started.elapsed(),
                    };
                    worker_active.store(0, Ordering::Release);
                    worker_completion_ready.store(1, Ordering::Release);
                    if completion_sender.send(completion).is_err() {
                        break;
                    }
                }
            })
            .context("failed to start regional publication worker")?;
        Ok(Self {
            sender,
            completions,
            in_flight: None,
            retained_tile_bytes,
            boundary_cache_bytes,
            rebuilt_boundaries,
            active,
            completion_ready,
            snapshot_bytes,
        })
    }

    fn submit(&mut self, work: PublicationWork, token: PublicationToken) -> Result<bool> {
        if self.in_flight.is_some() {
            return Ok(false);
        }
        let in_flight = PublicationInFlight {
            dependencies: Arc::clone(&token.dependencies),
            retained_tile_bytes: token.retained_tile_bytes,
            queued_at: std::time::Instant::now(),
        };
        if let Some(trace) = &token.trace {
            let _span = engine_profile::span("Boundary queue trace");
            for key in token.keys.values() {
                trace.event_key(key, Stage::BoundaryQueued);
            }
        }
        match self.sender.try_send(PublicationJob { work, token }) {
            Ok(()) => {
                self.in_flight = Some(in_flight);
                Ok(true)
            }
            Err(TrySendError::Full(_)) => Ok(false),
            Err(TrySendError::Disconnected(_)) => {
                anyhow::bail!("regional publication worker stopped")
            }
        }
    }

    fn take_completion(&mut self) -> Option<PublicationCompletion> {
        match self.completions.try_recv() {
            Ok(completion) => {
                self.completion_ready.store(0, Ordering::Release);
                self.in_flight = None;
                Some(completion)
            }
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct NativeFrameSample {
    #[serde(flatten)]
    publication_stages: PublicationStages,
    frame: u64,
    interval_ms: f64,
    host_frame_ms: Option<f64>,
    frame_cpu_ms: Option<f64>,
    update_ms: Option<f64>,
    render_present_ms: Option<f64>,
    diagnostics_ms: Option<f64>,
    completion_draining_ms: f64,
    gpu_validation_dependency_ms: Option<f64>,
    gpu_resource_allocation_ms: Option<f64>,
    gpu_upload_preparation_ms: Option<f64>,
    gpu_drawable_metadata_ms: Option<f64>,
    regional_advance_ms: f64,
    selector_ms: f64,
    publication_ms: f64,
    publication_collection_ms: f64,
    publication_completion_ms: f64,
    publication_group_advance_ms: f64,
    gpu_admission_ms: f64,
    publication_admission_ms: f64,
    publication_budget_overruns: u64,
    preparation_ms: Option<f64>,
    gpu_preparation_ms: f64,
    gpu_terrain_ms: Option<f64>,
    gpu_source_frame: Option<u64>,
    desired: usize,
    drawable: usize,
    transitions: usize,
    tile_upload_bytes: u64,
    boundary_upload_bytes: u64,
    metadata_upload_bytes: u64,
    cumulative_tile_upload_bytes: u64,
    cumulative_boundary_upload_bytes: u64,
    cumulative_tile_upload_count: u64,
    cumulative_boundary_upload_count: u64,
    tile_upload_bytes_per_second: f64,
    tile_uploads_per_second: f64,
    completed_generation_tiles: u64,
    generation_throughput_tiles_per_second: f64,
    boundary_preparation_completed_groups: u64,
    publication_adopted_groups: u64,
    boundary_throughput_groups_per_second: f64,
    publication_throughput_groups_per_second: f64,
    fully_prepared_results: usize,
    immediately_publishable_results: usize,
    blocked_by_split: usize,
    blocked_by_merge: usize,
    blocked_by_neighbor_dependency: usize,
    blocked_by_residency_slot: usize,
    publication_backlog_age_ms: f64,
}

/// Native CPU elapsed scopes. Background boundary work is reported separately.
#[derive(Clone, Debug, Default, serde::Serialize)]
struct PublicationStages {
    gpu_frontier_policy_comparison_ms: f64,
    gpu_merge_frontiers_ms: f64,
    gpu_base_pins_ms: f64,
    gpu_split_frontiers_ms: f64,
    gpu_frontier_selection_ms: f64,
    gpu_upload_allowlist_ms: f64,
    publication_candidate_zone_ms: f64,
    publication_payload_collection_ms: f64,
    publication_dispatch_ms: f64,
    publication_collection_unattributed_ms: f64,
    publication_candidate_discovery_ms: f64,
    publication_dependency_checks_ms: f64,
    publication_state_mutation_ms: f64,
    publication_conflict_index_ms: f64,
    resident_slot_allocation_ms: f64,
    resident_draw_preparation_ms: f64,
    boundary_preparation_background_ms: f64,
}

/// Immutable, bounded planetary history omitted from per-frame summaries and
/// merged into the legacy JSON shape only when a snapshot is serialized.
#[derive(Clone, Debug)]
pub(crate) struct RegionalFrameHistory {
    pub(crate) frame_intervals_ms: Vec<f64>,
    pub(crate) native_frame_samples: Vec<NativeFrameSample>,
    pub(crate) events: Arc<[serde_json::Value]>,
}

impl serde::Serialize for RegionalFrameHistory {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("RegionalFrameHistory", 3)?;
        state.serialize_field("frame_intervals_ms", &self.frame_intervals_ms)?;
        state.serialize_field("native_frame_samples", &self.native_frame_samples)?;
        state.serialize_field("events", self.events.as_ref())?;
        state.end()
    }
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
    let additional_capacity = capacity.saturating_sub(base.len());
    let mut unique_missing =
        HashSet::with_capacity(additional_capacity.min(frontiers.len().saturating_mul(4)));
    let mut constrained = base.len() > capacity;
    let mut minimum_peak_slots = None;
    let mut retained = None;
    let mut best = None;

    for group in frontiers {
        // Preserve the original per-group peak semantics: repeated children
        // in one frontier count repeatedly here, even though the global union
        // below counts addresses uniquely.
        let peak = base.len()
            + group
                .children
                .iter()
                .filter(|child| !base.contains(child))
                .count();
        minimum_peak_slots =
            Some(minimum_peak_slots.map_or(peak, |minimum: usize| minimum.min(peak)));

        if previous == Some(group.parent) && peak <= capacity && retained.is_none() {
            retained = Some(group);
        }
        if peak <= capacity
            && best.is_none_or(|current: &RegionalSplitFrontier| {
                frontier_rank_cmp(group, current).is_lt()
            })
        {
            best = Some(group);
        }

        if !constrained {
            for child in group.children {
                if !base.contains(&child) && !unique_missing.contains(&child) {
                    if unique_missing.len() == additional_capacity {
                        constrained = true;
                        break;
                    }
                    unique_missing.insert(child);
                }
            }
        }
    }
    if !constrained {
        return GpuFrontierAdmission {
            reserved_parent: None,
            constrained: false,
            pressure: false,
            minimum_peak_slots,
        };
    }
    let reserved_parent = retained.or(best).map(|group| group.parent);
    GpuFrontierAdmission {
        reserved_parent,
        constrained: true,
        pressure: reserved_parent.is_none() && !frontiers.is_empty(),
        minimum_peak_slots,
    }
}

fn frontier_rank_cmp(
    left: &RegionalSplitFrontier,
    right: &RegionalSplitFrontier,
) -> std::cmp::Ordering {
    right
        .ready_children
        .cmp(&left.ready_children)
        .then_with(|| right.aggregate_priority.total_cmp(&left.aggregate_priority))
        .then_with(|| left.parent.cmp(&right.parent))
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
struct TraceExport {
    sampled_at: Option<std::time::Instant>,
    value: Option<Arc<serde_json::Value>>,
    sampler: crate::performance_capture::TerrainTraceSampler,
    identity: usize,
}

struct BasePinCache {
    topology_revision: u64,
    groups: Vec<u64>,
    pins: Arc<BTreeSet<CubePatchAddress>>,
}

#[derive(Default)]
pub(super) struct RegionalFixture {
    trace_snapshot_cache: std::cell::RefCell<TraceExport>,
    base_pin_cache: std::cell::RefCell<Option<BasePinCache>>,
    pub enabled: bool,
    core: Option<RegionalTerrain>,
    settings: Option<RegionalSettings>,
    slot_by_key: HashMap<TileKey, usize>,
    slot_scan_cursor: usize,
    publication_worker: Option<PublicationWorker>,
    publication_stages: PublicationStages,
    prepared_publications: VecDeque<PendingPublication>,
    boundary_overlays: BTreeMap<CubePatchAddress, Option<TileBoundary>>,
    boundary_completed: u64,
    adoption_completed: u64,
    publication_max_backlog_age_ms: f64,
    blocked_by_split: usize,
    blocked_by_merge: usize,
    blocked_by_dependency: usize,
    blocked_by_residency: usize,
    immediately_publishable: usize,
    publication_commits_this_frame: usize,
    publication_candidate_age: HashMap<CubePatchAddress, std::time::Instant>,
    topology_revision: u64,
    planetary: bool,
    planetary_identity: Option<TileKey>,
    bootstrap_published: bool,
    planetary_reconfigure_pending: bool,
    planetary_rotation: Option<DMat3>,
    planetary_projection: Option<CelestialProjection>,
    publication_build_ms: f64,
    publication_admission_ms: f64,
    publication_admission_total_ms: f64,
    publication_collection_ms: f64,
    publication_completion_ms: f64,
    publication_group_advance_ms: f64,
    gpu_admission_ms: f64,
    publication_admission_overrun_this_frame: bool,
    publication_builds: u64,
    publication_stale: u64,
    publication_budget_overruns: u64,
    publication_failures: u64,
    publication_last_state: &'static str,
    slots: Vec<Option<Slot>>,
    boundaries: Arc<BTreeMap<CubePatchAddress, TileBoundary>>,
    patches: BTreeMap<CubePatchAddress, Patch>,
    groups: Vec<Group>,
    publication_blocked_groups: Vec<u64>,
    publication_blocked_drawable: Vec<CubePatchAddress>,
    next_group: u64,
    frame: u64,
    gpu_use_clock: u64,
    last_camera: Option<DVec3>,
    frame_intervals_ms: VecDeque<f64>,
    native_frame_samples: VecDeque<NativeFrameSample>,
    regional_advance_ms: f64,
    events: Vec<serde_json::Value>,
    event_history_cache: std::cell::RefCell<Option<Arc<[serde_json::Value]>>>,
    topology_deferred: u64,
    slot_pressure: bool,
    gpu_admission_pressure: bool,
    gpu_split_reservation: Option<CubePatchAddress>,
    gpu_frontier_pins: BTreeSet<CubePatchAddress>,
    gpu_merge_parents: BTreeSet<CubePatchAddress>,
    gpu_policy_initialized: bool,
    gpu_policy_desired: Vec<CubePatchAddress>,
    gpu_policy_priority_revision: u64,
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
    pub fn configure_planetary(
        &mut self,
        generator: SurfaceGenerator,
        identity: TileBuildIdentity,
        cells: u32,
    ) -> Result<bool> {
        ensure!(
            cells.is_power_of_two() && (1..=TileData::MAX_CELLS).contains(&cells),
            "planetary tile cells must be a supported power of two"
        );
        let identity_key = ResidentTileBuilder::tile_key(
            &generator,
            identity,
            CubePatchAddress::root(CubeFace::PositiveX),
            cells,
        )?;
        if self.enabled && self.planetary {
            let same_authority = self.core.as_ref().is_some_and(|core| {
                core.config().cells == cells
                    && core.config().roots.len() == 6
                    && self.planetary_identity.as_ref() == Some(&identity_key)
            });
            if same_authority && !self.planetary_reconfigure_pending {
                return Ok(true);
            }
            if !self.planetary_reconfigure_pending {
                self.topology_revision = self.topology_revision.saturating_add(1);
            }
            self.planetary_reconfigure_pending = true;
        }
        if self.core.as_ref().is_some_and(|core| !core.is_idle()) {
            if !self.planetary_reconfigure_pending {
                self.topology_revision = self.topology_revision.saturating_add(1);
            }
            if let Some(core) = &mut self.core {
                core.request_cancel_all();
                core.drain_cancelled();
            }
            self.planetary_reconfigure_pending = true;
        }
        if let Some(completion) = self
            .publication_worker
            .as_mut()
            .and_then(PublicationWorker::take_completion)
        {
            drop(completion);
            self.publication_stale = self.publication_stale.saturating_add(1);
            self.publication_last_state = "stale_authority_discarded";
        }
        if self.planetary_reconfigure_pending
            && (self.core.as_ref().is_some_and(|core| !core.is_idle())
                || self
                    .publication_worker
                    .as_ref()
                    .is_some_and(|worker| worker.in_flight.is_some()))
        {
            if let Some(core) = &mut self.core {
                core.request_cancel_all();
                core.drain_cancelled();
            }
            return Ok(false);
        }
        let mut roots = CubeFace::ALL
            .into_iter()
            .map(CubePatchAddress::root)
            .collect::<Vec<_>>();
        roots.sort();
        let config = RegionalConfig {
            roots,
            max_level: 24,
            cells,
            cpu_tile_cap: PLANETARY_CPU_TILES,
            cpu_byte_cap: PLANETARY_CPU_TILES.saturating_mul(
                (cells as usize + 3)
                    .saturating_mul(cells as usize + 3)
                    .saturating_mul(32),
            ),
            worker_count: 4,
            worker_delay: Duration::ZERO,
            queue_cap: 32,
            completion_cap: 8,
            admission_cap_per_tick: 4,
            max_desired_patches: PLANETARY_MAX_DESIRED_PATCHES,
            upload_tile_cap: 4,
            upload_byte_cap: 8 * 1024 * 1024,
            publication_cap_per_tick: 8,
            transition_cap: 16,
            split_threshold_px: 0.15,
            merge_threshold_px: 0.075,
            prediction_seconds: 0.15,
            high_speed_mps: 500.0,
        };
        let core = RegionalTerrain::new(generator, identity, config)?;
        if self.publication_worker.is_none() {
            self.publication_worker = Some(PublicationWorker::new()?);
        }
        let slots = std::mem::take(&mut self.slots);
        let report = std::mem::take(&mut self.last_report);
        let gpu_use_clock = self.gpu_use_clock;
        let next_group = self.next_group;
        *self = Self {
            enabled: true,
            core: Some(core),
            settings: Some(RegionalSettings {
                enabled: true,
                max_depth: 24,
                gpu_slots: PLANETARY_GPU_SLOTS,
                cpu_tiles: PLANETARY_CPU_TILES,
                worker_count: 4,
                worker_delay_ms: 0,
                upload_tiles_per_frame: 4,
                upload_bytes_per_frame: 8 * 1024 * 1024,
                publication_groups_per_frame: 8,
                transition_limit: 16,
                morph_duration_ms: 150,
                split_error_px: 0.15,
                merge_error_px: 0.075,
            }),
            publication_worker: self.publication_worker.take(),
            slots: {
                let mut slots = slots;
                slots.resize_with(PLANETARY_GPU_SLOTS, || None);
                slots
            },
            last_report: report,
            gpu_use_clock,
            next_group,
            topology_revision: self.topology_revision.saturating_add(1),
            planetary: true,
            planetary_identity: Some(identity_key),
            planetary_reconfigure_pending: false,
            publication_last_state: "roots_pending",
            ..Self::default()
        };
        self.rebuild_slot_index();
        Ok(true)
    }

    pub fn template_tile(&self) -> Option<Arc<TileData>> {
        let core = self.core.as_ref()?;
        let root = *core.config().roots.first()?;
        core.tile(root)
    }

    pub fn has_coverage(&self) -> bool {
        let Some(core) = &self.core else { return false };
        if !self.planetary {
            return !self.patches.is_empty();
        }
        !self.patches.is_empty()
            && self.bootstrap_published
            && core.drawable().iter().all(|address| {
                self.slot_for(*address).is_some_and(|index| {
                    let Some(tile) = core.tile(*address) else {
                        return false;
                    };
                    self.last_report
                        .slots
                        .get(index)
                        .and_then(|slot| slot.key.as_ref())
                        == Some(&tile.key)
                })
            })
    }

    pub fn set_planetary_view(
        &mut self,
        rotation: DMat3,
        projection: CelestialProjection,
    ) -> Result<()> {
        self.planetary_rotation = Some(rotation);
        self.planetary_projection = Some(projection);
        if let Some(core) = &mut self.core {
            core.set_planetary_view(rotation, projection)?;
        }
        Ok(())
    }

    pub fn radial_levels(&self, position_body_m: DVec3) -> (Option<u8>, Option<u8>) {
        let Some(core) = &self.core else {
            return (None, None);
        };
        let direction = match Direction3::try_new(position_body_m.normalize_or_zero()) {
            Ok(direction) => direction,
            Err(_) => return (None, None),
        };
        let (face, uv) = SurfaceLocation::new(direction).face_uv();
        let level = |addresses: &[CubePatchAddress]| {
            addresses
                .iter()
                .filter(|address| address.face() == face && address.patch_local(uv).is_ok())
                .map(|address| address.level())
                .max()
        };
        (level(core.desired()), level(core.drawable()))
    }

    #[cfg_attr(not(any(feature = "developer-tools", test)), allow(dead_code))]
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
        ensure!(
            self.publication_worker
                .as_ref()
                .is_none_or(|worker| worker.in_flight.is_none()),
            "regional reconfiguration waits for publication work to drain"
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
        if self.publication_worker.is_none() {
            self.publication_worker = Some(PublicationWorker::new()?);
        }
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
        let publication_worker = self.publication_worker.take();
        let next_group = self.next_group;
        let gpu_use_clock = self.gpu_use_clock;
        *self = Self {
            enabled: true,
            core: Some(core),
            settings: Some(settings),
            slots,
            publication_worker,
            last_report: report,
            next_group,
            gpu_use_clock,
            ..Self::default()
        };
        self.rebuild_slot_index();
        Ok(())
    }

    pub fn disable(&mut self) {
        self.enabled = false;
        self.suspend();
    }

    /// Cancel obsolete generation while retaining the exact resident cover.
    pub fn suspend(&mut self) {
        if let Some(core) = &mut self.core {
            core.request_cancel_all();
            core.drain_cancelled();
        }
    }

    #[cfg_attr(not(any(feature = "developer-tools", test)), allow(dead_code))]
    pub fn pump_shutdown(&mut self) {
        if let Some(core) = &mut self.core {
            core.drain_cancelled();
        }
    }

    fn record(&mut self, event: serde_json::Value) {
        self.event_history_cache.get_mut().take();
        if self.events.len() == 4096 {
            self.events.remove(0);
        }
        self.events.push(event);
    }

    pub fn observe(&mut self, report: RegionalResidentReport) {
        let resident_keys: HashSet<_> = report
            .slots
            .iter()
            .filter_map(|slot| slot.key.as_ref())
            .collect();
        if let Some(core) = &mut self.core {
            for slot in self.slots.iter().flatten() {
                if core
                    .tile(slot.tile.key.address)
                    .is_some_and(|tile| tile.key == slot.tile.key)
                    && !resident_keys.contains(&slot.tile.key)
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
        let index = *self.slot_by_key.get(key)?;
        self.slots
            .get(index)
            .and_then(Option::as_ref)
            .is_some_and(|slot| slot.tile.key == *key)
            .then_some(index)
    }

    fn rebuild_slot_index(&mut self) {
        self.slot_by_key.clear();
        let slot_limit = self
            .settings
            .map_or(self.slots.len(), |settings| settings.gpu_slots);
        for (index, slot) in self.slots.iter().take(slot_limit).enumerate() {
            if let Some(slot) = slot {
                self.slot_by_key
                    .entry(slot.tile.key.clone())
                    .or_insert(index);
            }
        }
    }

    fn base_pins(&self) -> BTreeSet<CubePatchAddress> {
        self.shared_base_pins().as_ref().clone()
    }

    fn shared_base_pins(&self) -> Arc<BTreeSet<CubePatchAddress>> {
        let mut cache = self.base_pin_cache.borrow_mut();
        if let Some(cached) = cache.as_ref()
            && cached.topology_revision == self.topology_revision
            && cached
                .groups
                .iter()
                .copied()
                .eq(self.groups.iter().map(|group| group.id))
        {
            return Arc::clone(&cached.pins);
        }
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
        let pins = Arc::new(pins);
        *cache = Some(BasePinCache {
            topology_revision: self.topology_revision,
            groups: self.groups.iter().map(|group| group.id).collect(),
            pins: Arc::clone(&pins),
        });
        pins
    }

    fn pins(&self) -> BTreeSet<CubePatchAddress> {
        let mut pins = self.base_pins();
        if let Some(work) = self
            .publication_worker
            .as_ref()
            .and_then(|worker| worker.in_flight.as_ref())
        {
            pins.extend(work.dependencies.iter().copied());
        }
        for prepared in &self.prepared_publications {
            pins.extend(prepared.keys.keys().copied());
            pins.extend(prepared.prepared.affected.iter().copied());
        }
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
        let _span = engine_profile::span("GPU admission");
        let policy_started = std::time::Instant::now();
        let core = self.core.as_ref().context("regional core unavailable")?;
        let desired_changed = self.gpu_policy_desired != core.desired();
        let priority_revision = core.desired_priority_revision();
        let priorities_changed = self.gpu_policy_priority_revision != priority_revision;
        let drawable_changed = self.gpu_policy_drawable != core.drawable();
        self.publication_stages.gpu_frontier_policy_comparison_ms =
            policy_started.elapsed().as_secs_f64() * 1000.0;
        if !self.gpu_policy_initialized || desired_changed || drawable_changed {
            let merge_started = std::time::Instant::now();
            let _span = engine_profile::span("GPU merge frontiers");
            self.gpu_merge_parents = core.merge_frontiers().into_iter().collect();
            self.publication_stages.gpu_merge_frontiers_ms =
                merge_started.elapsed().as_secs_f64() * 1000.0;
        }
        let pins_started = std::time::Instant::now();
        let pins_span = engine_profile::span("GPU base pins");
        let mut shared_base = self.shared_base_pins();
        for parent in &self.gpu_merge_parents {
            if !shared_base.contains(parent) && self.slot_for(*parent).is_some() {
                Arc::make_mut(&mut shared_base).insert(*parent);
            }
        }
        drop(pins_span);
        self.publication_stages.gpu_base_pins_ms = pins_started.elapsed().as_secs_f64() * 1000.0;
        if self.gpu_policy_initialized
            && !desired_changed
            && !priorities_changed
            && !drawable_changed
            && shared_base.as_ref() == &self.gpu_policy_base_pins
        {
            return Ok(());
        }
        let frontier_started = std::time::Instant::now();
        let frontier_span = engine_profile::span("GPU split frontiers");
        let mut frontiers = core.split_frontiers();
        for group in &mut frontiers {
            group.ready_children = group
                .children
                .iter()
                .filter(|child| self.slot_for(**child).is_some())
                .count();
        }
        drop(frontier_span);
        self.publication_stages.gpu_split_frontiers_ms =
            frontier_started.elapsed().as_secs_f64() * 1000.0;
        let selection_started = std::time::Instant::now();
        let mut base = shared_base.as_ref().clone();
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
                    .map(|group| {
                        base.len()
                            + group
                                .children
                                .iter()
                                .filter(|child| !base.contains(child))
                                .count()
                    })
                    .min(),
            }
        };
        self.gpu_split_reservation = decision.reserved_parent;
        self.gpu_admission_pressure = decision.pressure;
        self.gpu_minimum_peak_split_slots = decision.minimum_peak_slots;
        self.gpu_split_frontier_count = frontiers.len();
        self.gpu_policy_desired = core.desired().to_vec();
        self.gpu_policy_priority_revision = priority_revision;
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
        self.publication_stages.gpu_frontier_selection_ms =
            selection_started.elapsed().as_secs_f64() * 1000.0;
        let allowlist_started = std::time::Instant::now();
        self.core
            .as_mut()
            .context("regional core unavailable")?
            .set_upload_allowlist(allowed.as_deref());
        self.publication_stages.gpu_upload_allowlist_ms =
            allowlist_started.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    pub fn advance(
        &mut self,
        position: DVec3,
        projection_scale_px: f64,
        dt: Duration,
        deterministic: bool,
    ) -> Result<()> {
        let _span = engine_profile::span("Terrain");
        let regional_started = std::time::Instant::now();
        if !self.enabled {
            return Ok(());
        }
        self.frame += 1;
        self.publication_stages = PublicationStages::default();
        self.blocked_by_split = 0;
        self.blocked_by_merge = 0;
        self.blocked_by_dependency = 0;
        self.blocked_by_residency = 0;
        self.immediately_publishable = 0;
        self.publication_commits_this_frame = 0;
        self.publication_admission_ms = 0.0;
        self.publication_collection_ms = 0.0;
        self.publication_completion_ms = 0.0;
        self.publication_group_advance_ms = 0.0;
        self.gpu_admission_ms = 0.0;
        self.publication_admission_overrun_this_frame = false;
        self.gpu_use_clock = self.gpu_use_clock.saturating_add(1);
        if !deterministic {
            if self.frame_intervals_ms.len() == 8192 {
                self.frame_intervals_ms.pop_front();
            }
            self.frame_intervals_ms.push_back(dt.as_secs_f64() * 1000.0);
        }
        if self.planetary_reconfigure_pending {
            if let Some(core) = &mut self.core {
                core.request_cancel_all();
                core.drain_cancelled();
            }
            if let Some(completion) = self
                .publication_worker
                .as_mut()
                .and_then(PublicationWorker::take_completion)
            {
                drop(completion);
                self.publication_stale = self.publication_stale.saturating_add(1);
                self.publication_last_state = "stale_authority_discarded";
            }
            self.publication_last_state = if self
                .publication_worker
                .as_ref()
                .is_some_and(|worker| worker.in_flight.is_some())
                || self.core.as_ref().is_some_and(|core| !core.is_idle())
            {
                "authority_drain_pending"
            } else {
                "authority_drained_waiting_reconfigure"
            };
            self.regional_advance_ms = regional_started.elapsed().as_secs_f64() * 1000.0;
            return Ok(());
        }
        let velocity = self.last_camera.map_or(DVec3::ZERO, |old| {
            (position - old) / dt.as_secs_f64().max(1.0e-6)
        });
        self.last_camera = Some(position);
        let pins: Vec<_> = self.pins().into_iter().collect();
        let conflict_started = std::time::Instant::now();
        let group_ids: Vec<_> = self.groups.iter().map(|group| group.id).collect();
        let drawable = self
            .core
            .as_ref()
            .context("regional core unavailable")?
            .drawable();
        if !self.planetary
            && (group_ids != self.publication_blocked_groups
                || drawable != self.publication_blocked_drawable)
            && self
                .publication_worker
                .as_ref()
                .is_none_or(|worker| worker.in_flight.is_none())
        {
            let blocked: Vec<_> = drawable
                .iter()
                .copied()
                .filter(|parent| {
                    let Ok(children) = parent.children() else {
                        return false;
                    };
                    self.conflicts_with_active_group(&RegionalPublication::Split {
                        parent: *parent,
                        children,
                        cover: Vec::new(),
                    })
                })
                .collect();
            self.publication_blocked_drawable = drawable.to_vec();
            self.publication_blocked_groups = group_ids;
            self.core
                .as_mut()
                .context("regional core unavailable")?
                .set_publication_blocked_parents(&blocked);
        }
        let core = self.core.as_mut().context("regional core unavailable")?;
        self.publication_stages.publication_conflict_index_ms =
            conflict_started.elapsed().as_secs_f64() * 1000.0;
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
        let groups_before_advance = self.groups.len();
        let group_advance_started = std::time::Instant::now();
        let completed_merges = self.advance_groups(dt)?;
        self.publication_group_advance_ms = group_advance_started.elapsed().as_secs_f64() * 1000.0;
        self.record_publication_admission(group_advance_started.elapsed());
        let transition_completed = self.groups.len() < groups_before_advance;
        let completion = self.finish_publication_worker()?;
        if self.planetary {
            self.adopt_prepared_publications()?;
        }
        if !completion {
            self.bootstrap()?;
            if self.bootstrap_published && !transition_completed && completed_merges == 0 {
                if self.planetary {
                    self.schedule_publication_batch()?;
                } else {
                    self.schedule_transition()?;
                }
            }
        }
        let gpu_admission_started = std::time::Instant::now();
        self.update_gpu_admission()?;
        self.publication_stages.publication_candidate_discovery_ms +=
            self.core.as_ref().map_or(0.0, |core| {
                core.publication_discovery_time_micros() as f64 / 1000.0
            });
        self.gpu_admission_ms = gpu_admission_started.elapsed().as_secs_f64() * 1000.0;
        self.cpu_publication_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.publication_max_backlog_age_ms = self
            .publication_max_backlog_age_ms
            .max(self.publication_backlog_age_ms());
        self.regional_advance_ms = regional_started.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    pub fn record_native_frame(
        &mut self,
        performance: &crate::developer_snapshot::PerformanceSnapshot,
        submitted_frame: u64,
    ) {
        if self.native_frame_samples.len() == 8192 {
            self.native_frame_samples.pop_front();
        }
        let interval_ms = self.frame_intervals_ms.back().copied().unwrap_or(0.0);
        let previous = self.native_frame_samples.back();
        let upload_byte_delta = previous.map_or(0, |sample| {
            self.last_report
                .cumulative_tile_upload_bytes
                .saturating_sub(sample.cumulative_tile_upload_bytes)
        });
        let upload_count_delta = previous.map_or(0, |sample| {
            self.last_report
                .cumulative_tile_upload_count
                .saturating_sub(sample.cumulative_tile_upload_count)
        });
        let completed_generation_tiles = self
            .core
            .as_ref()
            .map_or(0, RegionalTerrain::completed_build_count);
        let generation_delta = previous.map_or(0, |sample| {
            completed_generation_tiles.saturating_sub(sample.completed_generation_tiles)
        });
        let boundary_delta = previous.map_or(0, |sample| {
            self.boundary_completed
                .saturating_sub(sample.boundary_preparation_completed_groups)
        });
        let adoption_delta = previous.map_or(0, |sample| {
            self.adoption_completed
                .saturating_sub(sample.publication_adopted_groups)
        });
        let backlog_age_ms = self.publication_backlog_age_ms();
        self.native_frame_samples.push_back(NativeFrameSample {
            publication_stages: self.publication_stages.clone(),
            frame: submitted_frame,
            interval_ms,
            host_frame_ms: performance.host_frame_ms,
            frame_cpu_ms: performance.frame_cpu_ms,
            update_ms: performance.update_ms,
            render_present_ms: performance.render_present_ms,
            diagnostics_ms: performance.diagnostics_ms,
            completion_draining_ms: self.core.as_ref().map_or(0.0, |core| {
                core.completion_drain_time_micros() as f64 / 1000.0
            }),
            gpu_validation_dependency_ms: self
                .last_report
                .validation_dependency_micros
                .map(|v| v as f64 / 1000.0),
            gpu_resource_allocation_ms: self
                .last_report
                .resource_allocation_micros
                .map(|v| v as f64 / 1000.0),
            gpu_upload_preparation_ms: self
                .last_report
                .gpu_upload_preparation_micros
                .map(|v| v as f64 / 1000.0),
            gpu_drawable_metadata_ms: self
                .last_report
                .drawable_metadata_micros
                .map(|v| v as f64 / 1000.0),
            regional_advance_ms: self.regional_advance_ms,
            selector_ms: self
                .core
                .as_ref()
                .map_or(0.0, |core| core.selection_time_micros() as f64 / 1000.0),
            publication_ms: self.cpu_publication_ms,
            publication_collection_ms: self.publication_collection_ms,
            publication_completion_ms: self.publication_completion_ms,
            publication_group_advance_ms: self.publication_group_advance_ms,
            gpu_admission_ms: self.gpu_admission_ms,
            publication_admission_ms: self.publication_admission_ms,
            publication_budget_overruns: self.publication_budget_overruns,
            preparation_ms: performance.preparation_ms,
            gpu_preparation_ms: self.last_report.preparation_micros as f64 / 1000.0,
            gpu_terrain_ms: performance.gpu_terrain_ms,
            gpu_source_frame: performance.gpu_source_frame,
            desired: self.core.as_ref().map_or(0, |core| core.desired().len()),
            drawable: self.core.as_ref().map_or(0, |core| core.drawable().len()),
            transitions: self.groups.len(),
            tile_upload_bytes: self.last_report.tile_upload_bytes,
            boundary_upload_bytes: self.last_report.boundary_upload_bytes,
            metadata_upload_bytes: self.last_report.metadata_upload_bytes,
            cumulative_tile_upload_bytes: self.last_report.cumulative_tile_upload_bytes,
            cumulative_boundary_upload_bytes: self.last_report.cumulative_boundary_upload_bytes,
            cumulative_tile_upload_count: self.last_report.cumulative_tile_upload_count,
            cumulative_boundary_upload_count: self.last_report.cumulative_boundary_upload_count,
            tile_upload_bytes_per_second: if interval_ms > 0.0 {
                upload_byte_delta as f64 * 1000.0 / interval_ms
            } else {
                0.0
            },
            tile_uploads_per_second: if interval_ms > 0.0 {
                upload_count_delta as f64 * 1000.0 / interval_ms
            } else {
                0.0
            },
            completed_generation_tiles,
            boundary_preparation_completed_groups: self.boundary_completed,
            publication_adopted_groups: self.adoption_completed,
            boundary_throughput_groups_per_second: if interval_ms > 0.0 {
                boundary_delta as f64 * 1000.0 / interval_ms
            } else {
                0.0
            },
            publication_throughput_groups_per_second: if interval_ms > 0.0 {
                adoption_delta as f64 * 1000.0 / interval_ms
            } else {
                0.0
            },
            fully_prepared_results: self.prepared_publications.len(),
            immediately_publishable_results: self.immediately_publishable,
            blocked_by_split: self.blocked_by_split,
            blocked_by_merge: self.blocked_by_merge,
            blocked_by_neighbor_dependency: self.blocked_by_dependency,
            blocked_by_residency_slot: self.blocked_by_residency,
            publication_backlog_age_ms: backlog_age_ms,
            generation_throughput_tiles_per_second: if interval_ms > 0.0 {
                generation_delta as f64 * 1000.0 / interval_ms
            } else {
                0.0
            },
        });
    }

    fn record_publication_admission(&mut self, elapsed: std::time::Duration) {
        let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
        self.publication_admission_ms += elapsed_ms;
        self.publication_admission_total_ms += elapsed_ms;
        if self.publication_admission_ms > 2.0 && !self.publication_admission_overrun_this_frame {
            self.publication_budget_overruns = self.publication_budget_overruns.saturating_add(1);
            self.publication_admission_overrun_this_frame = true;
        }
    }

    fn bootstrap(&mut self) -> Result<()> {
        let started = std::time::Instant::now();
        let result = self.bootstrap_inner();
        self.record_publication_admission(started.elapsed());
        result
    }

    fn bootstrap_inner(&mut self) -> Result<()> {
        if self.publication_admission_ms >= 2.0 {
            self.publication_last_state = "deferred_publication_budget";
            return Ok(());
        }
        if self.bootstrap_published {
            return Ok(());
        }
        if self
            .publication_worker
            .as_ref()
            .is_none_or(|worker| worker.in_flight.is_some())
        {
            return Ok(());
        }
        let core = self.core.as_ref().context("regional core unavailable")?;
        let cover = core.config().roots.clone();
        if cover.is_empty()
            || core.drawable() != cover.as_slice()
            || cover
                .iter()
                .any(|address| self.slot_for(*address).is_none())
        {
            return Ok(());
        }
        let tiles = cover
            .iter()
            .map(|address| {
                let tile = core
                    .tile(*address)
                    .context("bootstrap root payload missing")?;
                Ok((*address, tile))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let keys = tiles
            .iter()
            .map(|(address, tile)| (*address, tile.key.clone()))
            .collect();
        let retained_tile_bytes = tiles
            .values()
            .map(|tile| tile.retained_payload_bytes())
            .sum();
        let token = PublicationToken {
            trace: self.core.as_ref().map(RegionalTerrain::trace),
            revision: self.topology_revision,
            kind: PublicationKind::Bootstrap {
                cover: cover.clone(),
            },
            keys,
            dependencies: Arc::from(cover.clone()),
            retained_tile_bytes,
        };
        let work = PublicationWork::Bootstrap(Box::new(BootstrapPublicationWork { tiles }));
        if self
            .publication_worker
            .as_mut()
            .context("regional publication worker unavailable")?
            .submit(work, token)?
        {
            self.publication_last_state = "bootstrap_building";
        }
        Ok(())
    }

    fn schedule_transition(&mut self) -> Result<()> {
        let started = std::time::Instant::now();
        let result = self.schedule_transition_inner(started);
        self.publication_collection_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.record_publication_admission(started.elapsed());
        result
    }

    fn logical_boundary(&self, address: CubePatchAddress) -> Option<&TileBoundary> {
        self.boundary_overlays
            .get(&address)
            .map_or_else(|| self.boundaries.get(&address), Option::as_ref)
    }

    fn publication_backlog_age_ms(&self) -> f64 {
        let prepared = self
            .prepared_publications
            .iter()
            .map(|pending| pending.ready_at.elapsed().as_secs_f64() * 1000.0);
        let candidates = self
            .publication_candidate_age
            .values()
            .map(|ready_at| ready_at.elapsed().as_secs_f64() * 1000.0);
        let building = self
            .publication_worker
            .as_ref()
            .and_then(|worker| worker.in_flight.as_ref())
            .map(|work| work.queued_at.elapsed().as_secs_f64() * 1000.0);
        prepared
            .chain(candidates)
            .chain(building)
            .fold(0.0, f64::max)
    }

    fn schedule_publication_batch(&mut self) -> Result<()> {
        let _span = engine_profile::span("Publication scan");
        let started = std::time::Instant::now();
        let stages_before = self.publication_stages.publication_candidate_zone_ms
            + self.publication_stages.publication_payload_collection_ms
            + self.publication_stages.publication_dispatch_ms
            + self.publication_stages.publication_candidate_discovery_ms
            + self.publication_stages.publication_dependency_checks_ms;
        let result = self.schedule_publication_batch_inner(started);
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let stages_after = self.publication_stages.publication_candidate_zone_ms
            + self.publication_stages.publication_payload_collection_ms
            + self.publication_stages.publication_dispatch_ms
            + self.publication_stages.publication_candidate_discovery_ms
            + self.publication_stages.publication_dependency_checks_ms;
        self.publication_stages
            .publication_collection_unattributed_ms +=
            (elapsed_ms - (stages_after - stages_before)).max(0.0);
        self.publication_collection_ms += elapsed_ms;
        self.record_publication_admission(started.elapsed());
        result
    }

    fn schedule_publication_batch_inner(&mut self, started: std::time::Instant) -> Result<()> {
        const PREPARED_CAPACITY: usize = 8;
        if self.publication_admission_ms >= 1.5
            || self.prepared_publications.len() == PREPARED_CAPACITY
            || self
                .publication_worker
                .as_ref()
                .is_none_or(|worker| worker.in_flight.is_some())
        {
            return Ok(());
        }
        let settings = self.settings.context("regional settings unavailable")?;
        let capacity = PREPARED_CAPACITY
            .saturating_sub(self.prepared_publications.len())
            .min(
                settings
                    .transition_limit
                    .saturating_sub(self.groups.len() + self.prepared_publications.len()),
            );
        if capacity == 0 {
            return Ok(());
        }
        let discovery_started = std::time::Instant::now();
        let candidates = self
            .core
            .as_ref()
            .context("regional core unavailable")?
            .publication_local_candidates();
        let current_candidates: HashSet<_> = candidates.iter().map(publication_parent).collect();
        self.publication_candidate_age
            .retain(|address, _| current_candidates.contains(address));
        for parent in &current_candidates {
            self.publication_candidate_age
                .entry(*parent)
                .or_insert(discovery_started);
        }
        self.publication_stages.publication_candidate_discovery_ms +=
            discovery_started.elapsed().as_secs_f64() * 1000.0;
        let mut transitions = Vec::new();
        let mut selected_zones: Vec<BTreeSet<CubePatchAddress>> = Vec::new();
        let mut keys = BTreeMap::new();
        let mut dependencies = BTreeSet::new();
        let mut seen_tiles = HashSet::new();
        let mut retained_tile_bytes = 0usize;
        for candidate in candidates {
            if transitions.len() == capacity
                || self.publication_admission_ms + started.elapsed().as_secs_f64() * 1000.0 >= 1.5
            {
                break;
            }
            let parent = publication_parent(&candidate);
            let zone_started = std::time::Instant::now();
            let zone = publication_zone(parent);
            let zone_conflict = self.prepared_publications.iter().any(|pending| {
                pending
                    .prepared
                    .affected
                    .iter()
                    .any(|address| zone_contains(&zone, *address))
            }) || selected_zones.iter().any(|selected| {
                selected
                    .iter()
                    .any(|address| zone_contains(&zone, *address))
            });
            self.publication_stages.publication_candidate_zone_ms +=
                zone_started.elapsed().as_secs_f64() * 1000.0;
            if zone_conflict {
                continue;
            }
            let dependency_started = std::time::Instant::now();
            let mut split_conflict = false;
            let mut merge_conflict = false;
            for group in &self.groups {
                if group
                    .affected
                    .iter()
                    .any(|address| zone_contains(&zone, *address))
                {
                    if group.merge.is_some() {
                        merge_conflict = true;
                    } else {
                        split_conflict = true;
                    }
                }
            }
            self.publication_stages.publication_dependency_checks_ms +=
                dependency_started.elapsed().as_secs_f64() * 1000.0;
            if split_conflict || merge_conflict {
                if let Some(core) = &self.core {
                    core.trace().block(
                        parent,
                        if merge_conflict {
                            BlockReason::ActiveMergeOverlap
                        } else {
                            BlockReason::ActiveSplitOverlap
                        },
                    );
                }
                self.blocked_by_split += usize::from(split_conflict);
                self.blocked_by_merge += usize::from(merge_conflict);
                continue;
            }
            let children = match &candidate {
                RegionalPublication::Split { children, .. }
                | RegionalPublication::Merge { children, .. } => *children,
            };
            let payload_started = std::time::Instant::now();
            if std::iter::once(parent)
                .chain(children)
                .any(|address| self.slot_for(address).is_none())
            {
                if let Some(core) = &self.core {
                    core.trace()
                        .block(parent, BlockReason::ResidentSlotUnavailable);
                }
                self.blocked_by_residency += 1;
                continue;
            }
            let core = self.core.as_ref().context("regional core unavailable")?;
            let Some(parent_tile) = core.tile(parent) else {
                core.trace().block(parent, BlockReason::ParentDependency);
                self.blocked_by_dependency += 1;
                continue;
            };
            let child_tiles = children
                .into_iter()
                .map(|child| core.tile(child).map(|tile| (child, tile)))
                .collect::<Option<BTreeMap<_, _>>>();
            let Some(child_tiles) = child_tiles else {
                core.trace().block(parent, BlockReason::ChildDependency);
                self.blocked_by_dependency += 1;
                continue;
            };
            for tile in std::iter::once(&parent_tile).chain(child_tiles.values()) {
                keys.insert(tile.key.address, tile.key.clone());
                dependencies.insert(tile.key.address);
                if seen_tiles.insert(Arc::as_ptr(tile)) {
                    retained_tile_bytes += tile.retained_payload_bytes();
                }
            }
            self.next_group = self.next_group.saturating_add(1);
            transitions.push(TransitionPublicationWork {
                candidate,
                tiles: BTreeMap::new(),
                parent_tile,
                child_tiles,
                outgoing_boundaries: Arc::clone(&self.boundaries),
                group_id: self.next_group,
            });
            selected_zones.push(zone);
            self.publication_stages.publication_payload_collection_ms +=
                payload_started.elapsed().as_secs_f64() * 1000.0;
        }
        if transitions.is_empty() {
            return Ok(());
        }
        let dispatch_started = std::time::Instant::now();
        let mut keep_alive = vec![Arc::clone(&self.boundaries)];
        for pending in &self.prepared_publications {
            for map in [&pending.expected, &pending.prepared.target] {
                if !keep_alive.iter().any(|existing| Arc::ptr_eq(existing, map)) {
                    keep_alive.push(Arc::clone(map));
                }
            }
        }
        let token = PublicationToken {
            trace: self.core.as_ref().map(RegionalTerrain::trace),
            revision: self.topology_revision,
            kind: PublicationKind::Batch {
                overlays: self.boundary_overlays.keys().copied().collect(),
            },
            keys,
            dependencies: Arc::from(dependencies.into_iter().collect::<Vec<_>>()),
            retained_tile_bytes,
        };
        let count = transitions.len() as u64;
        let overlays = {
            let _span = engine_profile::span("Publication overlay snapshot");
            self.boundary_overlays.clone()
        };
        let work = PublicationWork::Batch(Box::new(BatchPublicationWork {
            base: Arc::clone(&self.boundaries),
            overlays,
            transitions,
            keep_alive,
        }));
        if self
            .publication_worker
            .as_mut()
            .context("regional publication worker unavailable")?
            .submit(work, token)?
        {
            self.publication_builds += count;
            self.publication_last_state = "batch_preparing";
        }
        self.publication_stages.publication_dispatch_ms +=
            dispatch_started.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    fn adopt_prepared_publications(&mut self) -> Result<()> {
        let _span = engine_profile::span("Adoption");
        let started = std::time::Instant::now();
        let result = self.adopt_prepared_publications_inner(started);
        self.publication_completion_ms += started.elapsed().as_secs_f64() * 1000.0;
        self.record_publication_admission(started.elapsed());
        result
    }

    fn adopt_prepared_publications_inner(&mut self, started: std::time::Instant) -> Result<()> {
        let core = self.core.as_ref().context("regional core unavailable")?;
        self.prepared_publications
            .make_contiguous()
            .sort_by(|a, b| {
                // Age protection overrides priority after five seconds, with stable
                // group identity resolving equal visual contribution deterministically.
                let a_old = a.ready_at.elapsed().as_secs_f64() >= 5.0;
                let b_old = b.ready_at.elapsed().as_secs_f64() >= 5.0;
                b_old
                    .cmp(&a_old)
                    .then_with(|| {
                        if a_old && b_old {
                            a.ready_at.cmp(&b.ready_at)
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
                    .then_with(|| {
                        core.publication_priority(&b.candidate)
                            .total_cmp(&core.publication_priority(&a.candidate))
                    })
                    .then(a.prepared.group_id.cmp(&b.prepared.group_id))
            });
        let mut index = 0;
        while index < self.prepared_publications.len() {
            let pending = &self.prepared_publications[index];
            self.publication_max_backlog_age_ms = self
                .publication_max_backlog_age_ms
                .max(pending.ready_at.elapsed().as_secs_f64() * 1000.0);
            let dependency_started = std::time::Instant::now();
            let split_conflict = self.groups.iter().any(|group| {
                group.merge.is_none()
                    && pending
                        .prepared
                        .affected
                        .iter()
                        .any(|address| group.affected.contains(address))
            });
            let merge_conflict = self.groups.iter().any(|group| {
                group.merge.is_some()
                    && pending
                        .prepared
                        .affected
                        .iter()
                        .any(|address| group.affected.contains(address))
            });
            if split_conflict || merge_conflict {
                if let Some(core) = &self.core {
                    core.trace().block(
                        publication_parent(&pending.candidate),
                        if merge_conflict {
                            BlockReason::ActiveMergeOverlap
                        } else {
                            BlockReason::ActiveSplitOverlap
                        },
                    );
                }
                self.blocked_by_split += usize::from(split_conflict);
                self.blocked_by_merge += usize::from(merge_conflict);
                self.publication_stages.publication_dependency_checks_ms +=
                    dependency_started.elapsed().as_secs_f64() * 1000.0;
                index += 1;
                continue;
            }
            let keys_current = pending.keys.iter().all(|(address, key)| {
                self.core
                    .as_ref()
                    .and_then(|core| core.tile(*address))
                    .is_some_and(|tile| tile.key == *key)
            });
            let local_current = self
                .core
                .as_ref()
                .is_some_and(|core| core.publication_locally_current(&pending.candidate));
            let boundaries_current =
                pending.prepared.affected.iter().all(|address| {
                    self.logical_boundary(*address) == pending.expected.get(address)
                });
            self.publication_stages.publication_dependency_checks_ms +=
                dependency_started.elapsed().as_secs_f64() * 1000.0;
            if !keys_current || !local_current || !boundaries_current {
                if let Some(core) = &self.core {
                    core.trace().block(
                        publication_parent(&pending.candidate),
                        if !keys_current {
                            BlockReason::RevisionMismatch
                        } else {
                            BlockReason::StaleResult
                        },
                    );
                    core.trace().event(
                        publication_parent(&pending.candidate),
                        Stage::DiscardedStale,
                    );
                }
                self.publication_candidate_age
                    .remove(&publication_parent(&pending.candidate));
                self.prepared_publications.remove(index);
                self.publication_stale += 1;
                self.publication_last_state = "stale_local_product_discarded";
                continue;
            }
            if !self.keys_and_slots_are_current(&pending.keys) {
                if let Some(core) = &self.core {
                    core.trace().block(
                        publication_parent(&pending.candidate),
                        BlockReason::ResidentSlotUnavailable,
                    );
                }
                self.blocked_by_residency += 1;
                index += 1;
                continue;
            }
            self.immediately_publishable += 1;
            if let Some(core) = &self.core {
                let trace = core.trace();
                trace.unblock(publication_parent(&pending.candidate));
                trace.event(publication_parent(&pending.candidate), Stage::Publishable);
            }
            if self.groups.len()
                >= self
                    .settings
                    .map_or(1, |settings| settings.transition_limit)
                || self.publication_commits_this_frame
                    >= self
                        .settings
                        .map_or(1, |settings| settings.publication_groups_per_frame)
                || self.publication_admission_ms + started.elapsed().as_secs_f64() * 1000.0 >= 1.5
            {
                if let Some(core) = &self.core {
                    let reason = if self.groups.len()
                        >= self
                            .settings
                            .map_or(1, |settings| settings.transition_limit)
                    {
                        BlockReason::TransitionCapacity
                    } else {
                        BlockReason::PublicationBudgetExhausted
                    };
                    core.trace()
                        .block(publication_parent(&pending.candidate), reason);
                }
                index += 1;
                continue;
            }
            let pending = self
                .prepared_publications
                .remove(index)
                .context("prepared publication missing")?;
            self.publication_candidate_age
                .remove(&publication_parent(&pending.candidate));
            self.commit_prepared_transition(pending.candidate, pending.prepared)?;
            self.adoption_completed += 1;
            self.immediately_publishable = self.immediately_publishable.saturating_sub(1);
            self.publication_commits_this_frame += 1;
        }
        Ok(())
    }

    fn schedule_transition_inner(&mut self, started: std::time::Instant) -> Result<()> {
        let already_admitted_ms = self.publication_admission_ms;
        if already_admitted_ms + started.elapsed().as_secs_f64() * 1000.0 >= 2.0 {
            self.publication_last_state = "deferred_publication_budget";
            return Ok(());
        }
        if self
            .publication_worker
            .as_ref()
            .is_none_or(|worker| worker.in_flight.is_some())
            || self.groups.len()
                >= self
                    .settings
                    .context("regional settings unavailable")?
                    .transition_limit
            // A merge keeps children drawable until its transition ends while
            // the target boundary map already contains the parent. Wait for
            // that cover handoff before preparing another global boundary map.
            || self.groups.iter().any(|group| group.merge.is_some())
        {
            return Ok(());
        }
        let discovery_started = std::time::Instant::now();
        let candidates = self
            .core
            .as_ref()
            .context("regional core unavailable")?
            .publication_candidates();
        self.publication_stages.publication_candidate_discovery_ms +=
            discovery_started.elapsed().as_secs_f64() * 1000.0;
        let Some(candidate) = candidates.into_iter().next() else {
            self.publication_last_state = "idle";
            return Ok(());
        };
        let (parent, children, merging) = match &candidate {
            RegionalPublication::Split {
                parent, children, ..
            } => (*parent, *children, false),
            RegionalPublication::Merge {
                parent, children, ..
            } => (*parent, *children, true),
        };
        let dependency_started = std::time::Instant::now();
        if self.conflicts_with_active_group(&candidate) {
            self.topology_deferred = self.topology_deferred.saturating_add(1);
            self.publication_last_state = "deferred_local_transition_conflict";
            return Ok(());
        }
        let core = self.core.as_ref().context("regional core unavailable")?;
        // The scheduler candidate carries a previously validated balanced cover.
        // Existing outgoing patches remain pinned while the worker runs; only
        // the replaced parent/children need an exact residency preflight here.
        if std::iter::once(parent)
            .chain(children)
            .any(|address| self.slot_for(address).is_none())
        {
            self.publication_last_state = "deferred_missing_residency";
            return Ok(());
        }
        let mut seen_tiles = HashSet::new();
        let mut retained_tile_bytes = 0usize;
        let parent_tile = core
            .tile(parent)
            .context("publication parent tile missing")?;
        let mut child_tiles = BTreeMap::new();
        for child in children {
            let Some(tile) = core.tile(child) else {
                self.publication_last_state = "deferred_missing_child_tile";
                return Ok(());
            };
            if seen_tiles.insert(Arc::as_ptr(&tile)) {
                retained_tile_bytes =
                    retained_tile_bytes.saturating_add(tile.retained_payload_bytes());
            }
            child_tiles.insert(child, tile);
        }
        if seen_tiles.insert(Arc::as_ptr(&parent_tile)) {
            retained_tile_bytes =
                retained_tile_bytes.saturating_add(parent_tile.retained_payload_bytes());
        }
        let mut keys = BTreeMap::new();
        keys.insert(parent, parent_tile.key.clone());
        for (child, tile) in &child_tiles {
            keys.insert(*child, tile.key.clone());
        }
        self.publication_stages.publication_dependency_checks_ms +=
            dependency_started.elapsed().as_secs_f64() * 1000.0;
        if already_admitted_ms + started.elapsed().as_secs_f64() * 1000.0 >= 2.0 {
            self.publication_last_state = "deferred_publication_budget";
            return Ok(());
        }
        let mut dependencies = vec![parent];
        dependencies.extend(children);
        dependencies.sort();
        dependencies.dedup();
        let token = PublicationToken {
            trace: self.core.as_ref().map(RegionalTerrain::trace),
            revision: self.topology_revision,
            kind: PublicationKind::Transition {
                parent,
                children,
                merging,
            },
            keys,
            dependencies: Arc::from(dependencies),
            retained_tile_bytes,
        };
        let work = PublicationWork::Transition(Box::new(TransitionPublicationWork {
            candidate,
            tiles: BTreeMap::new(),
            parent_tile,
            child_tiles,
            outgoing_boundaries: Arc::clone(&self.boundaries),
            group_id: self.next_group.saturating_add(1),
        }));
        let submitted = self
            .publication_worker
            .as_mut()
            .context("regional publication worker unavailable")?
            .submit(work, token)?;
        if submitted {
            self.publication_builds = self.publication_builds.saturating_add(1);
            self.publication_last_state = "transition_building";
        }
        Ok(())
    }

    fn finish_publication_worker(&mut self) -> Result<bool> {
        let _span = engine_profile::span("Boundary completion drain");
        let started = std::time::Instant::now();
        let result = self.finish_publication_worker_inner();
        self.publication_completion_ms = started.elapsed().as_secs_f64() * 1000.0;
        self.record_publication_admission(started.elapsed());
        result
    }

    fn finish_publication_worker_inner(&mut self) -> Result<bool> {
        let Some(completion) = self
            .publication_worker
            .as_mut()
            .and_then(PublicationWorker::take_completion)
        else {
            return Ok(false);
        };
        self.publication_build_ms = completion.elapsed.as_secs_f64() * 1000.0;
        self.publication_stages.boundary_preparation_background_ms = self.publication_build_ms;
        self.publication_last_state = "completion_ready";
        let prepared = match completion.prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                self.publication_failures = self.publication_failures.saturating_add(1);
                self.publication_last_state = "build_failed";
                self.record(serde_json::json!({"event":"publication_build_failed","error":error,"frame":self.frame}));
                return Ok(true);
            }
        };
        match (completion.token.kind, prepared) {
            (
                PublicationKind::Batch { overlays },
                PreparedPublication::Batch { base, transitions },
            ) => {
                self.boundaries = Arc::clone(&base);
                for address in overlays {
                    self.boundary_overlays.remove(&address);
                }
                let ready_at = std::time::Instant::now();
                self.boundary_completed += transitions.len() as u64;
                for (candidate, prepared) in transitions {
                    let parent = publication_parent(&candidate);
                    let children = parent.children()?;
                    let keys = std::iter::once(parent)
                        .chain(children)
                        .filter_map(|address| {
                            completion
                                .token
                                .keys
                                .get(&address)
                                .map(|key| (address, key.clone()))
                        })
                        .collect();
                    let ready_at = self
                        .publication_candidate_age
                        .get(&parent)
                        .copied()
                        .unwrap_or(ready_at);
                    self.prepared_publications.push_back(PendingPublication {
                        candidate,
                        prepared,
                        expected: Arc::clone(&base),
                        keys,
                        ready_at,
                    });
                }
                self.publication_last_state = "batch_prepared";
                Ok(true)
            }
            (PublicationKind::Bootstrap { cover }, PreparedPublication::Bootstrap(target)) => {
                let core = self.core.as_ref().context("regional core unavailable")?;
                let dependency_started = std::time::Instant::now();
                let valid = completion.token.revision == self.topology_revision
                    && !self.bootstrap_published
                    && core.drawable() == cover.as_slice()
                    && self.keys_and_slots_are_current(&completion.token.keys);
                self.publication_stages.publication_dependency_checks_ms +=
                    dependency_started.elapsed().as_secs_f64() * 1000.0;
                if !valid {
                    self.publication_stale = self.publication_stale.saturating_add(1);
                    self.publication_last_state = "stale_bootstrap_discarded";
                    return Ok(true);
                }
                let mut patches = BTreeMap::new();
                for address in &cover {
                    let boundary = target
                        .get(address)
                        .context("bootstrap boundary missing")?
                        .clone();
                    self.next_group = self.next_group.saturating_add(1);
                    patches.insert(
                        *address,
                        Patch {
                            parent: *address,
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
                self.patches = patches;
                self.boundaries = target;
                self.bootstrap_published = true;
                self.topology_revision = self.topology_revision.saturating_add(1);
                self.publication_last_state = "bootstrap_published";
                Ok(true)
            }
            (
                PublicationKind::Transition {
                    parent,
                    children,
                    merging,
                },
                PreparedPublication::Transition(mut prepared),
            ) => {
                let cover = std::mem::take(&mut prepared.cover);
                let candidate = if merging {
                    RegionalPublication::Merge {
                        parent,
                        children,
                        cover,
                    }
                } else {
                    RegionalPublication::Split {
                        parent,
                        children,
                        cover,
                    }
                };
                let dependency_started = std::time::Instant::now();
                let valid = completion.token.revision == self.topology_revision
                    && prepared.group_id == self.next_group.saturating_add(1)
                    && self.keys_and_slots_are_current(&completion.token.keys)
                    && self
                        .core
                        .as_ref()
                        .is_some_and(|core| core.is_current_publication(&candidate));
                self.publication_stages.publication_dependency_checks_ms +=
                    dependency_started.elapsed().as_secs_f64() * 1000.0;
                if !valid {
                    self.publication_stale = self.publication_stale.saturating_add(1);
                    self.publication_last_state = "stale_transition_discarded";
                    return Ok(true);
                }
                self.commit_prepared_transition(candidate, prepared)?;
                Ok(true)
            }
            _ => anyhow::bail!("publication worker returned mismatched prepared result"),
        }
    }

    fn keys_and_slots_are_current(&self, keys: &BTreeMap<CubePatchAddress, TileKey>) -> bool {
        let Some(core) = &self.core else { return false };
        keys.iter().all(|(address, expected)| {
            core.tile(*address)
                .is_some_and(|tile| tile.key == *expected)
                && self.slot_for(*address).is_some_and(|index| {
                    self.slots[index]
                        .as_ref()
                        .is_some_and(|slot| slot.tile.key == *expected)
                        && self
                            .last_report
                            .slots
                            .get(index)
                            .and_then(|slot| slot.key.as_ref())
                            == Some(expected)
                })
        })
    }

    fn conflicts_with_active_group(&self, candidate: &RegionalPublication) -> bool {
        if self.groups.is_empty() {
            return false;
        }
        let parent = match candidate {
            RegionalPublication::Split { parent, .. }
            | RegionalPublication::Merge { parent, .. } => *parent,
        };
        // Expand through edge neighbors twice: the first ring covers changed
        // shared edges, and the second catches tiles that meet the region only
        // at a cube-chart corner.
        let mut zone = BTreeSet::from([parent]);
        let mut frontier = vec![parent];
        for _ in 0..2 {
            let mut next = Vec::new();
            for address in frontier {
                for edge in PatchEdge::ALL {
                    let neighbor = address.neighbor(edge).address;
                    if zone.insert(neighbor) {
                        next.push(neighbor);
                    }
                }
            }
            frontier = next;
        }
        self.groups.iter().any(|group| {
            group.affected.iter().any(|affected| {
                zone.iter()
                    .any(|region| region.contains(*affected) || affected.contains(*region))
            })
        })
    }

    fn commit_prepared_transition(
        &mut self,
        candidate: RegionalPublication,
        prepared: PreparedTransition,
    ) -> Result<()> {
        let _span = engine_profile::span("Resident state update");
        let mutation_started = std::time::Instant::now();
        let (parent, children, merging) = match &candidate {
            RegionalPublication::Split {
                parent, children, ..
            } => (*parent, *children, false),
            RegionalPublication::Merge {
                parent, children, ..
            } => (*parent, *children, true),
        };
        if let Some(core) = &self.core {
            let trace = core.trace();
            for address in std::iter::once(parent).chain(children) {
                trace.unblock(address);
                trace.event(address, Stage::PublicationStarted);
            }
        }
        let PreparedTransition {
            cover: _,
            target,
            affected,
            staged,
            group_id: id,
            completion_parent,
        } = prepared;
        if !merging {
            let core = self.core.as_mut().context("regional core missing")?;
            if self.planetary {
                core.ack_local_publication(&candidate)?;
            } else {
                core.ack_publication(&candidate)?;
            }
            self.patches.remove(&parent);
        }
        self.patches.extend(staged);
        if self.planetary {
            for address in &affected {
                self.boundary_overlays
                    .insert(*address, target.get(address).cloned());
            }
        } else {
            self.boundaries = target;
            self.next_group = id;
        }
        self.topology_revision = self.topology_revision.saturating_add(1);
        self.groups.push(Group {
            id,
            fraction: 0.0,
            affected,
            merge: merging.then_some((parent, children)),
            completion_parent,
        });
        self.record(serde_json::json!({"event":if merging{"merge"}else{"split"},"group":id,"parent":format!("{parent:?}"),"frame":self.frame}));
        self.publication_last_state = "transition_published";
        if let Some(core) = &self.core {
            let trace = core.trace();
            for address in std::iter::once(parent).chain(children) {
                trace.event(address, Stage::PublicationFinished);
                trace.event(address, Stage::TransitionStarted);
            }
        }
        self.publication_stages.publication_state_mutation_ms +=
            mutation_started.elapsed().as_secs_f64() * 1000.0;
        Ok(())
    }

    fn advance_groups(&mut self, dt: Duration) -> Result<usize> {
        let _span = engine_profile::span("Transition advance");
        let started = std::time::Instant::now();
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
            if self.planetary
                && (self.publication_commits_this_frame
                    >= self
                        .settings
                        .map_or(1, |settings| settings.publication_groups_per_frame)
                    || self.publication_admission_ms + started.elapsed().as_secs_f64() * 1000.0
                        >= 1.5)
            {
                break;
            }
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
                }
                self.patches.insert(
                    parent,
                    Patch {
                        parent,
                        quadrant: None,
                        endpoints: group
                            .completion_parent
                            .context("merge completion endpoints missing")?,
                        group: None,
                        merging: false,
                    },
                );
                let core = self.core.as_mut().context("regional core missing")?;
                if self.planetary {
                    core.ack_prepared_merge(parent, children)?;
                } else {
                    let cover: Vec<_> = self.patches.keys().copied().collect();
                    core.ack_local_merge(parent, children, &cover)?;
                }
                merged += 1;
                self.publication_commits_this_frame += 1;
            }
            for address in group.affected {
                if let Some(core) = &self.core {
                    core.trace().event(address, Stage::TransitionCompleted);
                }
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
        if merged > 0 {
            self.topology_revision = self.topology_revision.saturating_add(1);
        }
        Ok(merged)
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

    fn scan_available_slot(
        &mut self,
        pins: &BTreeSet<CubePatchAddress>,
        uploads: &[RegionalTileUpload],
        capacity: usize,
    ) -> Option<usize> {
        if capacity == 0 {
            return None;
        }
        let mut oldest = None;
        // A full pool never turns one upload into an unbounded frame scan.
        // Deferred uploads remain queued; the cursor advances on the next frame.
        for _ in 0..capacity.min(256) {
            let index = self.slot_scan_cursor % capacity;
            self.slot_scan_cursor = (index + 1) % capacity;
            if uploads.iter().any(|upload| upload.slot == index) {
                continue;
            }
            let report = self.last_report.slots.get(index);
            match self.slots.get(index).and_then(Option::as_ref) {
                None if report.is_none_or(|report| report.key.is_none() || report.reuse_safe) => {
                    return Some(index);
                }
                Some(slot)
                    if report.is_some_and(|report| report.reuse_safe)
                        && (!pins.contains(&slot.tile.key.address)
                            || self.core.as_ref().is_none_or(|core| {
                                core.tile(slot.tile.key.address)
                                    .is_none_or(|tile| tile.key != slot.tile.key)
                            }))
                        && oldest.is_none_or(|(_, age)| slot.last_use < age) =>
                {
                    oldest = Some((index, slot.last_use));
                }
                _ => {}
            }
        }
        oldest.map(|(index, _)| index)
    }

    pub fn draw(&mut self, root_draw: &TileDraw) -> Result<RegionalResidentDraw> {
        let _span = engine_profile::span("Drawable preparation");
        let draw_started = std::time::Instant::now();
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
            if let Some(index) = if self.planetary {
                self.slot_by_key.get(&tile.key).copied()
            } else {
                self.slots
                    .iter()
                    .take(settings.gpu_slots)
                    .position(|s| s.as_ref().is_some_and(|s| s.tile.key == tile.key))
            } {
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
            let free = if self.planetary {
                self.scan_available_slot(&pins, &uploads, settings.gpu_slots)
            } else {
                self.slots
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
                    })
            };
            let Some(index) = free else {
                self.slot_pressure = true;
                break;
            };
            let mut state = self.slots[index]
                .as_ref()
                .map_or_else(TileSlotState::default, |s| s.state.clone());
            let token = state.request(&tile.key)?;
            if let Some(old) = self.slots[index].as_ref() {
                if self.planetary && self.slot_by_key.get(&old.tile.key) == Some(&index) {
                    self.slot_by_key.remove(&old.tile.key);
                }
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
            if self.planetary {
                self.slot_by_key.insert(tile.key.clone(), index);
            } else {
                self.rebuild_slot_index();
            }
            uploads.push(RegionalTileUpload {
                slot: index,
                tile: self.tile_draw(tile.key.address, root_draw)?,
            });
            pins.insert(tile.key.address);
        }
        self.publication_stages.resident_slot_allocation_ms =
            draw_started.elapsed().as_secs_f64() * 1000.0;
        let mut desired_ancestors = HashSet::new();
        if let Some(core) = &self.core {
            for desired in core.desired() {
                let mut ancestor = desired.parent();
                while let Some(address) = ancestor {
                    desired_ancestors.insert(address);
                    ancestor = address.parent();
                }
            }
        }
        let mut patches = Vec::new();
        for (address, patch) in &self.patches {
            if self.planetary
                && self.planetary_projection.is_some()
                && !self
                    .core
                    .as_ref()
                    .is_some_and(|core| core.patch_visible(*address))
            {
                continue;
            }
            let own_slot = self.slot_for(*address).context("draw slot missing")?;
            let parent_slot = self
                .slot_for(patch.parent)
                .context("parent reconstruction slot missing")?;
            let t = patch
                .group
                .and_then(|id| self.groups.iter().find(|g| g.id == id).map(|g| g.fraction))
                .unwrap_or(1.0);
            patches.push(RegionalPatchDraw {
                quality_fallback: desired_ancestors.contains(address),
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
        self.publication_stages.resident_draw_preparation_ms = draw_started.elapsed().as_secs_f64()
            * 1000.0
            - self.publication_stages.resident_slot_allocation_ms;
        Ok(RegionalResidentDraw {
            capacity: settings.gpu_slots,
            cells: root_draw.tile.key.cells,
            planetary: self.planetary,
            uploads,
            patches,
        })
    }

    pub(super) fn trace(&self) -> Option<Arc<TerrainTrace>> {
        self.core.as_ref().map(RegionalTerrain::trace)
    }

    #[cfg(any(test, feature = "developer-tools"))]
    pub fn snapshot(&self) -> Option<serde_json::Value> {
        self.snapshot_impl(true)
            .and_then(|snapshot| serde_json::to_value(snapshot).ok())
    }

    pub fn snapshot_frame(&self) -> Option<ResidentDiagnosticSnapshot> {
        self.snapshot_impl(false)
    }

    fn snapshot_impl(&self, include_history_values: bool) -> Option<ResidentDiagnosticSnapshot> {
        if !self.enabled {
            return None;
        }
        let core = self.core.as_ref()?;
        let _span = engine_profile::span("Terrain diagnostic snapshot");
        let drawable_addresses: HashSet<_> = core.drawable().iter().copied().collect();
        let core_snapshot = if self.planetary {
            core.snapshot_summary()
        } else {
            core.snapshot()
        };
        let selection_micros = core_snapshot.selection_time_micros;
        let cached_bytes = core_snapshot.cpu_cached_bytes;
        let boundary_table_bytes = boundary_table_retained_bytes(&self.boundaries);
        let endpoint_bytes = endpoint_tables_retained_bytes(&self.patches);
        let worker_cache_bytes = self.publication_worker.as_ref().map_or(0, |worker| {
            worker.boundary_cache_bytes.load(Ordering::Acquire)
        });
        let renderer_cache_bytes = self.last_report.metadata_cpu_capacity_bytes
            + self.last_report.boundary_cpu_capacity_bytes;
        let worker_boundary_workspace_bound_bytes = self.settings.map_or(0, |_| {
            boundary_workspace_bound_bytes(core.config().max_desired_patches, core.config().cells)
        });
        let mut snapshot = serde_json::to_value(&core_snapshot).ok()?;
        let object = snapshot.as_object_mut()?;
        let mut terrain_trace = None;
        if engine_profile::is_enabled() {
            let mut cache = self.trace_snapshot_cache.borrow_mut();
            let trace = core.trace();
            let identity = Arc::as_ptr(&trace) as usize;
            if cache.identity != identity {
                *cache = TraceExport {
                    identity,
                    ..TraceExport::default()
                };
            }
            if let Some(value) = cache.sampler.poll() {
                cache.value = Some(Arc::new(value));
            }
            if cache
                .sampled_at
                .is_none_or(|at| at.elapsed() >= Duration::from_millis(250))
            {
                cache.sampler.request(trace);
                cache.sampled_at = Some(std::time::Instant::now());
            }
            // Share the sampled JSON until serialization. Copying this value
            // into the per-frame summary cloned the entire trace tree each
            // frame even though new samples arrive only every 250 ms.
            terrain_trace = cache.value.clone();
        }
        object.insert("gpu_preparation_stages".into(), serde_json::json!({
            "cached_endpoint_validation_ms": self.last_report.cached_endpoint_validation_micros.map(|v| v as f64 / 1000.0),
            "proposed_slot_state_ms": self.last_report.proposed_slot_state_micros.map(|v| v as f64 / 1000.0),
            "slot_dependency_check_ms": self.last_report.slot_dependency_check_micros.map(|v| v as f64 / 1000.0),
            "metadata_pack_queue_write_ms": self.last_report.metadata_pack_queue_write_micros.map(|v| v as f64 / 1000.0),
            "draw_retention_ms": self.last_report.draw_retention_micros.map(|v| v as f64 / 1000.0),
            "report_assembly_ms": self.last_report.report_assembly_micros.map(|v| v as f64 / 1000.0),
            "patch_count": self.last_report.prepare_patch_count,
            "upload_count": self.last_report.prepare_upload_count,
            "distinct_slot_count": self.last_report.prepare_distinct_slot_count,
            "endpoint_cache_hits": self.last_report.endpoint_validation_cache_hits,
            "endpoint_cache_misses": self.last_report.endpoint_validation_cache_misses,
            "validation_dependency_ms": self.last_report.validation_dependency_micros.map(|v| v as f64 / 1000.0),
            "resource_allocation_ms": self.last_report.resource_allocation_micros.map(|v| v as f64 / 1000.0),
            "gpu_upload_preparation_ms": self.last_report.gpu_upload_preparation_micros.map(|v| v as f64 / 1000.0),
            "drawable_metadata_ms": self.last_report.drawable_metadata_micros.map(|v| v as f64 / 1000.0),
        }));
        let worker = self.publication_worker.as_ref();
        let active = worker.map_or(0, |worker| worker.active.load(Ordering::Acquire));
        let completed = worker.map_or(0, |worker| worker.completion_ready.load(Ordering::Acquire));
        let building = worker.is_some_and(|worker| worker.in_flight.is_some());
        let preparing: HashSet<_> = worker
            .and_then(|worker| worker.in_flight.as_ref())
            .map_or_else(HashSet::new, |work| {
                work.dependencies.iter().copied().collect()
            });
        let prepared: HashSet<_> = self
            .prepared_publications
            .iter()
            .flat_map(|pending| pending.keys.keys().copied())
            .collect();
        let generated_waiting: HashSet<_> = if self.planetary {
            core.publication_local_candidates()
                .iter()
                .flat_map(|candidate| {
                    let parent = publication_parent(candidate);
                    std::iter::once(parent).chain(parent.children().into_iter().flatten())
                })
                .filter(|address| !preparing.contains(address) && !prepared.contains(address))
                .collect()
        } else {
            HashSet::new()
        };
        object.insert("publication_pipeline".into(), serde_json::json!({
            "frontier_discovery_ms": core.frontier_discovery_time_micros() as f64 / 1000.0,
            "maximum_candidate_age_ticks": core.publication_local_candidates().iter()
                .map(|candidate| core.publication_candidate_age_ticks(candidate)).max().unwrap_or(0),
            "generation_queued": core_snapshot.worker_queued,
            "generation_active": core_snapshot.worker_running,
            "generated_awaiting_boundary_preparation": generated_waiting.len(),
            "generated_waiting_scope": "unique tiles of resident-ready local replacement proposals; historical cache and deeper future cover excluded; upload backlog reported separately",
            "boundary_preparation_queued_batches": usize::from(building && active == 0 && completed == 0),
            "boundary_preparation_active_batches": active,
            "boundary_preparation_completed_batches_awaiting_drain": completed,
            "boundary_parallel_task_limit": if self.planetary { 2 } else { 1 },
            "prepared_result_capacity": 8,
            "fully_prepared_results": self.prepared_publications.len(),
            "immediately_publishable_results": self.immediately_publishable,
            "blocked_by_split": self.blocked_by_split,
            "blocked_by_merge": self.blocked_by_merge,
            "blocked_by_neighbor_dependency": self.blocked_by_dependency,
            "blocked_by_residency_slot": self.blocked_by_residency,
            "publication_backlog_age_ms": self.publication_backlog_age_ms(),
            "maximum_publication_backlog_age_ms": self.publication_max_backlog_age_ms,
            "boundary_preparation_completed_groups": self.boundary_completed,
            "publication_adopted_groups": self.adoption_completed,
            "count_scope": "replacement groups except generation/generated-waiting tile counts; split adopts four children, merge adopts one parent; completed transitions remain separately visible",
        }));
        if let Ok(serde_json::Value::Object(stages)) =
            serde_json::to_value(&self.publication_stages)
        {
            object.extend(stages);
        }
        if self.planetary {
            for field in ["desired", "resident", "drawable"] {
                object.remove(field);
            }
            object.insert("detail_scope".into(), serde_json::json!("current scalar coverage and job evidence; per-tile arrays omitted from planetary frame snapshots; finite fixtures retain detailed arrays"));
        }
        object.insert(
            "active_transitions".into(),
            serde_json::json!(self.groups.len()),
        );
        object.insert(
            "quality_pending".into(),
            serde_json::json!(
                !self.groups.is_empty()
                    || !self.prepared_publications.is_empty()
                    || self
                        .publication_worker
                        .as_ref()
                        .is_some_and(|worker| worker.in_flight.is_some())
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
            "publication_worker_state".into(),
            serde_json::json!(self.publication_last_state),
        );
        object.insert(
            "publication_worker_in_flight".into(),
            serde_json::json!(
                self.publication_worker
                    .as_ref()
                    .is_some_and(|worker| worker.in_flight.is_some())
            ),
        );
        object.insert(
            "publication_worker_completion_capacity".into(),
            serde_json::json!(1),
        );
        object.insert(
            "publication_worker_boundary_workspace_bound_bytes".into(),
            serde_json::json!(worker_boundary_workspace_bound_bytes),
        );
        object.insert(
            "publication_build_ms".into(),
            serde_json::json!(self.publication_build_ms),
        );
        object.insert(
            "publication_admission_ms".into(),
            serde_json::json!(self.publication_admission_ms),
        );
        object.insert(
            "publication_admission_total_ms".into(),
            serde_json::json!(self.publication_admission_total_ms),
        );
        object.insert(
            "publication_builds".into(),
            serde_json::json!(self.publication_builds),
        );
        object.insert(
            "publication_stale_completions".into(),
            serde_json::json!(self.publication_stale),
        );
        object.insert(
            "publication_budget_overruns".into(),
            serde_json::json!(self.publication_budget_overruns),
        );
        object.insert(
            "publication_failures".into(),
            serde_json::json!(self.publication_failures),
        );
        object.insert(
            "publication_admission_budget_ms".into(),
            serde_json::json!(2.0),
        );
        object.insert(
            "gpu_resident_tiles".into(),
            serde_json::json!(self.last_report.resident_count),
        );
        object.insert(
            "visible_drawable_count".into(),
            serde_json::json!(if self.planetary {
                self.patches
                    .keys()
                    .filter(|address| core.patch_visible(**address))
                    .count()
            } else {
                self.patches.len()
            }),
        );
        object.insert(
            "gpu_fallback_count".into(),
            serde_json::json!(
                core.desired()
                    .iter()
                    .filter(|desired| {
                        let mut ancestor = desired.parent();
                        while let Some(address) = ancestor {
                            if drawable_addresses.contains(&address) {
                                return true;
                            }
                            ancestor = address.parent();
                        }
                        false
                    })
                    .count()
            ),
        );
        object.insert(
            "gpu_preparation_ms".into(),
            serde_json::json!(self.last_report.preparation_micros as f64 / 1000.0),
        );
        object.insert(
            "selector_ms".into(),
            serde_json::json!(selection_micros as f64 / 1000.0),
        );
        if include_history_values || !self.planetary {
            object.insert(
                "frame_intervals_ms".into(),
                serde_json::json!(if self.planetary {
                    self.frame_intervals_ms
                        .iter()
                        .rev()
                        .take(256)
                        .copied()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                } else {
                    self.frame_intervals_ms.iter().copied().collect::<Vec<_>>()
                }),
            );
        }
        object.insert(
            "frame_interval_scope".into(),
            serde_json::json!("native_wall_elapsed; offscreen excluded"),
        );
        if include_history_values || !self.planetary {
            object.insert(
                "native_frame_samples".into(),
                serde_json::json!(if self.planetary {
                    self.native_frame_samples
                        .iter()
                        .rev()
                        .take(256)
                        .cloned()
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect::<Vec<_>>()
                } else {
                    self.native_frame_samples
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                }),
            );
        }
        object.insert("native_frame_sample_scope".into(), serde_json::json!("retains up to 8192 ordinary native frames internally and exports the newest 256 in planetary mode; CPU elapsed stages, GPU latest_completed with source frame; capture-free scenario required for distributions"));
        if include_history_values || !self.planetary {
            object.insert(
                "events".into(),
                serde_json::json!(if self.planetary {
                    self.events
                        .iter()
                        .rev()
                        .take(256)
                        .rev()
                        .cloned()
                        .collect::<Vec<_>>()
                } else {
                    self.events.clone()
                }),
            );
        }
        object.insert(
            "retained_event_count".into(),
            serde_json::json!(self.events.len()),
        );
        object.insert(
            "event_scope".into(),
            serde_json::json!(if self.planetary {
                "newest 256 of up to 4096 retained events, in chronological order"
            } else {
                "all retained events, in chronological order"
            }),
        );
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
        object.insert("gpu_slots".into(),serde_json::json!(self.last_report.slots.iter().enumerate().filter(|(_,slot)| slot.key.is_some()).take(if self.planetary { 256 } else { usize::MAX }).map(|(i,s)|serde_json::json!({"slot":i,"key":s.key.as_ref().map(|k| if self.planetary {format!("body:{};surface:{};material:{};address:{:?};cells:{}",k.body_identity,k.surface_revision,k.material_revision,k.address,k.cells)} else {format!("{k:?}")}),"address":s.key.as_ref().map(|k|format!("{:?}",k.address)),"generation":s.generation,"reuse_safe":s.reuse_safe,"pinned":s.pinned,"in_flight":s.in_flight})).collect::<Vec<_>>()));
        object.insert("terrain_attributable_waits".into(), serde_json::json!(0));
        object.insert("wait_counter_scope".into(), serde_json::json!("explicit regional worker joins and GPU waits; excludes driver/present and offscreen capture readback"));
        object.insert("ready".into(), serde_json::json!(!self.patches.is_empty()));
        object.insert(
            "has_coverage".into(),
            serde_json::json!(self.has_coverage()),
        );
        object.insert(
            "gpu_upload_bytes_per_frame".into(),
            serde_json::json!(self.last_report.tile_upload_bytes),
        );
        object.insert(
            "gpu_upload_tiles_per_frame".into(),
            serde_json::json!(self.last_report.tile_upload_count),
        );
        object.insert(
            "gpu_cumulative_tile_upload_bytes".into(),
            serde_json::json!(self.last_report.cumulative_tile_upload_bytes),
        );
        object.insert(
            "gpu_cumulative_boundary_upload_bytes".into(),
            serde_json::json!(self.last_report.cumulative_boundary_upload_bytes),
        );
        object.insert(
            "gpu_cumulative_tile_upload_count".into(),
            serde_json::json!(self.last_report.cumulative_tile_upload_count),
        );
        object.insert(
            "gpu_cumulative_boundary_upload_count".into(),
            serde_json::json!(self.last_report.cumulative_boundary_upload_count),
        );
        let latest_native = self.native_frame_samples.back();
        object.insert(
            "latest_native_frame".into(),
            latest_native
                .and_then(|sample| serde_json::to_value(sample).ok())
                .unwrap_or(serde_json::Value::Null),
        );
        object.insert(
            "gpu_tile_upload_bytes_per_second".into(),
            serde_json::json!(
                latest_native.map_or(0.0, |sample| sample.tile_upload_bytes_per_second)
            ),
        );
        object.insert(
            "gpu_tile_uploads_per_second".into(),
            serde_json::json!(latest_native.map_or(0.0, |sample| sample.tile_uploads_per_second)),
        );
        object.insert(
            "completed_generation_tiles".into(),
            serde_json::json!(latest_native.map_or(0, |sample| sample.completed_generation_tiles)),
        );
        object.insert(
            "generation_throughput_tiles_per_second".into(),
            serde_json::json!(
                latest_native.map_or(0.0, |sample| sample.generation_throughput_tiles_per_second)
            ),
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
            "cpu_cache_payload_bytes":cached_bytes,
            "cpu_generation_query_workspace_bound_bytes":core_snapshot.estimated_worker_scratch_bytes,
            "cpu_frontier_priority_cache_bound_bytes":core_snapshot.frontier_priority_cache_bytes_upper_bound,
            "cpu_frontier_priority_index_entries":core_snapshot.frontier_priority_index_entries,
            "cpu_renderer_metadata_capacity_bytes":self.last_report.metadata_cpu_capacity_bytes,
            "cpu_renderer_boundary_capacity_bytes":self.last_report.boundary_cpu_capacity_bytes,
            "cpu_slot_retained_payload_bytes":self.slots.iter().flatten().map(|s|s.tile.retained_payload_bytes()).sum::<usize>(),
            "cpu_payload_accounting":"cache, slot and publication-worker references overlap; not process RSS",
            "cpu_boundary_endpoint_bytes":endpoint_bytes,
            "cpu_boundary_table_bytes":boundary_table_bytes,
            "cpu_boundary_overlay_bytes":self.boundary_overlays.values().flatten().map(boundary_retained_bytes).sum::<usize>(),
            "prepared_snapshot_map_limit":19,
            "prepared_snapshot_payload_bound_bytes":19usize.saturating_mul(core.config().max_desired_patches)
                .saturating_mul(std::mem::size_of::<TileBoundary>() + 4 * (core.config().cells as usize + 1)
                    * std::mem::size_of::<mundaris_renderer::regional_edges::BoundaryVertex>()),
            "cpu_endpoint_bytes":endpoint_bytes,
            "total_accounted_cpu_geometry_bytes":cached_bytes.saturating_add(boundary_table_bytes).saturating_add(endpoint_bytes).saturating_add(worker_cache_bytes).saturating_add(renderer_cache_bytes as usize)
                .saturating_add(self.publication_worker.as_ref().map_or(0, |worker| worker.snapshot_bytes.load(Ordering::Acquire))),
            "publication_worker_snapshot_payload_bytes":self.publication_worker.as_ref().map_or(0, |worker| worker.snapshot_bytes.load(Ordering::Acquire)),
            "publication_worker_boundary_cache_owned_bytes":worker_cache_bytes,
            "publication_worker_last_rebuilt_boundaries":self.publication_worker.as_ref().map_or(0, |worker|worker.rebuilt_boundaries.load(Ordering::Acquire)),
            "publication_worker_retained_tile_bytes":self.publication_worker.as_ref().and_then(|worker|worker.in_flight.as_ref()).map_or(0, |work|work.retained_tile_bytes),
            "publication_worker_cover_reference_bytes":self.publication_worker.as_ref().map_or(0, |worker|worker.retained_tile_bytes.load(Ordering::Acquire)),
            "publication_worker_stack_virtual_reservation_bytes":2*1024*1024,
            "worker_stack_virtual_reservation_bytes":self.settings.map_or(0,|s|s.worker_count*4*1024*1024),
            "gpu_tile_capacity_bytes":self.last_report.tile_capacity_bytes,
            "gpu_allocated_slots":self.last_report.allocated_slot_count,
            "gpu_active_pool_tile_capacity_bytes":self.last_report.active_tile_capacity_bytes,
            "gpu_active_pool_boundary_capacity_bytes":self.last_report.active_boundary_capacity_bytes,
            "gpu_resident_payload_bytes":self.last_report.resident_tile_capacity_bytes,
            "gpu_pinned_payload_bytes":self.last_report.pinned_tile_capacity_bytes,
            "gpu_in_flight_payload_bytes":self.last_report.in_flight_tile_capacity_bytes,
            "gpu_evictable_payload_bytes":self.last_report.evictable_tile_capacity_bytes,
            "gpu_boundary_capacity_bytes":self.last_report.boundary_capacity_bytes,
            "gpu_metadata_capacity_bytes":self.last_report.metadata_capacity_bytes,
            "gpu_grid_capacity_bytes":self.last_report.grid_capacity_bytes,
            "gpu_validation_capacity_bytes":self.last_report.validation_capacity_bytes,
            "gpu_total_capacity_bytes":self.last_report.tile_capacity_bytes.saturating_add(self.last_report.boundary_capacity_bytes).saturating_add(self.last_report.metadata_capacity_bytes).saturating_add(self.last_report.grid_capacity_bytes).saturating_add(self.last_report.validation_capacity_bytes),
            "transfer_logical_staging_bytes":self.last_report.transfer_staging_bytes,
            "gpu_accounting":"requested wgpu buffers; not physical VRAM or driver staging",
        }));
        let history = (self.planetary && !include_history_values).then(|| {
            Arc::new(RegionalFrameHistory {
                frame_intervals_ms: self
                    .frame_intervals_ms
                    .iter()
                    .rev()
                    .take(256)
                    .copied()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect(),
                native_frame_samples: self
                    .native_frame_samples
                    .iter()
                    .rev()
                    .take(256)
                    .cloned()
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect(),
                events: {
                    let cached = self.event_history_cache.borrow().clone();
                    cached.unwrap_or_else(|| {
                        let events: Arc<[serde_json::Value]> = self
                            .events
                            .iter()
                            .rev()
                            .take(256)
                            .rev()
                            .cloned()
                            .collect::<Vec<_>>()
                            .into();
                        *self.event_history_cache.borrow_mut() = Some(Arc::clone(&events));
                        events
                    })
                },
            })
        });
        Some(ResidentDiagnosticSnapshot::new(
            snapshot,
            history,
            terrain_trace,
        ))
    }
}
