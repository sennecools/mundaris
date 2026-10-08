//! Bounded canonical-surface route authoring data for seam and corner inspection.
//!
//! The emitted waypoints are body-fixed samples; transitions between them are
//! teleports unless a consumer explicitly interpolates and checks the path.

use anyhow::{Context, ensure};
use glam::{DQuat, DVec3};
use mundaris_app::shared_system::SharedTestSystem;
use mundaris_math::{Direction3, surface::SurfaceLocation};
use mundaris_world::terrain::SurfaceGenerator;
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    num::NonZeroU64,
    path::PathBuf,
};

const INSPECTION_CLEARANCE_M: f64 = 25.0;
const TRAVERSAL_DURATION_S: f64 = 6.0;
const TRAVERSAL_SPEED_MULTIPLIER: f64 = 5.0;
const TRAVERSAL_BOOST_MULTIPLIER: f64 = 1.0;
const MOON_CUBE_DIRECTIONS: [(&str, [f64; 3]); 4] = [
    ("finite_moon_positive_xz_seam_inside", [1.0, 0.0, 0.98]),
    ("finite_moon_positive_xz_seam_outside", [1.0, 0.0, 1.02]),
    ("finite_moon_positive_xz_corner_x_low", [1.0, 0.98, 1.02]),
    ("finite_moon_positive_xz_corner_x_high", [1.0, 1.02, 0.98]),
];

#[derive(Serialize)]
struct RouteDocument {
    schema: u32,
    source: SourceIdentity,
    route_semantics: RouteSemantics,
    canonical_moon_pose: SavedPose,
    steps: Vec<RouteStep>,
    caveats: Vec<&'static str>,
}

#[derive(Serialize)]
struct SourceIdentity {
    system: &'static str,
    scene_sha256: String,
    camera_sha256: String,
}

#[derive(Serialize)]
struct RouteSemantics {
    body_frame: &'static str,
    body_resolution: &'static str,
    position_clearance_m: f64,
    pose_control: &'static str,
    clearance_control: &'static str,
    clearance_sequence: &'static str,
    navigation_control: &'static str,
    navigation_speed_model: &'static str,
}

#[derive(Debug, Clone, Serialize)]
struct SavedPose {
    body_semantic_id: String,
    position_body_m: [f64; 3],
    orientation_xyzw: [f64; 4],
}

#[derive(Serialize)]
struct RouteStep {
    index: usize,
    label: String,
    movement_kind: &'static str,
    body_semantic_id: String,
    body_identity: u64,
    surface_definition_sha256: Option<String>,
    surface_definition_revision: Option<u32>,
    source_direction: [f64; 3],
    direction_unit: [f64; 3],
    complete_surface_radius_m: f64,
    terrain_height_m: f64,
    target_clearance_m: Option<f64>,
    resulting_radial_clearance_m: f64,
    pose: SavedPose,
    controls: Vec<ControlAction>,
    traversal: Option<TraversalExpectation>,
}

#[derive(Serialize)]
struct TraversalExpectation {
    requested_body_tangent_direction: [f64; 3],
    camera_local_direction_after_tangent_projection: [f64; 3],
    camera_right_tangent_projection: f64,
    camera_up_tangent_projection: f64,
    surface_inspection_translation: [f64; 3],
    direction_sign_assumption: String,
    initial_base_speed_m_s: f64,
    nominal_translation_distance_m: f64,
    snapshot_verification: SnapshotVerification,
}

#[derive(Serialize)]
struct SnapshotVerification {
    body_frame_name: &'static str,
    camera_position_path: &'static str,
    dominant_face_rule: &'static str,
    expected_initial_face: &'static str,
    expected_changed_face: &'static str,
    require_observed_face_change: bool,
}

#[derive(Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum ControlAction {
    SurfacePose {
        body_semantic_id: String,
        position_body_m: [f64; 3],
        orientation_xyzw: [f64; 4],
        resolve_body_handle_from_live_snapshot: bool,
    },
    Clearance {
        meters: f64,
    },
    Navigation {
        translation: [f64; 3],
        speed_multiplier: f64,
        boost_multiplier: f64,
        duration_s: f64,
        input_space: &'static str,
    },
}

struct SurfaceBody<'a> {
    semantic_id: &'a str,
    identity: u64,
    definition_sha256: Option<String>,
    definition_revision: Option<u32>,
    generator: SurfaceGenerator,
}

fn quaternion_array(quaternion: DQuat) -> [f64; 4] {
    quaternion.to_array()
}

fn vec3_array(vector: DVec3) -> [f64; 3] {
    vector.to_array()
}

