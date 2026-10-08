// Grid hydraulic (virtual pipes) and thermal erosion on a periodic grid.
// Units: cell length 1, heights and water depth in cells. Each pass reads only
// values no other invocation of the same pass writes, so results do not depend
// on scheduling and the bake is repeatable on one adapter. Sediment transport
// and thermal relaxation move material through paired per-direction fluxes, so
// terrain plus suspended sediment is conserved up to rounding.

struct Params {
    n: u32,
    // Base level in cells: at or below it water and sediment leave the tile.
    outlet_level: f32,
    max_speed: f32,
    _pad2: u32,
    dt: f32,
    gravity: f32,
    rain: f32,
    evaporation: f32,
    capacity: f32,
    dissolve: f32,
    deposit: f32,
    min_tilt: f32,
    depth_reference: f32,
    talus_tangent: f32,
    thermal_rate: f32,
    max_erosion: f32,
}

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read_write> terrain: array<f32>;
@group(0) @binding(2) var<storage, read_write> terrain_tmp: array<f32>;
@group(0) @binding(3) var<storage, read_write> water: array<f32>;
@group(0) @binding(4) var<storage, read_write> sediment_in: array<f32>;
@group(0) @binding(5) var<storage, read_write> sediment_out: array<f32>;
// Outflow per direction: x = -x, y = +x, z = -y, w = +y.
@group(0) @binding(6) var<storage, read_write> flux: array<vec4<f32>>;
@group(0) @binding(7) var<storage, read_write> velocity: array<vec2<f32>>;
// Thermal outflow: axis as flux; diagonal x = (-x,-y), y = (+x,-y), z = (-x,+y), w = (+x,+y).
@group(0) @binding(8) var<storage, read_write> thermal_axis: array<vec4<f32>>;
@group(0) @binding(9) var<storage, read_write> thermal_diagonal: array<vec4<f32>>;
@group(0) @binding(10) var<storage, read> hardness: array<f32>;

const SQRT2: f32 = 1.41421356237;
const DRY: f32 = 1.0e-6;

fn wrap(v: i32) -> u32 {
    let n = i32(params.n);
    return u32(((v % n) + n) % n);
}

fn at(x: i32, y: i32) -> u32 {
    return wrap(y) * params.n + wrap(x);
}

fn sum4(v: vec4<f32>) -> f32 {
    return v.x + v.y + v.z + v.w;
}

fn surface(i: u32) -> f32 {
    return terrain[i] + water[i];
}

@compute @workgroup_size(16, 16)
fn flux_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.n || id.y >= params.n {
        return;
    }
    let x = i32(id.x);
    let y = i32(id.y);
    let i = at(x, y);
    let h = surface(i);
    let dh = vec4<f32>(
        h - surface(at(x - 1, y)),
        h - surface(at(x + 1, y)),
        h - surface(at(x, y - 1)),
        h - surface(at(x, y + 1)),
    );
    var f = max(vec4<f32>(0.0), flux[i] + params.dt * params.gravity * dh);
    let total = sum4(f);
    if total > 0.0 {
        f *= min(1.0, water[i] / (total * params.dt));
    }
    flux[i] = f;
}

fn out_fraction(i: u32) -> vec4<f32> {
    let d = water[i];
    if d <= DRY {
        return vec4<f32>(0.0);
    }
    return flux[i] * (params.dt / d);
}

@compute @workgroup_size(16, 16)
fn sediment_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.n || id.y >= params.n {
        return;
    }
    let x = i32(id.x);
    let y = i32(id.y);
    let i = at(x, y);
    var s = sediment_in[i] * max(0.0, 1.0 - sum4(out_fraction(i)));
    let left = at(x - 1, y);
    let right = at(x + 1, y);
    let down = at(x, y - 1);
    let up = at(x, y + 1);
    s += sediment_in[left] * out_fraction(left).y;
    s += sediment_in[right] * out_fraction(right).x;
    s += sediment_in[down] * out_fraction(down).w;
    s += sediment_in[up] * out_fraction(up).z;
    sediment_out[i] = s;
}

@compute @workgroup_size(16, 16)
fn water_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.n || id.y >= params.n {
        return;
    }
    let x = i32(id.x);
    let y = i32(id.y);
    let i = at(x, y);
    let f = flux[i];
    let fl = flux[at(x - 1, y)];
    let fr = flux[at(x + 1, y)];
    let fd = flux[at(x, y - 1)];
    let fu = flux[at(x, y + 1)];
    let inflow = fl.y + fr.x + fd.w + fu.z;
    let d0 = water[i];
    let d1 = max(0.0, d0 + params.dt * (inflow - sum4(f)));
    water[i] = d1;
    let mean_depth = 0.5 * (d0 + d1);
    var v = vec2<f32>(0.0);
    if mean_depth > 1.0e-4 {
        v = vec2<f32>(0.5 * (fl.y - f.x + f.y - fr.x), 0.5 * (fd.w - f.z + f.w - fu.z)) / mean_depth;
    }
    velocity[i] = v;
}

