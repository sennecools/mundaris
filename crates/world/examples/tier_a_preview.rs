//! Equirectangular PNG previews of a Tier A CPU bake (elevation, temperature,
//! moisture) for tuning archetypes:
//! `cargo run --release -p astrum_world --example tier_a_preview -- <archetype.ron> <seed> <face_cells> <out_dir>`
use astrum_world::terrain::{
    archetype::PlanetArchetype,
    tier_a::{TierAInputs, bake},
};
use glam::DVec3;
use std::{fs::File, io::BufWriter, path::Path};

fn write_png(path: &Path, width: u32, height: u32, rgb: &[u8]) {
    let file = BufWriter::new(File::create(path).expect("create png"));
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .and_then(|mut w| w.write_image_data(rgb))
        .expect("write png");
}

fn ramp(t: f64, stops: &[(f64, [f64; 3])]) -> [u8; 3] {
    let t = t.clamp(stops[0].0, stops[stops.len() - 1].0);
    for pair in stops.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if t <= b {
            let f = (t - a) / (b - a).max(1e-12);
            return std::array::from_fn(|i| ((ca[i] + (cb[i] - ca[i]) * f) * 255.0) as u8);
        }
    }
    [0; 3]
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let archetype: PlanetArchetype =
        ron::from_str(&std::fs::read_to_string(&args[1]).expect("read archetype"))
            .expect("parse archetype");
    archetype.validate().expect("valid archetype");
    let seed: u64 = args[2].parse().expect("seed");
    let n: usize = args[3].parse().expect("face cells");
    let out = Path::new(&args[4]);
    std::fs::create_dir_all(out).expect("output dir");
    let inputs = TierAInputs {
        params: archetype.sample(seed),
        stages: archetype.stages.clone(),
        radius_m: 338_950.0,
        pole: DVec3::Y,
        face_cells: n,
        landform_rules: Vec::new(),
    };
    let started = std::time::Instant::now();
    let fields = bake(&inputs).expect("bake");
    println!(
        "bake {n}^2: {:.2} s, sea level {:.4}, ocean {:.3} (target {:.3})",
        started.elapsed().as_secs_f64(),
        fields.sea_level,
        fields.ocean_fraction,
        inputs.params.ocean_coverage
    );
    let (w, h) = (1024u32, 512u32);
    let mut images = [vec![0u8; (w * h * 3) as usize], vec![0u8; (w * h * 3) as usize], vec![0u8; (w * h * 3) as usize]];
    for y in 0..h {
        let lat = std::f64::consts::FRAC_PI_2 - std::f64::consts::PI * (f64::from(y) + 0.5) / f64::from(h);
        for x in 0..w {
            let lon = std::f64::consts::TAU * (f64::from(x) + 0.5) / f64::from(w);
            let d = DVec3::new(lat.cos() * lon.cos(), lat.sin(), lat.cos() * lon.sin());
            let elevation = fields.elevation.bilinear(d);
            let k = ((y * w + x) * 3) as usize;
            let colours = [
                if elevation < 0.0 {
                    ramp(elevation / inputs.params.ocean_depth_m, &[(-1.0, [0.02, 0.05, 0.2]), (0.0, [0.2, 0.45, 0.7])])
                } else {
                    ramp(elevation / inputs.params.land_height_m, &[(0.0, [0.2, 0.5, 0.2]), (0.4, [0.6, 0.55, 0.3]), (1.0, [0.95, 0.95, 0.95])])
                },
                ramp(fields.temperature.bilinear(d), &[(-40.0, [0.2, 0.2, 0.9]), (0.0, [0.9, 0.9, 0.9]), (35.0, [0.9, 0.2, 0.1])]),
                ramp(fields.moisture.bilinear(d), &[(0.0, [0.8, 0.6, 0.3]), (0.5, [0.3, 0.7, 0.3]), (1.0, [0.1, 0.3, 0.8])]),
            ];
            for (image, colour) in images.iter_mut().zip(colours) {
                image[k..k + 3].copy_from_slice(&colour);
            }
        }
    }
    for (name, image) in ["elevation", "temperature", "moisture"].iter().zip(&images) {
        write_png(&out.join(format!("{name}.png")), w, h, image);
    }
}
