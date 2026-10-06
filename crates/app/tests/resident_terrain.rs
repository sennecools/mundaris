use mundaris_app::resident_terrain::{
    DEFAULT_TILE_CELLS, ResidentTileBuilder, TILE_FILTER_VERSION, TILE_FORMAT_VERSION,
    TileBuildIdentity,
};
use mundaris_math::surface::{CubeFace, CubePatchAddress, PatchEdge};
use mundaris_world::terrain::{
    ShapeDefinition, SurfaceAlgorithm, SurfaceAtmosphere, SurfaceDefinition, SurfaceGenerator,
    SurfaceMaterialDefinition, SurfaceTerrainDefinition, TerrainIdentity, TerrainSeed,
};

fn generated(algorithm: SurfaceAlgorithm, radius_m: f64) -> SurfaceGenerator {
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(0x612a_0001),
        TerrainSeed(0x005e_ed2a),
        algorithm,
    );
    SurfaceGenerator::new(&definition, radius_m).unwrap()
}

fn build_identity() -> TileBuildIdentity {
    TileBuildIdentity {
        body_identity: 0xface_1234,
        surface_revision: 9,
        material_revision: 4,
    }
}

#[test]
fn filtered_tile_build_is_bitwise_deterministic_and_reports_cost() {
    let generator = generated(SurfaceAlgorithm::RockyV5, 109_000.0);
    let address = CubePatchAddress::try_new(CubeFace::PositiveZ, 3, 3, 4).unwrap();
    let (first, first_cost) =
        ResidentTileBuilder::build(&generator, build_identity(), address, 32).unwrap();
    let (second, _second_cost) =
        ResidentTileBuilder::build(&generator, build_identity(), address, 32).unwrap();

    assert_eq!(first.key, second.key);
    assert_eq!(
        first.anchor_radius_m.to_bits(),
        second.anchor_radius_m.to_bits()
    );
    assert_eq!(
        first.min_max_radial_offset_m.map(f64::to_bits),
        second.min_max_radial_offset_m.map(f64::to_bits)
    );
    for (left, right) in first.texels.iter().zip(&second.texels) {
        assert_eq!(
            left.radial_offset_m.to_bits(),
            right.radial_offset_m.to_bits()
        );
        assert_eq!(
            left.material.map(f32::to_bits),
            right.material.map(f32::to_bits)
        );
    }
    assert_eq!(first.key.cells, 32);
    assert_eq!(first.texels.len(), 35 * 35);
    assert_eq!(first_cost.payload_bytes, 35 * 35 * 20);
    assert_eq!(first_cost.texel_dimensions, [35, 35]);
    assert_eq!(first_cost.patch_grid_vertex_dimensions, [33, 33]);
    assert_eq!(first_cost.authoritative_query_count, 1 + 35 * 35 * 7 + 6);
    assert!(first_cost.nominal_grid_spacing_m.is_finite());
    assert!(
        first_cost
            .actual_chart_center_grid_spacing_m
            .iter()
            .all(|value| value.is_finite())
    );
    assert!(
        first_cost
            .filter_actual_surface_offsets_m
            .iter()
            .all(|value| value.is_finite())
    );
    assert!(first_cost.elapsed > std::time::Duration::ZERO);
    assert!(first.min_max_radial_offset_m[0] <= first.min_max_radial_offset_m[1]);
    assert!(first.validate().is_ok());
    for texel in &first.texels {
        assert!(texel.radial_offset_m.is_finite());
        assert!(
            (first.min_max_radial_offset_m[0]..=first.min_max_radial_offset_m[1])
                .contains(&f64::from(texel.radial_offset_m))
        );
        assert!(texel.material.iter().all(|value| value.is_finite()));
        assert!((texel.material.iter().sum::<f32>() - 1.0).abs() < 2.0e-6);
    }
}

#[test]
fn canonical_neighbor_borders_reconstruct_the_same_filtered_surface() {
    let generator = generated(SurfaceAlgorithm::IcyV3, 109_000.0);
    let cells = 32;
    let left_address = CubePatchAddress::try_new(CubeFace::PositiveZ, 2, 1, 1).unwrap();
    let right_address = CubePatchAddress::try_new(CubeFace::PositiveZ, 2, 2, 1).unwrap();
    let (left, _) =
        ResidentTileBuilder::build(&generator, build_identity(), left_address, cells).unwrap();
    let (right, _) =
        ResidentTileBuilder::build(&generator, build_identity(), right_address, cells).unwrap();
    let side = cells + 3;

    for y in 0..=cells {
        let left_texel = left.texels[((y + 1) * side + cells + 1) as usize];
        let right_texel = right.texels[((y + 1) * side + 1) as usize];
        let left_radius = left.anchor_radius_m + f64::from(left_texel.radial_offset_m);
        let right_radius = right.anchor_radius_m + f64::from(right_texel.radial_offset_m);
        assert!((left_radius - right_radius).abs() < 1.0e-3);
        assert_eq!(left_texel.material, right_texel.material);
    }
}

