//! Bounded lossless RLE storage/access experiment for the canonical terrain bodies.
//!
//! This example is intentionally manual: it writes `results.json` only inside a
//! caller-supplied directory that must not already exist. It is not a native or
//! full-frame performance test.

use anyhow::{Context, ensure};
use glam::DVec3;
use mundaris_app::resident_terrain::{
    FieldDensity, ResidentTileBuilder, SharedFieldPages, TileBuildIdentity, TileData, TileTexel,
};
use mundaris_app::shared_system::SharedTestSystem;
use mundaris_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, SurfaceLocation},
};
use mundaris_world::terrain::SurfaceGenerator;
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::Write,
    num::NonZeroU64,
    path::PathBuf,
    sync::Arc,
    time::Instant,
};

const TILE_CELLS: u32 = 32;
const FIELD_CACHE_BYTES: usize = 140_000;
const INDEX_STRIDE_RUNS: usize = 64;
const RANDOM_READS: usize = 32_768;
const NOISY_VALUES: usize = 65_536;

#[derive(Debug, Clone, Copy, PartialEq)]
struct RawFieldValue {
    radius_m: f64,
    material: [f64; 4],
}

#[derive(Debug, Clone, Copy)]
struct Run<T> {
    value: T,
    length: u32,
}

#[derive(Debug, Clone, Copy, Serialize)]
struct RunIndex {
    logical_start: u32,
    run_index: u32,
}

struct Encoded<T> {
    runs: Vec<Run<T>>,
    index: Vec<RunIndex>,
    logical_len: usize,
}

#[derive(Debug, Serialize)]
struct AccessMetrics {
    reads: usize,
    service_ns: u128,
    checksum: u64,
}

#[derive(Debug, Serialize)]
struct DatasetMetrics {
    name: String,
    elements: usize,
    element_bytes: usize,
    input_payload_bytes: usize,
    input_vector_metadata_bytes: usize,
    runs: usize,
    run_payload_bytes_including_padding: usize,
    run_vector_capacity_bytes: usize,
    index_entries: usize,
    index_bytes: usize,
    index_vector_capacity_bytes: usize,
    encoded_container_metadata_bytes: usize,
    encoded_total_bytes: usize,
    encoded_total_capacity_bytes: usize,
    encoded_to_input_ratio: f64,
    encode_service_ns: u128,
    decode_service_ns: u128,
    temporary_decoded_payload_bytes: usize,
    temporary_decoded_vector_metadata_bytes: usize,
    sequential_logical_read: AccessMetrics,
    random_read: AccessMetrics,
    halo_read: AccessMetrics,
}

#[derive(Debug, Serialize)]
struct TileBuildRecord {
    face: String,
    address_level: u8,
    address_xy: [u32; 2],
    elapsed_ns: u128,
    authority_query_count: u64,
    tile_payload_bytes: usize,
}

#[derive(Debug, Serialize)]
struct BodyRecord {
    semantic_id: String,
    identity: u64,
    terrain_definition_sha256: Option<String>,
    terrain_definition_revision: Option<u32>,
    algorithm: String,
    reference_radius_m: f64,
    field_density_cells: u32,
    field_cache_statistics: mundaris_app::resident_terrain::FieldStatistics,
    tile_builds: Vec<TileBuildRecord>,
    source_field: DatasetMetrics,
    resident_tile_texels: DatasetMetrics,
}

