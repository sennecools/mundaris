//! Deterministic CPU terrain inspection, independent of patch UV and renderer.
use anyhow::Result;
use glam::DVec3;
use mundaris_app::planet_terrain::checkpoint_terrain_definition_version;
use mundaris_math::{Direction3, surface::SurfaceLocation};
use mundaris_world::terrain::*;
use std::{fs, path::Path};

const RADIUS: f64 = 6_371_000.0;
const SIZE: usize = 256;

fn sample(field: &TerrainGenerator, n: DVec3, footprint: f64) -> Result<TerrainSample> {
    Ok(field.evaluate_point(TerrainQuery {
        location: SurfaceLocation::new(Direction3::try_new(n)?),
        footprint: TerrainFootprint::new(footprint)?,
    })?)
}
fn bitmap(path: &Path, pixels: &[[u8; 3]]) -> Result<()> {
    let bytes = SIZE * SIZE * 3;
    let mut output = vec![0u8; 54];
    output[..2].copy_from_slice(b"BM");
    output[2..6].copy_from_slice(&((54 + bytes) as u32).to_le_bytes());
    output[10..14].copy_from_slice(&54u32.to_le_bytes());
    output[14..18].copy_from_slice(&40u32.to_le_bytes());
    output[18..22].copy_from_slice(&(SIZE as i32).to_le_bytes());
    output[22..26].copy_from_slice(&(-(SIZE as i32)).to_le_bytes());
    output[26..28].copy_from_slice(&1u16.to_le_bytes());
    output[28..30].copy_from_slice(&24u16.to_le_bytes());
    for pixel in pixels {
        output.extend_from_slice(&[pixel[2], pixel[1], pixel[0]]);
    }
    fs::write(path, output)?;
    Ok(())
}
fn main() -> Result<()> {
    let directory = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "target/phase55-captures".into());
    fs::create_dir_all(&directory)?;
    let a = TerrainGenerator::new(
        &checkpoint_terrain_definition_version(RADIUS, TerrainGeneratorVersion::V1)?,
        RADIUS,
    )?;
    let b = TerrainGenerator::new(
        &checkpoint_terrain_definition_version(RADIUS, TerrainGeneratorVersion::V2)?,
        RADIUS,
    )?;
    let definition = checkpoint_terrain_definition_version(RADIUS, TerrainGeneratorVersion::V2)?;
    let mut bands = TerrainBand::ALL.map(|band| definition.config().band(band));
    for band in &mut bands[2..] {
        *band = TerrainBandConfig::new(0.0, band.scale(), band.octaves())?;
    }
    let broad_definition = TerrainDefinition::new(
        definition.identity(),
        definition.seed(),
        definition.version(),
        TerrainConfig::new(bands, definition.config().controls())?
            .with_erosion(ErosionConfig::new(3, 0.0)?),
    );
    let broad = TerrainGenerator::new(&broad_definition, RADIUS)?;
    // Search reproducible directions, not patch/cube coordinates, for a high
    // mountain face. Selection is shared by both A/B captures.
    let mut center = DVec3::X;
    let mut largest_slope = 0.0;
    for i in 0..4096 {
        let z = 1.0 - 2.0 * (i as f64 + 0.5) / 4096.0;
        let angle = i as f64 * 2.399963229728653;
        let n = DVec3::new(
            (1.0 - z * z).sqrt() * angle.cos(),
            (1.0 - z * z).sqrt() * angle.sin(),
            z,
        );
        let s = sample(&broad, n, 0.0)?;
        let slope = s.tangent_gradient_m_per_unit_direction().length();
        if slope > largest_slope {
            largest_slope = slope;
            center = n;
        }
    }
    eprintln!(
        "shared seed=17 center={center:?}, broad slope={}",
        largest_slope / RADIUS
    );
    let tangent_x = center
        .cross(if center.x.abs() < 0.8 {
            DVec3::X
        } else {
            DVec3::Y
        })
        .normalize();
    let tangent_y = center.cross(tangent_x);
    for (label, field) in [("legacy", &a), ("erosion", &b)] {
        let mut pixels = Vec::with_capacity(SIZE * SIZE);
        for row in 0..SIZE {
            for col in 0..SIZE {
                let u = ((col as f64 + 0.5) / SIZE as f64 - 0.5) * 2.1;
                let v = ((row as f64 + 0.5) / SIZE as f64 - 0.5) * 2.1;
                if u * u + v * v >= 1.0 {
                    pixels.push([8, 8, 16]);
                    continue;
                }
                let n = center * (1.0 - u * u - v * v).sqrt() + tangent_x * u + tangent_y * v;
                let s = sample(field, n, 50_000.0)?;
                let value = (s.height_m() / 5000.0 + 0.5).clamp(0.0, 1.0);
                let light = (center * 0.7 + tangent_x * 0.5 + tangent_y * 0.3).normalize();
                let shade = 0.2 + 0.8 * n.dot(light).max(0.0);
                pixels.push([
                    (value * 220.0 * shade) as u8,
                    ((0.3 + value * 0.7) * 200.0 * shade) as u8,
                    ((1.0 - value) * 200.0 * shade) as u8,
                ]);
            }
        }
        bitmap(
            &Path::new(&directory).join(format!("planet-{label}.bmp")),
            &pixels,
        )?;
    }
    let mut peak = center;
    let mut valley = center;
    let (mut highest, mut lowest) = (f64::NEG_INFINITY, f64::INFINITY);
    for i in 0..32 {
        for j in 0..32 {
            let n = (center
                + tangent_x * (i as f64 / 31.0 - 0.5) * 256_000.0 / RADIUS
                + tangent_y * (j as f64 / 31.0 - 0.5) * 256_000.0 / RADIUS)
                .normalize();
            let height = sample(&broad, n, 0.0)?.height_m();
            if height > highest {
                highest = height;
                peak = n;
            }
            if height < lowest {
                lowest = height;
                valley = n;
            }
        }
    }
    let views = [
        ("continent", center, 2_000_000.0, 10_000.0),
        ("range", center, 256_000.0, 100.0),
        ("mountain_face", center, 64_000.0, 10.0),
        ("gully", center, 16_000.0, 2.0),
        ("peak", peak, 64_000.0, 10.0),
        ("valley", valley, 64_000.0, 10.0),
        (
            "face_boundary",
            DVec3::new(1.0, 1.0, 0.3).normalize(),
            256_000.0,
            100.0,
        ),
        ("corner", DVec3::ONE.normalize(), 256_000.0, 100.0),
        ("same_face_rho1000", center, 64_000.0, 1000.0),
        ("same_face_rho100", center, 64_000.0, 100.0),
        ("same_face_rho2", center, 64_000.0, 2.0),
    ];
    for (name, n, width, rho) in views {
        let x = n
            .cross(if n.x.abs() < 0.8 { DVec3::X } else { DVec3::Y })
            .normalize();
        let y = n.cross(x);
        let light = (n * 0.3 + x * 0.5 + y * 0.8).normalize();
        for (label, field) in [("legacy", &a), ("erosion", &b)] {
            let mut pixels = Vec::with_capacity(SIZE * SIZE);
            for row in 0..SIZE {
                for col in 0..SIZE {
                    let q = (n
                        + x * ((col as f64 + 0.5) / SIZE as f64 - 0.5) * width / RADIUS
                        + y * ((row as f64 + 0.5) / SIZE as f64 - 0.5) * width / RADIUS)
                        .normalize();
                    let s = sample(field, q, rho)?;
                    let normal = s
                        .normal_body(SurfaceLocation::new(Direction3::try_new(q)?), RADIUS)?
                        .unit();
                    // Deliberately exaggerate relief lighting for inspecting
                    // gully coherence, not a physical renderer or normal claim.
                    let inspection = (q - (q - normal) * 20.0).normalize();
                    let shade = (0.2 + 0.8 * inspection.dot(light).max(0.0)) * 255.0;
                    pixels.push([shade as u8; 3]);
                }
            }
            bitmap(
                &Path::new(&directory).join(format!("{name}-{label}.bmp")),
                &pixels,
            )?;
            eprintln!("captured {name}-{label}, width={width}m rho={rho}m");
        }
    }
    Ok(())
}
