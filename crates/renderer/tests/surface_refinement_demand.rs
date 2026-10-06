use glam::DVec3;
use mundaris_math::{
    FramePose, FramePosition, FrameTree, LocalPosition, UnitRotation, surface::CubePatchAddress,
};
use mundaris_renderer::{
    CelestialProjection, PreparedView, RenderPrecisionBudget, RenderPreparationError,
    planet_surface::*,
};
use std::{collections::BTreeSet, num::NonZeroU64};

struct HighCertificatePolicy;

impl SurfaceGeometryPolicy for HighCertificatePolicy {
    fn certificate(
        &mut self,
        _address: CubePatchAddress,
        _metadata: PatchMetadata,
        _radius_m: f64,
    ) -> Result<(SurfaceExtent, SurfaceErrorContributions), RenderPreparationError> {
        Ok((
            SurfaceExtent::smooth(1.0),
            SurfaceErrorContributions {
                unresolved_m: 1_000_000.0,
                ..SurfaceErrorContributions::default()
            },
        ))
    }

    fn request_ready(&mut self, _address: CubePatchAddress) -> bool {
        true
    }
}

struct LowDemandPolicy;

impl SurfaceGeometryPolicy for LowDemandPolicy {
    fn certificate(
        &mut self,
        address: CubePatchAddress,
        metadata: PatchMetadata,
        radius_m: f64,
    ) -> Result<(SurfaceExtent, SurfaceErrorContributions), RenderPreparationError> {
        HighCertificatePolicy.certificate(address, metadata, radius_m)
    }

    fn refinement_demand_pixels(
        &mut self,
        _input: SurfaceRefinementInput,
    ) -> Result<f64, RenderPreparationError> {
        Ok(0.0)
    }

    fn request_ready(&mut self, _address: CubePatchAddress) -> bool {
        true
    }
}

struct RefinementBatchPolicy {
    committed: Vec<CubePatchAddress>,
    requested: Vec<CubePatchAddress>,
    ready_after_commits: Option<usize>,
    maximum_cover: usize,
    largest_admitted_cover: usize,
}

impl RefinementBatchPolicy {
    fn ready(maximum_cover: usize) -> Self {
        Self {
            committed: Vec::new(),
            requested: Vec::new(),
            ready_after_commits: None,
            maximum_cover,
            largest_admitted_cover: 0,
        }
    }

    fn cold_after(maximum_cover: usize, ready_commits: usize) -> Self {
        Self {
            ready_after_commits: Some(ready_commits),
            ..Self::ready(maximum_cover)
        }
    }

    fn unblock(&mut self) {
        self.ready_after_commits = None;
    }
}

impl SurfaceGeometryPolicy for RefinementBatchPolicy {
    fn certificate(
        &mut self,
        _address: CubePatchAddress,
        _metadata: PatchMetadata,
        _radius_m: f64,
    ) -> Result<(SurfaceExtent, SurfaceErrorContributions), RenderPreparationError> {
        Ok((
            SurfaceExtent::smooth(1.0),
            SurfaceErrorContributions {
                unresolved_m: 1_000_000.0,
                ..SurfaceErrorContributions::default()
            },
        ))
    }

    fn request_ready(&mut self, address: CubePatchAddress) -> bool {
        self.requested.push(address);
        self.ready_after_commits
            .is_none_or(|limit| self.committed.len() < limit)
    }

    fn allow_replacement(&self) -> bool {
        self.committed.len() < 8
    }

    fn refinement_committed(&mut self, parent: CubePatchAddress) {
        self.committed.push(parent);
    }

    fn admit_replacement(&mut self, cover_patches: usize) -> bool {
        self.largest_admitted_cover = self.largest_admitted_cover.max(cover_patches);
        cover_patches <= self.maximum_cover
    }
}

struct OnePatchDemandPolicy {
    target: CubePatchAddress,
    committed: Vec<CubePatchAddress>,
    requested: Vec<CubePatchAddress>,
}

impl SurfaceGeometryPolicy for OnePatchDemandPolicy {
    fn certificate(
        &mut self,
        address: CubePatchAddress,
        metadata: PatchMetadata,
        radius_m: f64,
    ) -> Result<(SurfaceExtent, SurfaceErrorContributions), RenderPreparationError> {
        HighCertificatePolicy.certificate(address, metadata, radius_m)
    }

    fn refinement_demand_pixels(
        &mut self,
        input: SurfaceRefinementInput,
    ) -> Result<f64, RenderPreparationError> {
        Ok(if input.address == self.target {
            input.certified_error_pixels
        } else {
            0.0
        })
    }

    fn request_ready(&mut self, address: CubePatchAddress) -> bool {
        self.requested.push(address);
        true
    }

