//! Discriminating filtered-field prototype; never used as world or render authority.
//! Compares bilinear reconstruction of complete canonical samples with the exact
//! seven-tap resident filter, retaining nearby samples in a bounded field cache.
use anyhow::{Context, Result, ensure};
use glam::DVec3;
use mundaris_app::resident_terrain::{ResidentTileBuilder, TileBuildIdentity};
use mundaris_math::{
    Direction3,
    surface::{CubeFace, CubePatchAddress, SurfaceLocation},
};
use mundaris_world::terrain::{
    SurfaceAlgorithm, SurfaceDefinition, SurfaceGenerator, TerrainIdentity, TerrainSeed,
};
use serde_json::json;
use std::{collections::HashMap, fs, path::PathBuf, time::Instant};

const CELLS: i64 = 32;
const SIDE: usize = 37;
const FIELD_CAP: usize = 8192;

#[derive(Clone, Copy)]
struct Value {
    radius: f64,
    material: [f64; 4],
}

fn direction(address: CubePatchAddress, i: i64, j: i64) -> Result<Direction3> {
    let denominator = CELLS << address.level();
    let [x, y] = address.coordinates();
    let [normal, u, v] = address.face().basis();
    let a = 2 * (CELLS * i64::from(x) + i) - denominator;
    let b = 2 * (CELLS * i64::from(y) + j) - denominator;
    let mut xyz = std::array::from_fn::<_, 3, _>(|axis| {
        normal[axis] as i64 * denominator + u[axis] as i64 * a + v[axis] as i64 * b
    });
    let mut divisor = denominator;
    while divisor > 1 && divisor % 2 == 0 && xyz.iter().all(|value| value % 2 == 0) {
        xyz = xyz.map(|value| value / 2);
        divisor /= 2;
    }
    Ok(Direction3::try_new(DVec3::new(
        xyz[0] as f64,
        xyz[1] as f64,
        xyz[2] as f64,
    ))?)
}

fn reconstruct(raw: &[Value], address: CubePatchAddress, n: DVec3) -> Result<Value> {
    let [normal, u, v] = address.face().basis();
    let denominator = n.dot(normal);
    ensure!(denominator > 0.0, "tap outside chart hemisphere");
    let [x, y] = address.coordinates();
    let scale = (1u64 << address.level()) as f64;
    let st = [
        (n.dot(u) / denominator + 1.0) * 0.5 * scale * CELLS as f64 - f64::from(x) * CELLS as f64
            + 2.0,
        (n.dot(v) / denominator + 1.0) * 0.5 * scale * CELLS as f64 - f64::from(y) * CELLS as f64
            + 2.0,
    ];
    ensure!(
        st.iter()
            .all(|coordinate| *coordinate >= 0.0 && *coordinate < (SIDE - 1) as f64),
        "tap outside bounded halo"
    );
    let [ix, iy] = st.map(|value| value.floor() as usize);
    let [tx, ty] = [st[0] - ix as f64, st[1] - iy as f64];
    let mut result = Value {
        radius: 0.0,
        material: [0.0; 4],
    };
    for (dx, dy, weight) in [
        (0, 0, (1.0 - tx) * (1.0 - ty)),
        (1, 0, tx * (1.0 - ty)),
        (0, 1, (1.0 - tx) * ty),
        (1, 1, tx * ty),
    ] {
        let value = raw[(iy + dy) * SIDE + ix + dx];
        result.radius += value.radius * weight;
        for (out, input) in result.material.iter_mut().zip(value.material) {
            *out += input * weight;
        }
    }
    Ok(result)
}