fn direction(vector: DVec3) -> anyhow::Result<Direction3> {
    Direction3::try_new(vector).context("route direction must be finite and nonzero")
}

fn body_for<'a>(
    shared: &'a SharedTestSystem,
    semantic_id: &'a str,
) -> anyhow::Result<SurfaceBody<'a>> {
    let index = shared
        .presentation
        .iter()
        .position(|entry| entry.semantic_id == semantic_id)
        .with_context(|| format!("canonical scene has no body {semantic_id}"))?;
    let (_, body) = shared
        .system
        .bodies()
        .nth(index)
        .with_context(|| format!("canonical scene has no body state for {semantic_id}"))?;
    let presentation = &shared.presentation[index];
    let definition = body
        .surface_definition()
        .with_context(|| format!("canonical body {semantic_id} has no complete surface"))?;
    Ok(SurfaceBody {
        semantic_id,
        identity: presentation.identity,
        definition_sha256: presentation.definition_sha256.clone(),
        definition_revision: presentation.definition_revision,
        generator: SurfaceGenerator::new(definition, body.properties().reference_radius_m())?,
    })
}

#[allow(clippy::too_many_arguments)] // One recorded source pose and navigation specification.
fn sampled_step(
    index: usize,
    label: impl Into<String>,
    body: &SurfaceBody<'_>,
    radial: Direction3,
    source_direction: [f64; 3],
    orientation_xyzw: [f64; 4],
    target_clearance_m: f64,
    generator_controls: bool,
) -> anyhow::Result<RouteStep> {
    let location = SurfaceLocation::new(radial);
    let sample = body.generator.evaluate_point(location)?;
    let direction = radial.unit();
    let position = direction * (sample.radius_m() + target_clearance_m);
    let measured_clearance = position.length() - sample.radius_m();
    let pose = SavedPose {
        body_semantic_id: body.semantic_id.to_owned(),
        position_body_m: vec3_array(position),
        orientation_xyzw,
    };
    let mut controls = vec![ControlAction::SurfacePose {
        body_semantic_id: body.semantic_id.to_owned(),
        position_body_m: pose.position_body_m,
        orientation_xyzw,
        resolve_body_handle_from_live_snapshot: true,
    }];
    if generator_controls {
        controls.push(ControlAction::Clearance {
            meters: target_clearance_m,
        });
    }
    Ok(RouteStep {
        index,
        label: label.into(),
        movement_kind: "finite_sample",
        body_semantic_id: body.semantic_id.to_owned(),
        body_identity: body.identity,
        surface_definition_sha256: body.definition_sha256.clone(),
        surface_definition_revision: body.definition_revision,
        source_direction,
        direction_unit: vec3_array(direction),
        complete_surface_radius_m: sample.radius_m(),
        terrain_height_m: sample.terrain().height_m(),
        target_clearance_m: Some(target_clearance_m),
        resulting_radial_clearance_m: measured_clearance,
        pose,
        controls,
        traversal: None,
    })
}

fn canonical_return_step(
    index: usize,
    body: &SurfaceBody<'_>,
    saved: &SavedPose,
) -> anyhow::Result<RouteStep> {
    let radial = direction(DVec3::from_array(saved.position_body_m))?;
    let sample = body
        .generator
        .evaluate_point(SurfaceLocation::new(radial))?;
    let measured_clearance = DVec3::from_array(saved.position_body_m).length() - sample.radius_m();
    Ok(RouteStep {
        index,
        label: "return_exact_canonical_moon_pose".to_owned(),
        movement_kind: "exact_pose_restoration",
        body_semantic_id: body.semantic_id.to_owned(),
        body_identity: body.identity,
        surface_definition_sha256: body.definition_sha256.clone(),
        surface_definition_revision: body.definition_revision,
        source_direction: saved.position_body_m,
        direction_unit: vec3_array(radial.unit()),
        complete_surface_radius_m: sample.radius_m(),
        terrain_height_m: sample.terrain().height_m(),
        target_clearance_m: None,
        resulting_radial_clearance_m: measured_clearance,
        pose: saved.clone(),
        controls: vec![ControlAction::SurfacePose {
            body_semantic_id: body.semantic_id.to_owned(),
            position_body_m: saved.position_body_m,
            orientation_xyzw: saved.orientation_xyzw,
            resolve_body_handle_from_live_snapshot: true,
        }],
        traversal: None,
    })
}

