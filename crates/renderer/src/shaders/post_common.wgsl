// Shared post-processing declarations (docs/RENDER_PIPELINE_HDR.md §2).
// Prepended to every post pass; binding 0 of group 0 is always `post`.

struct Post {
    size: vec4<f32>,           // full width, height, AO width, height (pixels)
    camera: vec4<f32>,         // near m, focal px, -, view mode
    ao: vec4<f32>,             // radius m, distance scale, intensity, enabled
    exposure: vec4<f32>,       // mode (0 auto, 1 manual), manual EV100, compensation, dt s
    exposure_range: vec4<f32>, // min EV100, max EV100, speed up, speed down (EV/s)
    bloom: vec4<f32>,          // enabled, intensity, radius, levels
    tonemap: vec4<f32>,        // operator, dither, frame index, debug passthrough
}

struct Exposure {
    value: f32,
    ev100: f32,
    previous: f32,
    luminance: f32,
}

struct FullscreenOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

const PI: f32 = 3.14159265359;
const VIEW_AO_ONLY: u32 = 10u;
const VIEW_LUMINANCE: u32 = 12u;

@vertex
fn vs_fullscreen(@builtin(vertex_index) index: u32) -> FullscreenOut {
    let uv = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    var out: FullscreenOut;
    out.position = vec4<f32>(uv * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.uv = uv;
    return out;
}

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// Interleaved gradient noise (Jimenez 2014), stable per pixel and frame.
fn ign(pixel: vec2<f32>, frame: f32) -> f32 {
    let p = pixel + 5.588238 * frame;
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}
