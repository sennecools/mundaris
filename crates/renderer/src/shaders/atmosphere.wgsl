// Atmosphere: sky and aerial perspective in one post pass (look preset
// prototype, docs/RENDER_PIPELINE_HDR.md amendment 2026-10-10c).
//
// Per pixel the view ray is marched through the atmosphere shell of the
// nearest body (exponential Rayleigh and Mie layers) up to the drawn surface
// (depth) or out of the shell. Single scattering of the sun; the light path
// to each sample uses an air-mass approximation, so low suns turn the sky
// and the haze warm. The result is out = scene x transmittance + inscatter,
// in pre-exposed radiance like the main pass. Sky pixels (depth 0) keep the
// stars through the transmittance. The "stylised" look scales the haze and
// tints and lifts the scattered light toward the art-direction references.

struct Atmosphere {
    up: vec4<f32>,          // camera "up" (from the body centre), w = camera altitude m
    sun: vec4<f32>,         // direction to the sun (view), w = sun illuminance lux at the body
    shell: vec4<f32>,       // body radius m, atmosphere top height m, Rayleigh H m, Mie H m
    beta: vec4<f32>,        // Rayleigh scattering per m (rgb), Mie scattering per m
    camera: vec4<f32>,      // Mie g, -, tan(half fov y), aspect
    params: vec4<f32>,      // near m, enabled, haze multiplier, sky multiplier
    tint: vec4<f32>,        // inscatter tint (rgb), saturation
    extra: vec4<f32>,       // -
}

@group(0) @binding(0) var<uniform> atmo: Atmosphere;
@group(0) @binding(1) var scene_tex: texture_2d<f32>;
@group(0) @binding(2) var depth_tex: texture_depth_2d;
@group(0) @binding(3) var<storage, read> exposure: Exposure;

const STEPS: i32 = 12;

// Distance along a ray from radius r0 with cosine mu (to the local up) to
// the sphere of radius r, or -1. Written for f32 at planet scale: the
// discriminant uses (r - r0)(r + r0) instead of r^2 - r0^2.
fn ray_sphere_exit(r0: f32, mu: f32, r: f32) -> f32 {
    let d = (r - r0) * (r + r0) + r0 * r0 * mu * mu;
    if d < 0.0 {
        return -1.0;
    }
    return -r0 * mu + sqrt(d);
}

fn ray_sphere_ground(r0: f32, mu: f32, r: f32) -> f32 {
    let d = (r - r0) * (r + r0) + r0 * r0 * mu * mu;
    if d < 0.0 || mu >= 0.0 {
        return -1.0;
    }
    return -r0 * mu - sqrt(d);
}

// Relative optical air mass toward the sun at a zenith cosine (Kasten-Young),
// capped near and below the horizon.
fn air_mass(cos_zenith: f32) -> f32 {
    let c = max(cos_zenith, -0.05);
    let zenith_deg = degrees(acos(clamp(c, -1.0, 1.0)));
    return 1.0 / max(c + 0.50572 * pow(max(96.07995 - zenith_deg, 0.1), -1.6364), 0.025);
}

fn phase_rayleigh(c: f32) -> f32 {
    return 3.0 / (16.0 * PI) * (1.0 + c * c);
}

fn phase_mie(c: f32, g: f32) -> f32 {
    let g2 = g * g;
    return (1.0 - g2) / (4.0 * PI * pow(max(1.0 + g2 - 2.0 * g * c, 1.0e-4), 1.5));
}

