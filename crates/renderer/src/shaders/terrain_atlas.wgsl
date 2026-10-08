// Instanced CDLOD terrain over the height/normal atlas (ADR 0016).
// One shared grid mesh; each instance is one selected quadtree node drawn from
// its own atlas source (or an ancestor's sub-rectangle while it streams in).

struct Projection { matrix: mat4x4<f32> }

struct Instance {
    anchor: vec4<f32>,      // camera-relative chart-centre anchor (view axes), w = radius
    b2v_x: vec4<f32>,       // body-to-view rotation columns
    b2v_y: vec4<f32>,
    b2v_z: vec4<f32>,
    n0: vec4<f32>,          // chart-centre direction (body axes), w = |q0|
    face_u: vec4<f32>,      // w = chart width
    face_v: vec4<f32>,      // w = level
    own: vec4<f32>,         // layer, rect origin xy, rect scale
    parent: vec4<f32>,      // layer, rect origin xy, rect scale
    morph: vec4<f32>,       // morph start, morph end (view distance m), arrival fade, skirt depth m
    sun: vec4<f32>,         // sun direction (body axes), w = render mode
}

@group(0) @binding(0) var<uniform> projection: Projection;
@group(1) @binding(0) var height_atlas: texture_2d_array<f32>;
@group(1) @binding(1) var normal_atlas: texture_2d_array<f32>;
@group(1) @binding(2) var normal_sampler: sampler;
@group(1) @binding(3) var<storage, read> instances: array<Instance>;
struct Grid {
    data: vec4<f32>, // data cells, normal cells, height side, normal side
    draw: vec4<f32>, // draw grid cells
}
@group(1) @binding(4) var<uniform> grid: Grid;

fn height_at(layer: i32, st: vec2<f32>) -> f32 {
    let cells = grid.data.x;
    let p = clamp(st * cells + vec2<f32>(1.0), vec2<f32>(0.0), vec2<f32>(cells + 2.0));
    let base = min(vec2<i32>(floor(p)), vec2<i32>(i32(cells) + 1));
    let f = p - vec2<f32>(base);
    let a0 = textureLoad(height_atlas, base, layer, 0).x;
    let a1 = textureLoad(height_atlas, base + vec2<i32>(1, 0), layer, 0).x;
    let b0 = textureLoad(height_atlas, base + vec2<i32>(0, 1), layer, 0).x;
    let b1 = textureLoad(height_atlas, base + vec2<i32>(1, 1), layer, 0).x;
    return mix(mix(a0, a1, f.x), mix(b0, b1, f.x), f.y);
}

fn normal_uv(st: vec2<f32>) -> vec2<f32> {
    return (st * grid.data.y + vec2<f32>(1.5)) / grid.data.w;
}

fn chart_diff(inst: Instance, st: vec2<f32>) -> vec3<f32> {
    let n0 = inst.n0.xyz;
    let tangent = (inst.face_u.xyz * ((st.x - 0.5) * inst.face_u.w)
        + inst.face_v.xyz * ((st.y - 0.5) * inst.face_u.w)) / inst.n0.w;
    let s = 2.0 * dot(n0, tangent) + dot(tangent, tangent);
    let root = sqrt(1.0 + s);
    let k = 1.0 / root;
    return tangent * k - n0 * (s * k / (1.0 + root));
}

fn to_view(inst: Instance, body: vec3<f32>) -> vec3<f32> {
    return inst.anchor.xyz + inst.b2v_x.xyz * body.x + inst.b2v_y.xyz * body.y + inst.b2v_z.xyz * body.z;
}

fn own_st(inst: Instance, st: vec2<f32>) -> vec2<f32> {
    return inst.own.yz + st * inst.own.w;
}

fn parent_st(inst: Instance, st: vec2<f32>) -> vec2<f32> {
    return inst.parent.yz + st * inst.parent.w;
}

fn blended_height(inst: Instance, st: vec2<f32>, morph: f32) -> f32 {
    let own = height_at(i32(inst.own.x), own_st(inst, st));
    let coarse = height_at(i32(inst.parent.x), parent_st(inst, st));
    let arrived = mix(coarse, own, inst.morph.z);
    return mix(arrived, coarse, morph);
}

