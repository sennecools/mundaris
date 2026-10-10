// PROTOTYPE (flora lane): grown species meshes and their impostors for the
// plants that cs_scatter routed into flora buckets (scatter_cull.wgsl). One
// indirect draw per (entry, variant, tier) bucket; each vertex carries its
// bucket id and the bucket's first instance slot comes from the header
// written by cs_flora_prefix. Appended to the draw shader after
// scatter_vs.wgsl (same `plants` binding and `shade`).
//
// Plants keep their size at every tier. Fades are screen-door dithers:
// e1.w = rank fade (plant dissolving as the thinning drops it), up.w = tier
// split of a cross-fade (> 0 keeps dither < s, < 0 keeps dither ≥ 1 + s), so
// the two tiers of one plant cover complementary pixels.

@group(3) @binding(1) var flora_albedo: texture_2d_array<f32>;
@group(3) @binding(2) var flora_normal: texture_2d_array<f32>;
@group(3) @binding(3) var flora_sampler: sampler;

const FLORA_HEADER_START: u32 = 32768u;
const IMPOSTOR_GRID: f32 = 4.0;

fn fl_bucket_base(b: u32) -> u32 {
    let h = plants[FLORA_HEADER_START + b / 4u].info;
    return select(select(select(h.w, h.z, b % 4u == 2u), h.y, b % 4u == 1u), h.x, b % 4u == 0u);
}

// Interleaved gradient noise, offset per plant (stable over frames).
fn fl_dither(px: vec2<f32>, seed: u32) -> f32 {
    let o = vec2<f32>(f32(seed & 63u), f32((seed >> 6u) & 63u));
    let q = floor(px) + o;
    return fract(52.9829189 * fract(0.06711056 * q.x + 0.00583715 * q.y));
}

// Two independent dithers: rank fade and tier split.
fn fl_keep(px: vec2<f32>, seed: u32, fade: f32, split: f32) -> bool {
    if fl_dither(px, seed) >= fade {
        return false;
    }
    let d = fl_dither(px.yx + vec2<f32>(17.0, 31.0), seed ^ 0x2c1b3c6du);
    return select(d >= 1.0 + split, (d < split), (split > 0.0));
}

// Main pass under MSAA: the fragment runs per sample (sample_index), and
// each sample shifts the pixel's dither thresholds by i·golden ratio, so a
// 4× pixel resolves fades in quarter steps instead of an on/off stipple. The
// split thresholds are the same for both tiers, so their samples stay
// complementary. With one sample it equals fl_keep.
fn fl_keep_sample(px: vec2<f32>, seed: u32, fade: f32, split: f32, si: u32) -> bool {
    let o = f32(si) * 0.618034;
    if fract(fl_dither(px, seed) + o) >= fade {
        return false;
    }
    let d = fract(fl_dither(px.yx + vec2<f32>(17.0, 31.0), seed ^ 0x2c1b3c6du) + o);
    return select(d >= 1.0 + split, (d < split), (split > 0.0));
}

struct FloraIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) pivot: vec3<f32>,
    @location(4) wind: vec4<u32>,
    @location(5) bucket: u32,
}

struct FloraOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) up: vec3<f32>,
    @location(3) albedo: vec3<f32>,
    // rank fade, tier split, seed (bitcast).
    @location(4) @interpolate(flat) fade: vec3<f32>,
}

@vertex
fn vs_flora(v: FloraIn, @builtin(instance_index) index: u32) -> FloraOut {
    let p = plants[fl_bucket_base(v.bucket) + index];
    let scale = p.base.w;
    let e1 = p.e1.xyz;
    let up = p.up.xyz;
    let e2 = cross(up, e1);
    let local = v.position * scale;
    let view_position = p.base.xyz + e1 * local.x + e2 * local.y + up * local.z;
    let nl = normalize(v.normal.xyz);
    var out: FloraOut;
    out.clip_position = projection.matrix * vec4<f32>(view_position, 1.0);
    out.view_pos = view_position;
    out.normal = e1 * nl.x + e2 * nl.y + up * nl.z;
    out.up = up;
    let c = v.color.rgb * v.color.rgb * v.color.a;
    let tint = 0.85 + 0.3 * sc_unit(p.info.z);
    let hue = sc_unit(p.info.w);
    var shift = vec3<f32>(1.0 + 0.2 * (hue - 0.5), 1.0, 1.0 - 0.25 * (hue - 0.5));
    // Hue jitter on organs only (bark, fruit and rock keep their colour).
    if v.wind.w != 1u {
        shift = vec3<f32>(1.0);
    }
    out.albedo = c * tint * shift;
    out.fade = vec3<f32>(p.e1.w, p.up.w, bitcast<f32>(p.info.w));
    return out;
}

@fragment
fn fs_flora(input: FloraOut, @builtin(sample_index) si: u32) -> SceneOut {
    if !fl_keep_sample(input.clip_position.xy, bitcast<u32>(input.fade.z), input.fade.x, input.fade.y, si) {
        discard;
    }
    var n = normalize(input.normal);
    // Organs are two-sided cards: light the side that faces the viewer.
    if dot(n, input.view_pos) > 0.0 {
        n = -n;
    }
    return shade(input.view_pos, n, input.up, input.albedo, 0.0, true);
}

// Shadow casters: same instances, same dither.
@fragment
fn fs_flora_shadow(input: FloraOut) {
    if !fl_keep(input.clip_position.xy, bitcast<u32>(input.fade.z), input.fade.x, input.fade.y) {
        discard;
    }
}

// ---------------------------------------------------------------- impostors

