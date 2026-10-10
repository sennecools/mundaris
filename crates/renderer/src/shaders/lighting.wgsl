// Shared scene lighting (docs/RENDER_PIPELINE_HDR.md §3). Prepended to every
// main-pass shader that shades geometry. Group 2 is the lighting group.
//
// Radiance is written pre-exposed: value = luminance (cd/m²) × exposure of the
// previous adaptation, which keeps the HDR target in a well-conditioned f16
// range from a sunlit limb to a starlit night side.

struct Lighting {
    sun: vec4<f32>,          // centre (view m), w = radius m
    sun_color: vec4<f32>,    // rgb = colour × illuminance (lux) at w = reference distance m
    ambient: vec4<f32>,      // rgb ambient illuminance (lux), w = bounce fraction of sun
    params: vec4<f32>,       // near m, view mode, occluder count, lighting present
    shadow: vec4<f32>,       // cascade count, normal bias (texels), softness, -
    splits: vec4<f32>,       // far view depth per cascade
    texel: vec4<f32>,        // world texel per cascade
    depth_range: vec4<f32>,  // light depth range per cascade (m)
    occluders: array<vec4<f32>, 8>,
    cascades: array<mat4x4<f32>, 4>,
    sky: vec4<f32>,          // rgb = sky colour × sky fraction of the sun (lux per lux), w = stylised look
}

struct Exposure {
    value: f32,     // exposure used by this frame's main pass (pre-exposure)
    ev100: f32,
    previous: f32,  // exposure before the latest adaptation
    luminance: f32, // metered average luminance
}

@group(2) @binding(0) var<uniform> lighting: Lighting;
@group(2) @binding(1) var<storage, read> exposure: Exposure;
@group(2) @binding(2) var shadow_maps: texture_depth_2d_array;
@group(2) @binding(3) var shadow_compare: sampler_comparison;

const PI: f32 = 3.14159265359;

// View modes shared with TerrainViewMode::shader_mode.
const VIEW_LIT: u32 = 0u;
const VIEW_UNLIT: u32 = 9u;
const VIEW_SHADOWS: u32 = 11u;

struct SceneOut {
    @location(0) direct: vec4<f32>,
    @location(1) normal: vec2<f32>,
    @location(2) ambient: vec4<f32>,
}

fn view_mode() -> u32 {
    return u32(lighting.params.y + 0.5);
}

fn oct_wrap(v: vec2<f32>) -> vec2<f32> {
    return (vec2<f32>(1.0) - abs(v.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), v >= vec2<f32>(0.0));
}

fn encode_normal(n: vec3<f32>) -> vec2<f32> {
    let p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    return select(oct_wrap(p), p, n.z >= 0.0);
}

// Area of the lens where two small disks of angular radii a, b at separation d overlap.
fn disk_overlap(a: f32, b: f32, d: f32) -> f32 {
    if d >= a + b {
        return 0.0;
    }
    if d <= abs(a - b) {
        let r = min(a, b);
        return PI * r * r;
    }
    let ca = acos(clamp((d * d + a * a - b * b) / (2.0 * d * a), -1.0, 1.0));
    let cb = acos(clamp((d * d + b * b - a * a) / (2.0 * d * b), -1.0, 1.0));
    let k = max((-d + a + b) * (d + a - b) * (d - a + b) * (d + a + b), 0.0);
    return a * a * ca + b * b * cb - 0.5 * sqrt(k);
}

// Visible fraction of the sun disk from p (view m): body-on-body eclipses and
// the soft limb of a body's own horizon. Points inside an occluder skip it.
fn sun_disk_visibility(p: vec3<f32>) -> f32 {
    let to_sun = lighting.sun.xyz - p;
    let ds = length(to_sun);
    let dir = to_sun / ds;
    let rs = asin(clamp(lighting.sun.w / ds, 0.0, 1.0));
    let count = u32(lighting.params.z + 0.5);
    var visible = 1.0;
    for (var i = 0u; i < count; i++) {
        let o = lighting.occluders[i];
        let c = o.xyz - p;
        let dc = length(c);
        if dc <= o.w * 1.0001 || dc >= ds || dot(c, dir) <= 0.0 {
            continue;
        }
        let ro = asin(clamp(o.w / dc, 0.0, 1.0));
        let sep = acos(clamp(dot(c / dc, dir), -1.0, 1.0));
        visible *= clamp(1.0 - disk_overlap(rs, ro, sep) / (PI * rs * rs), 0.0, 1.0);
    }
    return visible;
}

