struct ClusterMeta {
    sphere: vec4<f32>,
    command: vec4<u32>, // first index, index count, base vertex, detail set
}

struct SelectionParams {
    planes: array<vec4<f32>, 5>,
    region_origin_view: vec4<f32>,
    body_to_view_x: vec4<f32>,
    body_to_view_y: vec4<f32>,
    body_to_view_z: vec4<f32>,
    detail: vec4<f32>, // focal pixels, near plane, coarse deviation metres, fine-mesh sphere radius
    controls: vec4<u32>, // count, mode (0 culling / 1 lod / 2-4 frozen sets), transition active, coarse eligible
}

struct DrawIndexedIndirectArgs {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

@group(0) @binding(0) var<uniform> params: SelectionParams;
@group(0) @binding(1) var<storage, read> clusters: array<ClusterMeta>;
@group(0) @binding(2) var<storage, read_write> commands: array<DrawIndexedIndirectArgs>;

fn body_to_view(value: vec3<f32>) -> vec3<f32> {
    return params.body_to_view_x.xyz * value.x
        + params.body_to_view_y.xyz * value.y
        + params.body_to_view_z.xyz * value.z;
}

fn sphere_visible(center: vec3<f32>, radius: f32) -> bool {
    for (var i = 0u; i < 5u; i += 1u) {
        let plane = params.planes[i];
        if dot(plane.xyz, center) + plane.w < -radius {
            return false;
        }
    }
    return true;
}

@compute @workgroup_size(64)
fn select_main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let index = invocation.x;
    if index >= params.controls.x { return; }

    let cluster = clusters[index];
    let center_view = params.region_origin_view.xyz + body_to_view(cluster.sphere.xyz);
    let cluster_radius = cluster.sphere.w;
    var selected_set = 0u; // fine cut
    let center = params.region_origin_view.xyz;
    let depth = -center.z;
    let epsilon = params.detail.z;
    let region_radius = params.detail.w;
    let dmin = depth - region_radius - epsilon;
    let crosses_near = dmin <= params.detail.y;
    if params.controls.y >= 2u {
        selected_set = params.controls.y - 2u; // frozen fine/coarse/transition cut
    } else {
        if params.controls.y == 1u {
            if params.controls.w == 1u && !crosses_near {
                let off_axis = max(abs(center.x), abs(center.y)) + region_radius + epsilon;
                let projected_error = (params.detail.x * epsilon / dmin) * (1.0 + off_axis / dmin);
                if projected_error < 0.15 {
                    selected_set = 1u; // strict threshold: equality remains fine
                }
            }
        }
        if params.controls.z == 1u {
            selected_set = 2u; // exact common-refinement endpoints, CPU-timed morph
        }
    }

    let visible = sphere_visible(center_view, cluster_radius);
    let chosen = visible && cluster.command.w == selected_set;
    commands[index].index_count = select(0u, cluster.command.y, chosen);
    commands[index].instance_count = 1u;
    commands[index].first_index = cluster.command.x;
    commands[index].base_vertex = i32(cluster.command.z);
    commands[index].first_instance = 0u;
}
