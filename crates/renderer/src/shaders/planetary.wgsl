// Visual noise and scattering are renderer approximations, never terrain authority.
diagnostic(off, derivative_uniformity);
struct Environment {
    observer_radius: vec4<f32>,
    sun_body: vec4<f32>,
    axis_x: vec4<f32>,
    axis_y: vec4<f32>,
    axis_z: vec4<f32>,
    shape: vec4<f32>,
    view: vec4<f32>, // width, height, tan-half-fov, near
    atmosphere: vec4<f32>, // height, density falloff, reserved, reserved
    options: vec4<u32>, // layer bits, profile, srgb target, reserved
    sphere_constants: vec4<f32>, // f64-factored |observer|^2 - shell_radius^2
    scattering: vec4<f32>, // vertical Rayleigh RGB and Mie optical depth
}
@group(0) @binding(0) var<uniform> env: Environment;
@group(1) @binding(0) var scene_depth: texture_depth_2d;

struct VertexOut { @builtin(position) position: vec4<f32>, @location(0) uv: vec2<f32> }
@vertex fn vs_main(@builtin(vertex_index) i: u32) -> VertexOut {
    var p = array<vec2<f32>, 3>(vec2<f32>(-1.0,-1.0), vec2<f32>(3.0,-1.0), vec2<f32>(-1.0,3.0));
    var o: VertexOut; o.position = vec4<f32>(p[i], 0.0, 1.0); o.uv = p[i] * vec2<f32>(0.5,-0.5) + vec2<f32>(0.5); return o;
}
fn ray_direction(uv: vec2<f32>) -> vec3<f32> {
    let ndc = uv * 2.0 - vec2<f32>(1.0);
    return normalize(vec3<f32>(ndc.x * env.view.x / env.view.y * env.view.z, -ndc.y * env.view.z, -1.0));
}
fn body_ray(view_ray: vec3<f32>) -> vec3<f32> {
    return normalize(vec3<f32>(dot(view_ray,env.axis_x.xyz),dot(view_ray,env.axis_y.xyz),dot(view_ray,env.axis_z.xyz)));
}
fn factored_hit(ro: vec3<f32>, rd: vec3<f32>, c: f32) -> vec2<f32> {
    let b = dot(ro, rd); let d = b*b - c;
    if (d < 0.0) { return vec2<f32>(-1.0); }
    // q avoids cancellation of the near hit when descending to a shell. The
    // constant is factored in f64 on CPU, preserving metre clearances at real scale.
    let q = -b - select(-1.0, 1.0, b >= 0.0) * sqrt(d);
    if (abs(q) < 1e-20) { return vec2<f32>(0.0); }
    let other = c / q;
    return vec2<f32>(min(q,other), max(q,other));
}
fn sphere_hit(ro: vec3<f32>, rd: vec3<f32>, radius: f32) -> vec2<f32> {
    return factored_hit(ro,rd,dot(ro,ro)-radius*radius);
}
fn hash_cell(cell: vec3<u32>) -> f32 {
    var h = cell.x*1597334677u ^ cell.y*3812015801u ^ cell.z*2798796415u;
    h = (h ^ (h >> 16u))*2246822519u;
    h = (h ^ (h >> 13u))*3266489917u;
    return f32(h ^ (h >> 16u)) / 4294967295.0;
}
fn noise(p: vec3<f32>) -> f32 {
    let cell = vec3<u32>(vec3<i32>(floor(p)));
    let t = fract(p); let w = t*t*t*(t*(t*6.0-15.0)+10.0);
    let a = mix(hash_cell(cell),hash_cell(cell+vec3<u32>(1u,0u,0u)),w.x);
    let b = mix(hash_cell(cell+vec3<u32>(0u,1u,0u)),hash_cell(cell+vec3<u32>(1u,1u,0u)),w.x);
    let c = mix(hash_cell(cell+vec3<u32>(0u,0u,1u)),hash_cell(cell+vec3<u32>(1u,0u,1u)),w.x);
    let d = mix(hash_cell(cell+vec3<u32>(0u,1u,1u)),hash_cell(cell+vec3<u32>(1u,1u,1u)),w.x);
    return mix(mix(a,b,w.y),mix(c,d,w.y),w.z);
}
fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(max(c,vec3<f32>(0.0)),vec3<f32>(1.0/2.4))-0.055, c*12.92, c <= vec3<f32>(0.0031308));
}
struct ColorDepth { @location(0) color: vec4<f32>, @builtin(frag_depth) depth: f32 }
@fragment fn fs_ocean(input: VertexOut) -> ColorDepth {
    let view_rd = ray_direction(input.uv); let rd=body_ray(view_rd); let ro=env.observer_radius.xyz;
    let radius = 1.0 + env.shape.x/env.observer_radius.w;
    let hit = factored_hit(ro, rd, env.sphere_constants.x); let t = select(hit.x, hit.y, hit.x <= 0.0);
    if (t <= 0.0) { discard; }
    let p = ro + rd*t; let z = t*env.observer_radius.w*-view_rd.z;
    if (z < env.view.w) { discard; }
    let sun = normalize(env.sun_body.xyz); let n = normalize(p);
    let diffuse = max(dot(n,sun),0.0);
    let v = -rd; let nv = max(abs(dot(n,v)),0.001);
    let fresnel = 0.02 + 0.98*pow(1.0-nv,5.0);
    let halfway = normalize(v+sun+vec3<f32>(1e-8));
    let roughness = clamp(env.atmosphere.z,0.08,0.8);
    let exponent = min(2.0/pow(roughness,4.0),2048.0);
    let highlight = pow(max(dot(n,halfway),0.0),exponent)*(exponent+2.0)*0.125*fresnel*diffuse;
    // A bounded sky-reflection approximation plus a coherent solar specular lobe.
    // No water displacement or hidden land/sea classification changes occur.
    var rgb = vec3<f32>(0.001,0.003,0.007)
        + vec3<f32>(0.004,0.025,0.055)*diffuse
        + vec3<f32>(0.09,0.19,0.32)*fresnel*smoothstep(-0.08,0.25,dot(n,sun))
        + vec3<f32>(1.0,0.94,0.82)*highlight*env.atmosphere.w;
    rgb = rgb/(vec3<f32>(1.0)+rgb);
    if (env.options.z == 0u) { rgb = srgb_encode(rgb); }
    var o: ColorDepth; o.color = vec4<f32>(rgb,1.0); o.depth = env.view.w/z; return o;
}
@fragment fn fs_cloud(input: VertexOut) -> ColorDepth {
    let view_rd=ray_direction(input.uv); let rd=body_ray(view_rd); let hit=factored_hit(env.observer_radius.xyz,rd,env.sphere_constants.y);
    let t=select(hit.x,hit.y,hit.x<=0.0); if (t<=0.0) { discard; }
    let p=env.observer_radius.xyz+rd*t; let z=t*env.observer_radius.w*-view_rd.z; if (z<env.view.w) { discard; }
    let n=normalize(p);
    let footprint = length(fwidth(n));
    let broad = noise(n*4.0+vec3<f32>(37.0,13.0,23.0));
    let regional = noise(n*13.0+vec3<f32>(11.0,43.0,17.0));
    let medium = mix(0.5,noise(n*37.0+vec3<f32>(71.0)),1.0-smoothstep(0.25,0.8,footprint*37.0));
    let fine = mix(0.5,noise(n*97.0+vec3<f32>(131.0)),1.0-smoothstep(0.25,0.8,footprint*97.0));
    let field = 0.48*broad+0.28*regional+0.16*medium+0.08*fine;
    let threshold = 0.5+(0.5-env.shape.z)*0.7;
    let softness = 0.035+env.shape.w*0.10;
    let cloud=smoothstep(threshold-softness,threshold+softness,field);
    if (cloud<0.01) { discard; }
    let sun=normalize(env.sun_body.xyz);
    let lit=0.015+0.85*max(dot(n,sun),0.0)+0.10*pow(max(dot(rd,sun),0.0),8.0)*smoothstep(-0.05,0.1,dot(n,sun));
    var rgb=vec3<f32>(0.88,0.91,0.96)*lit; if (env.options.z==0u) { rgb=srgb_encode(rgb); }
    var o: ColorDepth; o.color=vec4<f32>(rgb,cloud*0.82); o.depth=env.view.w/z; return o;
}
fn density(p: vec3<f32>, height: f32) -> f32 {
    return exp(-max(length(p)-1.0,0.0)*env.atmosphere.y/height);
}
@fragment fn fs_atmosphere(input: VertexOut) -> @location(0) vec4<f32> {
    let dims=textureDimensions(scene_depth); let xy=vec2<i32>(input.position.xy);
    let depth=textureLoad(scene_depth,clamp(xy,vec2<i32>(0),vec2<i32>(dims)-vec2<i32>(1)),0);
    let view_rd=ray_direction(input.uv); let rd=body_ray(view_rd); let base=1.0; let outer=base+env.atmosphere.x/env.observer_radius.w;
    let interval=factored_hit(env.observer_radius.xyz,rd,env.sphere_constants.z); if (interval.y<=0.0) { discard; }
    var end_t=interval.y;
    if (depth>0.0) { end_t=min(end_t,(env.view.w/depth/max(-view_rd.z,1e-5))/env.observer_radius.w); }
    var start_t=max(interval.x,0.0); if (end_t<=start_t) { discard; }
    let inner=factored_hit(env.observer_radius.xyz,rd,env.sphere_constants.w);
    if (inner.x>start_t) { end_t=min(end_t,inner.x); }
    else if (inner.y>start_t) { start_t=inner.y; }
    if (end_t<=start_t) { discard; }
    let height = max(outer-1.0,1e-6);
    // Optical depth scales with the configured layer, not a hidden gameplay radius.
    let beta_r = env.scattering.xyz*env.atmosphere.y/height;
    let beta_m = vec3<f32>(env.scattering.w)*env.atmosphere.y/height;
    let beta = beta_r+beta_m;
    let sun=normalize(env.sun_body.xyz); let mu=dot(rd,sun);
    let phase_r=0.0596831*(1.0+mu*mu);
    let g=0.72; let phase_m=(1.0-g*g)/(12.566371*pow(1.0+g*g-2.0*g*mu,1.5));
    let step=(end_t-start_t)/12.0; var optical=0.0; var scattering=vec3<f32>(0.0);
    for (var i=0u;i<12u;i=i+1u) {
        let p=env.observer_radius.xyz+rd*(start_t+(f32(i)+0.5)*step);
        let local = density(p,height)*step;
        let sun_interval=sphere_hit(p,sun,outer);
        let shadow=dot(p,sun)<0.0 && dot(cross(p,sun),cross(p,sun))<1.0;
        var sun_optical=0.0;
        let sun_step=max(sun_interval.y,0.0)/4.0;
        for (var j=0u;j<4u;j=j+1u) {
            sun_optical+=density(p+sun*(f32(j)+0.5)*sun_step,height)*sun_step;
        }
        if (!shadow) {
            scattering+=exp(-beta*(optical+0.5*local+sun_optical))
                *(beta_r*phase_r+beta_m*phase_m)*local*env.atmosphere.w*4.0;
        }
        optical+=local;
    }
    // Scalar extinction is a compact approximation to wavelength-dependent
    // transmission; scattering itself retains Rayleigh/Mie wavelength behaviour.
    var alpha=clamp(1.0-exp(-optical*0.5*(beta.x+beta.z)),0.0,0.95);
    // Decorative sky is not calibrated stellar radiance. Scalar extinction alone
    // leaves it visible over daylight scattering in this bounded LDR composition.
    // Suppress only zero-depth sky near a lit surface; foreground transmission,
    // night/airless views and observers above the atmosphere remain unchanged.
    if (depth==0.0) {
        let altitude_fraction=clamp((length(env.observer_radius.xyz)-1.0)/height,0.0,1.0);
        let near_surface=1.0-smoothstep(0.2,0.8,altitude_fraction);
        let daylight=smoothstep(-0.12,0.15,dot(normalize(env.observer_radius.xyz),sun));
        let decorative_visibility=mix(1.0,0.0005,near_surface*daylight);
        alpha=1.0-(1.0-alpha)*decorative_visibility;
    }
    let rgb=scattering/max(alpha,1e-5);
    var out=vec4<f32>(rgb,alpha);
    if (env.options.z==0u) { out=vec4<f32>(srgb_encode(rgb),out.a); }
    return out;
}
