struct SkyUniforms {
    axes_x: vec4<f32>,
    axes_y: vec4<f32>,
    axes_z: vec4<f32>,
    observer: vec4<f32>,
    projection: vec4<f32>,
    appearance: vec4<f32>,
    viewport: vec4<f32>,
};
@group(0) @binding(0) var<uniform> sky: SkyUniforms;
@group(0) @binding(1) var galaxy: texture_2d<f32>;
@group(0) @binding(2) var galaxy_sampler: sampler;
struct FocalChart { right: vec4<f32>, up: vec4<f32>, forward: vec4<f32> };
struct FocalCharts { charts: array<FocalChart, 4>, count: vec4<u32> };
@group(0) @binding(3) var focal_details: texture_2d_array<f32>;
@group(0) @binding(4) var<uniform> focal: FocalCharts;

struct Star {
    position_flux: vec4<f32>,
    color_radius: vec4<f32>,
};

struct StarOut {
    @builtin(position) position: vec4<f32>,
    @location(0) local: vec2<f32>,
    @location(1) color_flux: vec4<f32>,
    @location(2) radius_halo: vec2<f32>,
};

@vertex
fn background_vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(positions[index], 0.0, 1.0);
}

@fragment
fn background_fragment(@builtin(position) pixel: vec4<f32>) -> @location(0) vec4<f32> {
    let screen = (pixel.xy - sky.viewport.xy) / sky.projection.zw;
    let ray_view = vec3<f32>((screen.x * 2.0 - 1.0) / sky.projection.x,
        (1.0 - screen.y * 2.0) / sky.projection.y, -1.0);
    let ray = normalize(vec3<f32>(dot(ray_view, sky.axes_x.xyz), dot(ray_view, sky.axes_y.xyz), dot(ray_view, sky.axes_z.xyz)));
    let uv = vec2<f32>(atan2(ray.z, ray.x) / 6.28318530718 + 0.5,
        0.5 - asin(clamp(ray.y, -1.0, 1.0)) / 3.14159265359);
    // Wrapped longitude must not produce a spurious full-texture derivative at
    // the seam; mip filtering follows the local directional footprint instead.
    var dx = dpdx(uv); var dy = dpdy(uv);
    dx.x -= round(dx.x); dy.x -= round(dy.x);
    var color = textureSampleGrad(galaxy, galaxy_sampler, uv, dx, dy).rgb;
    let ray_dx = dpdx(ray);
    let ray_dy = dpdy(ray);
    for (var index = 0u; index < focal.count.x; index += 1u) {
        let chart = focal.charts[index];
        let z = dot(ray, chart.forward.xyz);
        let denominator = max(z, 0.01);
        let xy = vec2<f32>(dot(ray, chart.right.xyz), dot(ray, chart.up.xyz));
        let local = xy * chart.right.w / denominator;
        let detail_uv = local * vec2<f32>(0.5, -0.5) + 0.5;
        // Quotient-rule angular gradients are computed before regional rejection.
        // Cached charts retain ordinary linear/mip filtering even while magnified.
        let derivative_x = (vec2<f32>(dot(ray_dx, chart.right.xyz), dot(ray_dx, chart.up.xyz)) * denominator - xy * dot(ray_dx, chart.forward.xyz)) * chart.right.w / (denominator * denominator);
        let derivative_y = (vec2<f32>(dot(ray_dy, chart.right.xyz), dot(ray_dy, chart.up.xyz)) * denominator - xy * dot(ray_dy, chart.forward.xyz)) * chart.right.w / (denominator * denominator);
        if (z > 0.01 && max(abs(local.x), abs(local.y)) < 1.0) {
            let detail = textureSampleGrad(focal_details, galaxy_sampler, detail_uv, i32(index), derivative_x * vec2<f32>(0.5, -0.5), derivative_y * vec2<f32>(0.5, -0.5)).rgb;
            let weight = 1.0 - smoothstep(0.78, 1.0, max(abs(local.x), abs(local.y)));
            color = mix(color, detail, weight);
        }
    }
    color *= sky.appearance.y;
    if (sky.appearance.w < 0.5) {
        color = select(color * 12.92, 1.055 * pow(max(color, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055, color > vec3<f32>(0.0031308));
    }
    return vec4<f32>(color, 1.0);
}

@vertex
fn star_vertex(@builtin(vertex_index) vertex: u32, @location(0) position_flux: vec4<f32>, @location(1) color_radius: vec4<f32>) -> StarOut {
    let corners = array<vec2<f32>, 6>(vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let corner = corners[vertex];
    let relative_galactic = position_flux.xyz - sky.observer.xyz;
    let view = sky.axes_x.xyz * relative_galactic.x + sky.axes_y.xyz * relative_galactic.y + sky.axes_z.xyz * relative_galactic.z;
    var out: StarOut;
    if (view.z >= -0.000001) {
        out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.local = corner;
        out.color_flux = vec4<f32>(0.0);
        out.radius_halo = vec2<f32>(1.0, 0.0);
        return out;
    }
    let ndc = vec2<f32>(view.x * sky.projection.x / -view.z, view.y * sky.projection.y / -view.z);
    let center = vec2<f32>(sky.viewport.x, sky.viewport.y) + (ndc * vec2<f32>(0.5, -0.5) + 0.5) * sky.projection.zw;
    let radius = max(color_radius.w, 0.35);
    let halo = select(0.0, sky.appearance.z, position_flux.w > 3.0);
    let halo_sigma = max(1.4, radius * 2.5);
    let extent = select(radius * 4.0 + 0.5, halo_sigma * 4.0 + 0.5, halo > 0.0);
    let screen = center + corner * extent;
    let viewport_local = screen - sky.viewport.xy;
    out.position = vec4<f32>(viewport_local / sky.projection.zw * vec2<f32>(2.0, -2.0) + vec2<f32>(-1.0, 1.0), 0.0, 1.0);
    out.local = corner * extent;
    out.color_flux = vec4<f32>(color_radius.xyz, position_flux.w * sky.appearance.x);
    out.radius_halo = vec2<f32>(radius, halo);
    return out;
}

// Integrate the core over the physical pixel aperture. Merely evaluating a narrow
// Gaussian at pixel centres makes its total flux vary with subpixel motion.
fn normal_cdf(x: f32) -> f32 {
    let t = 1.0 / (1.0 + 0.2316419 * abs(x));
    let polynomial = t * (0.319381530 + t * (-0.356563782 + t * (1.781477937 + t * (-1.821255978 + t * 1.330274429))));
    let tail = 0.3989422804 * exp(-0.5 * x * x) * polynomial;
    return select(tail, 1.0 - tail, x >= 0.0);
}

@fragment
fn star_fragment(in: StarOut) -> @location(0) vec4<f32> {
    let sigma = in.radius_halo.x;
    let r2 = dot(in.local, in.local);
    let core = (normal_cdf((in.local.x + 0.5) / sigma) - normal_cdf((in.local.x - 0.5) / sigma))
        * (normal_cdf((in.local.y + 0.5) / sigma) - normal_cdf((in.local.y - 0.5) / sigma));
    let halo_sigma = max(1.4, in.radius_halo.x * 2.5);
    let halo = exp(-0.5 * r2 / (halo_sigma * halo_sigma)) / (6.28318530718 * halo_sigma * halo_sigma) * 0.15 * in.radius_halo.y;
    let rgb = in.color_flux.rgb * in.color_flux.a * (core + halo);
    if (sky.appearance.w < 0.5) {
        return vec4<f32>(select(rgb * 12.92, 1.055 * pow(max(rgb, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - 0.055, rgb > vec3<f32>(0.0031308)), 0.0);
    }
    return vec4<f32>(rgb, 0.0);
}