// Sun illuminance at p (inverse square from the authored reference distance).
fn sun_illuminance(p: vec3<f32>) -> vec3<f32> {
    let d = max(length(lighting.sun.xyz - p), lighting.sun.w);
    let ratio = lighting.sun_color.w / d;
    return lighting.sun_color.rgb * ratio * ratio;
}

// Reflectance factor F with radiance = albedo / π · E · F.
// Lambert: F = μ0. Lunar-Lambert (McEwen 1991): F = 2Lμ0/(μ0+μ) + (1−L)μ0.
fn reflectance(n: vec3<f32>, l: vec3<f32>, v: vec3<f32>, brdf: f32) -> f32 {
    let mu0 = max(dot(n, l), 0.0);
    if brdf < 0.5 || mu0 <= 0.0 {
        return mu0;
    }
    let mu = max(dot(n, v), 1.0e-3);
    let g = degrees(acos(clamp(dot(l, v), -1.0, 1.0)));
    let big_l = clamp(1.0 - 0.019 * g + 2.42e-4 * g * g - 1.46e-6 * g * g * g, 0.0, 1.0);
    return 2.0 * big_l * mu0 / (mu0 + mu) + (1.0 - big_l) * mu0;
}

const POISSON: array<vec2<f32>, 16> = array<vec2<f32>, 16>(
    vec2<f32>(-0.94201624, -0.39906216), vec2<f32>(0.94558609, -0.76890725),
    vec2<f32>(-0.09418410, -0.92938870), vec2<f32>(0.34495938, 0.29387760),
    vec2<f32>(-0.91588581, 0.45771432), vec2<f32>(-0.81544232, -0.87912464),
    vec2<f32>(-0.38277543, 0.27676845), vec2<f32>(0.97484398, 0.75648379),
    vec2<f32>(0.44323325, -0.97511554), vec2<f32>(0.53742981, -0.47373420),
    vec2<f32>(-0.26496911, -0.41893023), vec2<f32>(0.79197514, 0.19090188),
    vec2<f32>(-0.24188840, 0.99706507), vec2<f32>(-0.81409955, 0.91437590),
    vec2<f32>(0.19984126, 0.78641367), vec2<f32>(0.14383161, -0.14100790),
);

// Percentage-closer soft shadow in one cascade: blocker search, then a PCF
// kernel sized by the physical penumbra of the sun disk.
fn cascade_visibility(c: u32, p: vec3<f32>, n: vec3<f32>) -> f32 {
    let texel = lighting.texel[c];
    let biased = p + n * texel * lighting.shadow.y;
    let clip = lighting.cascades[c] * vec4<f32>(biased, 1.0);
    let uv = vec2<f32>(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5);
    let z = clip.z;
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || z > 1.0 {
        return 1.0;
    }
    let size = vec2<f32>(textureDimensions(shadow_maps).xy);
    let range = lighting.depth_range[c];
    let sun_tan = lighting.sun.w / max(length(lighting.sun.xyz - p), lighting.sun.w);
    let softness = lighting.shadow.z;
    // Blocker search over a radius that covers a 2 km occluder distance.
    let search = clamp(2000.0 * sun_tan * softness / texel, 1.0, 24.0);
    var blockers = 0.0;
    var blocker_depth = 0.0;
    let dims = vec2<i32>(size);
    var poisson = POISSON;
    for (var i = 0; i < 16; i++) {
        let s = uv + poisson[i] * search / size;
        let t = clamp(vec2<i32>(s * size), vec2<i32>(0), dims - vec2<i32>(1));
        let d = textureLoad(shadow_maps, t, i32(c), 0);
        if d < z {
            blockers += 1.0;
            blocker_depth += d;
        }
    }
    if blockers == 0.0 {
        return 1.0;
    }
    let distance_m = (z - blocker_depth / blockers) * range;
    let kernel = clamp(distance_m * sun_tan * softness / texel, 1.0, 32.0);
    var lit = 0.0;
    for (var i = 0; i < 16; i++) {
        let s = uv + poisson[i] * kernel / size;
        lit += textureSampleCompareLevel(shadow_maps, shadow_compare, s, i32(c), z);
    }
    return lit / 16.0;
}