fn main() -> Result<()> {
    let output = PathBuf::from(
        std::env::args_os()
            .nth(1)
            .context("provide new output JSON path")?,
    );
    ensure!(!output.exists(), "refusing to overwrite evidence");
    let definition = SurfaceDefinition::generated(
        TerrainIdentity(0x4d4f_4f4e),
        TerrainSeed(0x4d4f_4f4e),
        SurfaceAlgorithm::RockyV5,
    );
    let identity = TileBuildIdentity {
        body_identity: 5,
        surface_revision: 0,
        material_revision: 0,
    };
    let mut rows = Vec::new();
    for radius in [1_737_400.0 * (400_000.0 / 6_371_000.0), 1_737_400.0] {
        for level in [4u8, 8, 12, 16] {
            let generator = SurfaceGenerator::new(&definition, radius)?;
            let mut context = generator.prepared_query_context();
            let mut field = HashMap::<[u64; 3], Value>::new();
            for offset in 0..2 {
                let address = CubePatchAddress::try_new(
                    CubeFace::PositiveZ,
                    level,
                    (1u32 << (level - 1)) + offset,
                    1u32 << (level - 1),
                )?;
                let start = Instant::now();
                let (exact, _) = ResidentTileBuilder::build_uncached(
                    &generator,
                    identity,
                    address,
                    CELLS as u32,
                )?;
                let exact_ms = start.elapsed().as_secs_f64() * 1000.0;
                let start = Instant::now();
                let mut raw = Vec::with_capacity(SIDE * SIDE);
                let mut queries = 0;
                let mut reused = 0;
                for j in -2..=CELLS + 2 {
                    for i in -2..=CELLS + 2 {
                        let d = direction(address, i, j)?;
                        let key = d.unit().to_array().map(f64::to_bits);
                        let value = if let Some(value) = field.get(&key) {
                            reused += 1;
                            *value
                        } else {
                            let sample = context.evaluate_point(SurfaceLocation::new(d))?;
                            queries += 1;
                            let value = Value {
                                radius: sample.radius_m(),
                                material: sample.material_weights(),
                            };
                            if field.len() < FIELD_CAP {
                                field.insert(key, value);
                            }
                            value
                        };
                        raw.push(value);
                    }
                }
                let mut max_radial_error = 0.0f64;
                let mut square_error = 0.0;
                let mut max_material_error = 0.0f64;
                let step = 2.0 / ((1u64 << level) as f64 * CELLS as f64) * 0.25;
                for j in -1..=CELLS + 1 {
                    for i in -1..=CELLS + 1 {
                        let n = direction(address, i, j)?.unit();
                        let mut filtered = reconstruct(&raw, address, n)?;
                        filtered.radius *= 0.25;
                        filtered.material = filtered.material.map(|value| value * 0.25);
                        for axis in [DVec3::X, DVec3::Y, DVec3::Z] {
                            let tangent = axis - n * n.dot(axis);
                            for sign in [1.0, -1.0] {
                                let tap = (n + tangent * (step * sign)).normalize();
                                let value = reconstruct(&raw, address, tap)?;
                                filtered.radius += value.radius * 0.125;
                                for (out, input) in filtered.material.iter_mut().zip(value.material)
                                {
                                    *out += input * 0.125;
                                }
                            }
                        }
                        let index = ((j + 1) * (CELLS + 3) + i + 1) as usize;
                        let reference = &exact.texels[index];
                        let error = (filtered.radius
                            - exact.anchor_radius_m
                            - f64::from(reference.radial_offset_m))
                        .abs();
                        max_radial_error = max_radial_error.max(error);
                        square_error += error * error;
                        let total = filtered.material.iter().sum::<f64>();
                        for (value, reference) in filtered.material.iter().zip(reference.material) {
                            max_material_error = max_material_error
                                .max((value / total - f64::from(reference)).abs());
                        }
                    }
                }
                rows.push(json!({"radius_m":radius,"level":level,"neighbor_offset":offset,"exact_ms":exact_ms,"derived_ms":start.elapsed().as_secs_f64()*1000.0,"complete_queries":queries,"reused_unique_position_samples":reused,"retained_field_samples":field.len(),"max_radial_filter_error_m":max_radial_error,"rms_radial_filter_error_m":(square_error/1225.0).sqrt(),"max_material_weight_error":max_material_error}));
            }
        }
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        output,
        serde_json::to_string_pretty(
            &json!({"schema":"mundaris.phase2f.derived-discrimination.v1","production":false,"representation":"bilinear reconstruction of complete canonical sample fields at unchanged grid spacing; exact original 7tap directions/weights","bounds":{"field_sample_cap":FIELD_CAP,"field_payload_bound_bytes":FIELD_CAP*std::mem::size_of::<([u64;3],Value)>(),"scratch_samples":SIDE*SIDE},"limitations":["Approximate rendered filtering only; authority unchanged. No transition/native/GPU acceptance.","Same-face neighboring cache reuse only; face seam mismatch and displaced reconstruction need independent validation.","Timings serial release microbenchmark; not native whole-view arrival."],"rows":rows}),
        )?,
    )?;
    Ok(())
}