    fn allow_coarsening_replacement(&self) -> bool {
        false
    }

    fn refinement_committed(&mut self, parent: CubePatchAddress) {
        self.committed.push(parent);
    }
}

fn seeded_session(split_faces: &[mundaris_math::surface::CubeFace]) -> SurfaceLodSession {
    let mut session = SurfaceLodSession::default();
    let mut patches = Vec::with_capacity(24);
    for face in mundaris_math::surface::CubeFace::ALL {
        if split_faces.contains(&face) {
            for y in 0..2 {
                for x in 0..2 {
                    let address = CubePatchAddress::try_new(face, 1, x, y).unwrap();
                    patches.push(ActiveSurfacePatch {
                        address,
                        stitch_mask: 0,
                        metadata: PatchMetadata::build(address, session.topology()).unwrap(),
                        error_pixels: 1.0,
                    });
                }
            }
        } else {
            let address = CubePatchAddress::root(face);
            patches.push(ActiveSurfacePatch {
                address,
                stitch_mask: 0,
                metadata: PatchMetadata::build(address, session.topology()).unwrap(),
                error_pixels: 1.0,
            });
        }
    }
    session.restore_published_cover(&patches).unwrap();
    session
}

fn seeded_level_one_session() -> SurfaceLodSession {
    seeded_session(&mundaris_math::surface::CubeFace::ALL)
}

fn assert_complete_balanced_cover(session: &SurfaceLodSession) {
    let cover: BTreeSet<_> = session.covering_leaves().collect();
    for face in mundaris_math::surface::CubeFace::ALL {
        let area: f64 = cover
            .iter()
            .filter(|patch| patch.face() == face)
            .map(|patch| 4.0_f64.powi(-i32::from(patch.level())))
            .sum();
        assert_eq!(area, 1.0, "face {face:?} is not completely covered");
    }
    for &patch in &cover {
        let mut ancestor = patch.parent();
        while let Some(parent) = ancestor {
            assert!(!cover.contains(&parent), "overlapping leaves at {parent:?}");
            ancestor = parent.parent();
        }
        for edge in mundaris_math::surface::PatchEdge::ALL {
            let mut neighbor = patch.neighbor(edge).address;
            while !cover.contains(&neighbor) {
                let Some(parent) = neighbor.parent() else {
                    break;
                };
                neighbor = parent;
            }
            if cover.contains(&neighbor) {
                assert!(
                    patch.level() <= neighbor.level() + 1,
                    "unbalanced neighbors {patch:?} and {neighbor:?}"
                );
            }
        }
    }
}

fn selector_view<'a>(tree: &'a FrameTree) -> PreparedView<'a> {
    let frame = tree.root();
    PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(frame, LocalPosition::try_metres(DVec3::ZERO).unwrap()),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap()
}

fn selector_input<'view, 'tree>(
    view: &'view PreparedView<'tree>,
    tree: &'tree FrameTree,
) -> SurfaceViewInput<'view, 'tree> {
    SurfaceViewInput {
        view,
        body_fixed_frame: tree.root(),
        reference_radius_m: 6.4e6,
        projection: CelestialProjection::try_new(1280, 800, 170.0_f64.to_radians(), 0.1).unwrap(),
    }
}

#[test]
fn selector_commits_at_most_eight_ready_refinements_and_reports_each_parent() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let view = selector_view(&tree);
    let input = selector_input(&view, &tree);
    let mut session = seeded_level_one_session();
    let settings = LodSettings::default().with_limits(4096, 65_536, 2).unwrap();
    let mut policy = RefinementBatchPolicy::ready(65_536);

    let report = session
        .update_with_policy(&input, &settings, &mut policy)
        .unwrap();

    assert_eq!(report.splits, 8);
    assert_eq!(policy.committed.len(), 8);
    assert!(policy.committed.iter().all(|parent| parent.level() == 1));
    assert_eq!(
        policy
            .committed
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len(),
        8
    );
    assert!(policy.requested.len() >= 8 * 4);
    assert_eq!(report.active_patches, 48);
    assert_complete_balanced_cover(&session);
}

