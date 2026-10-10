//! Flora placement: the GPU niche/forest functions (scatter_niche.wgsl with
//! the generated table scatter_species.wgsl) agree with the CPU reference
//! (astrum_flora::scatter), and the checked-in table matches the content.

use astrum_flora::niche::{Layer, Site};
use astrum_flora::scatter::{Placement, species_wgsl, value};
use astrum_flora::{SpeciesFile, load_species_dir};
use astrum_renderer::GpuContext;
use wgpu::util::DeviceExt;

const TABLE: &str = include_str!("../src/shaders/scatter_species.wgsl");
const NICHE: &str = include_str!("../src/shaders/scatter_niche.wgsl");

fn species() -> Vec<SpeciesFile> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../content/flora/species");
    load_species_dir(&dir).unwrap()
}

#[test]
fn flora_species_table_matches_the_content() {
    assert_eq!(
        TABLE.replace("\r\n", "\n"),
        species_wgsl(&species()),
        "content/flora/species changed: run `cargo run -p astrum_flora --example species_wgsl`"
    );
}

const KERNEL: &str = r#"
struct Probe {
    site: vec4<f32>,
    extra: vec4<f32>,
}
@group(0) @binding(0) var<storage, read> probes: array<Probe>;
@group(0) @binding(1) var<storage, read_write> results: array<vec4<f32>>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if i >= arrayLength(&probes) {
        return;
    }
    let p = probes[i];
    let s = FlSite(p.site.x, p.site.y, p.site.z, p.site.w, p.extra.x);
    let f = sc_forest(2u, p.extra.y, p.extra.z, 0.0, s);
    results[i * 4u] = vec4<f32>(f.tree, f.shrub, f.boulder, f.core);
    results[i * 4u + 1u] = vec4<f32>(f.canopy, f.shrubland, f.stature, sc_value(2u, p.extra.y / 25.0, p.extra.z / 25.0, 42u));
    let pc = fl_pick(FL_CANOPY_MASK, s, p.extra.w);
    let ps = fl_pick(FL_SHRUB_MASK, s, p.extra.w);
    results[i * 4u + 2u] = vec4<f32>(pc.x, pc.y, ps.x, ps.y);
    results[i * 4u + 3u] = vec4<f32>(f.color, f.cover);
    let far = sc_forest(2u, p.extra.y, p.extra.z, 30.0 + 300.0 * p.extra.w, s);
    results[arrayLength(&probes) * 4u + i * 2u] = vec4<f32>(far.tree, far.shrub, far.core, far.cover);
    results[arrayLength(&probes) * 4u + i * 2u + 1u] = vec4<f32>(far.stature, far.boulder, 0.0, 0.0);
}
"#;

fn gpu() -> Option<GpuContext> {
    if let Ok(c) = GpuContext::new().or_else(|_| GpuContext::new_software()) {
        return Some(c);
    }
    assert!(
        std::env::var("ASTRUM_SKIP_GPU_TESTS").as_deref() == Ok("1"),
        "no GPU or software adapter; set ASTRUM_SKIP_GPU_TESTS=1 to skip explicitly"
    );
    None
}

/// Deterministic probe sites spread over Rust-like climate ranges.
fn probes(n: usize) -> Vec<[f32; 8]> {
    (0..n)
        .map(|i| {
            let h = |salt: u32| astrum_flora::scatter::unit(astrum_flora::scatter::pcg3d([i as u32, salt, 977])[0]) as f32;
            [
                -25.0 + 65.0 * h(1),
                h(2),
                -300.0 + 4800.0 * h(3),
                1.3 * h(4),
                h(5),
                60000.0 * h(6),
                60000.0 * h(7),
                h(8),
            ]
        })
        .collect()
}

