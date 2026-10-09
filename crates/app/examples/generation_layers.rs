//! Bounded opt-in source-layer service diagnostics for the canonical terrain bodies.
//!
//! This is a CPU evaluator diagnostic, not a native frame-time or rendering test.

use anyhow::{Context, ensure};
use glam::DVec3;
use astrum_app::shared_system::SharedTestSystem;
use astrum_math::{
    Direction3,
    surface::{CubeFace, SurfaceLocation},
};
use astrum_world::terrain::{MoonProfileEvaluationDiagnostics, SurfaceGenerator};
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    num::NonZeroU64,
    path::PathBuf,
    time::Instant,
};

const CORPUS_EDGE_UVS: [[f64; 2]; 4] = [[-1.0, 0.0], [1.0, 0.0], [0.0, -1.0], [0.0, 1.0]];

#[derive(Serialize)]
struct Report {
    schema: u32,
    scene_sha256: String,
    camera_sha256: String,
    camera_body: String,
    corpus_points: usize,
    bodies: Vec<BodyReport>,
    caveats: Vec<&'static str>,
}

#[derive(Serialize)]
struct BodyReport {
    semantic_id: String,
    body_identity: u64,
    definition_sha256: Option<String>,
    definition_revision: Option<u32>,
    algorithm: String,
    layer_diagnostics_available: bool,
    bitwise_profiled_vs_ordinary_match: Option<bool>,
    first_pass: PassReport,
    second_pass: PassReport,
}

#[derive(Serialize)]
struct PassReport {
    label: &'static str,
    service_ns: u128,
    points: Vec<PointReport>,
}

#[derive(Serialize)]
struct PointReport {
    corpus_label: String,
    service_ns: u128,
    profile: Option<MoonProfileEvaluationDiagnostics>,
    existing_surface_work: ExistingWorkReport,
    output_bits: [u64; 20],
}

#[derive(Serialize)]
struct ExistingWorkReport {
    cells_visited: u32,
    candidate_features: u32,
    accepted_features: Option<u32>,
}

fn sample_bits(sample: astrum_world::terrain::SurfaceSample) -> [u64; 20] {
    let terrain = sample.terrain();
    let shape = sample.shape();
    let tg = terrain.tangent_gradient_m_per_unit_direction();
    let sg = shape.gradient_m();
    let normal = sample.normal();
    let weights = sample.material_weights();
    [
        terrain.height_m().to_bits(),
        tg.x.to_bits(),
        tg.y.to_bits(),
        tg.z.to_bits(),
        shape.radius_m().to_bits(),
        sg.x.to_bits(),
        sg.y.to_bits(),
        sg.z.to_bits(),
        sample.radius_m().to_bits(),
        normal.x.to_bits(),
        normal.y.to_bits(),
        normal.z.to_bits(),
        weights[0].to_bits(),
        weights[1].to_bits(),
        weights[2].to_bits(),
        weights[3].to_bits(),
        shape.normal().x.to_bits(),
        shape.normal().y.to_bits(),
        shape.normal().z.to_bits(),
        shape.direction().length_squared().to_bits(),
    ]
}

fn corpus(camera_position: [f64; 3]) -> anyhow::Result<Vec<(String, SurfaceLocation)>> {
    let camera = DVec3::from_array(camera_position);
    let camera_direction =
        Direction3::try_new(camera).context("saved camera pose has no direction")?;
    let mut points = Vec::with_capacity(1 + CubeFace::ALL.len() * CORPUS_EDGE_UVS.len());
    points.push((
        "saved_camera_direction".to_owned(),
        SurfaceLocation::new(camera_direction),
    ));
    for face in CubeFace::ALL {
        for (edge_index, uv) in CORPUS_EDGE_UVS.iter().copied().enumerate() {
            let direction = face
                .direction(uv)
                .context("constructing cube seam direction")?;
            points.push((
                format!("{face:?}_edge_{edge_index}"),
                SurfaceLocation::new(direction),
            ));
        }
    }
    Ok(points)
}

