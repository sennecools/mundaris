struct Projection { matrix: mat4x4<f32> }

struct DrawParams {
    region_origin_view: vec4<f32>,
    body_to_view_x: vec4<f32>,
    body_to_view_y: vec4<f32>,
    body_to_view_z: vec4<f32>,
    sun_body: vec4<f32>,
    natural_color_0: vec4<f32>,
    natural_color_1: vec4<f32>,
    natural_color_2: vec4<f32>,
    natural_color_3: vec4<f32>,
    base_color: vec4<f32>,
    dark_color: vec4<f32>,
    appearance: vec4<f32>, // ambient, diffuse, curvature darkening, curvature lightening
    controls: vec4<u32>, // debug view, triangle edges, cluster edges, residency state
    transition: vec4<f32>, // morph fine fraction, reserved
}

@group(0) @binding(0) var<uniform> projection: Projection;
@group(1) @binding(0) var<uniform> params: DrawParams;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) normal_varying: vec3<f32>,
    @location(1) material: vec4<f32>,
    @location(2) barycentric: vec3<f32>,
    @location(3) cluster_color: vec3<f32>,
    @location(4) lod_color: vec3<f32>,
    @location(5) @interpolate(flat) cluster_edge_mask: u32,
    @location(6) @interpolate(flat) detail_set: u32,
}

@vertex
fn vs_main(
    @location(0) fine_position: vec3<f32>,
    @location(1) coarse_position: vec3<f32>,
    @location(2) fine_normal: vec3<f32>,
    @location(3) coarse_normal: vec3<f32>,
    @location(4) fine_material: vec4<f32>,
    @location(5) coarse_material: vec4<f32>,
    @location(6) _uv: vec2<f32>,
    @location(7) barycentric: vec3<f32>,
    @location(8) cluster_color: vec3<f32>,
    @location(9) lod_color: vec3<f32>,
    @location(10) metadata: u32,
) -> VertexOut {
    let morph = clamp(params.transition.x, 0.0, 1.0);
    let local_position = mix(coarse_position, fine_position, morph);
    let local_normal = mix(coarse_normal, fine_normal, morph);
    let view_position = params.region_origin_view.xyz
        + params.body_to_view_x.xyz * local_position.x
        + params.body_to_view_y.xyz * local_position.y
        + params.body_to_view_z.xyz * local_position.z;
    var output: VertexOut;
    output.clip_position = projection.matrix * vec4<f32>(view_position, 1.0);
    output.normal_varying = local_normal;
    output.material = mix(coarse_material, fine_material, morph);
    output.barycentric = barycentric;
    output.cluster_color = cluster_color;
    output.lod_color = lod_color;
    output.cluster_edge_mask = metadata & 7u;
    output.detail_set = (metadata >> 4u) & 3u;
    return output;
}

@fragment
fn fs_main(input: VertexOut) -> @location(0) vec4<f32> {
    let normal = normalize(input.normal_varying);
    var color = vec3<f32>(0.35, 0.38, 0.34);
    color = input.material.x * params.natural_color_0.xyz
        + input.material.y * params.natural_color_1.xyz
        + input.material.z * params.natural_color_2.xyz
        + input.material.w * params.natural_color_3.xyz;
    let curvature = clamp(1.0 - length(input.normal_varying), 0.0, 1.0);
    color = mix(color, params.dark_color.xyz, curvature * params.appearance.z);
    color = mix(color, params.base_color.xyz, curvature * params.appearance.w);
    let diffuse = max(dot(normal, normalize(params.sun_body.xyz)), 0.0);
    color *= params.appearance.x + params.appearance.y * diffuse;

    if params.controls.x == 1u {
        color = input.cluster_color;
    } else if params.controls.x == 2u {
        color = input.lod_color;
    } else if params.controls.x == 3u {
        if params.controls.w == 1u {
            color = vec3<f32>(0.93, 0.68, 0.33); // retained coarse fallback
        } else if params.controls.w == 2u {
            color = vec3<f32>(0.65, 0.51, 0.94); // finer work pending
        } else {
            color = vec3<f32>(0.27, 0.80, 0.86); // selected resident detail
        }
    }

    if params.controls.y == 1u {
        let derivative = max(fwidth(input.barycentric), vec3<f32>(1.0e-6));
        let edge_distance = input.barycentric / derivative;
        let triangle_edge = min(edge_distance.x, min(edge_distance.y, edge_distance.z));
        color = mix(color, vec3<f32>(0.025, 0.035, 0.045), 1.0 - smoothstep(0.75, 1.65, triangle_edge));
    }
    if params.controls.z == 1u {
        let derivative = max(fwidth(input.barycentric), vec3<f32>(1.0e-6));
        var boundary = 1.0e6;
        if (input.cluster_edge_mask & 1u) != 0u { boundary = min(boundary, input.barycentric.x / derivative.x); }
        if (input.cluster_edge_mask & 2u) != 0u { boundary = min(boundary, input.barycentric.y / derivative.y); }
        if (input.cluster_edge_mask & 4u) != 0u { boundary = min(boundary, input.barycentric.z / derivative.z); }
        color = mix(color, vec3<f32>(0.98, 0.91, 0.72), 1.0 - smoothstep(0.75, 1.65, boundary));
    }
    return vec4<f32>(color, 1.0);
}
