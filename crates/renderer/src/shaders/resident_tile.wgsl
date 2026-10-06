struct Projection { matrix: mat4x4<f32> }
struct TileTexel { geometry: vec4<f32>, material: vec4<f32> }
struct BoundaryValue { position: vec4<f32>, normal_varying: vec4<f32>, material: vec4<f32> }
struct TileParams {
    anchor_view: vec4<f32>,
    body_to_view_x: vec4<f32>,
    body_to_view_y: vec4<f32>,
    body_to_view_z: vec4<f32>,
    face_normal: vec4<f32>,
    face_u: vec4<f32>,
    face_v: vec4<f32>,
    chart: vec4<f32>, // patch width, |q0|, cells, mode
    sun: vec4<f32>,
    address: vec4<f32>, // chart-center direction, anchor radius
    bounds: vec4<f32>, // minimum and maximum radial offset
}
struct DrawParams {
    own: TileParams,
    parent: TileParams,
    hierarchy: vec4<f32>, // child anchor in parent axes, morph fraction
    patch_info: vec4<f32>, // 0 single, 1 hierarchy parent, 2 child; quadrant xy
}
struct TileSample { geometry: vec4<f32>, material: vec4<f32> }
struct TriangleValue { position: vec3<f32>, normal_varying: vec3<f32>, material: vec4<f32> }
struct Reconstructed { position: vec3<f32>, normal_varying: vec3<f32>, material: vec4<f32> }
struct ValidationVertex { position: vec4<f32>, position_view: vec4<f32>, normal: vec4<f32>, material: vec4<f32> }

@group(0) @binding(0) var<uniform> projection: Projection;
@group(1) @binding(0) var<storage, read> own_texels: array<TileTexel>;
@group(1) @binding(1) var<storage, read> parent_texels: array<TileTexel>;
@group(1) @binding(2) var<storage, read> own_boundaries: array<BoundaryValue>;
@group(1) @binding(3) var<storage, read> parent_boundaries: array<BoundaryValue>;
@group(2) @binding(0) var<uniform> params: DrawParams;
@group(2) @binding(1) var<storage, read_write> validation_output: array<ValidationVertex>;