#[test]
fn root_face_edges_and_corners_use_the_same_filtered_texels() {
    let generator = generated(SurfaceAlgorithm::VolcanicV3, 109_000.0);
    let cells = 32;
    let tiles = CubeFace::ALL.map(|face| {
        ResidentTileBuilder::build(
            &generator,
            build_identity(),
            CubePatchAddress::root(face),
            cells,
        )
        .unwrap()
        .0
    });
    let side = cells + 3;

    for face in CubeFace::ALL {
        let address = CubePatchAddress::root(face);
        for edge in PatchEdge::ALL {
            let neighbor = address.neighbor(edge);
            let source = &tiles[CubeFace::ALL
                .iter()
                .position(|value| *value == face)
                .unwrap()];
            let destination = &tiles[CubeFace::ALL
                .iter()
                .position(|value| *value == neighbor.address.face())
                .unwrap()];
            for along in 0..=cells {
                let source_grid = edge.grid(along, cells);
                let target_along = if neighbor.reversed {
                    cells - along
                } else {
                    along
                };
                let target_grid = neighbor.edge.grid(target_along, cells);
                let source_index = ((source_grid[1] + 1) * side + source_grid[0] + 1) as usize;
                let target_index = ((target_grid[1] + 1) * side + target_grid[0] + 1) as usize;
                let a = source.texels[source_index];
                let b = destination.texels[target_index];
                let radius_a = source.anchor_radius_m + f64::from(a.radial_offset_m);
                let radius_b = destination.anchor_radius_m + f64::from(b.radial_offset_m);
                assert!(
                    (radius_a - radius_b).abs() < 1.0e-3,
                    "{face:?} {edge:?} at {along}"
                );
                assert_eq!(a.material, b.material, "{face:?} {edge:?} at {along}");
            }
        }
    }
}

#[test]
fn the_generic_builder_handles_all_current_surface_families_and_large_radii() {
    let default_resolution = generated(SurfaceAlgorithm::RockyV5, 109_000.0);
    let (default_tile, _) = ResidentTileBuilder::build(
        &default_resolution,
        build_identity(),
        CubePatchAddress::root(CubeFace::PositiveZ),
        DEFAULT_TILE_CELLS,
    )
    .unwrap();
    assert_eq!(default_tile.texels.len(), 67 * 67);

    for algorithm in [
        SurfaceAlgorithm::RockyV5,
        SurfaceAlgorithm::IcyV3,
        SurfaceAlgorithm::VolcanicV3,
    ] {
        let generator = generated(algorithm, 6_371_000.0);
        let (tile, diagnostics) = ResidentTileBuilder::build(
            &generator,
            build_identity(),
            CubePatchAddress::root(CubeFace::PositiveZ),
            32,
        )
        .unwrap();
        let local_center = tile.position_local([0.5, 0.5]).unwrap();
        assert!(local_center.is_finite());
        assert!(diagnostics.payload_bytes > 0);
        let metrics = ResidentTileBuilder::measure_approximation(&generator, &tile).unwrap();
        assert!(metrics.max_texel_radial_error_m.is_finite());
        assert!(metrics.rms_texel_radial_error_m.is_finite());
        assert!(metrics.max_triangle_centroid_error_m.is_finite());
        assert!(metrics.rms_triangle_centroid_error_m.is_finite());
        assert!(metrics.max_normal_angular_error_radians.is_finite());
        assert!(metrics.rms_normal_angular_error_radians.is_finite());
    }
}