#[derive(Debug, Serialize)]
struct ExperimentReport {
    schema: u32,
    experiment: &'static str,
    scene_sha256: String,
    tile_cells: u32,
    field_cache_capacity_bytes_requested: usize,
    index_stride_runs: usize,
    random_reads_per_dataset: usize,
    bodies: Vec<BodyRecord>,
    synthetic_worst_case_noisy_field: DatasetMetrics,
    caveats: Vec<&'static str>,
}

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args_os().skip(1);
    let output_directory = args.next().map(PathBuf::from).context(
        "usage: cargo run -p mundaris_app --example generation_storage -- <new-output-directory>",
    )?;
    ensure!(
        args.next().is_none(),
        "expected exactly one output-directory argument"
    );
    // create_dir is an atomic refusal when the selected directory already exists.
    fs::create_dir(&output_directory).with_context(|| {
        format!(
            "creating new output directory {}",
            output_directory.display()
        )
    })?;

    let shared =
        SharedTestSystem::load_canonical(NonZeroU64::new(1).context("invalid namespace")?)?;
    let scene_sha256 = shared.scene_sha256.clone();
    let mut bodies = Vec::with_capacity(2);
    for semantic_id in ["moon", "rust"] {
        let presentation_index = shared
            .presentation
            .iter()
            .position(|entry| entry.semantic_id == semantic_id)
            .with_context(|| {
                format!("canonical shared system has no {semantic_id} presentation")
            })?;
        let (_, body) = shared
            .system
            .bodies()
            .nth(presentation_index)
            .with_context(|| format!("canonical shared system has no body for {semantic_id}"))?;
        let presentation = &shared.presentation[presentation_index];
        let definition = body
            .surface_definition()
            .with_context(|| format!("canonical body {semantic_id} has no surface definition"))?;
        let radius_m = body.properties().reference_radius_m();
        let generator = Arc::new(SurfaceGenerator::new(definition, radius_m)?);
        let identity = TileBuildIdentity {
            body_identity: presentation.identity,
            surface_revision: u64::from(presentation.definition_revision.unwrap_or(0)),
            material_revision: u64::from(presentation.definition_revision.unwrap_or(0)),
        };
        let pages = SharedFieldPages::new(
            Arc::clone(&generator),
            identity,
            FieldDensity::Cells32,
            FIELD_CACHE_BYTES,
        )?;

        let mut tile_builds = Vec::with_capacity(CubeFace::ALL.len());
        let mut representative_tile: Option<TileData> = None;
        for face in CubeFace::ALL {
            let address = CubePatchAddress::root(face);
            let started = Instant::now();
            let (tile, diagnostics) = ResidentTileBuilder::build_fields(
                &generator, identity, address, TILE_CELLS, &pages,
            )?;
            let elapsed_ns = started.elapsed().as_nanos();
            if face == CubeFace::PositiveZ {
                representative_tile = Some(tile.clone());
            }
            tile_builds.push(TileBuildRecord {
                face: format!("{face:?}"),
                address_level: address.level(),
                address_xy: address.coordinates(),
                elapsed_ns,
                authority_query_count: diagnostics.authoritative_query_count,
                tile_payload_bytes: tile.retained_payload_bytes(),
            });
        }
        let representative_tile = representative_tile.context("PositiveZ tile was not built")?;
        let source_field = raw_field_grid(
            &generator,
            CubePatchAddress::root(CubeFace::PositiveZ),
            FieldDensity::Cells32,
        )?;
        let source_metrics = measure_dataset(
            "canonical_source_field_nodes",
            &source_field,
            same_raw_field_bits,
            raw_field_checksum,
            &[],
            RANDOM_READS,
        );
        let halo = tile_halo_indices(TILE_CELLS);
        let tile_metrics = measure_dataset(
            "resident_tile_texels",
            &representative_tile.texels,
            same_texel_bits,
            texel_checksum,
            &halo,
            RANDOM_READS,
        );
        bodies.push(BodyRecord {
            semantic_id: semantic_id.to_owned(),
            identity: presentation.identity,
            terrain_definition_sha256: presentation.definition_sha256.clone(),
            terrain_definition_revision: presentation.definition_revision,
            algorithm: definition.terrain().algorithm().name().to_owned(),
            reference_radius_m: radius_m,
            field_density_cells: FieldDensity::Cells32.cells(),
            field_cache_statistics: pages.statistics(),
            tile_builds,
            source_field: source_metrics,
            resident_tile_texels: tile_metrics,
        });
    }

    let synthetic = noisy_field_values(NOISY_VALUES);
    let noisy_metrics = measure_dataset(
        "synthetic_worst_case_noisy_field",
        &synthetic,
        same_raw_field_bits,
        raw_field_checksum,
        &[],
        RANDOM_READS,
    );
    let report = ExperimentReport {
        schema: 1,
        experiment: "lossless_bitwise_rle_storage_and_access",
        scene_sha256,
        tile_cells: TILE_CELLS,
        field_cache_capacity_bytes_requested: FIELD_CACHE_BYTES,
        index_stride_runs: INDEX_STRIDE_RUNS,
        random_reads_per_dataset: RANDOM_READS,
        bodies,
        synthetic_worst_case_noisy_field: noisy_metrics,
        caveats: vec![
            "Raw field nodes are reconstructed from the same canonical integer cube directions and complete SurfaceGenerator queries; SharedFieldPages does not expose its private node array.",
            "The synthetic noisy dataset is a bounded storage/access stress input, not a body definition or terrain acceptance fixture.",
            "CPU encode/decode/access timings do not establish native frame-time or total-pipeline benefit.",
            "The cache-eviction count comes from the existing bounded field-page store while building all six canonical cube faces.",
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

fn raw_field_grid(
    generator: &SurfaceGenerator,
    address: CubePatchAddress,
    density: FieldDensity,
) -> anyhow::Result<Vec<RawFieldValue>> {
    let cells = u64::from(density.cells());
    let divisions = cells
        .checked_shl(u32::from(address.level()))
        .context("field grid divisions overflow")?;
    let [tile_x, tile_y] = address.coordinates();
    let side = density.cells() as usize + 1;
    let [normal, u, v] = address.face().basis();
    let mut values = Vec::with_capacity(side * side);
    for y in 0..=cells {
        for x in 0..=cells {
            let gx = u64::from(tile_x) * cells + x;
            let gy = u64::from(tile_y) * cells + y;
            let a = 2 * gx as i64 - divisions as i64;
            let b = 2 * gy as i64 - divisions as i64;
            let mut xyz: [i64; 3] = std::array::from_fn(|axis| {
                normal[axis] as i64 * divisions as i64 + u[axis] as i64 * a + v[axis] as i64 * b
            });
            while xyz.iter().all(|coordinate| coordinate % 2 == 0) {
                xyz = xyz.map(|coordinate| coordinate / 2);
            }
            let direction =
                Direction3::try_new(DVec3::new(xyz[0] as f64, xyz[1] as f64, xyz[2] as f64))?;
            let sample = generator.evaluate_point(SurfaceLocation::new(direction))?;
            values.push(RawFieldValue {
                radius_m: sample.radius_m(),
                material: sample.material_weights(),
            });
        }
    }
    ensure!(
        values.len() == side * side,
        "raw field grid has an invalid size"
    );
    Ok(values)
}

fn encode<T: Copy>(values: &[T], same: fn(&T, &T) -> bool) -> Encoded<T> {
    let mut runs: Vec<Run<T>> = Vec::new();
    let mut index = Vec::new();
    for (logical_position, value) in values.iter().enumerate() {
        if let Some(last) = runs.last_mut()
            && same(&last.value, value)
        {
            last.length += 1;
        } else {
            if runs.len().is_multiple_of(INDEX_STRIDE_RUNS) {
                index.push(RunIndex {
                    logical_start: logical_position as u32,
                    run_index: runs.len() as u32,
                });
            }
            runs.push(Run {
                value: *value,
                length: 1,
            });
        }
    }
    Encoded {
        runs,
        index,
        logical_len: values.len(),
    }
}

impl<T: Copy> Encoded<T> {
    fn get(&self, logical_index: usize) -> Option<T> {
        if logical_index >= self.logical_len || self.index.is_empty() {
            return None;
        }
        let upper = self
            .index
            .binary_search_by_key(&(logical_index as u32), |entry| entry.logical_start);
        let index_position = match upper {
            Ok(position) => position,
            Err(0) => 0,
            Err(position) => position - 1,
        };
        let entry = self.index[index_position];
        let mut start = entry.logical_start as usize;
        for run in self.runs.iter().skip(entry.run_index as usize) {
            let end = start + run.length as usize;
            if logical_index < end {
                return Some(run.value);
            }
            start = end;
        }
        None
    }

    fn decode(&self) -> Vec<T> {
        let mut output = Vec::with_capacity(self.logical_len);
        for run in &self.runs {
            output.extend(std::iter::repeat_n(run.value, run.length as usize));
        }
        output
    }
}

fn measure_dataset<T: Copy>(
    name: &str,
    values: &[T],
    same: fn(&T, &T) -> bool,
    checksum_value: fn(&T) -> u64,
    halo_indices: &[usize],
    random_reads: usize,
) -> DatasetMetrics {
    let encode_started = Instant::now();
    let encoded = encode(values, same);
    let encode_service_ns = encode_started.elapsed().as_nanos();

    let decode_started = Instant::now();
    let decoded = encoded.decode();
    let decode_service_ns = decode_started.elapsed().as_nanos();
    assert_eq!(decoded.len(), values.len(), "{name} decoded length changed");
    assert!(
        values.iter().zip(&decoded).all(|(a, b)| same(a, b)),
        "{name} RLE decode changed source bits"
    );

    let sequential_started = Instant::now();
    let mut sequential_checksum = 0u64;
    for run in &encoded.runs {
        for _ in 0..run.length {
            sequential_checksum = sequential_checksum.rotate_left(7) ^ checksum_value(&run.value);
        }
    }
    let sequential_logical_read = AccessMetrics {
        reads: values.len(),
        service_ns: sequential_started.elapsed().as_nanos(),
        checksum: std::hint::black_box(sequential_checksum),
    };

    let random_started = Instant::now();
    let mut random_checksum = 0u64;
    for index in 0..random_reads {
        let logical_index = deterministic_index(index, values.len(), 0xa076_1d64_78bd_642f);
        let value = encoded
            .get(logical_index)
            .expect("bounded index is readable");
        random_checksum = random_checksum.rotate_left(9) ^ checksum_value(&value);
    }
    let random_read = AccessMetrics {
        reads: random_reads,
        service_ns: random_started.elapsed().as_nanos(),
        checksum: std::hint::black_box(random_checksum),
    };

    let halo_started = Instant::now();
    let mut halo_checksum = 0u64;
    for &logical_index in halo_indices {
        let value = encoded.get(logical_index).expect("halo index is readable");
        halo_checksum = halo_checksum.rotate_left(11) ^ checksum_value(&value);
    }
    let halo_read = AccessMetrics {
        reads: halo_indices.len(),
        service_ns: halo_started.elapsed().as_nanos(),
        checksum: std::hint::black_box(halo_checksum),
    };

    let element_bytes = std::mem::size_of::<T>();
    let input_payload_bytes = std::mem::size_of_val(values);
    let run_payload_bytes = encoded.runs.len() * std::mem::size_of::<Run<T>>();
    let run_vector_capacity_bytes = encoded.runs.capacity() * std::mem::size_of::<Run<T>>();
    let index_bytes = encoded.index.len() * std::mem::size_of::<RunIndex>();
    let index_vector_capacity_bytes = encoded.index.capacity() * std::mem::size_of::<RunIndex>();
    let encoded_container_metadata_bytes = std::mem::size_of::<Encoded<T>>();
    let input_vector_metadata_bytes = std::mem::size_of::<Vec<T>>();
    let encoded_total_bytes = run_payload_bytes + index_bytes + encoded_container_metadata_bytes;
    DatasetMetrics {
        name: name.to_owned(),
        elements: values.len(),
        element_bytes,
        input_payload_bytes,
        input_vector_metadata_bytes,
        runs: encoded.runs.len(),
        run_payload_bytes_including_padding: run_payload_bytes,
        run_vector_capacity_bytes,
        index_entries: encoded.index.len(),
        index_bytes,
        index_vector_capacity_bytes,
        encoded_container_metadata_bytes,
        encoded_total_bytes,
        encoded_total_capacity_bytes: run_vector_capacity_bytes
            + index_vector_capacity_bytes
            + encoded_container_metadata_bytes,
        encoded_to_input_ratio: encoded_total_bytes as f64 / input_payload_bytes.max(1) as f64,
        encode_service_ns,
        decode_service_ns,
        temporary_decoded_payload_bytes: decoded.len() * element_bytes,
        temporary_decoded_vector_metadata_bytes: std::mem::size_of::<Vec<T>>(),
        sequential_logical_read,
        random_read,
        halo_read,
    }
}

fn tile_halo_indices(cells: u32) -> Vec<usize> {
    let side = cells as usize + 3;
    let last = side - 1;
    let mut indices = Vec::with_capacity(side * 4 - 4);
    for x in 0..side {
        indices.push(x);
        indices.push(last * side + x);
    }
    for y in 1..last {
        indices.push(y * side);
        indices.push(y * side + last);
    }
    indices
}

fn deterministic_index(index: usize, len: usize, seed: u64) -> usize {
    let mut value = seed ^ index as u64;
    value ^= value >> 30;
    value = value.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value ^= value >> 27;
    value = value.wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^= value >> 31;
    value as usize % len
}

fn noisy_field_values(count: usize) -> Vec<RawFieldValue> {
    let mut state = 0x05ee_dcaf_ed15_ca11_u64;
    let mut output = Vec::with_capacity(count);
    for _ in 0..count {
        let radius_m = 1_737_400.0 + next_unit(&mut state) * 200.0;
        let raw = std::array::from_fn::<_, 4, _>(|_| next_unit(&mut state));
        let total = raw.iter().sum::<f64>();
        output.push(RawFieldValue {
            radius_m,
            material: raw.map(|value| value / total),
        });
    }
    output
}

fn next_unit(state: &mut u64) -> f64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    (*state >> 11) as f64 * (1.0 / ((1u64 << 53) as f64))
}

fn same_raw_field_bits(a: &RawFieldValue, b: &RawFieldValue) -> bool {
    a.radius_m.to_bits() == b.radius_m.to_bits()
        && a.material
            .iter()
            .zip(b.material)
            .all(|(left, right)| left.to_bits() == right.to_bits())
}

fn raw_field_checksum(value: &RawFieldValue) -> u64 {
    value
        .material
        .iter()
        .fold(value.radius_m.to_bits(), |sum, weight| {
            sum.rotate_left(5) ^ weight.to_bits()
        })
}

fn same_texel_bits(a: &TileTexel, b: &TileTexel) -> bool {
    a.radial_offset_m.to_bits() == b.radial_offset_m.to_bits()
        && a.material
            .iter()
            .zip(b.material)
            .all(|(left, right)| left.to_bits() == right.to_bits())
}

fn texel_checksum(value: &TileTexel) -> u64 {
    value
        .material
        .iter()
        .fold(u64::from(value.radial_offset_m.to_bits()), |sum, weight| {
            sum.rotate_left(5) ^ u64::from(weight.to_bits())
        })
}
