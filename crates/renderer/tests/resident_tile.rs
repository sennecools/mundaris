use glam::DVec3;
use mundaris_math::surface::{CubeFace, CubePatchAddress};
use mundaris_renderer::{
    ResidentMaterialAppearance, TILE_FILTER_VERSION, TILE_FORMAT_VERSION, TileData,
    TileGeometryError, TileKey, TileSlotState, TileTexel,
};

fn tile(address: CubePatchAddress, cells: u32, displacement: impl Fn(u32, u32) -> f32) -> TileData {
    let stride = cells + 3;
    let mut texels = Vec::with_capacity((stride * stride) as usize);
    for y in 0..stride {
        for x in 0..stride {
            texels.push(TileTexel {
                radial_offset_m: displacement(x, y),
                material: [0.1, 0.2, 0.3, 0.4],
            });
        }
    }
    TileData {
        key: TileKey {
            body_identity: 7,
            definition_words: vec![1, 2, 3],
            radius_bits: 6_371_000.0_f64.to_bits(),
            surface_revision: 11,
            material_revision: 13,
            format_version: TILE_FORMAT_VERSION,
            filter_version: TILE_FILTER_VERSION,
            address,
            cells,
        },
        anchor_radius_m: 6_371_000.0,
        min_max_radial_offset_m: [-10.0, 10.0],
        texels,
    }
}

#[test]
fn publication_key_snapshots_survive_caller_edits_and_independent_slot_clones() {
    let mut key = tile(CubePatchAddress::root(CubeFace::PositiveZ), 4, |_, _| 0.0).key;
    let original = key.clone();
    let mut slot = TileSlotState::default();
    let token = slot.request(&key).unwrap();
    let repeated = slot.request(&key.clone()).unwrap();
    assert_eq!(token.generation(), repeated.generation());
    assert_eq!(token.key(), &original);
    let mut independent = slot.clone();
    let cloned_token = token.clone();
    key.definition_words[0] += 1;
    let replacement = independent.request(&key).unwrap();
    assert!(!independent.accept_publication(&cloned_token, &original));
    assert!(independent.accept_publication(&replacement, &key));
    assert!(slot.accept_publication(&cloned_token, &original));
    assert!(!slot.accept_publication(&cloned_token, &key));
    assert_eq!(cloned_token.key(), &original);
}

#[test]
fn deterministic_reconstruction_uses_patch_center_and_halo() {
    let address = CubePatchAddress::try_new(CubeFace::PositiveZ, 2, 1, 2).unwrap();
    let first = tile(address, 8, |x, y| (x as f32 - y as f32) * 0.125);
    let second = first.clone();
    first.validate().unwrap();
    assert_eq!(
        first.position_local([0.5, 0.5]),
        second.position_local([0.5, 0.5])
    );
    assert_eq!(first.material([0.0, 1.0]), Ok([0.1, 0.2, 0.3, 0.4]));
    assert_eq!(first.grid_uv_bounds(), [[0.0, 0.0], [1.0, 1.0]]);
    for point in [[0, 0], [8, 0], [0, 8], [8, 8], [4, 4]] {
        let normal = first.normal_local(point).unwrap();
        assert!(normal.is_finite());
        assert!((normal.length() - 1.0).abs() < 1.0e-12);
    }
    assert!(first.position_local([-1.0 / 8.0, 0.5]).unwrap().is_finite());
    assert!(matches!(
        first.position_local([-0.126, 0.5]),
        Err(TileGeometryError::InvalidCoordinate)
    ));
    assert!(first.anchor_position_body().unwrap().is_finite());
}

