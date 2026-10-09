use glam::DVec3;
use astrum_math::{
    AngularVelocity3, Direction3, LinearVelocity3, LocalPosition, UnitRotation,
    surface::SurfaceLocation,
};
use astrum_terrain_fields::{
    cube_direction,
    graph::{Graph, Node, templates},
};
use astrum_world::{
    BodyProperties, BodyState, CelestialSystem, SimulationInstant,
    terrain::{
        GraphSurface, SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity,
        TerrainSeed,
    },
};
use serde_json::Value;
use std::sync::Arc;

const MOON_RADIUS_M: f64 = 1_737_400.0;

fn graph() -> Graph {
    serde_json::from_value::<Graph>(templates()[0]["graph"].clone())
        .expect("first shared graph template is valid")
}

fn compiled(graph: Graph) -> Arc<GraphSurface> {
    Arc::new(GraphSurface::compile(graph).expect("graph compiles"))
}

fn definition(graph: Graph) -> SurfaceDefinition {
    SurfaceDefinition::from_graph(TerrainIdentity(0xabc), TerrainSeed(7), compiled(graph))
}

fn location(direction: [f64; 3]) -> SurfaceLocation {
    SurfaceLocation::new(Direction3::try_new(DVec3::from_array(direction)).unwrap())
}

fn node_mut<'a>(graph: &'a mut Graph, id: &str) -> &'a mut Node {
    graph.nodes.iter_mut().find(|node| node.id == id).unwrap()
}

#[test]
fn graph_radius_and_declared_envelope_are_rejected_at_surface_compile() {
    let source_graph = graph();
    let surface = definition(source_graph);
    assert!(SurfaceGenerator::new(&surface, MOON_RADIUS_M + 1.0).is_err());

    let mut invalid = graph();
    invalid.reference_radius_m = 1_000.0;
    assert!(GraphSurface::compile(invalid).is_err());
}

#[test]
fn graph_samples_and_gradients_are_deterministic_and_share_cube_edges() {
    let graph_surface = compiled(graph());
    let generator = SurfaceGenerator::new(
        &SurfaceDefinition::from_graph(
            TerrainIdentity(0xabc),
            TerrainSeed(7),
            Arc::clone(&graph_surface),
        ),
        MOON_RADIUS_M,
    )
    .unwrap();
    let direction = [0.31, -0.23, 0.91];
    let first = generator.evaluate_point(location(direction)).unwrap();
    let again = generator.evaluate_point(location(direction)).unwrap();
    assert_eq!(first, again);
    assert_eq!(generator.query_workspace_bytes(), 0);
    assert!(generator.query_stack_bytes() > 0);
    assert!(
        first
            .terrain()
            .tangent_gradient_m_per_unit_direction()
            .is_finite()
    );

    for v in [-1.0, -0.5, 0.0, 0.5, 1.0] {
        let shared = cube_direction(0, 1.0, v).unwrap();
        let other_face = cube_direction(5, -1.0, v).unwrap();
        let a = generator.evaluate_point(location(shared)).unwrap();
        let b = generator.evaluate_point(location(other_face)).unwrap();
        assert_eq!(a, b);
        assert_eq!(
            a.terrain().tangent_gradient_m_per_unit_direction(),
            b.terrain().tangent_gradient_m_per_unit_direction()
        );
    }
}

#[test]
fn allocation_free_native_query_matches_preview_and_differential_outputs() {
    let directions = [[0.2, 0.3, 0.9], [-0.7, 0.4, 0.2], [1.0, 1.0, 1.0]];
    for template in templates().as_array().unwrap() {
        let graph: Graph = serde_json::from_value(template["graph"].clone()).unwrap();
        let compiled = GraphSurface::compile(graph).unwrap();
        for direction in directions {
            let preview = compiled.compiled().evaluate(direction).unwrap();
            let native = compiled.compiled().evaluate_surface(direction).unwrap();
            assert_eq!(native.height_m.to_bits(), preview.height_m.to_bits());
            assert_eq!(native.humidity.to_bits(), preview.humidity.to_bits());
            assert_eq!(
                native.temperature_k.to_bits(),
                preview.temperature_k.to_bits()
            );
            assert_eq!(
                native.weights.map(f64::to_bits),
                preview.weights.map(f64::to_bits)
            );
            assert_eq!(native.support.to_bits(), preview.support.to_bits());

            let (preview_with_diff, preview_diff) = compiled
                .compiled()
                .evaluate_with_differential(direction)
                .unwrap();
            let (native_with_diff, native_diff) = compiled
                .compiled()
                .evaluate_surface_with_differential(direction)
                .unwrap();
            assert_eq!(
                native_with_diff.height_m.to_bits(),
                preview_with_diff.height_m.to_bits()
            );
            assert_eq!(
                native_with_diff.weights.map(f64::to_bits),
                preview_with_diff.weights.map(f64::to_bits)
            );
            assert_eq!(
                native_diff
                    .height_gradient_tangent_m_per_radian
                    .map(f64::to_bits),
                preview_diff
                    .height_gradient_tangent_m_per_radian
                    .map(f64::to_bits)
            );
            assert_eq!(
                native_diff.outward_normal.map(f64::to_bits),
                preview_diff.outward_normal.map(f64::to_bits)
            );
        }
    }
}