#[test]
fn gpu_forest_field_matches_the_cpu_reference() {
    let Some(ctx) = gpu() else {
        return;
    };
    let (device, queue) = (&ctx.device, &ctx.queue);
    let species = species();
    let placement = Placement { species: &species };
    let src = format!("{TABLE}\n{NICHE}\n{KERNEL}");
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("flora niche probe"),
        source: wgpu::ShaderSource::Wgsl(src.into()),
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("flora niche probe"),
        layout: None,
        module: &module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let input = probes(4096);
    let bytes: Vec<u8> = input.iter().flatten().flat_map(|v| v.to_le_bytes()).collect();
    let probe_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &bytes,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let out_size = (input.len() * 6 * 16) as u64;
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: out_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: out_size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: probe_buf.as_entire_binding() },
            wgpu::BindGroupEntry { binding: 1, resource: out.as_entire_binding() },
        ],
    });
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut pass = enc.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        pass.dispatch_workgroups((input.len() as u32).div_ceil(64), 1, 1);
    }
    enc.copy_buffer_to_buffer(&out, 0, &read, 0, out_size);
    queue.submit([enc.finish()]);
    read.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    let data: Vec<f32> = read.slice(..).get_mapped_range().unwrap().chunks_exact(4).map(|c| f32::from_le_bytes(c.try_into().unwrap())).collect();

    let tol = 2e-3;
    let mut worst = 0.0f64;
    let mut pick_mismatch = 0;
    let mut forest_sites = 0;
    for (i, p) in input.iter().enumerate() {
        let s = Site {
            temperature_c: p[0] as f64,
            moisture: p[1] as f64,
            height_m: p[2] as f64,
            slope: p[3] as f64,
            sediment: p[4] as f64,
        };
        let f = placement.forest(2, p[5] as f64, p[6] as f64, 0.0, &s);
        let g = &data[i * 16..i * 16 + 16];
        let pairs = [
            (f.tree, g[0]),
            (f.shrub, g[1]),
            (f.boulder, g[2]),
            (f.core, g[3]),
            (f.canopy, g[4]),
            (f.shrubland, g[5]),
            (f.stature, g[6]),
            (value(2, p[5] as f64 / 25.0, p[6] as f64 / 25.0, 42), g[7]),
            (f.color[0], g[12]),
            (f.color[1], g[13]),
            (f.color[2], g[14]),
            (f.cover, g[15]),
        ];
        let far = placement.forest(2, p[5] as f64, p[6] as f64, 30.0 + 300.0 * p[7] as f64, &s);
        let o = input.len() * 16 + i * 8;
        let gf = &data[o..o + 8];
        for (k, (c, gv)) in [(far.tree, gf[0]), (far.shrub, gf[1]), (far.core, gf[2]), (far.cover, gf[3]), (far.stature, gf[4]), (far.boulder, gf[5])].iter().enumerate() {
            let d = (c - *gv as f64).abs();
            worst = worst.max(d);
            assert!(d < tol, "probe {i} far field {k}: cpu {c} gpu {gv}");
        }
        for (k, (c, gv)) in pairs.iter().enumerate() {
            let d = (c - *gv as f64).abs();
            worst = worst.max(d);
            assert!(d < tol, "probe {i} field {k}: cpu {c} gpu {gv} site {p:?}");
        }
        if f.tree > 0.05 {
            forest_sites += 1;
        }
        for (layer, gi) in [(Layer::Canopy, 8), (Layer::Shrub, 10)] {
            let cpu = placement.pick(layer, &s, p[7] as f64).map_or(-1.0, |(k, _)| k as f64);
            if cpu != g[gi] as f64 {
                pick_mismatch += 1;
            }
        }
    }
    println!("worst |cpu - gpu| {worst:.2e}, pick mismatches {pick_mismatch} / {}, forest probes {forest_sites}", 2 * input.len());
    // Picks may flip only where the roll sits on a weight boundary (f32).
    assert!(pick_mismatch <= 4, "{pick_mismatch} species picks differ");
    assert!(forest_sites > 100, "probe set covers forests");
}
