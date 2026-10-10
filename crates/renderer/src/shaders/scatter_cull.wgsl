// PROTOTYPE (M5 Life): GPU culling of the plant candidates. One thread per
// (drawn node, slot); survivors are appended to `plants_out` and counted in
// the indirect draw arguments (`plant_args` = vertex count, instance count,
// first vertex, first instance, then the staged node count).

@group(3) @binding(0) var<storage, read_write> plants_out: array<Plant>;
@group(3) @binding(1) var<storage, read_write> plant_args: array<atomic<u32>>;
// Grass tufts: placed by cs_grass, counted in plant_args[9] (draw arguments
// at words 8..11).
@group(3) @binding(2) var<storage, read_write> grass_out: array<Plant>;

@compute @workgroup_size(64)
fn cs_scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x + id.y * 65535u * 64u;
    let staged = atomicLoad(&plant_args[4]);
    if index >= staged * SCATTER_SLOTS {
        return;
    }
    let placed = sc_place(index);
    if !placed.ok {
        return;
    }
    let slot = atomicAdd(&plant_args[1], 1u);
    if slot >= arrayLength(&plants_out) {
        // Full: undo the count (the final count settles at the capacity).
        atomicSub(&plant_args[1], 1u);
        return;
    }
    plants_out[slot] = placed.plant;
}

@compute @workgroup_size(64)
fn cs_grass(@builtin(global_invocation_id) id: vec3<u32>) {
    let index = id.x + id.y * 65535u * 64u;
    let staged = atomicLoad(&plant_args[4]);
    if index >= staged * SCATTER_SLOTS {
        return;
    }
    let placed = sc_place_grass(index);
    if !placed.ok {
        return;
    }
    let slot = atomicAdd(&plant_args[9], 1u);
    if slot >= arrayLength(&grass_out) {
        atomicSub(&plant_args[9], 1u);
        return;
    }
    grass_out[slot] = placed.plant;
}