fn add_same_scene_traversal(
    step: &mut RouteStep,
    desired_direction_derivative: DVec3,
    direction_sign_assumption: &str,
    expected_changed_face: &'static str,
) -> anyhow::Result<()> {
    let radial = DVec3::from_array(step.direction_unit).normalize();
    let tangent = desired_direction_derivative - radial * desired_direction_derivative.dot(radial);
    let tangent = tangent
        .try_normalize()
        .context("traversal direction is degenerate at its start radial")?;
    let orientation = DQuat::from_array(step.pose.orientation_xyzw).normalize();
    let camera_right = orientation * DVec3::X;
    let camera_up = orientation * DVec3::Y;
    let camera_forward = orientation * -DVec3::Z;
    // `surface_pose` enters SurfaceInspection. Its translation input is
    // controller-local right/up/forward as right*x + radial_up*y - heading*z.
    // Derive its horizontal components from the same projected camera look.
    let heading = (camera_forward - radial * camera_forward.dot(radial))
        .try_normalize()
        .context("camera look is radial at traversal start")?;
    let controller_right = heading.cross(radial).normalize();
    let translation = [tangent.dot(controller_right), 0.0, -tangent.dot(heading)];
    let camera_local = orientation.conjugate() * tangent;
    let initial_base_speed_m_s =
        (0.5 * step.resulting_radial_clearance_m.max(1.0)).clamp(1.0, 1e12);
    let nominal_translation_distance_m = initial_base_speed_m_s
        * TRAVERSAL_SPEED_MULTIPLIER
        * TRAVERSAL_BOOST_MULTIPLIER
        * TRAVERSAL_DURATION_S;
    step.controls.push(ControlAction::Navigation {
        translation,
        speed_multiplier: TRAVERSAL_SPEED_MULTIPLIER,
        boost_multiplier: TRAVERSAL_BOOST_MULTIPLIER,
        duration_s: TRAVERSAL_DURATION_S,
        input_space: "surface_inspection_right_radial_up_heading",
    });
    step.movement_kind = "continuous_navigation_attempt";
    step.traversal = Some(TraversalExpectation {
        requested_body_tangent_direction: vec3_array(tangent),
        camera_local_direction_after_tangent_projection: vec3_array(camera_local),
        camera_right_tangent_projection: camera_right.dot(tangent),
        camera_up_tangent_projection: camera_up.dot(tangent),
        surface_inspection_translation: translation,
        direction_sign_assumption: direction_sign_assumption.to_owned(),
        initial_base_speed_m_s,
        nominal_translation_distance_m,
        snapshot_verification: SnapshotVerification {
            body_frame_name: "Moon",
            camera_position_path: "camera.position_m",
            dominant_face_rule: "argmax(abs(x), abs(y), abs(z)) in the Moon body-fixed snapshot position; retain the selected component sign",
            expected_initial_face: "+X",
            expected_changed_face,
            require_observed_face_change: true,
        },
    });
    // Keep the projected camera basis live in this calculation so the
    // serialized local vector is explicitly derived from its orientation.
    ensure!(
        camera_right.is_finite() && camera_up.is_finite(),
        "invalid camera tangent basis"
    );
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output_directory = args.next().map(PathBuf::from).context(
        "usage: cargo run -p mundaris_app --example generation_route -- <new-output-directory>",
    )?;
    ensure!(
        args.next().is_none(),
        "expected exactly one output-directory argument"
    );
    fs::create_dir(&output_directory).with_context(|| {
        format!(
            "creating new output directory {}",
            output_directory.display()
        )
    })?;

    let shared =
        SharedTestSystem::load_canonical(NonZeroU64::new(1).context("invalid namespace")?)?;
    ensure!(
        shared.camera.body == "moon",
        "canonical route expects the saved camera on moon"
    );
    let saved_position = shared.camera.position_body_m;
    let saved_radial = direction(DVec3::from_array(saved_position))?;
    let saved_orientation = DQuat::from_array(shared.camera.orientation_xyzw);
    ensure!(
        saved_orientation.is_finite(),
        "canonical camera orientation is nonfinite"
    );
    let saved_moon_pose = SavedPose {
        body_semantic_id: "moon".to_owned(),
        position_body_m: saved_position,
        orientation_xyzw: shared.camera.orientation_xyzw,
    };

    let moon = body_for(&shared, "moon")?;
    let rust = body_for(&shared, "rust")?;
    let mut steps = Vec::with_capacity(8);
    let seam_start = [1.0, 0.0, 0.999];
    let seam_radial = direction(DVec3::from_array(seam_start))?;
    let seam_orientation = quaternion_array(
        (DQuat::from_rotation_arc(saved_radial.unit(), seam_radial.unit()) * saved_orientation)
            .normalize(),
    );
    let mut seam_traversal = sampled_step(
        steps.len(),
        "continuous_seam_traversal_start",
        &moon,
        seam_radial,
        seam_start,
        seam_orientation,
        INSPECTION_CLEARANCE_M,
        true,
    )?;
    add_same_scene_traversal(
        &mut seam_traversal,
        DVec3::new(-1.0, 0.0, 1.0),
        "increase z-x from a +X-dominant start; positive target projection crosses the +X/+Z seam",
        "+Z",
    )?;
    steps.push(seam_traversal);

    let corner_start = [1.0, 0.999, 0.999];
    let corner_radial = direction(DVec3::from_array(corner_start))?;
    let corner_orientation = quaternion_array(
        (DQuat::from_rotation_arc(saved_radial.unit(), corner_radial.unit()) * saved_orientation)
            .normalize(),
    );
    let mut corner_traversal = sampled_step(
        steps.len(),
        "continuous_corner_traversal_start",
        &moon,
        corner_radial,
        corner_start,
        corner_orientation,
        INSPECTION_CLEARANCE_M,
        true,
    )?;
    add_same_scene_traversal(
        &mut corner_traversal,
        DVec3::new(-2.0, 1.0, 1.0),
        "increase both y-x and z-x from a +X-dominant start; +Y and +Z are symmetric targets",
        "+Y or +Z (near tie; either must displace +X)",
    )?;
    steps.push(corner_traversal);

    for (label, components) in MOON_CUBE_DIRECTIONS {
        let radial = direction(DVec3::from_array(components))?;
        let rotation = DQuat::from_rotation_arc(saved_radial.unit(), radial.unit());
        let orientation = quaternion_array((rotation * saved_orientation).normalize());
        steps.push(sampled_step(
            steps.len(),
            label,
            &moon,
            radial,
            components,
            orientation,
            INSPECTION_CLEARANCE_M,
            true,
        )?);
    }
    steps.push(canonical_return_step(steps.len(), &moon, &saved_moon_pose)?);
    steps.push(sampled_step(
        steps.len(),
        "finite_rust_original_saved_camera_direction",
        &rust,
        saved_radial,
        vec3_array(saved_radial.unit()),
        shared.camera.orientation_xyzw,
        INSPECTION_CLEARANCE_M,
        true,
    )?);

    let report = RouteDocument {
        schema: 1,
        source: SourceIdentity {
            system: "canonical_shared_test_system",
            scene_sha256: shared.scene_sha256,
            camera_sha256: shared.camera_sha256,
        },
        route_semantics: RouteSemantics {
            body_frame: "body_fixed",
            body_resolution: "resolve body_semantic_id to the current body handle in each live snapshot; handles are intentionally not serialized",
            position_clearance_m: INSPECTION_CLEARANCE_M,
            pose_control: "surface_pose",
            clearance_control: "clearance",
            clearance_sequence: "apply surface_pose, then apply clearance=25m through the same ordinary surface inspection controller",
            navigation_control: "navigation",
            navigation_speed_model: "surface inspection base speed is 0.5 * measured clearance; at the 25m start clearance and speed_multiplier=5, nominal initial speed is 62.5m/s for six seconds",
        },
        canonical_moon_pose: saved_moon_pose,
        steps,
        caveats: vec![
            "Each complete-surface sample uses the canonical SurfaceGenerator and sets a body-fixed radial position at the sampled surface radius plus 25m, except the exact saved Moon-pose restoration step.",
            "The four finite edge/corner waypoints remain independent sampled positions; jumping directly between them is a teleport and does not prove a continuous crossing or gap-free traversal of the cube-chart seams.",
            "The two additional continuous traversal entries request same-scene six-second navigation attempts from [1,0,0.999] and [1,0.999,0.999]; sign assumptions and expected dominant-face changes are recorded, but only post-run live snapshots can confirm a crossing.",
            "Navigation translation is expressed in the ordinary SurfaceInspection controller basis (right, radial up, heading); the local camera tangent and projected right/up components are included to make the chosen direction auditable.",
            "The Rust waypoint uses the saved camera radial direction on Rust's own canonical complete surface; its body-fixed pose is newly sampled, not the Moon position.",
            "Body identities are semantic IDs and persistent identities only. Resolve runtime handles from current snapshots when applying the ordinary controls.",
        ],
    };
    let results_path = output_directory.join("route.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&results_path)
        .with_context(|| format!("creating {} without overwrite", results_path.display()))?;
    serde_json::to_writer_pretty(&mut file, &report)?;
    file.write_all(b"\n")?;
    file.flush()?;
    println!("{}", results_path.display());
    Ok(())
}