// Cascaded sun visibility at view position p with geometric normal n (view axes).
fn cascaded_shadow(p: vec3<f32>, n: vec3<f32>) -> f32 {
    let count = u32(lighting.shadow.x + 0.5);
    if count == 0u {
        return 1.0;
    }
    let depth = -p.z;
    for (var c = 0u; c < count; c++) {
        let far = lighting.splits[c];
        if depth <= far {
            let near = select(lighting.params.x, lighting.splits[max(c, 1u) - 1u], c > 0u);
            let blend_start = mix(near, far, 0.85);
            let v = cascade_visibility(c, p, n);
            if depth > blend_start && c + 1u < count {
                let t = (depth - blend_start) / max(far - blend_start, 1.0e-3);
                return mix(v, cascade_visibility(c + 1u, p, n), t);
            }
            if depth > blend_start && c + 1u == count {
                let t = (depth - blend_start) / max(far - blend_start, 1.0e-3);
                return mix(v, 1.0, t);
            }
            return v;
        }
    }
    return 1.0;
}

fn cascade_index(p: vec3<f32>) -> u32 {
    let count = u32(lighting.shadow.x + 0.5);
    for (var c = 0u; c < count; c++) {
        if -p.z <= lighting.splits[c] {
            return c;
        }
    }
    return 4u;
}

// Physically ordered shading. `up` is the local surface radial; `shadowed`
// enables the cascaded term (terrain).
fn shade(p: vec3<f32>, n: vec3<f32>, up: vec3<f32>, albedo: vec3<f32>, brdf: f32, shadowed: bool) -> SceneOut {
    var out: SceneOut;
    out.normal = encode_normal(n);
    let mode = view_mode();
    if mode == VIEW_UNLIT || lighting.params.w < 0.5 {
        // Albedo as a mid-grey-exposed value: tonemapped like a lit scene.
        out.direct = vec4<f32>(albedo, 1.0);
        out.ambient = vec4<f32>(0.0);
        return out;
    }
    let l = normalize(lighting.sun.xyz - p);
    let v = normalize(-p);
    let e = sun_illuminance(p);
    let disk = sun_disk_visibility(p);
    var visibility = disk;
    if shadowed && visibility > 0.0 && dot(n, l) > 0.0 {
        visibility *= cascaded_shadow(p, n);
    }
    let pre = exposure.value;
    let direct = albedo / PI * e * reflectance(n, l, v, brdf) * visibility;
    // Bounce from sunlit ground: horizontal ground of albedo A has radiance
    // A/pi * E * sin(elevation); a surface sees it over (1 - n.up)/2 of its
    // hemisphere. Flat shadowed ground stays dark; slopes keep their shape.
    let ground = max(dot(up, l), 0.0) * 0.5 * (1.0 - dot(n, up));
    let bounce = e * lighting.ambient.w * ground * disk;
    // Sky light: the sunlit sky's irradiance on a horizontal surface, a
    // fraction of the sun's normal illuminance that fades with elevation and
    // through twilight; a surface sees the sky over (1 + n.up)/2 of its
    // hemisphere. Shadowed upward-facing ground keeps its sky light (AO and
    // the canopy occlusion of GTAO still apply to it).
    let elevation = dot(up, l);
    let sky_level = smoothstep(-0.1, 0.05, elevation) * sqrt(max(elevation, 0.02));
    let sky = e * lighting.sky.rgb * sky_level * 0.5 * (1.0 + dot(n, up)) * disk;
    // Stylised look: a soft sky-coloured rim on grazing surfaces (foliage
    // silhouettes, crown edges), lit by the sky like the ambient term.
    let rim = lighting.sky.w * pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.6
        * e * lighting.sky.rgb * sky_level * disk;
    let ambient = albedo / PI * (lighting.ambient.rgb + bounce + sky + rim);
    if mode == VIEW_SHADOWS {
        var tint = array<vec3<f32>, 5>(
            vec3<f32>(1.0, 0.35, 0.3), vec3<f32>(0.35, 1.0, 0.35),
            vec3<f32>(0.35, 0.5, 1.0), vec3<f32>(1.0, 0.9, 0.3), vec3<f32>(0.6),
        );
        let c = select(4u, cascade_index(p), shadowed);
        out.direct = vec4<f32>(tint[min(c, 4u)] * (0.25 + 0.75 * visibility), 1.0);
        out.ambient = vec4<f32>(0.0);
        return out;
    }
    out.direct = vec4<f32>(direct * pre, 1.0);
    out.ambient = vec4<f32>(ambient * pre, 0.0);
    return out;
}
