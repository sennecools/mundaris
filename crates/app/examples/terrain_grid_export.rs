//! Export a regular height grid of a canonical body's CPU-oracle surface for
//! the terrain scorecard (`ai/research/terrain/tools/terrain-scorecard`):
//!
//! `cargo run --release -p astrum_app --example terrain_grid_export -- <body> <dx> <dy> <dz> <cells> <spacing_m> <out.f32>`
//!
//! The grid lies on the tangent plane at body direction (dx, dy, dz); each
//! sample is the band-limited surface height (`ProducerRecipe::evaluate`) at
//! the grid spacing, so it matches what the GPU draws at that footprint.
//! Output: little-endian f32 heights, row-major, plus `<out>.json` with the
//! dimensions and spacing.
//!
//! World-map bodies include their archetype's landform relief. An optional
//! 8th argument names another landform set file to use instead, for tuning
//! on a scratch copy of `content/landforms`.
use astrum_app::shared_system::SharedTestSystem;
use astrum_world::terrain::SurfaceGenerator;
use astrum_world::terrain::landform::{LandformError, LandformSet, LandformSetFile, RecipeFile};
use glam::DVec3;

fn load_set(path: &std::path::Path) -> LandformSet {
    let dir = path.parent().expect("set directory").to_path_buf();
    let file: LandformSetFile = ron::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    LandformSet::compile(&file, |recipe| {
        let text = std::fs::read_to_string(dir.join(recipe)).map_err(|e| LandformError::Load {
            path: recipe.into(),
            message: e.to_string(),
        })?;
        ron::from_str::<RecipeFile>(&text).map_err(|e| LandformError::Load {
            path: recipe.into(),
            message: e.to_string(),
        })
    })
    .unwrap()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    assert!(
        args.len() == 8 || args.len() == 9,
        "usage: <body> <dx> <dy> <dz> <cells> <spacing_m> <out.f32> [landform set .ron]"
    );
    let body_name = &args[1];
    let centre = DVec3::new(
        args[2].parse().unwrap(),
        args[3].parse().unwrap(),
        args[4].parse().unwrap(),
    )
    .normalize();
    let cells: usize = args[5].parse().unwrap();
    let spacing: f64 = args[6].parse().unwrap();
    let out = std::path::PathBuf::from(&args[7]);

    let loaded = SharedTestSystem::load_canonical(std::num::NonZeroU64::new(97).unwrap()).unwrap();
    let (_, body) = loaded
        .system
        .bodies()
        .find(|(_, b)| b.name() == body_name.as_str())
        .expect("body in the canonical system");
    let radius = body.properties().reference_radius_m();
    // The body's own landforms come from its archetype; an 8th argument
    // replaces them (tuning experiments on a scratch copy of the content).
    let mut definition = body.surface_definition().expect("surface").clone();
    if let Some(path) = args.get(8) {
        let world = definition
            .world()
            .expect("landforms need a world-map body")
            .clone()
            .with_landforms(load_set(std::path::Path::new(path)));
        definition = definition.with_world(world).unwrap();
    }
    let recipe = SurfaceGenerator::new(&definition, radius)
        .unwrap()
        .producer_recipe()
        .unwrap();
    // Direction (0, 0, 0) → NaN after normalising: centre the window on the
    // highest macro terrain instead (Fibonacci search at a 20 km footprint).
    let centre = if centre.is_finite() {
        centre
    } else {
        let samples = 20_000;
        let golden = std::f64::consts::PI * (3.0 - 5.0f64.sqrt());
        (0..samples)
            .map(|k| {
                let y = 1.0 - 2.0 * (k as f64 + 0.5) / samples as f64;
                let r = (1.0 - y * y).sqrt();
                let a = golden * k as f64;
                DVec3::new(r * a.cos(), y, r * a.sin())
            })
            .max_by(|a, b| {
                let h = |d: &DVec3| {
                    recipe
                        .evaluate(*d, 20_000.0)
                        .map_or(f64::MIN, |s| s.height_m)
                };
                h(a).total_cmp(&h(b))
            })
            .unwrap()
    };
    eprintln!("centre direction {centre:?}");
    if let astrum_world::terrain::producer::ProducerRecipe::World(world) = &recipe
        && let Some(fields) = astrum_world::terrain::landform::world_source::MapsFields::new(
            world.field.maps().unwrap(),
            radius,
        )
    {
        let f = fields.rule_fields(centre);
        eprintln!(
            "centre fields: uplift {:.2} sediment {:.2} moisture {:.2} temperature {:.1} hardness {:.2} macro slope {:.3} elevation {:.0} m |boundary| {:.0} m",
            f[0], f[1], f[2], f[4], f[6], f[7], f[8], f[9]
        );
    }
    let east = centre.any_orthonormal_vector();
    let north = centre.cross(east);
    let half = cells as f64 * spacing / 2.0;
    // ASTRUM_EXPORT_RIVERS=1: carve rivers (M3 hydrology on the CPU maps,
    // prototype parameters) after the landforms, as the GPU producer does.
    let rivers = (std::env::var("ASTRUM_EXPORT_RIVERS").as_deref() == Ok("1")).then(|| {
        use astrum_world::terrain::hydrology::{self, HydrologyInput, HydrologyParams};
        let astrum_world::terrain::producer::ProducerRecipe::World(world) = &recipe else {
            panic!("rivers need a world-map body");
        };
        let maps = world.field.maps().unwrap();
        hydrology::run(&HydrologyInput {
            elevation: &maps.fields.elevation,
            moisture: &maps.fields.moisture,
            radius_m: radius,
            params: HydrologyParams::prototype(),
        })
    });
    let started = std::time::Instant::now();
    let mut heights = vec![0.0f32; cells * cells];
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let rows_per = cells.div_ceil(threads);
    std::thread::scope(|scope| {
        for (chunk, rows) in heights.chunks_mut(rows_per * cells).enumerate() {
            let (recipe, rivers) = (&recipe, &rivers);
            scope.spawn(move || {
                for (k, out) in rows.iter_mut().enumerate() {
                    let (i, j) = (k % cells, chunk * rows_per + k / cells);
                    let u = (i as f64 + 0.5) * spacing - half;
                    let v = (j as f64 + 0.5) * spacing - half;
                    let d = (centre * radius + east * u + north * v).normalize();
                    let h = recipe.evaluate(d, spacing).unwrap().height_m;
                    *out = rivers.as_ref().map_or(h, |r| r.carve(d, h).height_m) as f32;
                }
            });
        }
    });
    let bytes: Vec<u8> = heights.iter().flat_map(|h| h.to_le_bytes()).collect();
    std::fs::write(&out, bytes).unwrap();
    let meta = format!(
        "{{\"body\":\"{body_name}\",\"centre\":[{},{},{}],\"width\":{cells},\"height\":{cells},\"dx\":{spacing},\"dy\":{spacing}}}\n",
        centre.x, centre.y, centre.z
    );
    std::fs::write(out.with_extension("json"), meta).unwrap();
    eprintln!(
        "{body_name}: {cells}² samples at {spacing} m in {:.1} s → {}",
        started.elapsed().as_secs_f64(),
        out.display()
    );
}