#[test]
fn tile_identity_tracks_exact_configuration_radius_revisions_and_address() {
    let base = SurfaceDefinition::generated(
        TerrainIdentity(31),
        TerrainSeed(17),
        SurfaceAlgorithm::RockyV5,
    );
    let generator = SurfaceGenerator::new(&base, 109_000.0).unwrap();
    let address = CubePatchAddress::root(CubeFace::PositiveZ);
    let key =
        ResidentTileBuilder::tile_key(&generator, build_identity(), address, DEFAULT_TILE_CELLS)
            .unwrap();
    assert_eq!(key.format_version, TILE_FORMAT_VERSION);
    assert_eq!(key.filter_version, TILE_FILTER_VERSION);

    let changed_shape = SurfaceDefinition::new(
        base.identity(),
        base.seed(),
        ShapeDefinition::ellipsoid([1.001, 1.0, 0.999]).unwrap(),
        base.terrain(),
        base.material(),
        base.atmosphere(),
    )
    .unwrap();
    let changed_shape_generator = SurfaceGenerator::new(&changed_shape, 109_000.0).unwrap();
    let changed_shape_key = ResidentTileBuilder::tile_key(
        &changed_shape_generator,
        build_identity(),
        address,
        DEFAULT_TILE_CELLS,
    )
    .unwrap();
    assert_ne!(key.definition_words, changed_shape_key.definition_words);

    let params = base.terrain().parameters();
    let changed_params = SurfaceTerrainDefinition::new(
        SurfaceAlgorithm::RockyV5,
        mundaris_world::terrain::GeologicalParameters {
            activity: (params.activity + 0.001).min(1.0),
            ..params
        },
    )
    .unwrap();
    let changed_params_def = SurfaceDefinition::new(
        base.identity(),
        base.seed(),
        *base.shape(),
        changed_params,
        base.material(),
        base.atmosphere(),
    )
    .unwrap();
    let changed_params_generator = SurfaceGenerator::new(&changed_params_def, 109_000.0).unwrap();
    assert_ne!(
        key.definition_words,
        ResidentTileBuilder::tile_key(
            &changed_params_generator,
            build_identity(),
            address,
            DEFAULT_TILE_CELLS,
        )
        .unwrap()
        .definition_words
    );

    let atmosphere_definition = SurfaceDefinition::new(
        base.identity(),
        base.seed(),
        *base.shape(),
        base.terrain(),
        base.material(),
        SurfaceAtmosphere::Descriptor {
            pressure_pa: 800.0,
            scale_height_m: 4_000.0,
        },
    )
    .unwrap();
    let atmosphere_generator = SurfaceGenerator::new(&atmosphere_definition, 109_000.0).unwrap();
    assert_ne!(
        key.definition_words,
        ResidentTileBuilder::tile_key(
            &atmosphere_generator,
            build_identity(),
            address,
            DEFAULT_TILE_CELLS,
        )
        .unwrap()
        .definition_words
    );

    let changed_material = SurfaceMaterialDefinition::new(
        base.material().version(),
        (base.material().composition() + 0.01).min(1.0),
        base.material().regional_contrast(),
    )
    .unwrap();
    let changed_material_def = SurfaceDefinition::new(
        base.identity(),
        base.seed(),
        *base.shape(),
        base.terrain(),
        changed_material,
        SurfaceAtmosphere::Airless,
    )
    .unwrap();
    let changed_material_generator =
        SurfaceGenerator::new(&changed_material_def, 109_000.0).unwrap();
    assert_ne!(
        key.definition_words,
        ResidentTileBuilder::tile_key(
            &changed_material_generator,
            build_identity(),
            address,
            DEFAULT_TILE_CELLS,
        )
        .unwrap()
        .definition_words
    );

    let changed_identity = TileBuildIdentity {
        body_identity: build_identity().body_identity + 1,
        ..build_identity()
    };
    assert_ne!(
        key,
        ResidentTileBuilder::tile_key(&generator, changed_identity, address, DEFAULT_TILE_CELLS)
            .unwrap()
    );
    let changed_revision = TileBuildIdentity {
        surface_revision: build_identity().surface_revision + 1,
        ..build_identity()
    };
    assert_ne!(
        key,
        ResidentTileBuilder::tile_key(&generator, changed_revision, address, DEFAULT_TILE_CELLS)
            .unwrap()
    );
    let changed_material_revision = TileBuildIdentity {
        material_revision: build_identity().material_revision + 1,
        ..build_identity()
    };
    assert_ne!(
        key,
        ResidentTileBuilder::tile_key(
            &generator,
            changed_material_revision,
            address,
            DEFAULT_TILE_CELLS,
        )
        .unwrap()
    );
    let changed_radius = SurfaceGenerator::new(&base, 109_001.0).unwrap();
    assert_ne!(
        key,
        ResidentTileBuilder::tile_key(
            &changed_radius,
            build_identity(),
            address,
            DEFAULT_TILE_CELLS
        )
        .unwrap()
    );
    let changed_address = CubePatchAddress::root(CubeFace::NegativeZ);
    assert_ne!(
        key,
        ResidentTileBuilder::tile_key(
            &generator,
            build_identity(),
            changed_address,
            DEFAULT_TILE_CELLS
        )
        .unwrap()
    );
    assert_ne!(
        key,
        ResidentTileBuilder::tile_key(&generator, build_identity(), address, 32).unwrap()
    );
    assert_eq!(
        key,
        ResidentTileBuilder::tile_key(&generator, build_identity(), address, DEFAULT_TILE_CELLS)
            .unwrap()
    );
}
