// Instanced CDLOD terrain over the height/normal atlas (ADR 0016).
// One shared grid mesh; each instance is one selected quadtree node drawn from
// its own atlas source (or an ancestor's sub-rectangle while it streams in).

struct Projection { matrix: mat4x4<f32> }

struct Instance {
    anchor: vec4<f32>,      // camera-relative chart-centre anchor (view axes), w = radius
    b2v_x: vec4<f32>,       // body-to-view rotation columns; x.w = render mode
    b2v_y: vec4<f32>,       // w = coarser-neighbour edge mask (s=0, s=1, t=0, t=1)
    b2v_z: vec4<f32>,       // w = finer-neighbour edge mask
    n0: vec4<f32>,          // chart-centre direction (body axes), w = |q0|
    face_u: vec4<f32>,      // w = chart width
    face_v: vec4<f32>,      // w = level
    own: vec4<f32>,         // layer, rect origin xy, rect scale
    parent: vec4<f32>,      // layer, rect origin xy, rect scale
    morph: vec4<f32>,       // morph start, morph end (view distance m), arrival fade, skirt depth m
    material: vec4<f32>,    // linear albedo rgb, w = BRDF (0 Lambert, 1 lunar-Lambert)
    surface: vec4<f32>,     // x = flat ocean at height 0 (M1)
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
// Page albedo (sRGB-encoded; alpha 1 where the page owns its colour).
@group(1) @binding(5) var albedo_atlas: texture_2d_array<f32>;
// Page climate: temperature °C, moisture, wind east, wind north (zero off world maps).
@group(1) @binding(6) var climate_atlas: texture_2d_array<f32>;

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
    @location(7) view_pos: vec3<f32>,
    @location(8) ground: f32, // terrain height before the water clamp
}

@vertex
fn vs_main(@location(0) vertex: vec3<f32>, @builtin(instance_index) index: u32) -> VertexOut {
    let inst = instances[index];
    let cells = grid.draw.x;
    let st = vertex.xy;
    // Unmorphed distance decides the CDLOD morph towards the coarser grid.
    let h0 = blended_height(inst, st, 0.0);
    let d0 = length(to_view(inst, body_position(inst, st, h0)));
    var morph = clamp((d0 - inst.morph.x) / max(inst.morph.y - inst.morph.x, 1.0e-6), 0.0, 1.0);
    let g = st * cells;
    // Restricted quadtree edges (§9.8). On an edge shared with a coarser node,
    // odd vertices collapse onto the coarse edge line (t = 1); on an edge shared
    // with a finer node, this node stays at its own level (t = 0), which is
    // exactly the line the finer side snaps to. The coarser rule wins at corners.
    let edge_bits = select(0u, 1u, g.x < 0.5) | select(0u, 2u, g.x > cells - 0.5)
        | select(0u, 4u, g.y < 0.5) | select(0u, 8u, g.y > cells - 0.5);
    if (u32(inst.b2v_z.w + 0.5) & edge_bits) != 0u {
        morph = 0.0;
    }
    if (u32(inst.b2v_y.w + 0.5) & edge_bits) != 0u {
        morph = 1.0;
    }
    let morphed = st - fract(g * 0.5) * (2.0 / cells) * morph;
    var height = blended_height(inst, morphed, morph);
    let ground = height;
    if inst.surface.x > 0.5 {
        // Flat water: the drawn surface never dips below the reference radius.
        height = max(height, 0.0);
    }
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
    out.ground = ground;
    out.grid_st = morphed;
    out.view_pos = view_position;
    return out;
}

fn atlas_srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    return select(pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
}

fn level_color(level: f32) -> vec3<f32> {
    let t = fract(level * 0.137 + 0.11);
    return 0.5 + 0.5 * cos(6.2831853 * (t + vec3<f32>(0.0, 0.33, 0.67)));
}

// Field-overlay ramps are authored as display (sRGB) colours.
fn display(c: vec3<f32>) -> vec3<f32> {
    return atlas_srgb_to_linear(c);
}

fn elevation_ramp(h: f32) -> vec3<f32> {
    if h < 0.0 {
        let t = sqrt(clamp(-h / 4000.0, 0.0, 1.0));
        return display(mix(vec3<f32>(0.25, 0.85, 0.9), vec3<f32>(0.02, 0.06, 0.35), t));
    }
    let t = clamp(h / 3000.0, 0.0, 1.0);
    var c = mix(vec3<f32>(0.2, 0.55, 0.2), vec3<f32>(0.55, 0.7, 0.3), clamp(t / 0.2, 0.0, 1.0));
    c = mix(c, vec3<f32>(0.6, 0.45, 0.25), clamp((t - 0.2) / 0.25, 0.0, 1.0));
    c = mix(c, vec3<f32>(0.5, 0.42, 0.38), clamp((t - 0.45) / 0.3, 0.0, 1.0));
    c = mix(c, vec3<f32>(1.0), clamp((t - 0.75) / 0.25, 0.0, 1.0));
    return display(c);
}

fn temperature_ramp(celsius: f32) -> vec3<f32> {
    let t = clamp((celsius + 40.0) / 80.0, 0.0, 1.0);
    let cold = vec3<f32>(0.1, 0.25, 0.9);
    let hot = vec3<f32>(0.9, 0.12, 0.1);
    return display(select(mix(vec3<f32>(1.0), hot, (t - 0.5) * 2.0), mix(cold, vec3<f32>(1.0), t * 2.0), t < 0.5));
}