@fragment
fn fs_atmosphere(input: FullscreenOut) -> @location(0) vec4<f32> {
    let ip = vec2<i32>(input.position.xy);
    let scene = textureLoad(scene_tex, ip, 0);
    if atmo.params.y < 0.5 {
        return scene;
    }
    let size = vec2<f32>(textureDimensions(scene_tex));
    let ndc = vec2<f32>(input.position.x / size.x * 2.0 - 1.0, 1.0 - input.position.y / size.y * 2.0);
    let view_ray = vec3<f32>(
        ndc.x * atmo.camera.z * atmo.camera.w,
        ndc.y * atmo.camera.z,
        -1.0,
    );
    let ray_length = length(view_ray);
    let dir = view_ray / ray_length;
    let d = textureLoad(depth_tex, ip, 0);
    // Infinite reverse-Z: view depth = near / d along -z.
    var t_max = 1.0e30;
    if d > 0.0 {
        t_max = atmo.params.x / d * ray_length;
    }

    let radius = atmo.shell.x;
    let top = radius + atmo.shell.y;
    let h0 = atmo.up.w;
    let r0 = radius + h0;
    let up = atmo.up.xyz;
    let mu = dot(dir, up);
    let sun = atmo.sun.xyz;
    let mu_sun = dot(sun, up);
    let nu = dot(dir, sun);

    // Segment inside the shell: from the camera (or the shell entry when the
    // camera is above it) to the surface, the ground sphere or the shell exit.
    var t0 = 0.0;
    if r0 > top {
        let entry_d = (top - r0) * (top + r0) + r0 * r0 * mu * mu;
        if entry_d < 0.0 || mu >= 0.0 {
            return scene;
        }
        t0 = -r0 * mu - sqrt(entry_d);
    }
    var t1 = ray_sphere_exit(r0, mu, top);
    let ground = ray_sphere_ground(r0, mu, radius);
    if ground > 0.0 {
        t1 = min(t1, ground);
    }
    t1 = min(t1, t_max);
    if t1 <= t0 {
        return scene;
    }

    let haze = atmo.params.z;
    let beta_r = atmo.beta.rgb * haze;
    let beta_m = atmo.beta.w * haze;
    let h_r = atmo.shell.z;
    let h_m = atmo.shell.w;
    let dt = (t1 - t0) / f32(STEPS);
    var optical = vec3<f32>(0.0);
    var inscatter_r = vec3<f32>(0.0);
    var inscatter_m = vec3<f32>(0.0);
    let base = h0 * (2.0 * radius + h0);
    for (var i = 0; i < STEPS; i += 1) {
        let t = t0 + (f32(i) + 0.5) * dt;
        // r(t)^2 - R^2 without cancellation.
        let excess = base + t * t + 2.0 * r0 * t * mu;
        let r = sqrt(max(radius * radius + excess, 0.0));
        let h = max(excess / (r + radius), 0.0);
        let density_r = exp(-h / h_r);
        let density_m = exp(-h / h_m);
        let step_optical = (beta_r * density_r + vec3<f32>(beta_m * density_m * 1.1)) * dt;
        let view_t = exp(-(optical + 0.5 * step_optical));
        optical += step_optical;
        // Sun light reaching the sample through the column above it.
        let cos_sun = (mu_sun * r0 + t * nu) / r;
        let lit = smoothstep(-0.08, 0.02, cos_sun);
        let column = air_mass(cos_sun) * (beta_r * h_r * density_r + vec3<f32>(beta_m * 1.1 * h_m * density_m));
        let sun_t = exp(-column) * lit;
        inscatter_r += view_t * sun_t * density_r * dt;
        inscatter_m += view_t * sun_t * density_m * dt;
    }
    let transmittance = exp(-optical);
    var inscatter = atmo.sun.w * (inscatter_r * beta_r * phase_rayleigh(nu)
        + inscatter_m * beta_m * phase_mie(nu, atmo.camera.x));
    // Look: tint and saturate the scattered light, scale the sky's share.
    let grey = luminance(inscatter);
    inscatter = mix(vec3<f32>(grey), inscatter, atmo.tint.w) * atmo.tint.rgb * atmo.params.w;
    let pre = exposure.value;
    return vec4<f32>(scene.rgb * transmittance + max(inscatter, vec3<f32>(0.0)) * pre, scene.a);
}