#[test]
fn blocked_closure_keeps_ready_progress_and_can_resume() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let view = selector_view(&tree);
    let input = selector_input(&view, &tree);
    let mut session = seeded_level_one_session();
    let settings = LodSettings::default().with_limits(4096, 65_536, 2).unwrap();
    let mut policy = RefinementBatchPolicy::cold_after(65_536, 2);

    let blocked_report = session
        .update_with_policy(&input, &settings, &mut policy)
        .unwrap();

    assert_eq!(blocked_report.splits, 2);
    assert_eq!(blocked_report.deferred_transactions, 1);
    assert_eq!(policy.committed.len(), 2);
    assert_complete_balanced_cover(&session);
    let blocked_parent = blocked_report.refinement_parent.unwrap();
    assert!(policy.requested.iter().any(|address| address.level() == 2));

    policy.unblock();
    let resumed_report = session
        .update_with_policy(&input, &settings, &mut policy)
        .unwrap();

    assert!(resumed_report.splits > 0);
    assert_eq!(policy.committed[2], blocked_parent);
    assert_eq!(
        policy.committed.len(),
        resumed_report.splits + blocked_report.splits
    );
    assert_complete_balanced_cover(&session);
}

#[test]
fn ready_refinements_share_the_combined_cover_budget() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let view = selector_view(&tree);
    let input = selector_input(&view, &tree);
    let mut session = seeded_level_one_session();
    let settings = LodSettings::default().with_limits(4096, 36, 2).unwrap();
    let mut policy = RefinementBatchPolicy::ready(36);

    let report = session
        .update_with_policy(&input, &settings, &mut policy)
        .unwrap();

    assert_eq!(report.splits, 4);
    assert!(report.budget_constrained);
    assert!(report.active_patches <= 36);
    assert_eq!(policy.largest_admitted_cover, 36);
    assert_eq!(policy.committed.len(), report.splits);
    assert_complete_balanced_cover(&session);
}

#[test]
fn refinement_closure_splits_coarse_face_neighbor_before_committing() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let view = selector_view(&tree);
    let input = selector_input(&view, &tree);
    let mut session = seeded_session(&[mundaris_math::surface::CubeFace::NegativeZ]);
    let target =
        CubePatchAddress::try_new(mundaris_math::surface::CubeFace::NegativeZ, 1, 1, 1).unwrap();
    let settings = LodSettings::default().with_limits(4096, 65_536, 2).unwrap();
    let mut policy = OnePatchDemandPolicy {
        target,
        committed: Vec::new(),
        requested: Vec::new(),
    };

    let report = session
        .update_with_policy(&input, &settings, &mut policy)
        .unwrap();

    assert_eq!(report.splits, 1);
    assert!(report.balance_splits > 0);
    assert_eq!(policy.committed.as_slice(), &[target]);
    assert!(policy.requested.len() >= 8);
    assert_complete_balanced_cover(&session);
}

#[test]
fn low_demand_preserves_quality_pending_and_default_demand_refines() {
    let tree = FrameTree::new(NonZeroU64::new(1).unwrap());
    let frame = tree.root();
    let radius = 6.4e6;
    let projection = CelestialProjection::try_new(1280, 800, 60.0_f64.to_radians(), 0.1).unwrap();
    let view = PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(
                frame,
                LocalPosition::try_metres(DVec3::Z * (radius + 1.0e9)).unwrap(),
            ),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::try_new(2.0e9, 100.0).unwrap(),
    )
    .unwrap();
    let input = SurfaceViewInput {
        view: &view,
        body_fixed_frame: frame,
        reference_radius_m: radius,
        projection,
    };

    let mut low_demand_session = SurfaceLodSession::default();
    let mut low_demand_policy = LowDemandPolicy;
    let report = low_demand_session
        .update_with_policy(&input, &LodSettings::default(), &mut low_demand_policy)
        .unwrap();
    assert_eq!(report.splits, 0);
    assert!(report.max_error_pixels > LodSettings::default().split_pixels());
    assert!(report.quality_pending);
    assert!(!report.settled);
    assert!(
        low_demand_session
            .active_visible()
            .iter()
            .all(|patch| patch.error_pixels > LodSettings::default().split_pixels())
    );

    let mut certificate_demand_session = SurfaceLodSession::default();
    let mut certificate_demand_policy = HighCertificatePolicy;
    let report = certificate_demand_session
        .update_with_policy(
            &input,
            &LodSettings::default(),
            &mut certificate_demand_policy,
        )
        .unwrap();
    assert!(report.splits > 0);

    let near_view = PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(
                frame,
                LocalPosition::try_metres(DVec3::Z * (radius + 1.0)).unwrap(),
            ),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::try_new(2.0e9, 100.0).unwrap(),
    )
    .unwrap();
    let near_input = SurfaceViewInput {
        view: &near_view,
        ..input
    };
    let mut near_session = SurfaceLodSession::default();
    let report = near_session
        .update_with_policy(
            &near_input,
            &LodSettings::default(),
            &mut certificate_demand_policy,
        )
        .unwrap();
    assert!(report.quality_pending);
    assert!(report.max_error_pixels.is_infinite());
}
