// PROTOTYPE (flora lane): grown species meshes (crates/flora) drawn for the
// plants that cs_scatter routed into flora buckets (scatter_cull.wgsl). One
// indexed indirect draw per (species, variant, LOD) bucket; each mesh vertex
// carries its bucket's first instance slot (`base`), so no first-instance
// support is needed. Appended to the draw shader after scatter_vs.wgsl
// (same `plants` binding and `shade`).
//
// Vertex colour is sqrt-encoded linear RGB, alpha = baked ambient occlusion.
// Wind data (pivot, level, stiffness, phase) is uploaded but not animated yet.

struct FloraIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) color: vec4<f32>,
    @location(3) pivot: vec3<f32>,
    @location(4) wind: vec4<u32>,
    @location(5) base: u32,
}

struct FloraOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) @interpolate(flat) up: vec3<f32>,
    @location(3) albedo: vec3<f32>,
}

@vertex
fn vs_flora(v: FloraIn, @builtin(instance_index) index: u32) -> FloraOut {
    let p = plants[v.base + index];
    let scale = p.base.w;
    let spread = p.e1.w;
    let e1 = p.e1.xyz;
    let up = p.up.xyz;
    let e2 = cross(up, e1);
    // Far kept plants stand for their thinned neighbours: crowns spread
    // sideways (same rule as the procedural shapes).
    let stretch = vec3<f32>(spread, spread, 1.0);
    let local = v.position * stretch * scale;
    let view_position = p.base.xyz + e1 * local.x + e2 * local.y + up * local.z;
    let nl = normalize(v.normal.xyz / stretch);
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
    return out;
}

@fragment
fn fs_flora(input: FloraOut) -> SceneOut {
    var n = normalize(input.normal);
    // Organs are two-sided cards: light the side that faces the viewer.
    if dot(n, input.view_pos) > 0.0 {
        n = -n;
    }
    return shade(input.view_pos, n, input.up, input.albedo, 0.0, true);
}