#[test]
fn semantic_words_ignore_editor_labels_and_layout_but_include_complete_graph_data() {
    let source = graph();
    let original = compiled(source.clone());
    let mut decorated = source.clone();
    decorated.layout.insert(
        "base".into(),
        astrum_terrain_fields::graph::LayoutPoint { x: 23.0, y: -9.0 },
    );
    node_mut(&mut decorated, "base").label = "A UI label".into();
    let decorated = compiled(decorated);
    assert_eq!(original, decorated);
    assert_eq!(
        original.exact_definition_words(),
        decorated.exact_definition_words()
    );

    let mut edited = source;
    node_mut(&mut edited, "base").params["amplitude"] = Value::from(270.0);
    let edited = compiled(edited);
    assert_ne!(
        original.exact_definition_words(),
        edited.exact_definition_words()
    );
    assert_ne!(
        SurfaceDefinition::from_graph(TerrainIdentity(1), TerrainSeed(2), original)
            .geometry_identity(),
        SurfaceDefinition::from_graph(TerrainIdentity(1), TerrainSeed(2), edited)
            .geometry_identity()
    );
}

#[test]
fn palette_edits_change_palette_identity_without_changing_weights_or_geometry() {
    let graph_a = graph();
    let mut graph_b = graph_a.clone();
    node_mut(&mut graph_b, "materials").params["palette"] =
        serde_json::json!([[0.1, 0.2, 0.3], [0.4, 0.5, 0.6], [0.7, 0.8, 0.9]]);
    let surface_a = compiled(graph_a);
    let surface_b = compiled(graph_b);
    let definition_a =
        SurfaceDefinition::from_graph(TerrainIdentity(1), TerrainSeed(2), Arc::clone(&surface_a));
    let definition_b =
        SurfaceDefinition::from_graph(TerrainIdentity(1), TerrainSeed(2), Arc::clone(&surface_b));
    assert_eq!(
        definition_a.geometry_identity(),
        definition_b.geometry_identity()
    );
    assert_ne!(
        definition_a.material_identity(),
        definition_b.material_identity()
    );
    let weights_a = surface_a
        .compiled()
        .evaluate_surface([0.2, 0.7, 0.6])
        .unwrap()
        .weights;
    let weights_b = surface_b
        .compiled()
        .evaluate_surface([0.2, 0.7, 0.6])
        .unwrap()
        .weights;
    assert_eq!(weights_a, weights_b);
    assert_ne!(surface_a.palette(), surface_b.palette());

    let mut weight_graph = graph();
    node_mut(&mut weight_graph, "materials").params["humidity_bias"] =
        serde_json::json!([1.0, -0.5, 0.25]);
    let weight_surface = compiled(weight_graph);
    let weight_definition = SurfaceDefinition::from_graph(
        TerrainIdentity(1),
        TerrainSeed(2),
        Arc::clone(&weight_surface),
    );
    assert_eq!(
        definition_a.geometry_identity(),
        weight_definition.geometry_identity()
    );
    assert_ne!(
        definition_a.material_identity(),
        weight_definition.material_identity()
    );
    assert_ne!(
        weights_a,
        weight_surface
            .compiled()
            .evaluate_surface([0.2, 0.7, 0.6])
            .unwrap()
            .weights
    );
}

#[test]
fn graph_v1_is_explicit_and_definition_equality_tracks_graph_semantics() {
    let first = definition(graph());
    let mut changed = graph();
    node_mut(&mut changed, "base").params["seed"] = Value::from(88);
    let second = definition(changed);
    assert_eq!(first.terrain().algorithm(), SurfaceAlgorithm::GraphV1);
    assert_ne!(first, second);
    assert_ne!(
        first.configuration_identity(),
        second.configuration_identity()
    );
    assert_eq!(first.graph_surface().unwrap().graph().schema_version, 2);
}

#[test]
fn publishing_graph_surface_advances_only_the_surface_revision() {
    let mut world = CelestialSystem::new(
        std::num::NonZeroU64::new(811).unwrap(),
        SimulationInstant::ZERO,
    );
    let state = BodyState::new(
        LocalPosition::origin(),
        LinearVelocity3::zero(),
        UnitRotation::identity(),
        AngularVelocity3::zero(),
    );
    let body = world
        .insert_body(
            "graph body",
            BodyProperties::new(1.0e22, MOON_RADIUS_M).unwrap(),
            state,
        )
        .unwrap();
    let celestial_revision = world.revision();
    let surface = definition(graph());
    world
        .edit_surface_definition(body, Some(surface.clone()))
        .unwrap();
    assert_eq!(
        world.body(body).unwrap().surface_definition(),
        Some(&surface)
    );
    assert_eq!(world.body(body).unwrap().terrain_revision().value(), 1);
    assert_eq!(world.revision(), celestial_revision);

    let mut invalid = graph();
    invalid.reference_radius_m = MOON_RADIUS_M + 10.0;
    let rejected = definition(invalid);
    assert!(world.edit_surface_definition(body, Some(rejected)).is_err());
    assert_eq!(world.body(body).unwrap().terrain_revision().value(), 1);
    assert_eq!(world.revision(), celestial_revision);
}
