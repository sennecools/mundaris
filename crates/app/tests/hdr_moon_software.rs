//! The canonical Moon frame through the production renderer on the software
//! fallback adapter (WARP on Windows): terrain atlas, sun shadow cascades,
//! GTAO, exposure, bloom, tonemap and overlays under wgpu validation, without
//! occupying the GPU. Slow (software rasterisation), so ignored by default:
//!
//! `cargo test -p mundaris_app --features developer-tools --test hdr_moon_software -- --ignored`
//!
//! Set `MUNDARIS_SMOKE_PNG=<dir>` to keep the frames as evidence.
#![cfg(feature = "developer-tools")]
use mundaris_app::GravityOrbitsDemo;
use mundaris_app::render_settings::{SettingValue, index_of};
use mundaris_app::studio::view::StudioAction;
use mundaris_renderer::{GpuContext, Renderer};

const WIDTH: u32 = 480;
const HEIGHT: u32 = 270;

fn read_scene(context: &GpuContext, renderer: &Renderer) -> Vec<u8> {
    let texture = renderer.scene_texture().expect("scene texture");
    let row = (WIDTH * 4).div_ceil(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
        * wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("Software scene readback"),
        size: u64::from(row * HEIGHT),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = context.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(HEIGHT),
            },
        },
        wgpu::Extent3d {
            width: WIDTH,
            height: HEIGHT,
            depth_or_array_layers: 1,
        },
    );
    context.queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    context
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for line in mapped.chunks_exact(row as usize) {
        rgba.extend_from_slice(&line[..(WIDTH * 4) as usize]);
    }
    rgba
}

fn dump(name: &str, rgba: &[u8]) {
    let Some(dir) = std::env::var_os("MUNDARIS_SMOKE_PNG") else {
        return;
    };
    let file = std::io::BufWriter::new(
        std::fs::File::create(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap(),
    );
    let mut encoder = png::Encoder::new(file, WIDTH, HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(rgba)
        .unwrap();
}

fn mean_luma(rgba: &[u8]) -> f64 {
    rgba.as_chunks::<4>()
        .0
        .iter()
        .map(|p| 0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2]))
        .sum::<f64>()
        / (rgba.len() / 4) as f64
}

#[test]
#[ignore = "software adapter: full canonical Moon HDR frame, about a minute"]
fn canonical_moon_frame_renders_lit_terrain_with_cascades_on_software_adapter() {
    let context = GpuContext::new_software().expect("software fallback adapter");
    let mut renderer = Renderer::new(&context);
    let mut demo = GravityOrbitsDemo::shared_test_system().unwrap();
    let scope = context
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    let render = |demo: &mut GravityOrbitsDemo, renderer: &mut Renderer, frames: usize| {
        for _ in 0..frames {
            demo.render(renderer, WIDTH, HEIGHT, 1.0).unwrap();
            context
                .device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .unwrap();
        }
    };
    // Bind, produce resident tiles and let exposure settle.
    let mut casters = 0;
    for _ in 0..40 {
        render(&mut demo, &mut renderer, 1);
        casters = renderer.shadow_report().casters.iter().sum::<u32>();
        if casters > 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    // Physical light: the canonical pose is just after local sunset, so the
    // terrain is starlit and exposure adapts down to the night floor.
    render(&mut demo, &mut renderer, 40);
    let night = read_scene(&context, &renderer);
    dump("moon-night-adapted", &night);
    for stat in &demo.studio_view().render_stats {
        eprintln!("{}: {}", stat.label, stat.value);
    }

    let set = |demo: &mut GravityOrbitsDemo, id: &str, value: SettingValue| {
        demo.studio_action(StudioAction::SetSetting(index_of(id).unwrap(), value));
    };
    set(
        &mut demo,
        "render.lighting.sun_mode",
        SettingValue::Choice(1),
    );
    render(&mut demo, &mut renderer, 30);
    let report = renderer.shadow_report();
    let lit = read_scene(&context, &renderer);
    dump("moon-studio-sun-default", &lit);
    // Cast-shadow regression: a low sun behind the hill on the left must
    // darken the scene compared to the same frame without shadows.
    set(
        &mut demo,
        "render.lighting.sun_azimuth",
        SettingValue::Float(255.0),
    );
    set(
        &mut demo,
        "render.lighting.sun_elevation",
        SettingValue::Float(4.0),
    );
    render(&mut demo, &mut renderer, 6);
    let low = read_scene(&context, &renderer);
    dump("moon-low-sun-left", &low);
    set(
        &mut demo,
        "render.shadows.enabled",
        SettingValue::Bool(false),
    );
    render(&mut demo, &mut renderer, 3);
    let low_unshadowed = read_scene(&context, &renderer);
    dump("moon-low-sun-left-no-shadows", &low_unshadowed);
    set(
        &mut demo,
        "render.shadows.enabled",
        SettingValue::Bool(true),
    );
    set(
        &mut demo,
        "render.lighting.sun_azimuth",
        SettingValue::Float(100.0),
    );
    set(
        &mut demo,
        "render.lighting.sun_elevation",
        SettingValue::Float(15.0),
    );
    let shadowed_drop = mean_luma(&low_unshadowed) - mean_luma(&low);
    let mut views = Vec::new();
    for (index, name) in [
        (8, "moon-shadow-cascades"),
        (7, "moon-ao"),
        (9, "moon-luminance"),
    ] {
        set(&mut demo, "render.view_mode", SettingValue::Choice(index));
        render(&mut demo, &mut renderer, 2);
        let image = read_scene(&context, &renderer);
        dump(name, &image);
        views.push(image);
    }
    set(&mut demo, "render.view_mode", SettingValue::Choice(0));
    set(
        &mut demo,
        "render.lighting.sun_elevation",
        SettingValue::Float(6.0),
    );
    render(&mut demo, &mut renderer, 20);
    dump("moon-studio-sun-6", &read_scene(&context, &renderer));
    set(
        &mut demo,
        "render.shadows.enabled",
        SettingValue::Bool(false),
    );
    render(&mut demo, &mut renderer, 3);
    dump(
        "moon-studio-sun-6-no-shadows",
        &read_scene(&context, &renderer),
    );

    let error = pollster::block_on(scope.pop());
    assert!(error.is_none(), "wgpu validation: {error:?}");
    eprintln!(
        "cascades {} casters {:?} splits {:?} texel {:?}; night mean luma {:.1}; studio mean luma {:.1}",
        report.cascades,
        report.casters,
        report.splits_m,
        report.texel_m,
        mean_luma(&night),
        mean_luma(&lit)
    );
    assert!(
        casters > 0 && report.cascades > 0,
        "terrain casts sun shadows"
    );
    eprintln!("low-sun shadow luma drop {shadowed_drop:.1}");
    assert!(
        shadowed_drop > 1.0,
        "cascaded shadows darken a low-sun frame"
    );
    assert!(
        mean_luma(&night) > 2.0,
        "exposure adapts to the starlit night side"
    );
    assert!(mean_luma(&lit) > 10.0, "terrain is lit by the studio sun");
    assert!(
        views.iter().all(|image| mean_luma(image) > 1.0),
        "debug views draw"
    );
}