fn body_position(inst: Instance, st: vec2<f32>, height: f32) -> vec3<f32> {
    let diff = chart_diff(inst, st);
    return inst.anchor.w * diff + (inst.n0.xyz + diff) * height;
}

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) own_uv: vec2<f32>,
    @location(1) parent_uv: vec2<f32>,
    @location(2) @interpolate(flat) layers: vec2<i32>,
    @location(3) blend: vec2<f32>, // arrival fade, morph
    @location(4) @interpolate(flat) instance: u32,
    @location(5) height: f32,
    @location(6) grid_st: vec2<f32>,
}

@vertex
fn vs_main(@location(0) vertex: vec3<f32>, @builtin(instance_index) index: u32) -> VertexOut {
    let inst = instances[index];
    let cells = grid.draw.x;
    let st = vertex.xy;
    // Unmorphed distance decides the CDLOD morph towards the coarser grid.
    let h0 = blended_height(inst, st, 0.0);
    let d0 = length(to_view(inst, body_position(inst, st, h0)));
    let morph = clamp((d0 - inst.morph.x) / max(inst.morph.y - inst.morph.x, 1.0e-6), 0.0, 1.0);
    let g = st * cells;
    let morphed = st - fract(g * 0.5) * (2.0 / cells) * morph;
    var height = blended_height(inst, morphed, morph);
    if vertex.z > 0.5 {
        height -= inst.morph.w;
    }
    let view_position = to_view(inst, body_position(inst, morphed, height));
    var out: VertexOut;
    out.clip_position = projection.matrix * vec4<f32>(view_position, 1.0);
    out.own_uv = normal_uv(own_st(inst, morphed));
    out.parent_uv = normal_uv(parent_st(inst, morphed));
    out.layers = vec2<i32>(i32(inst.own.x), i32(inst.parent.x));
    out.blend = vec2<f32>(inst.morph.z, morph);
    out.instance = index;
    out.height = height;
    out.grid_st = morphed;
    return out;
}

fn level_color(level: f32) -> vec3<f32> {
    let t = fract(level * 0.137 + 0.11);
    return 0.5 + 0.5 * cos(6.2831853 * (t + vec3<f32>(0.0, 0.33, 0.67)));
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let inst = instances[input.instance];
    let own_n = textureSampleLevel(normal_atlas, normal_sampler, input.own_uv, input.layers.x, 0.0).xyz;
    let parent_n = textureSampleLevel(normal_atlas, normal_sampler, input.parent_uv, input.layers.y, 0.0).xyz;
    let arrived = mix(parent_n, own_n, input.blend.x);
    let normal = normalize(mix(arrived, parent_n, input.blend.y));
    let mode = u32(inst.sun.w + 0.5);
    var color = vec3<f32>(0.42, 0.42, 0.42);
    if mode == 1u {
        color = vec3<f32>(clamp(0.5 + input.height / 600.0, 0.0, 1.0));
    } else if mode == 2u {
        color = normal * 0.5 + vec3<f32>(0.5);
    } else if mode == 5u {
        let g = input.grid_st * grid.draw.x;
        let line = min(fract(g), vec2<f32>(1.0) - fract(g));
        color = select(vec3<f32>(0.12, 0.14, 0.18), vec3<f32>(0.85, 0.88, 0.8), min(line.x, line.y) < 0.04);
    } else if mode == 6u {
        color = level_color(inst.face_v.w);
        let diffuse = max(dot(normal, normalize(inst.sun.xyz)), 0.0);
        color *= 0.35 + 0.65 * diffuse;
    } else if mode == 8u {
        color = vec3<f32>(input.blend.y, 1.0 - input.blend.x, 0.25);
    } else {
        let diffuse = max(dot(normal, normalize(inst.sun.xyz)), 0.0);
        color *= 0.2 + 0.8 * diffuse;
    }
    return vec4<f32>(color, 1.0);
}