struct ImpostorIn {
    // corner (bits 0..3), bucket (bits 8..)
    @location(0) corner_bucket: u32,
    @location(1) centre: vec3<f32>,
    @location(2) radius: f32,
    @location(3) layer: u32,
}

struct ImpostorOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) layer: u32,
    @location(3) @interpolate(flat) e1: vec3<f32>,
    @location(4) @interpolate(flat) e2: vec3<f32>,
    @location(5) @interpolate(flat) up: vec3<f32>,
    @location(6) @interpolate(flat) fade: vec3<f32>,
    @location(7) @interpolate(flat) tint: vec3<f32>,
}

// astrum_flora::impostor::hemi_oct_encode / decode / frame_axes.
fn fl_hemi_encode(d_in: vec3<f32>) -> vec2<f32> {
    let d = vec3<f32>(d_in.xy, max(d_in.z, 0.0));
    let s = abs(d.x) + abs(d.y) + d.z;
    let p = select(vec2<f32>(0.0), d.xy / s, s > 0.0);
    return 0.5 * (vec2<f32>(p.x + p.y, p.x - p.y) + 1.0);
}

fn fl_hemi_decode(uv: vec2<f32>) -> vec3<f32> {
    let x = 2.0 * uv.x - 1.0;
    let y = 2.0 * uv.y - 1.0;
    let px = 0.5 * (x + y);
    let py = 0.5 * (x - y);
    return normalize(vec3<f32>(px, py, max(1.0 - abs(px) - abs(py), 0.0)));
}

fn vs_impostor_common(v: ImpostorIn, index: u32, toward: vec3<f32>) -> ImpostorOut {
    let bucket = v.corner_bucket >> 8u;
    let p = plants[fl_bucket_base(bucket) + index];
    let scale = p.base.w;
    let e1 = p.e1.xyz;
    let up = p.up.xyz;
    let e2 = cross(up, e1);
    // View direction in the plant frame → nearest baked frame.
    var dl = normalize(vec3<f32>(dot(toward, e1), dot(toward, e2), dot(toward, up)));
    let uv = fl_hemi_encode(dl);
    let cell = clamp(floor(uv * IMPOSTOR_GRID), vec2<f32>(0.0), vec2<f32>(IMPOSTOR_GRID - 1.0));
    let dir = fl_hemi_decode((cell + 0.5) / IMPOSTOR_GRID);
    var right = vec3<f32>(1.0, 0.0, 0.0);
    if dir.z <= 0.999 {
        right = normalize(cross(vec3<f32>(0.0, 0.0, 1.0), dir));
    }
    let upq = cross(dir, right);
    let k = v.corner_bucket & 15u;
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[k];
    let local = (v.centre + (right * c.x + upq * c.y) * v.radius) * scale;
    let view_position = p.base.xyz + e1 * local.x + e2 * local.y + up * local.z;
    var out: ImpostorOut;
    out.clip_position = projection.matrix * vec4<f32>(view_position, 1.0);
    out.view_pos = view_position;
    out.uv = (cell + vec2<f32>(0.5 + 0.5 * c.x, 0.5 - 0.5 * c.y)) / IMPOSTOR_GRID;
    out.layer = v.layer;
    out.e1 = e1;
    out.e2 = e2;
    out.up = up;
    out.fade = vec3<f32>(p.e1.w, p.up.w, bitcast<f32>(p.info.w));
    let tint = 0.85 + 0.3 * sc_unit(p.info.z);
    let hue = sc_unit(p.info.w);
    out.tint = vec3<f32>(1.0 + 0.2 * (hue - 0.5), 1.0, 1.0 - 0.25 * (hue - 0.5)) * tint;
    return out;
}

@vertex
fn vs_impostor(v: ImpostorIn, @builtin(instance_index) index: u32) -> ImpostorOut {
    let p = plants[fl_bucket_base(v.corner_bucket >> 8u) + index];
    return vs_impostor_common(v, index, normalize(-p.base.xyz));
}

// Shadow pass: face the light (the cascade's depth axis in view space).
@vertex
fn vs_impostor_shadow(v: ImpostorIn, @builtin(instance_index) index: u32) -> ImpostorOut {
    let p = plants[fl_bucket_base(v.corner_bucket >> 8u) + index];
    let m = projection.matrix;
    var toward = normalize(vec3<f32>(m[0].z, m[1].z, m[2].z));
    if dot(toward, p.up.xyz) < 0.0 {
        toward = -toward;
    }
    return vs_impostor_common(v, index, toward);
}

fn fl_impostor_sample(input: ImpostorOut) -> vec4<f32> {
    return textureSampleLevel(flora_albedo, flora_sampler, input.uv, i32(input.layer), 0.0);
}

@fragment
fn fs_impostor(input: ImpostorOut, @builtin(sample_index) si: u32) -> SceneOut {
    let a = fl_impostor_sample(input);
    if a.a < 0.5 || !fl_keep_sample(input.clip_position.xy, bitcast<u32>(input.fade.z), input.fade.x, input.fade.y, si) {
        discard;
    }
    let nt = textureSampleLevel(flora_normal, flora_sampler, input.uv, i32(input.layer), 0.0).xyz * 2.0 - 1.0;
    let n = normalize(input.e1 * nt.x + input.e2 * nt.y + input.up * nt.z);
    return shade(input.view_pos, n, input.up, a.rgb * input.tint, 0.0, true);
}

@fragment
fn fs_impostor_shadow(input: ImpostorOut) {
    let a = fl_impostor_sample(input);
    if a.a < 0.5 || !fl_keep(input.clip_position.xy, bitcast<u32>(input.fade.z), input.fade.x, input.fade.y) {
        discard;
    }
}