#[test]
fn cube_faces_edges_and_corner_positions_remain_finite() {
    for face in CubeFace::ALL {
        let address = CubePatchAddress::root(face);
        let candidate = tile(address, 4, |_, _| 3.0);
        candidate.validate().unwrap();
        for st in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.5, 0.5]] {
            let position = candidate.position_local(st).unwrap();
            assert!(position.is_finite());
            let index = [(st[0] * 4.0) as u32, (st[1] * 4.0) as u32];
            assert!(candidate.normal_local(index).unwrap().is_finite());
        }
        assert!(candidate.anchor_position_body().unwrap().is_finite());
    }
}

#[test]
fn slot_publication_rejects_delayed_old_key_after_newer_request() {
    let mut slot = TileSlotState::default();
    let old_tile = tile(CubePatchAddress::root(CubeFace::PositiveZ), 4, |_, _| 0.0);
    let new_tile = tile(CubePatchAddress::root(CubeFace::PositiveX), 4, |_, _| 0.0);
    let old = slot.request(&old_tile.key).unwrap();
    let same = slot.request(&old_tile.key).unwrap();
    assert_eq!(old, same);
    let new = slot.request(&new_tile.key).unwrap();
    assert!(new.generation() > old.generation());
    assert!(!slot.accept_publication(&old, &old_tile.key));
    assert!(slot.accept_publication(&new, &new_tile.key));
    assert!(!slot.accept_publication(&new, &old_tile.key));
    assert_eq!(slot.generation(), new.generation());
}

#[test]
fn shader_validation_matches_wgsl_entry_point_types() {
    let source = include_str!("../src/shaders/resident_tile.wgsl");
    let module = naga::front::wgsl::parse_str(source).expect("resident tile WGSL parses");
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .expect("resident tile WGSL validates");
    let draw_params = module
        .types
        .iter()
        .find(|(_, ty)| ty.name.as_deref() == Some("DrawParams"))
        .map(|(_, ty)| &ty.inner)
        .expect("resident draw parameter struct exists");
    let naga::TypeInner::Struct { span, .. } = draw_params else {
        panic!("resident draw parameters must be a struct");
    };
    assert_eq!(
        *span as u64, 560,
        "WGSL draw ABI matches the packed Rust size"
    );
}

#[test]
fn authored_resident_appearance_rejects_malformed_colors_and_shading() {
    let default = ResidentMaterialAppearance::default();
    assert_eq!(
        ResidentMaterialAppearance::try_new(default).unwrap(),
        default
    );

    let mut bad_color = default;
    bad_color.natural_colors[2][1] = f32::NAN;
    assert_eq!(bad_color.validate(), Err(TileGeometryError::InvalidTile));

    let mut bad_lighting = default;
    bad_lighting.ambient = 0.4;
    bad_lighting.diffuse = 0.8;
    assert_eq!(bad_lighting.validate(), Err(TileGeometryError::InvalidTile));
}

#[test]
fn chart_center_reconstruction_is_relative_and_finite_at_large_radius() {
    let candidate = tile(CubePatchAddress::root(CubeFace::NegativeY), 2, |_, _| 0.125);
    let center = candidate.position_local([0.5, 0.5]).unwrap();
    assert!((center.length() - 0.125).abs() < 1.0e-7);
    assert!(candidate.anchor_position_body().unwrap().length() > 6.0e6);
    assert!(DVec3::from_array(candidate.anchor_position_body().unwrap().to_array()).is_finite());
}

#[test]
fn staged_view_anchor_and_rotation_must_survive_checked_gpu_narrowing() {
    use glam::DMat3;
    use mundaris_renderer::{RenderPrecisionBudget, TileDraw};

    let data = tile(CubePatchAddress::root(CubeFace::PositiveZ), 4, |_, _| 0.0);
    let mut slot = TileSlotState::default();
    let publication = slot.request(&data.key).unwrap();
    let mut draw = TileDraw {
        tile: std::sync::Arc::new(data),
        publication,
        anchor_view_m: DVec3::new(5.0, -2.0, 1.0),
        body_to_view: DMat3::IDENTITY,
        mode: 0,
        sun_body: DVec3::Z,
        appearance: Default::default(),
    };
    let budget = RenderPrecisionBudget::near_debug();
    assert!(draw.validate_view_transform(budget).is_ok());

    draw.anchor_view_m = DVec3::new(1.0e12, 0.0, 0.0);
    assert!(matches!(
        draw.validate_view_transform(budget),
        Err(mundaris_renderer::RenderPreparationError::OutsideRenderRange { .. })
    ));

    draw.anchor_view_m = DVec3::ZERO;
    draw.body_to_view = DMat3::from_cols(DVec3::X, DVec3::new(0.01, 1.0, 0.0), DVec3::Z);
    assert!(matches!(
        draw.validate_view_transform(budget),
        Err(mundaris_renderer::RenderPreparationError::InvalidResidentTile)
    ));
}