fn dimensions(tile: TileParams) -> u32 { return u32(tile.chart.z) + 3u; }
fn texel_at(index: vec2<u32>, parent_tile: bool) -> TileTexel {
    var d = dimensions(params.own);
    if parent_tile { d = dimensions(params.parent); }
    let linear = index.y * d + index.x;
    if parent_tile { return parent_texels[linear]; }
    return own_texels[linear];
}
fn edge_value(st: vec2<f32>, cells: u32, layer: u32, parent_tile: bool) -> BoundaryValue {
    let n = f32(cells);
    var edge = 0u;
    var along = u32(round(st.y * n));
    if st.x == 1.0 {
        edge = 1u;
    } else if st.y == 0.0 {
        edge = 2u;
        along = u32(round(st.x * n));
    } else if st.y == 1.0 {
        edge = 3u;
        along = u32(round(st.x * n));
    }
    let index = layer * 4u * (cells + 1u) + edge * (cells + 1u) + min(along, cells);
    if parent_tile { return parent_boundaries[index]; }
    return own_boundaries[index];
}
fn on_boundary(st: vec2<f32>) -> bool {
    return st.x == 0.0 || st.x == 1.0 || st.y == 0.0 || st.y == 1.0;
}
fn texel_coord(st: vec2<f32>, tile: TileParams) -> vec2<f32> {
    let cells = tile.chart.z;
    return clamp(st * cells + vec2<f32>(1.0), vec2<f32>(0.0), vec2<f32>(cells + 2.0));
}
fn bilinear(st: vec2<f32>, tile: TileParams, parent_tile: bool) -> TileSample {
    let p = texel_coord(st, tile);
    let d = dimensions(tile);
    let base = min(vec2<u32>(floor(p)), vec2<u32>(d - 2u));
    let f = p - vec2<f32>(base);
    let a0 = texel_at(base, parent_tile);
    let a1 = texel_at(base + vec2<u32>(1u, 0u), parent_tile);
    let b0 = texel_at(base + vec2<u32>(0u, 1u), parent_tile);
    let b1 = texel_at(base + vec2<u32>(1u, 1u), parent_tile);
    var result: TileSample;
    result.geometry = mix(mix(a0.geometry, a1.geometry, f.x), mix(b0.geometry, b1.geometry, f.x), f.y);
    result.material = mix(mix(a0.material, a1.material, f.x), mix(b0.material, b1.material, f.x), f.y);
    return result;
}
fn position_for(st: vec2<f32>, tile: TileParams, parent_tile: bool) -> vec3<f32> {
    let n0 = tile.address.xyz;
    let tangent = (tile.face_u.xyz * ((st.x - 0.5) * tile.chart.x)
        + tile.face_v.xyz * ((st.y - 0.5) * tile.chart.x)) / tile.chart.y;
    let s = 2.0 * dot(n0, tangent) + dot(tangent, tangent);
    let root = sqrt(1.0 + s);
    let k = inverseSqrt(1.0 + s);
    let diff = tangent * k - n0 * (s * k / (1.0 + root));
    let radial_offset = bilinear(st, tile, parent_tile).geometry.x;
    return tile.address.w * diff + (n0 + diff) * radial_offset;
}
fn normal_for(st: vec2<f32>, tile: TileParams, parent_tile: bool) -> vec3<f32> {
    let step = 1.0 / tile.chart.z;
    let left = position_for(st - vec2<f32>(step, 0.0), tile, parent_tile);
    let right = position_for(st + vec2<f32>(step, 0.0), tile, parent_tile);
    let down = position_for(st - vec2<f32>(0.0, step), tile, parent_tile);
    let up = position_for(st + vec2<f32>(0.0, step), tile, parent_tile);
    return normalize(cross(right - left, up - down));
}
fn parent_vertex(st: vec2<f32>) -> TriangleValue {
    var result: TriangleValue;
    if u32(params.patch_info.x + 0.5) == 4u && on_boundary(st) {
        let boundary = edge_value(st, u32(params.parent.chart.z), 2u, true);
        result.position = boundary.position.xyz;
        result.normal_varying = boundary.normal_varying.xyz;
        result.material = boundary.material;
        return result;
    }
    let sample_value = bilinear(st, params.parent, true);
    result.position = position_for(st, params.parent, true);
    result.normal_varying = normal_for(st, params.parent, true);
    result.material = sample_value.material;
    return result;
}
fn parent_triangle(st: vec2<f32>) -> TriangleValue {
    let cells = u32(params.parent.chart.z);
    let coordinates = st * f32(cells);
    let base = min(vec2<u32>(floor(coordinates)), vec2<u32>(cells - 1u));
    let f = coordinates - vec2<f32>(base);
    let a = base;
    let b = base + vec2<u32>(1u, 0u);
    let c = base + vec2<u32>(0u, 1u);
    let d = base + vec2<u32>(1u, 1u);
    var nodes: array<vec2<u32>, 3>;
    var weights: vec3<f32>;
    if f.x + f.y <= 1.0 {
        nodes = array<vec2<u32>, 3>(a, b, c);
        weights = vec3<f32>(1.0 - f.x - f.y, f.x, f.y);
    } else {
        nodes = array<vec2<u32>, 3>(b, d, c);
        weights = vec3<f32>(1.0 - f.y, f.x + f.y - 1.0, 1.0 - f.x);
    }
    let va = parent_vertex(vec2<f32>(nodes[0]) / f32(cells));
    let vb = parent_vertex(vec2<f32>(nodes[1]) / f32(cells));
    let vc = parent_vertex(vec2<f32>(nodes[2]) / f32(cells));
    var result: TriangleValue;
    result.position = va.position * weights.x + vb.position * weights.y + vc.position * weights.z;
    result.normal_varying = va.normal_varying * weights.x + vb.normal_varying * weights.y + vc.normal_varying * weights.z;
    result.material = va.material * weights.x + vb.material * weights.y + vc.material * weights.z;
    return result;
}
fn reconstruct(st: vec2<f32>) -> Reconstructed {
    var result: Reconstructed;
    let own_sample = bilinear(st, params.own, false);
    let own_position = position_for(st, params.own, false);
    let own_normal = normal_for(st, params.own, false);
    if u32(params.patch_info.x + 0.5) == 2u || u32(params.patch_info.x + 0.5) == 4u {
        let parent_st = (params.patch_info.yz + st) * 0.5;
        let coarse = parent_triangle(parent_st);
        result.position = mix(coarse.position, own_position + params.hierarchy.xyz, params.hierarchy.w);
        result.normal_varying = mix(coarse.normal_varying, own_normal, params.hierarchy.w);
        result.material = mix(coarse.material, own_sample.material, params.hierarchy.w);
    } else {
        result.position = own_position;
        result.normal_varying = own_normal;
        result.material = own_sample.material;
    }
    if u32(params.patch_info.x + 0.5) >= 3u && on_boundary(st) {
        let coarse_edge = edge_value(st, u32(params.own.chart.z), 0u, false);
        let fine_edge = edge_value(st, u32(params.own.chart.z), 1u, false);
        let fraction = params.patch_info.w;
        var child_anchor_delta = vec3<f32>(0.0);
        if u32(params.patch_info.x + 0.5) == 4u {
            child_anchor_delta = params.hierarchy.xyz;
        }
        result.position = mix(coarse_edge.position.xyz, fine_edge.position.xyz, fraction)
            + child_anchor_delta;
        result.normal_varying = mix(coarse_edge.normal_varying.xyz, fine_edge.normal_varying.xyz, fraction);
        result.material = mix(coarse_edge.material, fine_edge.material, fraction);
    }
    return result;
}
fn body_to_view(value: vec3<f32>, tile: TileParams) -> vec3<f32> {
    return tile.body_to_view_x.xyz * value.x
        + tile.body_to_view_y.xyz * value.y
        + tile.body_to_view_z.xyz * value.z;
}
fn debug_level_color(level: f32) -> vec3<f32> {
    let t = fract(level * 0.137 + 0.11);
    return 0.5 + 0.5 * cos(6.2831853 * (t + vec3<f32>(0.0, 0.33, 0.67)));
}
fn debug_slot_color(slot: f32) -> vec3<f32> {
    let id = u32(slot + 0.5) + 1u;
    return vec3<f32>(
        f32((id * 97u) % 251u),
        f32((id * 57u + 71u) % 241u),
        f32((id * 23u + 149u) % 239u)
    ) / 238.0;
}
struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal_varying: vec3<f32>,
    @location(1) material: vec4<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) height: f32,
}
@vertex fn vs_main(@location(0) uv: vec2<f32>) -> VertexOut {
    let result = reconstruct(uv);
    var output: VertexOut;
    let view_position = params.parent.anchor_view.xyz + body_to_view(result.position, params.parent);
    output.clip_position = projection.matrix * vec4<f32>(view_position, 1.0);
    output.normal_varying = result.normal_varying;
    output.material = result.material;
    output.uv = uv;
    output.height = bilinear(uv, params.own, false).geometry.x;
    return output;
}
@fragment fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let mode = u32(params.own.chart.w);
    let normal = normalize(input.normal_varying);
    var color = vec3<f32>(0.35, 0.38, 0.34);
    if mode == 1u {
        let span = params.own.bounds.y - params.own.bounds.x;
        let shade = select(0.5, clamp((input.height - params.own.bounds.x) / span, 0.0, 1.0), span > 0.0);
        color = vec3<f32>(shade);
    } else if mode == 2u {
        color = normal * 0.5 + vec3<f32>(0.5);
    } else if mode == 3u {
        color = input.material.x * vec3<f32>(0.72, 0.54, 0.30)
            + input.material.y * vec3<f32>(0.34, 0.38, 0.42)
            + input.material.z * vec3<f32>(0.45, 0.30, 0.22)
            + input.material.w * vec3<f32>(0.78, 0.73, 0.64);
    } else if mode == 4u {
        color = vec3<f32>(input.uv, 0.0);
    } else if mode == 5u {
        let grid = min(fract(input.uv * params.own.chart.z), 1.0 - fract(input.uv * params.own.chart.z));
        color = select(vec3<f32>(0.12, 0.14, 0.18), vec3<f32>(0.85, 0.88, 0.8), min(grid.x, grid.y) < 0.035);
    } else if mode == 6u {
        color = debug_level_color(params.own.bounds.z);
    } else if mode == 7u {
        color = debug_slot_color(params.own.bounds.w);
    } else if mode == 8u {
        let morph = clamp(params.hierarchy.w, 0.0, 1.0);
        color = vec3<f32>(morph, 0.18 + 0.72 * (1.0 - abs(2.0 * morph - 1.0)), 1.0 - morph);
    } else if mode == 9u {
        let level_delta = params.own.bounds.z - params.parent.bounds.z;
        let dependency = select(0.0, 1.0, u32(params.patch_info.x + 0.5) == 4u);
        color = mix(
            debug_level_color(params.own.bounds.z),
            vec3<f32>(1.0, 0.38, 0.08),
            dependency * clamp(0.55 + 0.15 * abs(level_delta), 0.0, 1.0)
        );
    } else if mode == 10u {
        if params.parent.sun.w > 1.5 {
            color = vec3<f32>(1.0, 0.24, 0.68);
        } else if params.parent.sun.w > 0.5 {
            color = vec3<f32>(1.0, 0.68, 0.12);
        } else {
            color = vec3<f32>(0.12, 0.82, 0.28);
        }
    } else {
        let diffuse = max(dot(normal, normalize(params.own.sun.xyz)), 0.0);
        color *= 0.2 + 0.8 * diffuse;
        color = mix(color, input.material.xyz, 0.18);
    }
    return vec4<f32>(color, 1.0);
}
@compute @workgroup_size(64) fn validate_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let cells = u32(params.own.chart.z);
    let side = cells + 1u;
    let count = side * side;
    if id.x >= count { return; }
    let grid = vec2<u32>(id.x % side, id.x / side);
    let st = vec2<f32>(grid) / f32(cells);
    let value = reconstruct(st);
    let normal = normalize(value.normal_varying);
    let view_position = params.parent.anchor_view.xyz + body_to_view(value.position, params.parent);
    validation_output[id.x].position = vec4<f32>(value.position, 0.0);
    validation_output[id.x].position_view = vec4<f32>(view_position, 0.0);
    validation_output[id.x].normal = vec4<f32>(normal, 0.0);
    validation_output[id.x].material = value.material;
}
