//! Production HDR frame on the software fallback adapter (WARP on Windows):
//! main MRT pass, GTAO, composite, exposure, bloom, tonemap and anti-aliased
//! overlays, under wgpu validation. Runs without occupying the GPU.
#![cfg(feature = "terrain-capture")]
use glam::DVec3;
use mundaris_math::*;
use mundaris_renderer::{
    CelestialFrame, CelestialLineStyle, CelestialPolyline, CelestialProjection,
    CelestialRenderBody, CelestialStaging, FrameLighting, Icosphere, PreparedView,
    RenderPrecisionBudget, RenderSettings, SurfaceMaterial, TerrainAtlasConfig, TerrainViewMode,
    Tonemapper, terrain_capture::TerrainCaptureRenderer,
};
use std::num::NonZeroU64;

const SIZE: u32 = 256;

/// Writes `image` (tightly packed RGBA8) as a PNG when `MUNDARIS_SMOKE_PNG`
/// names a directory; evidence for visual review, never an assertion.
fn dump(name: &str, width: u32, height: u32, image: &[u8]) {
    let Some(dir) = std::env::var_os("MUNDARIS_SMOKE_PNG") else {
        return;
    };
    let path = std::path::Path::new(&dir).join(format!("{name}.png"));
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(image)
        .unwrap();
}

fn pixel(image: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * SIZE + x) * 4) as usize;
    image[at..at + 4].try_into().unwrap()
}

#[test]
fn hdr_frame_validates_and_lights_a_sphere_on_the_software_adapter() {
    let Ok(mut renderer) = TerrainCaptureRenderer::new_software(SIZE, SIZE) else {
        eprintln!("no software fallback adapter on this platform; skipped");
        return;
    };
    eprintln!(
        "adapter: {} ({})",
        renderer.adapter_name(),
        renderer.adapter_backend()
    );
    renderer
        .validate_terrain_pipelines(TerrainAtlasConfig {
            cells: 64,
            draw_cells: 16,
            layers: 32,
            normal_scale: 2,
        })
        .expect("terrain draw and shadow caster pipelines validate");

    let mut tree = FrameTree::new(NonZeroU64::new(77).unwrap());
    let root = tree.root();
    let body_frame = tree
        .insert(
            root,
            FrameState::stationary(RigidTransform::new(
                Displacement3::try_metres(DVec3::new(0.0, 0.0, -10.0)).unwrap(),
                UnitRotation::identity(),
            )),
        )
        .unwrap();
    let view = PreparedView::new(
        &tree.evaluate(),
        FramePose::new(
            FramePosition::new(root, LocalPosition::origin()),
            UnitRotation::identity(),
        ),
        RenderPrecisionBudget::near_debug(),
    )
    .unwrap();
    let sphere = Icosphere::new();
    let projection = CelestialProjection::try_new(SIZE, SIZE, 60_f64.to_radians(), 0.1).unwrap();
    let line = [
        DVec3::new(-20.0, -4.0, -30.0),
        DVec3::new(20.0, -4.0, -30.0),
    ]
    .map(|p| FramePosition::new(root, LocalPosition::try_metres(p).unwrap()));
    let mut staging = CelestialStaging::default();
    let mut render = |renderer: &mut TerrainCaptureRenderer, mode: TerrainViewMode| {
        let mut frame = CelestialFrame::new(&view, &mut staging, projection, &sphere);
        frame.set_view_mode(mode);
        // Sun up and to the right, behind the camera: the sphere's face is lit.
        let sun = DVec3::new(2.0e6, 2.0e6, 2.0e6);
        frame
            .set_lighting(FrameLighting {
                sun_center_view_m: sun,
                sun_radius_m: 1.0e4,
                sun_color: [1.0, 0.97, 0.92],
                illuminance_lux: 100_000.0,
                reference_distance_m: sun.length(),
                ambient_lux: 1.0,
                ambient_color: [1.0; 3],
                bounce_fraction: 0.01,
                occluders: vec![(DVec3::new(0.0, 0.0, -10.0), 3.0)],
            })
            .unwrap();
        frame
            .append_bodies(&[CelestialRenderBody {
                body_fixed_frame: body_frame,
                reference_radius_m: 3.0,
                color: [0.5; 4],
                unlit: false,
                selected: false,
                material: SurfaceMaterial::default(),
                emission_nits: 0.0,
            }])
            .unwrap();
        frame
            .append_polylines(&[CelestialPolyline {
                points: &line,
                colors: &[[0.2, 0.8, 1.0, 1.0]; 2],
                width_pixels: 2.0,
                style: CelestialLineStyle::Solid,
            }])
            .unwrap();
        renderer
            .render(&frame)
            .expect("frame validates and renders")
    };
    // Several frames so auto exposure adapts from its initial snap.
    let mut image = Vec::new();
    for _ in 0..4 {
        image = render(&mut renderer, TerrainViewMode::Lit);
    }
    dump("lit", SIZE, SIZE, &image);
    let centre = pixel(&image, SIZE / 2, SIZE / 2);
    let corner = pixel(&image, 2, 2);
    assert!(centre[0] > 40, "lit sphere centre {centre:?}");
    let towards_sun = pixel(&image, SIZE * 2 / 3, SIZE / 3);
    let away = pixel(&image, SIZE / 3, SIZE * 2 / 3);
    assert!(
        towards_sun[0] > away[0] + 30,
        "sun-facing quadrant {towards_sun:?} vs terminator side {away:?}"
    );
    assert!(
        corner[0] < 8 && corner[1] < 8,
        "space stays black {corner:?}"
    );
    // The guide line crosses below the sphere: some row near it is blue-ish.
    let line_hit = (SIZE * 6 / 10..SIZE * 9 / 10).any(|y| pixel(&image, 4, y)[2] > 100);
    assert!(line_hit, "anti-aliased guide line drawn");

    // Every view mode and display path validates.
    for mode in TerrainViewMode::ALL {
        let image = render(&mut renderer, mode);
        dump(mode.name(), SIZE, SIZE, &image);
    }
    for (tonemap, ao, bloom, half_res, resolution) in [
        (Tonemapper::AcesFitted, false, false, false, 1024),
        (Tonemapper::Clamp, true, true, true, 4096),
    ] {
        let mut settings = RenderSettings {
            tonemap,
            ..Default::default()
        };
        settings.ao.enabled = ao;
        settings.ao.half_res = half_res;
        settings.bloom.enabled = bloom;
        settings.shadows.resolution = resolution;
        renderer.set_render_settings(settings).unwrap();
        render(&mut renderer, TerrainViewMode::Lit);
    }
}
