//! Deterministic province selection and complete-query evidence. Selection never
//! changes the world field and its candidates are recorded for visual review.
use super::*;
use mundaris_world::terrain::GeologicalControls;
use std::{fs::File, io::BufWriter};

const CANDIDATES: usize = 4096;
const SCALES_M: [f64; 4] = [20_000.0, 2_000.0, 256.0, 32.0];
const HIERARCHY_SCALES_M: [f64; 5] = [20_000.0, 2_000.0, 256.0, 32.0, 8.0];

#[derive(Clone, Copy)]
struct Selection {
    direction: DVec3,
    index: usize,
    weight: f64,
    landmark: Option<&'static str>,
    landmark_probe_count: usize,
    direct_oracle_query_count: usize,
    selection_wall_ms: f64,
    linked_detail: Option<(&'static str, DVec3, f64)>,
    parent_feature_fallback: bool,
    parent_morphology: Option<f64>,
}

pub(super) fn controls_json(c: GeologicalControls, algorithm: SurfaceAlgorithm) -> Value {
    json!({"age":c.age,"activity":c.activity,"resurfacing":c.resurfacing,
        "impact_retention":c.impact_retention,"relief_potential":c.relief_potential,
        "province_names":algorithm.province_names(),"province_weights":c.province_weights,
        "process_names":algorithm.process_names(),"process_strengths":c.process_strengths,
        "structural_direction_body_axes":c.structural_direction.to_array()})
}

fn controls(generator: &SurfaceGenerator, direction: DVec3) -> Result<GeologicalControls> {
    generator
        .geological_controls(SurfaceLocation::new(Direction3::try_new(direction)?))?
        .context("province capture requires a director-enabled algorithm")
}

fn select(generator: &SurfaceGenerator) -> Result<[Selection; 4]> {
    let mut selected = [Selection {
        direction: DVec3::Z,
        index: 0,
        weight: -1.0,
        landmark: None,
        landmark_probe_count: 0,
        direct_oracle_query_count: 0,
        selection_wall_ms: 0.0,
        linked_detail: None,
        parent_feature_fallback: false,
        parent_morphology: None,
    }; 4];
    for index in 0..CANDIDATES {
        let direction = fibonacci_direction(index, CANDIDATES);
        let c = controls(generator, direction)?;
        for (province, target) in selected.iter_mut().enumerate() {
            if c.province_weights[province] > target.weight {
                *target = Selection {
                    direction,
                    index,
                    weight: c.province_weights[province],
                    landmark: None,
                    landmark_probe_count: 0,
                    direct_oracle_query_count: 0,
                    selection_wall_ms: 0.0,
                    linked_detail: None,
                    parent_feature_fallback: false,
                    parent_morphology: None,
                };
            }
        }
    }
    Ok(selected)
}

pub(super) fn summary(generator: &SurfaceGenerator) -> Result<Value> {
    let location = SurfaceLocation::new(Direction3::try_new(DVec3::Z)?);
    if generator.geological_controls(location)?.is_none() {
        return Ok(Value::Null);
    }
    let mut dominant_counts = [0usize; 4];
    let mut mean_weights = [0.0; 4];
    let mut mean_processes = [0.0; 4];
    let start = Instant::now();
    for index in 0..CANDIDATES {
        let c = controls(generator, fibonacci_direction(index, CANDIDATES))?;
        let dominant = (0..4)
            .max_by(|&a, &b| c.province_weights[a].total_cmp(&c.province_weights[b]))
            .context("four province channels")?;
        dominant_counts[dominant] += 1;
        for i in 0..4 {
            mean_weights[i] += c.province_weights[i] / CANDIDATES as f64;
            mean_processes[i] += c.process_strengths[i] / CANDIDATES as f64;
        }
    }
    Ok(
        json!({"selection_policy":"4096 equal-area Fibonacci directions; all fixed seeds retained",
        "sample_count":CANDIDATES,"dominant_province_fractions":dominant_counts.map(|n| n as f64 / CANDIDATES as f64),
        "mean_province_weights":mean_weights,"mean_process_strengths":mean_processes,
        "control_queries_wall_ms":start.elapsed().as_secs_f64()*1000.0,
        "timing_scope":"one software director-only scan; not production performance or a speedup benchmark"}),
    )
}

/// Near crops also inspect actual local process landmarks in the selected
/// province, rather than mistaking an empty province-centre crop for evidence.
fn local_landmark(
    generator: &SurfaceGenerator,
    center: Selection,
    province: usize,
) -> Result<Selection> {
    let near = SurfaceLocation::new(Direction3::try_new(center.direction)?);
    let selection_start = Instant::now();
    let mut best = center;
    let mut score = -1.0;
    let probes = generator.diagnostic_landmark_probes(near)?;
    let landmark_probe_count = probes.len();
    let mut direct_oracle_query_count = 0;
    for probe in probes {
        let direction = probe.location.direction().unit();
        let weight = controls(generator, direction)?.province_weights[province];
        direct_oracle_query_count += 1;
        if weight < center.weight * 0.85 || direction.dot(center.direction) < 0.995 {
            continue;
        }
        direct_oracle_query_count += 1;
        let gradient = generator
            .evaluate_point(probe.location)?
            .terrain()
            .tangent_gradient_m_per_unit_direction()
            .length();
        if gradient / generator.radius_m() > 1.0 {
            continue;
        }
        let helper = if direction.y.abs() < 0.8 {
            DVec3::Y
        } else {
            DVec3::X
        };
        let u = direction.cross(helper).normalize();
        let v = direction.cross(u);
        let step = 4.0 / generator.radius_m();
        let mut curvature = 0.0;
        for axis in [u, v] {
            let mut query_gradient = |d| -> Result<DVec3> {
                direct_oracle_query_count += 1;
                Ok(generator
                    .evaluate_point(SurfaceLocation::new(Direction3::try_new(d)?))?
                    .terrain()
                    .tangent_gradient_m_per_unit_direction())
            };
            curvature += (query_gradient(direction + axis * step)?
                - query_gradient(direction - axis * step)?)
            .dot(axis)
            .abs();
        }
        if curvature > score {
            score = curvature;
            best = Selection {
                direction,
                landmark: Some(probe.label),
                weight,
                landmark_probe_count,
                direct_oracle_query_count,
                selection_wall_ms: selection_start.elapsed().as_secs_f64() * 1000.0,
                ..center
            };
        }
    }
    best.landmark_probe_count = landmark_probe_count;
    best.direct_oracle_query_count = direct_oracle_query_count;
    best.selection_wall_ms = selection_start.elapsed().as_secs_f64() * 1000.0;
    Ok(best)
}

/// Hierarchy captures approach a parent geological feature at every scale.
/// Fine probes are recorded as context but never replace the parent anchor.
fn hierarchy_landmark(
    generator: &SurfaceGenerator,
    center: Selection,
    province: usize,
) -> Result<Selection> {
    let selection_start = Instant::now();
    let probes = generator
        .diagnostic_landmark_probes(SurfaceLocation::new(Direction3::try_new(center.direction)?))?;
    let initial_probe_count = probes.len();
    let mut parent = None;
    let mut parent_diagnostic_query_count = 0;
    for probe in probes {
        if !matches!(
            probe.label,
            "rocky-impact-rim"
                | "icy-intersecting-fracture-shoulder"
                | "volcanic-emplacement-front"
        ) || probe.location.direction().unit().distance(center.direction) >= 0.2
        {
            continue;
        }
        parent_diagnostic_query_count += 1;
        if let Some(diagnostics) = generator.detail_diagnostics(probe.location)?
            && diagnostics.parent_morphology.is_finite()
            && diagnostics.parent_morphology != 0.0
        {
            parent = Some((probe, diagnostics.parent_morphology));
            break;
        }
    }
    let Some((parent, parent_morphology)) = parent else {
        let mut fallback = local_landmark(generator, center, province)?;
        fallback.landmark_probe_count += initial_probe_count;
        fallback.direct_oracle_query_count += parent_diagnostic_query_count;
        fallback.selection_wall_ms += selection_start.elapsed().as_secs_f64() * 1000.0;
        fallback.parent_feature_fallback = true;
        return Ok(fallback);
    };
    let parent_label = parent.label;
    let parent_direction = parent.location.direction().unit().normalize();
    let parent_weight = controls(generator, parent_direction)?.province_weights[province];
    let nearby = generator
        .diagnostic_landmark_probes(SurfaceLocation::new(Direction3::try_new(parent_direction)?))?;
    let nearby_probe_count = nearby.len();
    let linked_detail = nearby
        .into_iter()
        .filter(|probe| {
            matches!(
                probe.label,
                "rocky-regional-impact-rim"
                    | "rocky-local-simple-impact"
                    | "rocky-fine-breccia-fracture"
                    | "icy-stress-fracture-intersection"
                    | "volcanic-lobate-emplacement-front"
                    | "hierarchical-detail-feature"
            )
        })
        .min_by(|a, b| {
            a.location
                .direction()
                .unit()
                .distance(parent_direction)
                .total_cmp(&b.location.direction().unit().distance(parent_direction))
        })
        .map(|probe| {
            let direction = probe.location.direction().unit();
            let chord = parent_direction.distance(direction).clamp(0.0, 2.0);
            let separation_m = generator.radius_m() * 2.0 * (chord * 0.5).asin();
            (probe.label, direction.normalize(), separation_m)
        });
    Ok(Selection {
        direction: parent_direction,
        index: center.index,
        weight: parent_weight,
        landmark: Some(parent_label),
        landmark_probe_count: initial_probe_count + nearby_probe_count,
        direct_oracle_query_count: parent_diagnostic_query_count + 1,
        selection_wall_ms: selection_start.elapsed().as_secs_f64() * 1000.0,
        linked_detail,
        parent_feature_fallback: false,
        parent_morphology: Some(parent_morphology),
    })
}

pub(super) fn capture_provinces(
    output: &Path,
    body_key: &str,
    generator: &SurfaceGenerator,
    family: Family,
    metadata: &Value,
    cells: usize,
    scenes: &mut Vec<Value>,
) -> Result<()> {
    let hierarchy = matches!(
        family.algorithm,
        SurfaceAlgorithm::RockyV5 | SurfaceAlgorithm::IcyV3 | SurfaceAlgorithm::VolcanicV3
    );
    let centers = select(generator)?;
    for (province, center) in centers.into_iter().enumerate() {
        let local = if hierarchy {
            hierarchy_landmark(generator, center, province)?
        } else {
            local_landmark(generator, center, province)?
        };
        let scales: &[f64] = if hierarchy {
            &HIERARCHY_SCALES_M
        } else {
            &SCALES_M
        };
        // Produce the smallest acceptance views first while preserving their
        // declared scale indices and the same immutable anchor at every scale.
        let scale_indices: Vec<usize> = if hierarchy {
            (0..scales.len()).rev().collect()
        } else {
            (0..scales.len()).collect()
        };
        for scale_index in scale_indices {
            let side_m = scales[scale_index];
            let selection = if hierarchy {
                local
            } else if scale_index == 0 {
                center
            } else {
                local
            };
            let view = format!("province-{province}-scale-{scale_index}");
            let rotated = reference::surface_adapter::RotatedSurfaceQuery::new(
                generator,
                chart_to_body_rotation(selection.direction),
            );
            let mut fixture = metadata.clone();
            let mut province_selection = json!({
                "province_index":province,"province_name":family.algorithm.province_names()[province],
                "policy":if hierarchy {"first maximum province weight across 4096 fixed Fibonacci directions; select the first returned parent rim/shoulder/emplacement-front probe within 0.2 radians and reuse its exact direction at every scale; query nearby hierarchy probes as linked context; fall back to the existing bounded local-landmark policy only when no parent probe is returned"} else {"first maximum province weight across 4096 fixed Fibonacci directions; local crops maximize gradient variation over +/-4m along two tangent axes among world landmarks within 0.1 radian, at least 85% of that weight, and slope <=1; otherwise use recorded centre fallback"},
                "candidate_count":CANDIDATES,"selected_candidate_index":selection.index,
                "province_center_direction_body_axes":center.direction.to_array(),
                "crop_direction_body_axes":selection.direction.to_array(),
                "selected_landmark_identity":selection.landmark,
                "landmark_fallback":if hierarchy {selection.parent_feature_fallback} else {selection.landmark.is_none()},
                "maximum_sampled_province_weight":center.weight,
                "selected_province_weight":selection.weight
            });
            if hierarchy {
                province_selection["parent_morphology_signal"] = json!(selection.parent_morphology);
                province_selection["authoritative_anchor"] = json!({
                    "identity":{
                        "surface_configuration_identity":metadata["surface_configuration_identity"],
                        "feature_probe_label":selection.landmark,
                        "direction_body_axes":selection.direction.to_array(),
                        "identity_policy":"the immutable terrain configuration, selected process-probe label and exact direction identify this stable query anchor; the label is not a globally unique feature object ID"
                    },
                    "direction_body_axes":selection.direction.to_array(),
                    "feature_probe_label":selection.landmark,
                    "linked_local_detail":selection.linked_detail.map(|(label,direction,separation_m)| json!({
                        "feature_probe_label":label,
                        "direction_body_axes":direction.to_array(),
                        "separation_from_parent_anchor_m":separation_m
                    })),
                    "selection_candidate_index":selection.index,
                    "selection_query_details":{
                        "province_center_candidate_count":CANDIDATES,
                        "local_landmark_probe_count":selection.landmark_probe_count,
                        "parent_diagnostic_query_count":selection.direct_oracle_query_count,
                        "direct_oracle_query_count_excluding_probe_generation":selection.direct_oracle_query_count,
                        "direct_selection_wall_ms":selection.selection_wall_ms
                    },
                    "reused_for_scales_m":scales
                });
            }
            fixture["province_selection"] = province_selection;
            fixture["queried_geological_controls"] =
                controls_json(controls(generator, selection.direction)?, family.algorithm);
            let mut local_scale = json!({"side_m":side_m,"altitude_m":if side_m >= 1000.0 {side_m * 0.6} else {side_m * 0.45},"fov_y_degrees":58.0,"camera_policy":"oblique morphological inspection crop; unbiased standing-height near views are retained separately"});
            if hierarchy {
                local_scale["scale_index"] = json!(scale_index);
                local_scale["anchor_reused_across_scale_sequence"] = json!(true);
            }
            fixture["local_scale"] = local_scale;
            eprintln!(
                "rendering {body_key}/{view} {}: {side_m} m",
                family.algorithm.province_names()[province]
            );
            let scene = reference::capture_family_scene(
                output,
                body_key,
                &view,
                cells,
                &rotated,
                reference::FamilyCaptureStyle {
                    palette: family.palette,
                    neutral_broad_light: false,
                },
                fixture,
            )?;
            let mut scene = scene;
            if hierarchy && side_m <= 256.0 {
                let map_metadata = write_detail_maps(
                    &output.join(body_key).join(&view),
                    &rotated,
                    side_m,
                    family.algorithm.process_names(),
                )?;
                scene["hierarchical_detail_maps"] = map_metadata;
                fs::write(
                    output.join(body_key).join(&view).join("metadata.json"),
                    serde_json::to_vec_pretty(&scene)?,
                )?;
            }
            scenes.push(json!({"view":view,"path":format!("{body_key}/{view}"),"metadata":scene}));
        }
    }
    Ok(())
}

fn write_detail_maps(
    scene_dir: &Path,
    generator: &impl reference::surface_adapter::SurfaceQuery,
    side_m: f64,
    process_names: [&'static str; 4],
) -> Result<Value> {
    const WIDTH: u32 = 256;
    const CHANNELS: [&str; 9] = [
        "detail-parent-morphology",
        "detail-inherited-height",
        "detail-regional-height",
        "detail-local-height",
        "detail-fine-height",
        "detail-parent-process-0",
        "detail-parent-process-1",
        "detail-parent-process-2",
        "detail-parent-process-3",
    ];
    let start = Instant::now();
    let mut fields: [Vec<f64>; CHANNELS.len()] =
        std::array::from_fn(|_| vec![0.0; (WIDTH * WIDTH) as usize]);
    let mut band_cells = [0u64; 3];
    let mut band_candidates = [0u64; 3];
    let mut band_accepted = [0u64; 3];
    let mut band_accepted_known = [true; 3];
    let mut query_count = 0usize;
    for y in 0..WIDTH {
        for x in 0..WIDTH {
            let tx = (f64::from(x) + 0.5) / f64::from(WIDTH) * side_m - side_m * 0.5;
            let ty = side_m * 0.5 - (f64::from(y) + 0.5) / f64::from(WIDTH) * side_m;
            let location = SurfaceLocation::new(Direction3::try_new(
                DVec3::Z * generator.radius_m() + DVec3::X * tx + DVec3::Y * ty,
            )?);
            let Some(diagnostics) = generator.detail_diagnostics(location)? else {
                anyhow::bail!("hierarchy diagnostic maps requested for a non-hierarchical surface");
            };
            query_count += 1;
            let pixel = (y * WIDTH + x) as usize;
            fields[0][pixel] = diagnostics.parent_morphology;
            fields[1][pixel] = diagnostics.inherited_height_m;
            fields[2][pixel] = diagnostics.regional_height_m;
            fields[3][pixel] = diagnostics.local_height_m;
            fields[4][pixel] = diagnostics.fine_height_m;
            for (channel, value) in diagnostics.parent_process_strengths.into_iter().enumerate() {
                fields[5 + channel][pixel] = value;
            }
            for (index, work) in diagnostics.band_work.into_iter().enumerate() {
                band_cells[index] += u64::from(work.cells_visited);
                band_candidates[index] += u64::from(work.candidate_features);
                if let Some(accepted) = work.accepted_features {
                    band_accepted[index] += u64::from(accepted);
                } else {
                    band_accepted_known[index] = false;
                }
            }
        }
    }
    let query_wall_ms = start.elapsed().as_secs_f64() * 1000.0;
    let mut mappings = Vec::new();
    for (channel_index, (channel, values)) in CHANNELS.into_iter().zip(fields).enumerate() {
        let minimum = values.iter().copied().fold(f64::INFINITY, f64::min);
        let maximum = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
        let span = maximum - minimum;
        let pixels = values
            .iter()
            .flat_map(|value| {
                let mapped = if span > 0.0 {
                    ((*value - minimum) / span * 255.0).round() as u8
                } else {
                    128
                };
                [mapped, mapped, mapped, 255]
            })
            .collect::<Vec<_>>();
        let mut encoder = png::Encoder::new(
            BufWriter::new(File::create(scene_dir.join(format!("{channel}.png")))?),
            WIDTH,
            WIDTH,
        );
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.write_header()?.write_image_data(&pixels)?;
        mappings.push(json!({
            "image":format!("{channel}.png"),
            "source":"SurfaceGenerator::detail_diagnostics",
            "parent_process_name":(5..9).contains(&channel_index).then(|| process_names[channel_index - 5]),
            "minimum":minimum,
            "maximum":maximum,
            "encoding":"each channel independently maps observed map min/max to grayscale; constant channels map to mid-grey"
        }));
    }
    Ok(json!({
        "projection":"capture-chart local tangent square centered on the selected body-fixed anchor",
        "side_m":side_m,
        "dimensions":[WIDTH,WIDTH],
        "query_count":query_count,
        "query_wall_ms":query_wall_ms,
        "query_wall_scope":"diagnostic sampling and map-value collection only; excludes PNG encoding",
        "band_work_sums":(0..3).map(|index| json!({
            "cells_visited":band_cells[index],
            "candidate_features":band_candidates[index],
            "accepted_features":if band_accepted_known[index] { json!(band_accepted[index]) } else { Value::Null }
        })).collect::<Vec<_>>(),
        "channels":mappings
    }))
}

pub(super) fn detail_json(diagnostics: mundaris_world::terrain::SurfaceDetailDiagnostics) -> Value {
    json!({
        "inherited_height_m":diagnostics.inherited_height_m,
        "parent_morphology":diagnostics.parent_morphology,
        "parent_morphology_gradient_per_unit_direction":diagnostics.parent_morphology_gradient_per_unit_direction.to_array(),
        "regional_height_m":diagnostics.regional_height_m,
        "local_height_m":diagnostics.local_height_m,
        "fine_height_m":diagnostics.fine_height_m,
        "inherited_gradient_m":diagnostics.inherited_gradient_m.to_array(),
        "regional_gradient_m":diagnostics.regional_gradient_m.to_array(),
        "local_gradient_m":diagnostics.local_gradient_m.to_array(),
        "fine_gradient_m":diagnostics.fine_gradient_m.to_array(),
        "band_work":diagnostics.band_work.map(|work| json!({
            "cells_visited":work.cells_visited,
            "candidate_features":work.candidate_features,
            "accepted_features":work.accepted_features
        })),
        "parent_process_strengths":diagnostics.parent_process_strengths
    })
}

pub(super) fn capture_unbiased_fine_views(
    output: &Path,
    body_key: &str,
    generator: &SurfaceGenerator,
    family: Family,
    metadata: &Value,
    cells: usize,
    scenes: &mut Vec<Value>,
) -> Result<()> {
    for side_m in [32.0, 8.0] {
        let view = format!("unbiased-{}m", side_m as u32);
        let mut fixture = metadata.clone();
        fixture["local_scale"] = json!({
            "side_m":side_m,
            "altitude_m":side_m * 0.45,
            "fov_y_degrees":58.0,
            "camera_policy":"fixed oblique inspection crop at body-fixed +Z; no feature selection",
            "anchor_direction_body_axes":DVec3::Z.to_array(),
            "anchor_identity":"fixed-body-axis-plus-z"
        });
        fixture["unbiased_view"] = json!({
            "selection_policy":"fixed body-fixed +Z direction shared across all fixtures",
            "direction_body_axes":DVec3::Z.to_array(),
            "feature_selection_performed":false
        });
        let scene = reference::capture_family_scene(
            output,
            body_key,
            &view,
            cells,
            generator,
            reference::FamilyCaptureStyle {
                palette: family.palette,
                neutral_broad_light: false,
            },
            fixture,
        )?;
        scenes.push(json!({"view":view,"path":format!("{body_key}/{view}"),"metadata":scene}));
    }
    Ok(())
}

pub(super) fn extend_corpus(
    fixtures: &[(String, SurfaceDefinition, f64, Value)],
    records: &mut Vec<Value>,
) -> Result<()> {
    for (key, definition, radius, metadata) in fixtures {
        let algorithm = definition.terrain().algorithm();
        let probe = SurfaceGenerator::new(definition, *radius)?;
        if probe
            .geological_controls(SurfaceLocation::new(Direction3::try_new(DVec3::Z)?))?
            .is_none()
        {
            continue;
        }
        for query_radius in [80_000.0, *radius, 1_200_000.0] {
            let generator = SurfaceGenerator::new(definition, query_radius)?;
            let centers = select(&generator)?;
            let mut locations: Vec<(String, DVec3)> = Vec::new();
            for (province, center) in centers.into_iter().enumerate() {
                locations.push((format!("province-{province}-center"), center.direction));
                let landmark = local_landmark(&generator, center, province)?;
                locations.push((
                    format!("province-{province}-local-process"),
                    landmark.direction,
                ));
                let next = (province + 1) % 4;
                let mut a = center.direction;
                let mut b = centers[next].direction;
                let delta = |n| -> Result<f64> {
                    let c = controls(&generator, n)?;
                    Ok(c.province_weights[province] - c.province_weights[next])
                };
                let mut da = delta(a)?;
                if da * delta(b)? <= 0.0 && a.dot(b) > -0.9999 {
                    for _ in 0..48 {
                        let midpoint = (a + b).normalize();
                        let dm = delta(midpoint)?;
                        if da * dm > 0.0 {
                            a = midpoint;
                            da = dm;
                        } else {
                            b = midpoint;
                        }
                    }
                    let midpoint = (a + b).normalize();
                    let tangent = (centers[next].direction - center.direction).normalize();
                    for offset in [-1e-7, 0.0, 1e-7] {
                        locations.push((
                            format!("province-{province}-{next}-equal-weight-boundary-{offset}"),
                            (midpoint + tangent * offset).normalize(),
                        ));
                    }
                }
                locations.push((
                    format!("province-{province}-{next}-mixed-transition"),
                    (center.direction + centers[next].direction).normalize(),
                ));
                if matches!(
                    algorithm,
                    SurfaceAlgorithm::RockyV5
                        | SurfaceAlgorithm::IcyV3
                        | SurfaceAlgorithm::VolcanicV3
                ) {
                    let near = SurfaceLocation::new(Direction3::try_new(center.direction)?);
                    for probe in generator.diagnostic_boundary_probes(near)? {
                        locations.push((
                            format!("province-{province}-{}", probe.label),
                            probe.location.direction().unit(),
                        ));
                    }
                    for probe in generator.diagnostic_landmark_probes(near)? {
                        locations.push((
                            format!("province-{province}-hierarchy-landmark-{}", probe.label),
                            probe.location.direction().unit(),
                        ));
                    }
                }
            }
            for (label, n) in locations {
                let location = SurfaceLocation::new(Direction3::try_new(n)?);
                let mut record = sample_record(
                    key,
                    *radius,
                    query_radius,
                    &label,
                    location,
                    generator.evaluate_point(location)?,
                    metadata,
                );
                record["geological_controls"] = controls_json(controls(&generator, n)?, algorithm);
                records.push(record);
            }
            // Add controls to canonical/cell/feature records already produced by
            // the shared corpus path; older numerical corpora remain unchanged.
            for record in records.iter_mut().filter(|record| {
                record["body_key"] == *key
                    && record["query_radius_m"] == query_radius
                    && record.get("geological_controls").is_none()
            }) {
                let values = record["direction_body_axes"]
                    .as_array()
                    .context("corpus direction array")?;
                let n = DVec3::new(
                    values[0].as_f64().context("x")?,
                    values[1].as_f64().context("y")?,
                    values[2].as_f64().context("z")?,
                );
                record["geological_controls"] = controls_json(controls(&generator, n)?, algorithm);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn province_selection_is_fixed_repeatable_and_inside_its_recorded_province() {
        for family in DIRECTED_FAMILIES {
            let definition =
                make_definition(family, 0, 80_000.0, ShapeDefinition::sphere()).unwrap();
            let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
            let first = select(&generator).unwrap();
            let repeated = select(&generator).unwrap();
            for (index, (a, b)) in first.into_iter().zip(repeated).enumerate() {
                assert_eq!(a.index, b.index);
                assert_eq!(a.direction, b.direction);
                assert_eq!(
                    a.weight,
                    controls(&generator, a.direction).unwrap().province_weights[index]
                );
                assert!(a.weight > 0.35);
            }
        }
    }

    #[test]
    fn hierarchy_anchor_tracks_a_parent_probe_deterministically_across_fixed_scales() {
        for family in HIERARCHICAL_FAMILIES {
            let definition =
                make_definition(family, 0, 80_000.0, ShapeDefinition::sphere()).unwrap();
            let generator = SurfaceGenerator::new(&definition, 80_000.0).unwrap();
            let centers = select(&generator).unwrap();
            for (province, center) in centers.into_iter().enumerate() {
                let first = hierarchy_landmark(&generator, center, province).unwrap();
                let repeated = hierarchy_landmark(&generator, center, province).unwrap();
                assert_eq!(first.direction, repeated.direction);
                assert_eq!(first.landmark, repeated.landmark);
                assert_eq!(first.linked_detail, repeated.linked_detail);
                assert!(first.landmark_probe_count >= 1);
                if !first.parent_feature_fallback {
                    assert!(matches!(
                        first.landmark,
                        Some(
                            "rocky-impact-rim"
                                | "icy-intersecting-fracture-shoulder"
                                | "volcanic-emplacement-front"
                        )
                    ));
                }
                let crop_rotation = chart_to_body_rotation(first.direction);
                assert!((crop_rotation * DVec3::Z).distance(first.direction) < 1e-12);
                assert_eq!(HIERARCHY_SCALES_M, [20_000.0, 2_000.0, 256.0, 32.0, 8.0]);
            }
        }
    }
}