fn run_pass(
    generator: &SurfaceGenerator,
    points: &[(String, SurfaceLocation)],
    profile_available: bool,
    label: &'static str,
) -> anyhow::Result<(PassReport, Option<bool>)> {
    let started = Instant::now();
    let mut rows = Vec::with_capacity(points.len());
    let mut all_bitwise_match = true;
    for (label, location) in points {
        let query_started = Instant::now();
        let (sample, profile) = if profile_available {
            let mut diagnostics = MoonProfileEvaluationDiagnostics::default();
            let sample =
                generator.evaluate_moon_profile_point_profiled(*location, &mut diagnostics)?;
            let ordinary = generator.evaluate_point(*location)?;
            all_bitwise_match &= sample_bits(sample) == sample_bits(ordinary);
            (sample, Some(diagnostics))
        } else {
            (generator.evaluate_point(*location)?, None)
        };
        rows.push(PointReport {
            corpus_label: label.clone(),
            service_ns: query_started.elapsed().as_nanos(),
            profile,
            existing_surface_work: {
                let work = sample.work();
                ExistingWorkReport {
                    cells_visited: work.cells_visited,
                    candidate_features: work.candidate_features,
                    accepted_features: work.accepted_features,
                }
            },
            output_bits: sample_bits(sample),
        });
    }
    Ok((
        PassReport {
            label,
            service_ns: started.elapsed().as_nanos(),
            points: rows,
        },
        profile_available.then_some(all_bitwise_match),
    ))
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output_directory = args.next().map(PathBuf::from).context(
        "usage: cargo run -p astrum_app --example generation_layers -- <new-output-directory>",
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
    let source_points = corpus(shared.camera.position_body_m)?;
    let mut bodies = Vec::with_capacity(2);
    for semantic_id in ["moon", "rust"] {
        let index = shared
            .presentation
            .iter()
            .position(|entry| entry.semantic_id == semantic_id)
            .with_context(|| {
                format!("canonical shared system has no {semantic_id} presentation")
            })?;
        let (_, body) =
            shared.system.bodies().nth(index).with_context(|| {
                format!("canonical shared system has no body for {semantic_id}")
            })?;
        let presentation = &shared.presentation[index];
        let definition = body
            .surface_definition()
            .with_context(|| format!("canonical body {semantic_id} has no surface definition"))?;
        let generator = SurfaceGenerator::new(definition, body.properties().reference_radius_m())?;
        let profile_available = definition.terrain().algorithm().name() == "MoonProfileV1";
        let (first_pass, first_match) =
            run_pass(&generator, &source_points, profile_available, "first_pass")?;
        let (second_pass, second_match) =
            run_pass(&generator, &source_points, profile_available, "second_pass")?;
        let bitwise_match = match (first_match, second_match) {
            (Some(first), Some(second)) => Some(first && second),
            _ => None,
        };
        bodies.push(BodyReport {
            semantic_id: semantic_id.to_owned(),
            body_identity: presentation.identity,
            definition_sha256: presentation.definition_sha256.clone(),
            definition_revision: presentation.definition_revision,
            algorithm: definition.terrain().algorithm().name().to_owned(),
            layer_diagnostics_available: profile_available,
            bitwise_profiled_vs_ordinary_match: bitwise_match,
            first_pass,
            second_pass,
        });
    }

    let report = Report {
        schema: 1,
        scene_sha256: shared.scene_sha256,
        camera_sha256: shared.camera_sha256,
        camera_body: shared.camera.body,
        corpus_points: source_points.len(),
        bodies,
        caveats: vec![
            "The first and second passes measure evaluator service with a fixed bounded direction corpus; they are not isolated hardware-cache states.",
            "MoonFieldsV1 exposes total point service and existing SurfaceQueryWork only; unavailable per-band counters are omitted.",
            "MoonProfile instrumentation adds clocks and counters only through its explicit profiled API; ordinary results are checked bitwise in this example.",
            "These CPU source diagnostics do not establish native frame-time or visual acceptance.",
        ],
    };
    let results_path = output_directory.join("results.json");
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
