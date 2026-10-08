use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{env, fs, io::Read, path::Path};
use terrain_field_prototype::{
    ALGORITHM_VERSION, Error, MAX_RESOLUTION, MAX_SAMPLES, Recipe, Seeds, cube_direction,
    graph::{CompiledGraph, Graph, MAX_GRAPH_BYTES},
    graph_bundle, server,
};

#[derive(Serialize, Deserialize)]
struct Manifest {
    manifest_version: u32,
    recipe_id: String,
    recipe_sha256: String,
    geometry_identity: String,
    climate_identity: String,
    material_identity: String,
    material_response_identity: String,
    palette_identity: String,
    algorithm_version: String,
    reference_radius_m: f64,
    resolution: u32,
    sample_count: usize,
    coordinate_space: String,
    mapping: String,
    boundary_rules: String,
    preview_policy: String,
    face_orientation: Vec<Face>,
    endian: String,
    record_layout: Vec<Field>,
    record_stride_bytes: u32,
    seed_namespaces: Seeds,
    material_names: [String; 3],
    bounds: Bounds,
    files: Vec<FileEntry>,
}
#[derive(Serialize, Deserialize)]
struct Face {
    id: String,
    direction: String,
    u_axis: String,
    v_axis: String,
}
#[derive(Serialize, Deserialize)]
struct Field {
    name: String,
    unit: String,
    offset_bytes: u32,
    encoding: String,
}
#[derive(Serialize, Deserialize)]
struct Bounds {
    height_m: [f64; 2],
    observed_height_m: [f64; 2],
    temperature_k: [f64; 2],
    observed_temperature_k: [f64; 2],
    humidity: [f64; 2],
    material_weights: [f64; 2],
}
#[derive(Serialize, Deserialize)]
struct FileEntry {
    path: String,
    bytes: u64,
    sha256: String,
}
fn digest(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn read_bounded(path: &Path, limit: u64, expected_parent: Option<&Path>) -> Result<Vec<u8>, Error> {
    let canonical = path.canonicalize()?;
    if expected_parent.is_some_and(|parent| canonical.parent() != Some(parent)) {
        return Err(Error::Invalid(
            "file resolves outside bundle directory".into(),
        ));
    }
    let metadata = fs::metadata(&canonical)?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(Error::Invalid(format!(
            "file exceeds its bounded read limit: {}",
            path.display()
        )));
    }
    let file = fs::File::open(canonical)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(Error::Invalid(format!(
            "file grew beyond its read limit: {}",
            path.display()
        )));
    }
    Ok(bytes)
}
fn usage() {
    eprintln!(
        "usage:\n  terrain-field-prototype export <recipe.json> <new-output-dir> <resolution>\n  terrain-field-prototype verify <output-dir>\n  terrain-field-prototype graph-export <graph.json> <external-output-root> <resolution>\n  terrain-field-prototype graph-verify <bundle-dir>\n  terrain-field-prototype graph-sample <graph.json> <resolution> <face>\n  terrain-field-prototype serve <external-output-root> [port]"
    );
}
fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(2);
    }
}
fn run() -> Result<(), Error> {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("serve") if (2..=3).contains(&args.len()) => {
            let port = if args.len() == 3 {
                args[2]
                    .parse()
                    .map_err(|_| Error::Invalid("port must be 1..=65535".into()))?
            } else {
                4179u16
            };
            server::serve(Path::new(&args[1]), port)
        }
        Some("graph-export") if args.len() == 4 => {
            let resolution = args[3]
                .parse()
                .map_err(|_| Error::Invalid("resolution must be an integer".into()))?;
            let graph = read_graph(Path::new(&args[1]))?;
            let (path, sha, files) =
                graph_bundle::publish(&graph, resolution, Path::new(&args[2]))?;
            println!(
                "published graph bundle at {} ({files} files; manifest sha256 {sha})",
                path.display()
            );
            Ok(())
        }
        Some("graph-verify") if args.len() == 2 => {
            let (sha, samples) = graph_bundle::verify(Path::new(&args[1]))?;
            println!("verified graph bundle ({samples} samples; manifest sha256 {sha})");
            Ok(())
        }
        Some("graph-sample") if args.len() == 4 => {
            let resolution = args[2]
                .parse()
                .map_err(|_| Error::Invalid("resolution must be an integer".into()))?;
            let face = args[3]
                .parse()
                .map_err(|_| Error::Invalid("face must be an integer".into()))?;
            let graph = read_graph(Path::new(&args[1]))?;
            let compiled = CompiledGraph::compile(graph)?;
            let map = compiled.sample_map(resolution, face, None)?;
            println!("{}", serde_json::to_string(&map)?);
            Ok(())
        }
        Some("export") if args.len() == 4 => export(
            Path::new(&args[1]),
            Path::new(&args[2]),
            args[3]
                .parse()
                .map_err(|_| Error::Invalid("resolution must be an integer".into()))?,
        ),
        Some("verify") if args.len() == 2 => verify(Path::new(&args[1])),
        _ => {
            usage();
            Err(Error::Invalid("invalid command line".into()))
        }
    }
}
fn read_graph(path: &Path) -> Result<Graph, Error> {
    let bytes = read_bounded(path, MAX_GRAPH_BYTES as u64, None)?;
    let graph: Graph = serde_json::from_slice(&bytes)?;
    CompiledGraph::compile(graph.clone())?;
    Ok(graph)
}
fn export(recipe_path: &Path, output: &Path, resolution: u32) -> Result<(), Error> {
    if !(1..=MAX_RESOLUTION).contains(&resolution) {
        return Err(Error::Invalid(format!(
            "resolution must be 1..={MAX_RESOLUTION}"
        )));
    }
    let count = 6usize
        .checked_mul(resolution as usize)
        .and_then(|v| v.checked_mul(resolution as usize))
        .ok_or_else(|| Error::Invalid("sample count overflow".into()))?;
    if count > MAX_SAMPLES {
        return Err(Error::Invalid("sample limit exceeded".into()));
    }
    if output.exists() {
        return Err(Error::Invalid(
            "output already exists; refusing overwrite".into(),
        ));
    }
    let bytes = read_bounded(recipe_path, 128 * 1024, None)?;
    let recipe: Recipe = serde_json::from_slice(&bytes)?;
    recipe.validate()?;
    let output_abs = if output.is_absolute() {
        output.to_path_buf()
    } else {
        env::current_dir()?.join(output)
    };
    let parent = output_abs
        .parent()
        .ok_or_else(|| Error::Invalid("output must have a parent directory".into()))?;
    fs::create_dir_all(parent)?;
    let stage = parent.join(format!(
        ".terrain-export-{}-{}.partial",
        std::process::id(),
        digest(recipe.recipe_id.as_bytes())[..10].to_owned()
    ));
    if stage.exists() {
        return Err(Error::Invalid("staging path already exists".into()));
    }
    fs::create_dir(&stage)?;
    let result = export_into(&recipe, &bytes, resolution, &stage).and_then(|_| {
        if output_abs.exists() {
            return Err(Error::Invalid(
                "output appeared during export; refusing overwrite".into(),
            ));
        }
        fs::rename(&stage, &output_abs)?;
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&stage);
    }
    result?;
    println!(
        "exported {} at {}x{} per face: {}",
        recipe.recipe_id,
        resolution,
        resolution,
        output_abs.display()
    );
    Ok(())
}
fn export_into(r: &Recipe, source: &[u8], resolution: u32, stage: &Path) -> Result<(), Error> {
    let mut files = Vec::new();
    let mut min_h = f64::INFINITY;
    let mut max_h = f64::NEG_INFINITY;
    let mut min_t = f64::INFINITY;
    let mut max_t = f64::NEG_INFINITY;
    for (face, id) in ["px", "nx", "py", "ny", "pz", "nz"].iter().enumerate() {
        let mut raw = Vec::with_capacity(resolution as usize * resolution as usize * 6 * 8);
        let mut rgb = Vec::with_capacity(resolution as usize * resolution as usize * 3);
        for y in 0..resolution {
            for x in 0..resolution {
                let u = 2.0 * (x as f64 + 0.5) / resolution as f64 - 1.0;
                let v = 2.0 * (y as f64 + 0.5) / resolution as f64 - 1.0;
                let sample = r.evaluate(cube_direction(face, u, v)?)?;
                min_h = min_h.min(sample.height_m);
                max_h = max_h.max(sample.height_m);
                min_t = min_t.min(sample.temperature_k);
                max_t = max_t.max(sample.temperature_k);
                for value in [
                    sample.height_m,
                    sample.temperature_k,
                    sample.humidity,
                    sample.material_weights[0],
                    sample.material_weights[1],
                    sample.material_weights[2],
                ] {
                    raw.extend_from_slice(&value.to_le_bytes());
                }
                for c in sample.color_linear {
                    rgb.push((c.clamp(0.0, 1.0).sqrt() * 255.0).round() as u8);
                }
            }
        }
        let raw_name = format!("{id}.f64le");
        let raw_path = stage.join(&raw_name);
        fs::write(&raw_path, &raw)?;
        files.push(entry(&raw_path, &raw_name)?);
        let png_name = format!("{id}.png");
        let png_path = stage.join(&png_name);
        write_png(&png_path, resolution, &rgb)?;
        files.push(entry(&png_path, &png_name)?);
        let ranges = [
            [
                r.height.base_m - r.height.amplitude_m,
                r.height.base_m + r.height.amplitude_m,
            ],
            [
                (r.climate.mean_temperature_k - r.climate.temperature_variation_k).max(0.0),
                (r.climate.mean_temperature_k + r.climate.temperature_variation_k).min(5000.0),
            ],
            [
                (r.climate.humidity_mean - r.climate.humidity_variation).max(0.0),
                (r.climate.humidity_mean + r.climate.humidity_variation).min(1.0),
            ],
            [0.0, 1.0],
            [0.0, 1.0],
            [0.0, 1.0],
        ];
        let names = [
            "height",
            "temperature",
            "humidity",
            "weight-0",
            "weight-1",
            "weight-2",
        ];
        for channel in 0..6 {
            let mut map = Vec::with_capacity(resolution as usize * resolution as usize);
            let lo = ranges[channel][0];
            let hi = ranges[channel][1];
            for record in raw.as_chunks::<48>().0 {
                let value = f64::from_le_bytes(
                    record[channel * 8..channel * 8 + 8]
                        .try_into()
                        .map_err(|_| Error::Invalid("malformed internal sample record".into()))?,
                );
                let gray = if hi > lo {
                    ((value - lo) / (hi - lo)).clamp(0.0, 1.0)
                } else {
                    0.5
                };
                let encoded = (gray * 255.0).round() as u8;
                map.extend_from_slice(&[encoded, encoded, encoded]);
            }
            let map_name = format!("{id}-{}.png", names[channel]);
            let map_path = stage.join(&map_name);
            write_png(&map_path, resolution, &map)?;
            files.push(entry(&map_path, &map_name)?);
        }
    }
    let recipe_path = stage.join("recipe.json");
    fs::write(&recipe_path, source)?;
    files.push(entry(&recipe_path, "recipe.json")?);
    let manifest = Manifest {
        manifest_version:1, recipe_id:r.recipe_id.clone(), recipe_sha256:digest(source),
        geometry_identity:r.geometry_identity()?, climate_identity:r.climate_identity()?, material_identity:r.material_identity()?, material_response_identity:r.material_response_identity()?, palette_identity:r.palette_identity()?,
        algorithm_version:ALGORITHM_VERSION.into(), reference_radius_m:r.reference_radius_m, resolution,
        sample_count:6*resolution as usize*resolution as usize, coordinate_space:"normalized body-local direction; no world transform".into(),
        mapping:"cube projection; face texel centers mapped to normalized direction".into(),
        boundary_rules:"No halo samples are stored. No wrap is applied within a face. Cross-face consumers must map through canonical body-local directions and query both fields at the same direction; faces are never independently normalized. Texel samples are face centers.".into(),
        preview_policy:"PNG outputs are diagnostic only. Scalar maps use the same recipe-declared bounds on every face; colors encode square-rooted linear palette RGB to 8-bit values.".into(),
        face_orientation:vec![
            Face{id:"px".into(),direction:"+X".into(),u_axis:"-Z".into(),v_axis:"+Y".into()}, Face{id:"nx".into(),direction:"-X".into(),u_axis:"+Z".into(),v_axis:"+Y".into()},
            Face{id:"py".into(),direction:"+Y".into(),u_axis:"+X".into(),v_axis:"-Z".into()}, Face{id:"ny".into(),direction:"-Y".into(),u_axis:"+X".into(),v_axis:"+Z".into()},
            Face{id:"pz".into(),direction:"+Z".into(),u_axis:"+X".into(),v_axis:"+Y".into()}, Face{id:"nz".into(),direction:"-Z".into(),u_axis:"-X".into(),v_axis:"+Y".into()}],
        endian:"little".into(), record_stride_bytes:48, seed_namespaces:Seeds{geometry:r.seeds.geometry,climate:r.seeds.climate,material:r.seeds.material}, record_layout:vec![
            field("height","m",0),field("temperature","K",8),field("humidity","1",16),field("material_0","weight",24),field("material_1","weight",32),field("material_2","weight",40)],
        material_names:r.materials.names.clone(), bounds:Bounds{
            height_m:[r.height.base_m-r.height.amplitude_m,r.height.base_m+r.height.amplitude_m], observed_height_m:[min_h,max_h],
            temperature_k:[(r.climate.mean_temperature_k-r.climate.temperature_variation_k).max(0.0),(r.climate.mean_temperature_k+r.climate.temperature_variation_k).min(5000.0)], observed_temperature_k:[min_t,max_t],
            humidity:[(r.climate.humidity_mean-r.climate.humidity_variation).max(0.0),(r.climate.humidity_mean+r.climate.humidity_variation).min(1.0)], material_weights:[0.0,1.0]}, files,
    };
    let manifest_path = stage.join("manifest.json");
    fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
    Ok(())
}
fn field(name: &str, unit: &str, offset: u32) -> Field {
    Field {
        name: name.into(),
        unit: unit.into(),
        offset_bytes: offset,
        encoding: "IEEE-754 binary64 little-endian".into(),
    }
}
fn entry(path: &Path, name: &str) -> Result<FileEntry, Error> {
    let data = fs::read(path)?;
    Ok(FileEntry {
        path: name.into(),
        bytes: data.len() as u64,
        sha256: digest(&data),
    })
}
fn write_png(path: &Path, size: u32, data: &[u8]) -> Result<(), Error> {
    let file = fs::File::create(path)?;
    let mut encoder = png::Encoder::new(file, size, size);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(data)?;
    Ok(())
}
fn verify(output: &Path) -> Result<(), Error> {
    let canonical_output = output.canonicalize()?;
    let manifest_path = output.join("manifest.json");
    let data = read_bounded(&manifest_path, 128 * 1024, Some(&canonical_output))?;
    let m: Manifest = serde_json::from_slice(&data)?;
    if m.manifest_version != 1
        || !(1..=MAX_RESOLUTION).contains(&m.resolution)
        || m.sample_count != 6 * m.resolution as usize * m.resolution as usize
        || m.files.len() != 49
        || m.record_stride_bytes != 48
        || m.endian != "little"
        || m.algorithm_version != ALGORITHM_VERSION
        || m.face_orientation.len() != 6
        || m.record_layout.len() != 6
        || m.mapping != "cube projection; face texel centers mapped to normalized direction"
        || m.coordinate_space != "normalized body-local direction; no world transform"
        || !m.boundary_rules.starts_with("No halo samples are stored.")
    {
        return Err(Error::Invalid(
            "manifest version, layout, resolution, sample count, or file count is invalid".into(),
        ));
    }
    let recipe_bytes = read_bounded(
        &output.join("recipe.json"),
        128 * 1024,
        Some(&canonical_output),
    )?;
    let recipe: Recipe = serde_json::from_slice(&recipe_bytes)?;
    recipe.validate()?;
    let expected_fields = [
        ("height", "m", 0),
        ("temperature", "K", 8),
        ("humidity", "1", 16),
        ("material_0", "weight", 24),
        ("material_1", "weight", 32),
        ("material_2", "weight", 40),
    ];
    for (field, (name, unit, offset)) in m.record_layout.iter().zip(expected_fields) {
        if field.name != name
            || field.unit != unit
            || field.offset_bytes != offset
            || field.encoding != "IEEE-754 binary64 little-endian"
        {
            return Err(Error::Invalid(
                "manifest raw field semantics are invalid".into(),
            ));
        }
    }
    let expected_faces = [
        ("px", "+X", "-Z", "+Y"),
        ("nx", "-X", "+Z", "+Y"),
        ("py", "+Y", "+X", "-Z"),
        ("ny", "-Y", "+X", "+Z"),
        ("pz", "+Z", "+X", "+Y"),
        ("nz", "-Z", "-X", "+Y"),
    ];
    for (face, (id, direction, u_axis, v_axis)) in m.face_orientation.iter().zip(expected_faces) {
        if face.id != id
            || face.direction != direction
            || face.u_axis != u_axis
            || face.v_axis != v_axis
        {
            return Err(Error::Invalid(
                "manifest face orientation is invalid".into(),
            ));
        }
    }
    if digest(&recipe_bytes) != m.recipe_sha256
        || recipe.recipe_id != m.recipe_id
        || recipe.reference_radius_m != m.reference_radius_m
        || recipe.seeds.geometry != m.seed_namespaces.geometry
        || recipe.seeds.climate != m.seed_namespaces.climate
        || recipe.seeds.material != m.seed_namespaces.material
        || recipe.materials.names != m.material_names
        || recipe.geometry_identity()? != m.geometry_identity
        || recipe.climate_identity()? != m.climate_identity
        || recipe.material_identity()? != m.material_identity
        || recipe.material_response_identity()? != m.material_response_identity
        || recipe.palette_identity()? != m.palette_identity
    {
        return Err(Error::Invalid(
            "recipe or field identities do not match manifest".into(),
        ));
    }
    let expected_bounds = [
        recipe.height.base_m - recipe.height.amplitude_m,
        recipe.height.base_m + recipe.height.amplitude_m,
        (recipe.climate.mean_temperature_k - recipe.climate.temperature_variation_k).max(0.0),
        (recipe.climate.mean_temperature_k + recipe.climate.temperature_variation_k).min(5000.0),
        (recipe.climate.humidity_mean - recipe.climate.humidity_variation).max(0.0),
        (recipe.climate.humidity_mean + recipe.climate.humidity_variation).min(1.0),
    ];
    if m.bounds.height_m != [expected_bounds[0], expected_bounds[1]]
        || m.bounds.temperature_k != [expected_bounds[2], expected_bounds[3]]
        || m.bounds.humidity != [expected_bounds[4], expected_bounds[5]]
        || m.bounds.material_weights != [0.0, 1.0]
    {
        return Err(Error::Invalid(
            "manifest conservative channel bounds do not match recipe".into(),
        ));
    }
    let expected_len = (m.resolution as u64) * (m.resolution as u64) * 48;
    let mut expected_names = Vec::new();
    for face in ["px", "nx", "py", "ny", "pz", "nz"] {
        expected_names.push(format!("{face}.f64le"));
        expected_names.push(format!("{face}.png"));
        for channel in [
            "height",
            "temperature",
            "humidity",
            "weight-0",
            "weight-1",
            "weight-2",
        ] {
            expected_names.push(format!("{face}-{channel}.png"));
        }
    }
    expected_names.push("recipe.json".into());
    let mut actual_names: Vec<String> = m.files.iter().map(|f| f.path.clone()).collect();
    actual_names.sort();
    expected_names.sort();
    if actual_names != expected_names {
        return Err(Error::Invalid(
            "manifest file set is incomplete or unexpected".into(),
        ));
    }
    let mut observed = [
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    for f in &m.files {
        if f.path.contains('/')
            || f.path.contains('\\')
            || f.path == "."
            || f.path == ".."
            || f.bytes > 16 * 1024 * 1024
        {
            return Err(Error::Invalid(
                "unsafe path or excessive file size in manifest".into(),
            ));
        }
        let p = output.join(&f.path);
        let limit = if f.path.ends_with(".f64le") {
            expected_len
        } else if f.path.ends_with(".png") {
            512 * 1024
        } else if f.path == "recipe.json" {
            128 * 1024
        } else {
            0
        };
        let data = read_bounded(&p, limit, Some(&canonical_output))?;
        if data.len() as u64 != f.bytes || digest(&data) != f.sha256 {
            return Err(Error::Invalid(format!(
                "integrity check failed for {}",
                f.path
            )));
        }
        if f.path.ends_with(".f64le") {
            if data.len() as u64 != expected_len {
                return Err(Error::Invalid(format!(
                    "raw channel length invalid for {}",
                    f.path
                )));
            }
            for record in data.as_chunks::<48>().0 {
                let values: [f64; 6] = std::array::from_fn(|i| {
                    f64::from_le_bytes(record[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]))
                });
                if values.iter().any(|v| !v.is_finite())
                    || values[0] < expected_bounds[0]
                    || values[0] > expected_bounds[1]
                    || values[1] < expected_bounds[2]
                    || values[1] > expected_bounds[3]
                    || !(0.0..=1.0).contains(&values[2])
                    || values[3..].iter().any(|v| !(0.0..=1.0).contains(v))
                    || (values[3..].iter().sum::<f64>() - 1.0).abs() > 1.0e-12
                {
                    return Err(Error::Invalid(format!(
                        "raw sample violates channel bounds in {}",
                        f.path
                    )));
                }
                observed[0] = observed[0].min(values[0]);
                observed[1] = observed[1].max(values[0]);
                observed[2] = observed[2].min(values[1]);
                observed[3] = observed[3].max(values[1]);
            }
        }
        if f.path.ends_with(".png") && data.len() > 512 * 1024 {
            return Err(Error::Invalid(format!(
                "PNG file exceeds limit: {}",
                f.path
            )));
        }
        if f.path == "recipe.json" && data.len() > 128 * 1024 {
            return Err(Error::Invalid("saved recipe exceeds size limit".into()));
        }
    }
    let close = |a: f64, b: f64| (a - b).abs() <= a.abs().max(b.abs()).max(1.0) * 1.0e-12;
    if !close(m.bounds.observed_height_m[0], observed[0])
        || !close(m.bounds.observed_height_m[1], observed[1])
        || !close(m.bounds.observed_temperature_k[0], observed[2])
        || !close(m.bounds.observed_temperature_k[1], observed[3])
    {
        return Err(Error::Invalid(
            "manifest observed extrema do not match raw field samples".into(),
        ));
    }
    println!("verified {} files for {}", m.files.len(), m.recipe_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::read_bounded;
    use std::fs;

    #[test]
    fn bounded_read_rejects_oversized_and_out_of_bundle_files() {
        let root =
            std::env::temp_dir().join(format!("terrain-bounded-read-{}", std::process::id()));
        let bundle = root.join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        let inside = bundle.join("inside.bin");
        let outside = root.join("outside.bin");
        fs::write(&inside, [1, 2, 3, 4, 5]).unwrap();
        fs::write(&outside, [7, 8]).unwrap();
        assert!(read_bounded(&inside, 4, Some(&bundle.canonicalize().unwrap())).is_err());
        assert!(read_bounded(&outside, 4, Some(&bundle.canonicalize().unwrap())).is_err());
        let _ = fs::remove_file(inside);
        let _ = fs::remove_file(outside);
        let _ = fs::remove_dir(bundle);
        let _ = fs::remove_dir(root);
    }
}
