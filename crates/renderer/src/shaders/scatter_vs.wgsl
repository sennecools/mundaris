// PROTOTYPE (M5 Life): draw stage of the plants placed by cs_scatter
// (scatter_cull.wgsl): one instance per placed plant, 72 procedural
// vertices, flat shaded and lit like terrain.

@group(3) @binding(0) var<storage, read> plants: array<Plant>;

struct ScatterOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) view_pos: vec3<f32>,
    @location(1) @interpolate(flat) normal: vec3<f32>,
    @location(2) @interpolate(flat) up: vec3<f32>,
    @location(3) @interpolate(flat) albedo: vec3<f32>,
}

@vertex
fn vs_scatter(@builtin(vertex_index) vertex: u32, @builtin(instance_index) index: u32) -> ScatterOut {
    let p = plants[index];
    let kind = p.info.x;
    let seed = p.info.y;
    let scale = p.base.w;
    let spread = p.e1.w;
    let e1 = p.e1.xyz;
    let up = p.up.xyz;
    let e2 = cross(up, e1);
    let tri = vertex / 3u;
    let corner = vertex % 3u;
    let shape = vec3<f32>(spread, spread, 1.0);
    let c0 = sc_corner(kind, tri, 0u, seed) * shape;
    let c1 = sc_corner(kind, tri, 1u, seed) * shape;
    let c2 = sc_corner(kind, tri, 2u, seed) * shape;
    let local = sc_corner(kind, tri, corner, seed) * shape * scale;
    var local_n = normalize(cross(c1 - c0, c2 - c0));
    // Face outward from the part's axis point.
    var axis_z = 0.0;
    if tri < 16u {
        axis_z = sc_part(kind, tri / 8u).ring_z;
    }
    let centre = (c0 + c1 + c2) / 3.0 - vec3<f32>(0.0, 0.0, axis_z);
    if dot(local_n, centre) < 0.0 {
        local_n = -local_n;
    }
    let view_position = p.base.xyz + e1 * local.x + e2 * local.y + up * local.z;
    var out: ScatterOut;
    out.clip_position = projection.matrix * vec4<f32>(view_position, 1.0);
    out.view_pos = view_position;
    out.normal = normalize(e1 * local_n.x + e2 * local_n.y + up * local_n.z);
    out.up = up;
    let tint = 0.8 + 0.4 * sc_unit(p.info.z);
    let hue = sc_unit(p.info.w);
    let shift = vec3<f32>(1.0 + 0.25 * (hue - 0.5), 1.0, 1.0 - 0.3 * (hue - 0.5));
    if kind == 3u {
        out.albedo = vec3<f32>(0.2, 0.19, 0.17) * tint;
    } else if tri >= 16u {
        out.albedo = vec3<f32>(0.075, 0.05, 0.03);
    } else if kind == 0u {
        out.albedo = vec3<f32>(0.022, 0.05, 0.028) * tint * shift;
    } else if kind == 1u {
        out.albedo = vec3<f32>(0.045, 0.085, 0.025) * tint * shift;
    } else {
        out.albedo = vec3<f32>(0.055, 0.07, 0.03) * tint * shift;
    }
    return out;
}

@fragment
fn fs_scatter(input: ScatterOut) -> SceneOut {
    return shade(input.view_pos, input.normal, input.up, input.albedo, 0.0, true);
}