fn moisture_ramp(m: f32) -> vec3<f32> {
    let t = clamp(m, 0.0, 1.0);
    let dry = vec3<f32>(0.76, 0.65, 0.42);
    let green = vec3<f32>(0.2, 0.6, 0.25);
    let blue = vec3<f32>(0.1, 0.3, 0.8);
    return display(select(mix(green, blue, (t - 0.5) * 2.0), mix(dry, green, t * 2.0), t < 0.5));
}

fn wind_colour(east: f32, north: f32) -> vec3<f32> {
    let speed = length(vec2<f32>(east, north));
    let angle = atan2(north, east);
    let hue = 0.5 + 0.5 * cos(angle + vec3<f32>(0.0, 2.0943951, 4.1887902));
    return display(hue * clamp(speed, 0.1, 1.0));
}

@fragment
fn fs_main(input: VertexOut) -> SceneOut {
    let inst = instances[input.instance];
    let own_n = textureSampleLevel(normal_atlas, normal_sampler, input.own_uv, input.layers.x, 0.0).xyz;
    let parent_n = textureSampleLevel(normal_atlas, normal_sampler, input.parent_uv, input.layers.y, 0.0).xyz;
    let arrived = mix(parent_n, own_n, input.blend.x);
    var normal = normalize(mix(arrived, parent_n, input.blend.y));
    let own_a = textureSampleLevel(albedo_atlas, normal_sampler, input.own_uv, input.layers.x, 0.0);
    let parent_a = textureSampleLevel(albedo_atlas, normal_sampler, input.parent_uv, input.layers.y, 0.0);
    let page = mix(mix(parent_a, own_a, input.blend.x), parent_a, input.blend.y);
    let albedo = mix(inst.material.rgb, atlas_srgb_to_linear(page.rgb), page.a);
    let n0_view = inst.b2v_x.xyz * inst.n0.x + inst.b2v_y.xyz * inst.n0.y + inst.b2v_z.xyz * inst.n0.z;
    let up = normalize(input.view_pos - (inst.anchor.xyz - n0_view * inst.anchor.w));
    let water = inst.surface.x > 0.5 && input.ground < 0.0;
    var n_view = normalize(inst.b2v_x.xyz * normal.x + inst.b2v_y.xyz * normal.y + inst.b2v_z.xyz * normal.z);
    if water {
        // Flat water surface: the sphere normal.
        n_view = up;
        normal = vec3<f32>(dot(inst.b2v_x.xyz, up), dot(inst.b2v_y.xyz, up), dot(inst.b2v_z.xyz, up));
    }
    let mode = u32(inst.b2v_x.w + 0.5);
    var debug_color = vec3<f32>(-1.0);
    if mode == 1u {
        debug_color = vec3<f32>(clamp(0.5 + input.ground / 600.0, 0.0, 1.0));
    } else if mode == 2u {
        debug_color = normal * 0.5 + vec3<f32>(0.5);
    } else if mode == 5u {
        let g = input.grid_st * grid.draw.x;
        let line = min(fract(g), vec2<f32>(1.0) - fract(g));
        debug_color = select(vec3<f32>(0.12, 0.14, 0.18), vec3<f32>(0.85, 0.88, 0.8), min(line.x, line.y) < 0.04);
    } else if mode == 6u {
        let l = normalize(lighting.sun.xyz - input.view_pos);
        debug_color = level_color(inst.face_v.w) * (0.35 + 0.65 * max(dot(n_view, l), 0.0));
    } else if mode == 8u {
        debug_color = vec3<f32>(input.blend.y, 1.0 - input.blend.x, 0.25);
    } else if mode == 13u {
        debug_color = elevation_ramp(input.ground);
    } else if mode == 14u {
        debug_color = select(display(vec3<f32>(0.45)), display(vec3<f32>(0.1, 0.3, 0.7)), water);
    } else if mode == 18u {
        // Page colour without light (linear; the display encodes it).
        debug_color = albedo;
    } else if mode >= 15u && mode <= 17u {
        let own_c = textureSampleLevel(climate_atlas, normal_sampler, input.own_uv, input.layers.x, 0.0);
        let parent_c = textureSampleLevel(climate_atlas, normal_sampler, input.parent_uv, input.layers.y, 0.0);
        let climate = mix(mix(parent_c, own_c, input.blend.x), parent_c, input.blend.y);
        // Bodies without a world map own no climate: neutral grey.
        debug_color = display(vec3<f32>(0.25));
        if page.a >= 0.5 {
            if mode == 15u {
                debug_color = temperature_ramp(climate.x);
            } else if mode == 16u {
                debug_color = moisture_ramp(climate.y);
            } else {
                debug_color = wind_colour(climate.z, climate.w);
            }
        }
    }
    if debug_color.x >= 0.0 {
        // Debug views bypass exposure and tonemapping (post pass clamps them).
        var out: SceneOut;
        out.direct = vec4<f32>(debug_color, 1.0);
        out.normal = encode_normal(n_view);
        out.ambient = vec4<f32>(0.0);
        return out;
    }
    return shade(input.view_pos, n_view, up, albedo, inst.material.w, true);
}