#[test]
fn gpu_uniform_f64_inputs_must_remain_finite_after_narrowing() {
    use glam::DMat3;
    use mundaris_renderer::{RenderPrecisionBudget, TileDraw};

    let data = tile(CubePatchAddress::root(CubeFace::PositiveZ), 4, |_, _| 0.0);
    let mut excessive_radius = data.clone();
    excessive_radius.anchor_radius_m = 1.0e100;
    assert_eq!(
        excessive_radius.validate(),
        Err(TileGeometryError::InvalidTile)
    );
    let mut excessive_height_range = data.clone();
    excessive_height_range.min_max_radial_offset_m[1] = 1.0e100;
    assert_eq!(
        excessive_height_range.validate(),
        Err(TileGeometryError::InvalidTile)
    );

    let mut slot = TileSlotState::default();
    let publication = slot.request(&data.key).unwrap();
    let mut draw = TileDraw {
        tile: std::sync::Arc::new(data),
        publication,
        anchor_view_m: DVec3::ZERO,
        body_to_view: DMat3::IDENTITY,
        mode: 0,
        sun_body: DVec3::Z,
        appearance: Default::default(),
    };
    draw.sun_body = DVec3::splat(1.0e39);
    assert!(matches!(
        draw.validate_view_transform(RenderPrecisionBudget::near_debug()),
        Err(mundaris_renderer::RenderPreparationError::InvalidResidentTile)
    ));
}

#[cfg(feature = "terrain-capture")]
mod gpu_precision {
    use super::*;
    use glam::DMat3;
    use mundaris_math::*;
    use mundaris_renderer::{
        CelestialFrame, CelestialProjection, CelestialStaging, Icosphere, PreparedView,
        RegionalBoundaryEndpoints, RegionalPatchDraw, RegionalResidentDraw, RegionalTileUpload,
        RenderPrecisionBudget, TileDraw, terrain_capture::TerrainCaptureRenderer,
    };
    use std::{collections::BTreeMap, num::NonZeroU64, sync::Arc};

    fn rotation(axis: DVec3, radians: f64) -> UnitRotation {
        UnitRotation::from_axis_angle(Direction3::try_new(axis).unwrap(), radians).unwrap()
    }

    fn position(frame: FrameId, metres: DVec3) -> FramePosition {
        FramePosition::new(frame, LocalPosition::try_metres(metres).unwrap())
    }

    fn gpu_tile(radius_m: f64, address: CubePatchAddress, cells: u32) -> TileData {
        let side = cells + 3;
        let texels: Vec<_> = (0..side)
            .flat_map(|y| {
                (0..side).map(move |x| TileTexel {
                    radial_offset_m: (0.22 * f64::from(x).sin() * f64::from(y).cos()) as f32,
                    material: [0.2, 0.3, 0.1, 0.4],
                })
            })
            .collect();
        TileData {
            key: TileKey {
                body_identity: 17,
                definition_words: vec![1, 0x2a],
                radius_bits: radius_m.to_bits(),
                surface_revision: 1,
                material_revision: 1,
                format_version: TILE_FORMAT_VERSION,
                filter_version: TILE_FILTER_VERSION,
                address,
                cells,
            },
            anchor_radius_m: radius_m,
            min_max_radial_offset_m: [-0.22, 0.22],
            texels,
        }
    }

