use glam::DVec3;
use mundaris_app::{
    celestial_labels::*, celestial_selection::*, gravity_fixtures::*, orbit_guides::*,
    system_view::*,
};
use mundaris_renderer::*;
use std::num::NonZeroU64;
#[test]
fn references_bounds_labels_and_selection_are_read_only() {
    let world = GravityFixture::Hierarchy
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let before: Vec<_> = world.bodies().map(|(id, b)| (id, b.clone())).collect();
    let revision = world.revision();
    let ids: Vec<_> = before.iter().map(|b| b.0).collect();
    let mut guides = OrbitGuides::default();
    guides.update(&world);
    assert_eq!(guides.guides()[0].reference, None);
    assert_eq!(guides.guides()[1].reference, Some(ids[0]));
    assert_eq!(guides.guides()[2].reference, Some(ids[1]));
    assert_eq!(guides.subsystem(ids[1]), [ids[1], ids[2]]);
    let bounds = SystemViewBounds::calculate(
        &world,
        &OverviewScope::WholeSystem,
        guides.guides(),
        &[],
        false,
    )
    .unwrap();
    for (w, h) in [(1280, 800), (500, 1200)] {
        let p = CelestialProjection::try_new(w, h, 60.0_f64.to_radians(), 0.1)
            .unwrap()
            .with_origin([300, 50])
            .unwrap();
        let distance = bounds.fit_distance_m(p).unwrap();
        for (_, b) in world.bodies() {
            assert!(
                p.project_marker(
                    b.state().center_in_system().metres() - bounds.center_m() - DVec3::Z * distance
                )
                .unwrap()
                .is_some()
            );
        }
    }
    let history = [DVec3::X * 1e14];
    let capped = SystemViewBounds::calculate(
        &world,
        &OverviewScope::WholeSystem,
        guides.guides(),
        &history,
        false,
    )
    .unwrap();
    let full = SystemViewBounds::calculate(
        &world,
        &OverviewScope::WholeSystem,
        guides.guides(),
        &history,
        true,
    )
    .unwrap();
    assert!(capped.history_outside_fit);
    assert!(full.radius_m() > capped.radius_m() * 10.0);
    let viewport = ScreenRect {
        min: [300.0, 50.0],
        max: [1280.0, 800.0],
    };
    let inputs: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(i, &body)| LabelInput {
            body,
            marker: [700.0, 400.0],
            size: [110.0, 24.0],
            selected: i == 2,
            focused: false,
            hovered: false,
            diameter: 0.01,
            distance_m: 1e11,
        })
        .collect();
    let mut labels = Vec::new();
    let mut layout = LabelLayout::default();
    layout.layout(&inputs, viewport, &[], &mut labels);
    assert_eq!(labels.len(), 3);
    assert_eq!(labels[0].body, ids[2]);
    for (i, a) in labels.iter().enumerate() {
        for b in &labels[i + 1..] {
            assert!(!a.rect.intersects(b.rect));
        }
    }
    let first = labels.clone();
    layout.layout(&inputs, viewport, &[], &mut labels);
    assert_eq!(
        first.iter().map(|l| l.rect).collect::<Vec<_>>(),
        labels.iter().map(|l| l.rect).collect::<Vec<_>>()
    );
    let projection = CelestialProjection::try_new(1280, 800, 1.0, 0.1).unwrap();
    let targets: Vec<_> = ids
        .iter()
        .map(|&body| BodyHitTarget {
            body,
            marker: Some([640.0, 400.0]),
            marker_radius_pixels: 8.0,
            label: labels.iter().find(|l| l.body == body).map(|l| l.rect),
            center_in_view_m: DVec3::new(0.0, 0.0, -100.0),
            radius_m: 10.0,
            occluded_overlay: false,
        })
        .collect();
    let hit = pick_body(&targets, [640.0, 400.0], projection).unwrap();
    assert_eq!(hit.candidates, ids);
    let mut cycle = PickCycle::default();
    for &id in &ids {
        assert_eq!(cycle.choose(&hit, [640.0, 400.0]), Some(id));
    }
    for l in labels {
        let point = [
            (l.rect.min[0] + l.rect.max[0]) * 0.5,
            (l.rect.min[1] + l.rect.max[1]) * 0.5,
        ];
        let hit = pick_body(&targets, point, projection).unwrap();
        assert_eq!(hit.candidates, [l.body]);
    }
    let mut selection = BodySelection::default();
    for &id in &ids {
        selection.select(&world, id).unwrap();
        assert_eq!(selection.selected(), Some(id));
    }
    assert_eq!(world.revision(), revision);
    assert_eq!(
        world
            .bodies()
            .map(|(id, b)| (id, b.clone()))
            .collect::<Vec<_>>(),
        before
    );
    assert_eq!(marker_opacity(8.0, true), 1.0);
    assert_eq!(marker_opacity(24.0, true), 0.0);
    assert_eq!(marker_opacity(16.0, true), 0.5);
    assert_eq!(marker_opacity(100.0, false), 1.0);
}

#[test]
fn visible_sphere_edge_offscreen_center_and_hidden_markers_pick() {
    let world = GravityFixture::Circular
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let p = CelestialProjection::try_new(800, 600, 60.0_f64.to_radians(), 0.1)
        .unwrap()
        .with_origin([250, 100])
        .unwrap();
    let target = BodyHitTarget {
        body: ids[0],
        marker: None,
        marker_radius_pixels: 8.0,
        label: None,
        center_in_view_m: DVec3::new(0.0, 0.0, -100.0),
        radius_m: 30.0,
        occluded_overlay: false,
    };
    assert_eq!(
        pick_body(&[target], [760.0, 400.0], p).unwrap().candidates,
        [ids[0]]
    );
    assert!(
        pick_body(&[target], [1000.0, 400.0], p)
            .unwrap()
            .candidates
            .is_empty()
    );
    let outside = BodyHitTarget {
        center_in_view_m: DVec3::new(90.0, 0.0, -100.0),
        radius_m: 30.0,
        ..target
    };
    assert!(
        p.project_marker(outside.center_in_view_m)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        pick_body(&[outside], [1040.0, 400.0], p)
            .unwrap()
            .candidates,
        [ids[0]]
    );
    let behind = BodyHitTarget {
        center_in_view_m: DVec3::new(0.0, 0.0, 100.0),
        ..target
    };
    assert!(
        pick_body(&[behind], [650.0, 400.0], p)
            .unwrap()
            .candidates
            .is_empty()
    );
}

#[test]
fn explicit_unbound_reference_classification_and_equal_mass_ambiguity() {
    let mut world = GravityFixture::Circular
        .create(NonZeroU64::new(1).unwrap())
        .unwrap();
    let ids: Vec<_> = world.bodies().map(|(id, _)| id).collect();
    let p = *world.body(ids[1]).unwrap().properties();
    world.edit_properties(ids[0], p).unwrap();
    let mut guides = OrbitGuides::default();
    guides.update(&world);
    assert!(guides.guides().iter().all(|g| g.reference.is_none()));
    guides
        .set_reference(&world, ids[1], OrbitGuideReference::Explicit(ids[0]))
        .unwrap();
    guides.update(&world);
    assert_eq!(
        guides.guides()[1].elements.unwrap().class(),
        mundaris_simulation::ConicClass::Hyperbolic
    );
    let mut vertices = Vec::new();
    guides.guides()[1].vertices(64, &mut vertices).unwrap();
    assert!(vertices.is_empty());
}