@compute @workgroup_size(16, 16)
fn erode_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.n || id.y >= params.n {
        return;
    }
    let x = i32(id.x);
    let y = i32(id.y);
    let i = at(x, y);
    let b = terrain[i];
    let gx = 0.5 * (terrain[at(x + 1, y)] - terrain[at(x - 1, y)]);
    let gy = 0.5 * (terrain[at(x, y + 1)] - terrain[at(x, y - 1)]);
    let tilt2 = gx * gx + gy * gy;
    let sine = max(params.min_tilt, sqrt(tilt2 / (1.0 + tilt2)));
    let d = water[i];
    // Capacity scales with the water volume present (up to the reference
    // depth) and a bounded speed, so a thin film cannot carry an unbounded
    // load that later collapses into a single-cell pillar.
    let speed = min(length(velocity[i]), params.max_speed);
    let capacity = params.capacity * sine * speed * min(d, params.depth_reference);
    var s = sediment_out[i];
    var next = b;
    if capacity > s {
        let amount = min(params.max_erosion, params.dt * params.dissolve * (1.0 - hardness[i]) * (capacity - s));
        next = b - amount;
        s += amount;
    } else {
        let amount = min(s, params.dt * params.deposit * (s - capacity));
        next = b + amount;
        s -= amount;
    }
    var water_next = d * max(0.0, 1.0 - params.evaporation * params.dt) + params.rain * params.dt;
    // The base level is fixed: outlet cells neither erode nor keep load.
    if b <= params.outlet_level {
        next = b;
        s = 0.0;
        water_next = 0.0;
    }
    terrain_tmp[i] = next;
    sediment_out[i] = s;
    water[i] = water_next;
}

@compute @workgroup_size(16, 16)
fn thermal_flux_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.n || id.y >= params.n {
        return;
    }
    let x = i32(id.x);
    let y = i32(id.y);
    let b = terrain_tmp[at(x, y)];
    let axis = max(vec4<f32>(0.0), vec4<f32>(
        b - terrain_tmp[at(x - 1, y)],
        b - terrain_tmp[at(x + 1, y)],
        b - terrain_tmp[at(x, y - 1)],
        b - terrain_tmp[at(x, y + 1)],
    ) - vec4<f32>(params.talus_tangent));
    let diagonal = max(vec4<f32>(0.0), vec4<f32>(
        b - terrain_tmp[at(x - 1, y - 1)],
        b - terrain_tmp[at(x + 1, y - 1)],
        b - terrain_tmp[at(x - 1, y + 1)],
        b - terrain_tmp[at(x + 1, y + 1)],
    ) - vec4<f32>(params.talus_tangent * SQRT2));
    let total = sum4(axis) + sum4(diagonal);
    var out_axis = vec4<f32>(0.0);
    var out_diagonal = vec4<f32>(0.0);
    if total > 0.0 {
        let largest = max(
            max(max(axis.x, axis.y), max(axis.z, axis.w)),
            max(max(diagonal.x, diagonal.y), max(diagonal.z, diagonal.w)),
        );
        // Moving half the largest excess (scaled by rate) cannot invert the slope.
        let amount = min(1.0, params.dt * params.thermal_rate) * 0.5 * largest;
        out_axis = axis * (amount / total);
        out_diagonal = diagonal * (amount / total);
    }
    let i = at(x, y);
    thermal_axis[i] = out_axis;
    thermal_diagonal[i] = out_diagonal;
}

@compute @workgroup_size(16, 16)
fn thermal_apply_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.n || id.y >= params.n {
        return;
    }
    let x = i32(id.x);
    let y = i32(id.y);
    let i = at(x, y);
    var b = terrain_tmp[i] - sum4(thermal_axis[i]) - sum4(thermal_diagonal[i]);
    b += thermal_axis[at(x - 1, y)].y + thermal_axis[at(x + 1, y)].x;
    b += thermal_axis[at(x, y - 1)].w + thermal_axis[at(x, y + 1)].z;
    b += thermal_diagonal[at(x - 1, y - 1)].w + thermal_diagonal[at(x + 1, y - 1)].z;
    b += thermal_diagonal[at(x - 1, y + 1)].y + thermal_diagonal[at(x + 1, y + 1)].x;
    terrain[i] = b;
}

// Settle phase: deposit the suspended load in place and drop the water,
// leaving terrain in terrain_tmp for a thermal-only relaxation step. It is
// bound so that sediment_out holds the latest load; clearing it means the
// load is deposited exactly once across repeated settle steps.
@compute @workgroup_size(16, 16)
fn settle_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.n || id.y >= params.n {
        return;
    }
    let i = id.y * params.n + id.x;
    terrain_tmp[i] = terrain[i] + sediment_out[i];
    sediment_out[i] = 0.0;
    water[i] = 0.0;
}