    fn regional_cover(
        tile: Arc<TileData>,
        publication: mundaris_renderer::TilePublicationToken,
        include_upload: bool,
    ) -> RegionalResidentDraw {
        let address = tile.key.address;
        let boundary = mundaris_renderer::regional_edges::build_boundaries(&BTreeMap::from([(
            address,
            Arc::clone(&tile),
        )]))
        .unwrap()
        .remove(&address)
        .unwrap();
        let draw = TileDraw {
            tile,
            publication,
            anchor_view_m: DVec3::ZERO,
            body_to_view: DMat3::IDENTITY,
            mode: 10,
            sun_body: DVec3::Z,
            appearance: Default::default(),
        };
        RegionalResidentDraw {
            planetary: false,
            capacity: 1,
            cells: draw.tile.key.cells,
            uploads: include_upload
                .then(|| RegionalTileUpload {
                    slot: 0,
                    tile: draw.clone(),
                })
                .into_iter()
                .collect(),
            patches: vec![RegionalPatchDraw {
                own_slot: 0,
                parent_slot: 0,
                own: draw.clone(),
                parent: draw,
                morph_fraction: 1.0,
                boundary_fraction: 1.0,
                quadrant: None,
                quality_fallback: false,
                boundary_endpoints: Arc::new(RegionalBoundaryEndpoints {
                    version: 1,
                    own_coarse: boundary.clone(),
                    own_fine: boundary.clone(),
                    parent: boundary,
                }),
            }],
        }
    }

    fn render_regional(
        renderer: &mut TerrainCaptureRenderer,
        staging: &mut CelestialStaging,
        view: &PreparedView<'_>,
        projection: CelestialProjection,
        sphere: &Icosphere,
        draw: RegionalResidentDraw,
    ) -> Result<Vec<u8>, mundaris_renderer::RenderPreparationError> {
        let mut frame = CelestialFrame::new(view, staging, projection, sphere);
        frame.set_resident_regional(draw)?;
        renderer.render(&frame)
    }

