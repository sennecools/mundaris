//! Lossless, content-identified graph bundles with bounded verification.
use crate::graph::{CompiledGraph, GRAPH_SCHEMA_VERSION, Graph, validate_serialized_size};
use crate::{Error, cube_direction};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
};
const MAX_EXPORT_WORK: u64 = 500_000_000;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    bundle_version: u32,
    graph_schema_version: u32,
    graph_sha256: String,
    identities: crate::graph::Identities,
    evaluator_version: String,
    differential_method: String,
    differential_step_rad: f64,
    minimum_feature_angular_ratio: Option<f64>,
    resolution: u32,
    sample_count: u64,
    coordinate_space: String,
    mapping: String,
    face_orientation: Vec<Face>,
    boundary_rules: String,
    normal_semantics: String,
    endian: String,
    encoding: String,
    record_stride_bytes: u32,
    channels: Vec<Channel>,
    files: Vec<FileEntry>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Face {
    id: String,
    direction: String,
    u_axis: String,
    v_axis: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Channel {
    name: String,
    unit: String,
    offset_bytes: u32,
    min: f64,
    max: f64,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileEntry {
    path: String,
    bytes: u64,
    sha256: String,
}

pub fn publish(
    graph: &Graph,
    resolution: u32,
    root: &Path,
) -> Result<(PathBuf, String, usize), Error> {
    if !(1..=256).contains(&resolution) {
        return Err(Error::Invalid(
            "graph export resolution must be 1..=256".into(),
        ));
    }
    validate_serialized_size(graph)?;
    let compiled = CompiledGraph::compile(graph.clone())?;
    if compiled.export_work(resolution) > MAX_EXPORT_WORK {
        return Err(Error::Invalid(
            "graph export exceeds the bounded work budget".into(),
        ));
    }
    fs::create_dir_all(root)?;
    let canonical = root.canonicalize()?;
    let stem = safe_id(&graph.recipe_id);
    let mut chosen = None;
    for attempt in 0..1000u32 {
        let dir = canonical.join(format!("{stem}-{}-{attempt:03}", unique_stamp()));
        if !dir.exists() {
            chosen = Some(dir);
            break;
        }
    }
    let target = chosen
        .ok_or_else(|| Error::Invalid("could not reserve unique graph export directory".into()))?;
    let stage = canonical.join(format!(".{}.{}.partial", stem, unique_stamp()));
    fs::create_dir(&stage)?;
    let result = write_bundle(&compiled, resolution, &stage).and_then(|(sha, files)| {
        let _ = verify(&stage)?;
        if target.exists() {
            return Err(Error::Invalid(
                "export destination appeared before publication".into(),
            ));
        };
        fs::rename(&stage, &target)?;
        Ok((target.clone(), sha, files))
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result
}
fn write_bundle(
    g: &CompiledGraph,
    resolution: u32,
    stage: &Path,
) -> Result<(String, usize), Error> {
    let graph_bytes = serde_json::to_vec(g.graph())?;
    let graph_sha = sha(&graph_bytes);
    fs::write(stage.join("graph.json"), &graph_bytes)?;
    let ranges = g.ranges();
    let scalar_ranges = [
        (
            ranges["height"][0].as_f64().unwrap_or(0.),
            ranges["height"][1].as_f64().unwrap_or(0.),
        ),
        (
            ranges["humidity"][0].as_f64().unwrap_or(0.),
            ranges["humidity"][1].as_f64().unwrap_or(0.),
        ),
        (
            ranges["temperature"][0].as_f64().unwrap_or(0.),
            ranges["temperature"][1].as_f64().unwrap_or(0.),
        ),
        (0., 1.),
        (0., 1.),
        (0., 1.),
        (0., 1.),
        (-1., 1.),
        (-1., 1.),
        (-1., 1.),
    ];
    let channels = vec![
        ("height", "m"),
        ("humidity", "1"),
        ("temperature", "K"),
        ("weight_0", "weight"),
        ("weight_1", "weight"),
        ("weight_2", "weight"),
        ("support", "1"),
        ("normal_x", "unit"),
        ("normal_y", "unit"),
        ("normal_z", "unit"),
    ]
    .into_iter()
    .enumerate()
    .map(|(i, (name, unit))| Channel {
        name: name.into(),
        unit: unit.into(),
        offset_bytes: (i * 8) as u32,
        min: scalar_ranges[i].0,
        max: scalar_ranges[i].1,
    })
    .collect();
    let mut files = Vec::new();
    let mut count = 0u64;
    for face in 0..6 {
        let mut raw = Vec::with_capacity(resolution as usize * resolution as usize * 80);
        for y in 0..resolution {
            for x in 0..resolution {
                let u = 2.0 * (x as f64 + 0.5) / resolution as f64 - 1.;
                let v = 2.0 * (y as f64 + 0.5) / resolution as f64 - 1.;
                let d = cube_direction(face, u, v)?;
                let (s, differential) = g.evaluate_with_differential(d)?;
                let n = differential.outward_normal.map(|v| 0.5 + 0.5 * v);
                for value in [
                    s.height_m,
                    s.humidity,
                    s.temperature_k,
                    s.weights[0],
                    s.weights[1],
                    s.weights[2],
                    s.support,
                    n[0] * 2. - 1.,
                    n[1] * 2. - 1.,
                    n[2] * 2. - 1.,
                ] {
                    raw.extend_from_slice(&value.to_le_bytes())
                }
                count += 1;
            }
        }
        let name = format!("face-{face}.f64le");
        fs::write(stage.join(&name), &raw)?;
        files.push(file_entry(stage, &name)?);
    }
    let entry = file_entry(stage, "graph.json")?;
    files.push(entry);
    let manifest = Manifest {
        bundle_version: 2,
        graph_schema_version: GRAPH_SCHEMA_VERSION,
        graph_sha256: graph_sha,
        identities: g.identities().clone(),
        evaluator_version: g.evaluator_version().into(),
        differential_method: g.differential_method_version().into(),
        differential_step_rad: g.differential_step_rad(),
        minimum_feature_angular_ratio: g.minimum_feature_angular_ratio(),
        resolution,
        sample_count: count,
        coordinate_space: "normalized body-local direction".into(),
        mapping: "six canonical cube faces; texel centers; shared directions use cube_direction"
            .into(),
        face_orientation: vec![
            Face{id:"px".into(),direction:"+X".into(),u_axis:"-Z".into(),v_axis:"+Y".into()},
            Face{id:"nx".into(),direction:"-X".into(),u_axis:"+Z".into(),v_axis:"+Y".into()},
            Face{id:"py".into(),direction:"+Y".into(),u_axis:"+X".into(),v_axis:"-Z".into()},
            Face{id:"ny".into(),direction:"-Y".into(),u_axis:"+X".into(),v_axis:"+Z".into()},
            Face{id:"pz".into(),direction:"+Z".into(),u_axis:"+X".into(),v_axis:"+Y".into()},
            Face{id:"nz".into(),direction:"-Z".into(),u_axis:"-X".into(),v_axis:"+Y".into()},
        ],
        boundary_rules:"No halo samples are stored. Faces are sampled from canonical normalized directions; no face-local wrap or normalization is applied.".into(),
        normal_semantics:"Outward radial-surface normal derived from centered height secants on a local tangent basis; evaluator and per-graph differential step are recorded in this manifest; RGB channels encode components from [-1,1] to [0,1].".into(),
        endian: "little".into(),
        encoding: "IEEE-754 binary64".into(),
        record_stride_bytes: 80,
        channels,
        files,
    };
    let bytes = serde_json::to_vec_pretty(&manifest)?;
    let manifest_sha = sha(&bytes);
    fs::write(stage.join("manifest.json"), bytes)?;
    Ok((manifest_sha, 8))
}

pub fn verify(bundle: &Path) -> Result<(String, usize), Error> {
    let root = bundle.canonicalize()?;
    if !root.is_dir() {
        return Err(Error::Invalid("bundle must be a directory".into()));
    }
    let manifest_bytes = read_child(&root, "manifest.json", 256 * 1024)?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes)?;
    if manifest.bundle_version != 2
        || manifest.graph_schema_version != GRAPH_SCHEMA_VERSION
        || !(1..=256).contains(&manifest.resolution)
        || manifest.sample_count != 6 * manifest.resolution as u64 * manifest.resolution as u64
        || manifest.record_stride_bytes != 80
        || manifest.endian != "little"
        || manifest.encoding != "IEEE-754 binary64"
        || manifest.coordinate_space != "normalized body-local direction"
        || manifest.mapping
            != "six canonical cube faces; texel centers; shared directions use cube_direction"
        || manifest.face_orientation.len() != 6
        || manifest.boundary_rules
            != "No halo samples are stored. Faces are sampled from canonical normalized directions; no face-local wrap or normalization is applied."
        || manifest.normal_semantics
            != "Outward radial-surface normal derived from centered height secants on a local tangent basis; evaluator and per-graph differential step are recorded in this manifest; RGB channels encode components from [-1,1] to [0,1]."
        || manifest.channels.len() != 10
        || manifest.files.len() != 7
    {
        return Err(Error::Invalid(
            "graph manifest semantics, version, resolution, or layout invalid".into(),
        ));
    }
    let graph_bytes = read_child(&root, "graph.json", 256 * 1024)?;
    if sha(&graph_bytes) != manifest.graph_sha256 {
        return Err(Error::Invalid("source graph hash mismatch".into()));
    };
    let graph: Graph = serde_json::from_slice(&graph_bytes)?;
    let compiled = CompiledGraph::compile(graph.clone())?;
    if compiled.export_work(manifest.resolution) > MAX_EXPORT_WORK {
        return Err(Error::Invalid(
            "bundle graph exceeds the verifier work budget".into(),
        ));
    }
    if compiled.identities() != &manifest.identities {
        return Err(Error::Invalid(
            "graph identities do not match manifest".into(),
        ));
    }
    if manifest.evaluator_version != compiled.evaluator_version()
        || manifest.differential_method != compiled.differential_method_version()
        || manifest.differential_step_rad.to_bits() != compiled.differential_step_rad().to_bits()
        || manifest.minimum_feature_angular_ratio.map(f64::to_bits)
            != compiled.minimum_feature_angular_ratio().map(f64::to_bits)
    {
        return Err(Error::Invalid(
            "bundle differential metadata does not match the source graph".into(),
        ));
    }
    let expected_channels = [
        ("height", "m"),
        ("humidity", "1"),
        ("temperature", "K"),
        ("weight_0", "weight"),
        ("weight_1", "weight"),
        ("weight_2", "weight"),
        ("support", "1"),
        ("normal_x", "unit"),
        ("normal_y", "unit"),
        ("normal_z", "unit"),
    ];
    let ranges = compiled.ranges();
    let expected_bounds = [
        (
            ranges["height"][0].as_f64().unwrap(),
            ranges["height"][1].as_f64().unwrap(),
        ),
        (
            ranges["humidity"][0].as_f64().unwrap(),
            ranges["humidity"][1].as_f64().unwrap(),
        ),
        (
            ranges["temperature"][0].as_f64().unwrap(),
            ranges["temperature"][1].as_f64().unwrap(),
        ),
        (0., 1.),
        (0., 1.),
        (0., 1.),
        (0., 1.),
        (-1., 1.),
        (-1., 1.),
        (-1., 1.),
    ];
    let faces = [
        ("px", "+X", "-Z", "+Y"),
        ("nx", "-X", "+Z", "+Y"),
        ("py", "+Y", "+X", "-Z"),
        ("ny", "-Y", "+X", "+Z"),
        ("pz", "+Z", "+X", "+Y"),
        ("nz", "-Z", "-X", "+Y"),
    ];
    for (f, (id, d, u, v)) in manifest.face_orientation.iter().zip(faces) {
        if (
            f.id.as_str(),
            f.direction.as_str(),
            f.u_axis.as_str(),
            f.v_axis.as_str(),
        ) != (id, d, u, v)
        {
            return Err(Error::Invalid("cube face orientation invalid".into()));
        }
    }
    for (i, (ch, (name, unit))) in manifest.channels.iter().zip(expected_channels).enumerate() {
        if ch.name != name
            || ch.unit != unit
            || ch.offset_bytes != (i * 8) as u32
            || !ch.min.is_finite()
            || !ch.max.is_finite()
            || ch.min > ch.max
            || ch.min != expected_bounds[i].0
            || ch.max != expected_bounds[i].1
        {
            return Err(Error::Invalid(
                "channel layout or declared bounds invalid".into(),
            ));
        }
    }
    let mut names = Vec::new();
    for face in 0..6 {
        names.push(format!("face-{face}.f64le"))
    }
    names.push("graph.json".into());
    let mut actual = manifest
        .files
        .iter()
        .map(|f| f.path.clone())
        .collect::<Vec<_>>();
    actual.sort();
    names.sort();
    if names != actual {
        return Err(Error::Invalid(
            "bundle file set is incomplete or unexpected".into(),
        ));
    }
    let per_face = manifest.resolution as usize * manifest.resolution as usize;
    let expect_len = (per_face * 80) as u64;
    for face in 0..6 {
        let name = format!("face-{face}.f64le");
        let entry = manifest
            .files
            .iter()
            .find(|e| e.path == name)
            .ok_or_else(|| Error::Invalid("missing face file".into()))?;
        if entry.bytes != expect_len {
            return Err(Error::Invalid("raw face byte length invalid".into()));
        };
        let data = read_child(&root, &name, expect_len as usize)?;
        if data.len() as u64 != entry.bytes || sha(&data) != entry.sha256 {
            return Err(Error::Invalid(format!("integrity check failed for {name}")));
        }
        for y in 0..manifest.resolution {
            for x in 0..manifest.resolution {
                let i = (y as usize * manifest.resolution as usize + x as usize) * 80;
                let v: [f64; 10] = std::array::from_fn(|c| {
                    f64::from_le_bytes(data[i + c * 8..i + c * 8 + 8].try_into().unwrap_or([0; 8]))
                });
                if v.iter().any(|n| !n.is_finite())
                    || v[0] < manifest.channels[0].min
                    || v[0] > manifest.channels[0].max
                    || v[1] < 0.
                    || v[1] > 1.
                    || v[2] < 0.
                    || v[2] > 5000.
                    || v[3..=6].iter().any(|n| !(0.0..=1.0).contains(n))
                    || (v[3] + v[4] + v[5] - 1.).abs() > 1e-10
                    || v[6] < 0.
                    || v[6] > 1.
                    || v[7..].iter().any(|n| !(-1.0..=1.0).contains(n))
                {
                    return Err(Error::Invalid(format!(
                        "physical field semantics invalid in {name}"
                    )));
                }
                let u = 2.0 * (x as f64 + 0.5) / manifest.resolution as f64 - 1.;
                let w = 2.0 * (y as f64 + 0.5) / manifest.resolution as f64 - 1.;
                let d = cube_direction(face, u, w)?;
                let (expected, differential) = compiled.evaluate_with_differential(d)?;
                let normal = differential.outward_normal;
                let actual = [
                    expected.height_m,
                    expected.humidity,
                    expected.temperature_k,
                    expected.weights[0],
                    expected.weights[1],
                    expected.weights[2],
                    expected.support,
                    normal[0],
                    normal[1],
                    normal[2],
                ];
                if v.iter().zip(actual).any(|(a, b)| (a - b).abs() > 1e-10) {
                    return Err(Error::Invalid(format!(
                        "raw data does not reproduce from source graph in {name}"
                    )));
                }
            }
        }
    }
    let entry = manifest
        .files
        .iter()
        .find(|e| e.path == "graph.json")
        .ok_or_else(|| Error::Invalid("missing source graph entry".into()))?;
    if entry.bytes != graph_bytes.len() as u64 || entry.sha256 != sha(&graph_bytes) {
        return Err(Error::Invalid("source graph file manifest mismatch".into()));
    };
    Ok((sha(&manifest_bytes), manifest.sample_count as usize))
}
fn read_child(root: &Path, name: &str, limit: usize) -> Result<Vec<u8>, Error> {
    if name.contains('/') || name.contains('\\') || name == "." || name == ".." {
        return Err(Error::Invalid("unsafe bundle path".into()));
    };
    let p = root.join(name);
    let canonical = p.canonicalize()?;
    if canonical.parent() != Some(root) {
        return Err(Error::Invalid(
            "bundle path escaped output directory".into(),
        ));
    };
    let md = fs::metadata(&canonical)?;
    if !md.is_file() || md.len() > limit as u64 {
        return Err(Error::Invalid("bundle file exceeds size limit".into()));
    };
    use std::io::Read;
    let mut bytes = Vec::with_capacity(md.len() as usize);
    fs::File::open(canonical)?
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(Error::Invalid("bundle file grew beyond size limit".into()));
    }
    Ok(bytes)
}
fn file_entry(root: &Path, name: &str) -> Result<FileEntry, Error> {
    let d = fs::read(root.join(name))?;
    Ok(FileEntry {
        path: name.into(),
        bytes: d.len() as u64,
        sha256: sha(&d),
    })
}
fn sha(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn safe_id(id: &str) -> String {
    let mut s = id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(48)
        .collect::<String>();
    if s.is_empty() {
        s = "recipe".into()
    }
    s
}
fn unique_stamp() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::templates;

    #[test]
    fn small_graph_bundle_round_trips_and_rejects_raw_tampering() {
        let graph: Graph = serde_json::from_value(templates()[0]["graph"].clone()).unwrap();
        let root =
            std::env::temp_dir().join(format!("terrain-graph-bundle-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let (bundle, _, files) = publish(&graph, 4, &root).unwrap();
        assert_eq!(files, 8);
        verify(&bundle).unwrap();
        let manifest = bundle.join("manifest.json");
        let manifest_bytes = fs::read(&manifest).unwrap();
        let mut value: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        value["differential_step_rad"] = serde_json::json!(1.0e-5);
        fs::write(&manifest, serde_json::to_vec(&value).unwrap()).unwrap();
        assert!(verify(&bundle).is_err());
        fs::write(&manifest, manifest_bytes).unwrap();
        let raw = bundle.join("face-0.f64le");
        let mut bytes = fs::read(&raw).unwrap();
        bytes[0] ^= 0x01;
        fs::write(&raw, bytes).unwrap();
        assert!(verify(&bundle).is_err());
        let _ = fs::remove_dir_all(root);
    }
}