    #[test]
    #[ignore = "requires a real GPU adapter; exercises cached regional preparation and invalidation"]
    fn regional_cached_prepare_reuses_latest_cover_and_invalidates_on_changes() {
        const CELLS: u32 = 2;
        let mut renderer = TerrainCaptureRenderer::new(32, 32)
            .expect("GPU adapter required; do not silently skip this renderer regression");
        let mut slot = TileSlotState::default();
        let address = CubePatchAddress::root(CubeFace::PositiveZ);
        let tile = Arc::new(gpu_tile(80_000.0, address, CELLS));
        let publication = slot.request(&tile.key).unwrap();
        let uploaded = regional_cover(Arc::clone(&tile), publication.clone(), true);
        let mut resident = uploaded.clone();
        resident.uploads.clear();

        let tree = FrameTree::new(NonZeroU64::new(73).unwrap());
        let root = tree.root();
        let evaluation = tree.evaluate();
        let observer = FramePose::new(
            position(root, DVec3::new(0.0, 0.0, 80_010.0)),
            UnitRotation::identity(),
        );
        let view =
            PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug()).unwrap();
        let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1).unwrap();
        let sphere = Icosphere::new();
        let mut staging = CelestialStaging::default();

        // Establish the physical slot, then run one ordinary upload-free prep
        // to establish an eligible latest cover.
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            uploaded,
        )
        .unwrap();
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            resident.clone(),
        )
        .unwrap();
        assert!(
            !renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );

        // The next exact cover skips validation, packing and metadata writes,
        // while its one used physical slot remains associated with submission.
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            resident.clone(),
        )
        .unwrap();
        let report = renderer.last_resident_regional_report();
        assert!(report.preparation_cache_hit);
        assert_eq!(report.prepare_distinct_slot_count, 1);
        assert_eq!(report.tile_upload_bytes, 0);
        assert_eq!(report.boundary_upload_bytes, 0);
        assert_eq!(report.metadata_upload_bytes, 0);
        assert_eq!(report.tile_upload_count, 0);
        assert_eq!(report.boundary_upload_count, 0);

        // Camera and transition changes remain on the full preparation path.
        let mut camera = resident.clone();
        camera.patches[0].own.anchor_view_m.x = 1.0;
        camera.patches[0].parent.anchor_view_m.x = 1.0;
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            camera.clone(),
        )
        .unwrap();
        assert!(
            !renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            camera.clone(),
        )
        .unwrap();
        assert!(
            renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );

        let mut morph = camera.clone();
        morph.patches[0].morph_fraction = 0.75;
        morph.patches[0].boundary_fraction = 0.75;
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            morph.clone(),
        )
        .unwrap();
        assert!(
            !renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );

        let mut endpoints = morph.clone();
        let mut replacement_endpoints = (*endpoints.patches[0].boundary_endpoints).clone();
        replacement_endpoints.version += 1;
        endpoints.patches[0].boundary_endpoints = Arc::new(replacement_endpoints);
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            endpoints,
        )
        .unwrap();
        assert!(
            !renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );

        // Growing the declared resource pool invalidates eligibility even if
        // the used patch and all of its presentation values remain unchanged.
        let mut grown = morph.clone();
        grown.capacity = 2;
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            grown.clone(),
        )
        .unwrap();
        assert!(
            !renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );

        // A rejected transaction clears eligibility; the following valid
        // cover must be fully prepared once before exact hits resume.
        let mut invalid = grown.clone();
        invalid.patches[0].morph_fraction = f32::NAN;
        assert!(
            render_regional(
                &mut renderer,
                &mut staging,
                &view,
                projection,
                &sphere,
                invalid,
            )
            .is_err()
        );
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            grown.clone(),
        )
        .unwrap();
        assert!(
            !renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            grown,
        )
        .unwrap();
        assert!(
            renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );

        // A changed tile key and publication generation with an upload also
        // misses the old latest-cover cache.
        let mut replacement_tile = gpu_tile(80_000.0, address, CELLS);
        replacement_tile.key.surface_revision += 1;
        let replacement_tile = Arc::new(replacement_tile);
        let replacement_key = replacement_tile.key.clone();
        let replacement_token = slot.request(&replacement_tile.key).unwrap();
        let mut replacement = regional_cover(replacement_tile, replacement_token, true);
        replacement.capacity = 2;
        replacement.uploads[0].slot = 1;
        replacement.patches[0].own_slot = 1;
        replacement.patches[0].parent_slot = 1;
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            replacement.clone(),
        )
        .unwrap();
        assert!(
            !renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );
        replacement.uploads.clear();
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            replacement.clone(),
        )
        .unwrap();
        assert!(
            !renderer
                .last_resident_regional_report()
                .preparation_cache_hit
        );
        render_regional(
            &mut renderer,
            &mut staging,
            &view,
            projection,
            &sphere,
            replacement,
        )
        .unwrap();
        let report = renderer.last_resident_regional_report();
        assert!(report.preparation_cache_hit);
        assert_eq!(report.prepare_distinct_slot_count, 1);
        assert_eq!(report.slots[1].key.as_ref(), Some(&replacement_key));
    }

    #[test]
    #[ignore = "requires a real GPU adapter; checks resident reconstruction under large common offsets"]
    fn resident_tile_view_reconstruction_matches_f64_across_common_offsets() {
        const CELLS: u32 = 8;
        let mut renderer = TerrainCaptureRenderer::new(32, 32)
            .expect("GPU adapter required; do not silently skip this precision gate");
        let mut slot = TileSlotState::default();
        let mut staging = CelestialStaging::default();
        let sphere = Icosphere::new();
        let projection = CelestialProjection::try_new(32, 32, 60.0_f64.to_radians(), 0.1).unwrap();
        let mut namespace = 42_u64;

        for radius_m in [109_000.0_f64, 6_371_000.0, 70_000_000.0] {
            let level = (radius_m / 128.0).log2().round() as u8;
            let extent = 1_u32 << level;
            let address =
                CubePatchAddress::try_new(CubeFace::PositiveZ, level, extent / 2 - 1, extent / 2)
                    .unwrap();
            let tile = Arc::new(gpu_tile(radius_m, address, CELLS));
            tile.validate().unwrap();
            let publication = slot.request(&tile.key).unwrap();
            let anchor_body = tile.anchor_position_body().unwrap();
            let mut baseline_view = None::<Vec<[f32; 3]>>;
            let mut baseline_reference = None::<Vec<DVec3>>;

            for common_offset_m in [0.0, 1.5e11, 1.0e16] {
                namespace += 1;
                let mut tree = FrameTree::new(NonZeroU64::new(namespace).unwrap());
                let root = tree.root();
                let common = tree
                    .insert(
                        root,
                        FrameState::stationary(RigidTransform::new(
                            Displacement3::try_metres(DVec3::new(common_offset_m, 0.0, 0.0))
                                .unwrap(),
                            UnitRotation::identity(),
                        )),
                    )
                    .unwrap();
                let body_rotation = rotation(DVec3::new(1.0, -2.0, 0.7), 0.63);
                let body = tree
                    .insert(
                        common,
                        FrameState::stationary(RigidTransform::new(
                            Displacement3::zero(),
                            body_rotation,
                        )),
                    )
                    .unwrap();
                let regional_twist = rotation(DVec3::new(-0.2, 0.8, 1.0), -0.37);
                let regional_rotation = body_rotation.compose(regional_twist);
                let regional_local_observer = DVec3::new(2.0, -1.5, 3.0);
                let observer_body = DVec3::Z * (radius_m + 40.0);
                let regional_origin_common = body_rotation.quaternion() * observer_body
                    - regional_rotation.quaternion() * regional_local_observer;
                let regional = tree
                    .insert(
                        common,
                        FrameState::stationary(RigidTransform::new(
                            Displacement3::try_metres(regional_origin_common).unwrap(),
                            regional_rotation,
                        )),
                    )
                    .unwrap();
                let camera_orientation = rotation(DVec3::new(0.4, 1.0, -0.3), 0.29);
                let observer = FramePose::new(
                    position(regional, regional_local_observer),
                    camera_orientation,
                );
                let evaluation = tree.evaluate();
                let view =
                    PreparedView::new(&evaluation, observer, RenderPrecisionBudget::near_debug())
                        .unwrap();
                let prepared = view.prepare_source(body).unwrap();
                let anchor = position(body, anchor_body);
                prepared.try_position(anchor).unwrap();
                let anchor_view_m = prepared.view_displacement(anchor).unwrap().metres();
                let source_to_regional = evaluation
                    .prepare_conversion(body, regional)
                    .unwrap()
                    .rotation();
                let body_to_view = DMat3::from_quat(
                    camera_orientation
                        .inverse()
                        .compose(source_to_regional)
                        .quaternion(),
                );
                let draw = TileDraw {
                    tile: Arc::clone(&tile),
                    publication: publication.clone(),
                    anchor_view_m,
                    body_to_view,
                    mode: 2,
                    sun_body: DVec3::new(0.3, -0.4, 0.8).normalize(),
                    appearance: Default::default(),
                };

                {
                    let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
                    frame.set_resident_tile(draw.clone()).unwrap();
                    renderer.render(&frame).unwrap();
                }
                let reconstructed = renderer.validate_resident_tile(&draw).unwrap();
                assert_eq!(reconstructed.len(), (CELLS + 1).pow(2) as usize);
                let mut current_view = Vec::with_capacity(reconstructed.len());
                let mut current_reference = Vec::with_capacity(reconstructed.len());
                let mut max_local_m = 0.0_f64;
                let mut max_view_m = 0.0_f64;
                let mut max_normal_rad = 0.0_f64;
                for (index, gpu) in reconstructed.iter().enumerate() {
                    assert!(
                        gpu.position_local_m
                            .iter()
                            .chain(&gpu.position_view_m)
                            .chain(&gpu.normal_body)
                            .chain(&gpu.material)
                            .all(|value| value.is_finite())
                    );
                    assert!((gpu.material.iter().sum::<f32>() - 1.0).abs() <= 1.0e-5);
                    for (actual, expected) in gpu.material.iter().zip([0.2, 0.3, 0.1, 0.4]) {
                        assert!((*actual - expected).abs() <= 1.0e-5);
                    }
                    let x = index as u32 % (CELLS + 1);
                    let y = index as u32 / (CELLS + 1);
                    let st = [
                        f64::from(x) / f64::from(CELLS),
                        f64::from(y) / f64::from(CELLS),
                    ];
                    let local = tile.position_local(st).unwrap();
                    let local_gpu = DVec3::from_array(gpu.position_local_m.map(f64::from));
                    max_local_m = max_local_m.max((local_gpu - local).length());
                    assert!(
                        (local_gpu - local).length() <= 1.0e-3,
                        "local residual {radius_m}m radius, common offset {common_offset_m}m, grid {x},{y}: {}m",
                        (local_gpu - local).length()
                    );

                    let exact_point = position(body, anchor_body + local);
                    prepared.try_position(exact_point).unwrap();
                    let reference_view = prepared.view_displacement(exact_point).unwrap().metres();
                    let gpu_view = DVec3::from_array(gpu.position_view_m.map(f64::from));
                    max_view_m = max_view_m.max((gpu_view - reference_view).length());
                    assert!(
                        (gpu_view - reference_view).length() <= 1.0e-3,
                        "view residual {radius_m}m radius, common offset {common_offset_m}m, grid {x},{y}: {}m",
                        (gpu_view - reference_view).length()
                    );

                    let cpu_normal = tile.normal_local([x, y]).unwrap();
                    let gpu_normal = DVec3::from_array(gpu.normal_body.map(f64::from)).normalize();
                    let angle_error = gpu_normal.dot(cpu_normal).clamp(-1.0, 1.0).acos();
                    max_normal_rad = max_normal_rad.max(angle_error);
                    assert!(
                        angle_error <= 1.0e-3,
                        "normal residual {radius_m}m radius, common offset {common_offset_m}m, grid {x},{y}: {angle_error}rad"
                    );
                    current_view.push(gpu.position_view_m);
                    current_reference.push(reference_view);
                }
                println!(
                    "resident_precision radius_m={radius_m} common_offset_m={common_offset_m} samples={} max_local_m={max_local_m:.12} max_view_m={max_view_m:.12} max_normal_rad={max_normal_rad:.12}",
                    reconstructed.len()
                );

                if let Some(baseline) = &baseline_view {
                    for (current, base) in current_view.iter().zip(baseline) {
                        assert!(
                            (DVec3::from_array(current.map(f64::from))
                                - DVec3::from_array(base.map(f64::from)))
                            .length()
                                <= 1.0e-3,
                            "GPU view position changed under common translation {common_offset_m}m"
                        );
                    }
                } else {
                    baseline_view = Some(current_view);
                }
                if let Some(baseline) = &baseline_reference {
                    for (current, base) in current_reference.iter().zip(baseline) {
                        assert!(
                            (*current - *base).length() <= 1.0e-3,
                            "f64 prepared-source result changed under common translation {common_offset_m}m"
                        );
                    }
                } else {
                    baseline_reference = Some(current_reference);
                }
            }
        }
    }
}
