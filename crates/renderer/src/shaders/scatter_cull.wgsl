// PROTOTYPE (M5 Life): GPU culling of the plant candidates. One thread per
// (drawn node, slot); survivors are appended to `plants_out` and counted in
// the indirect draw arguments (`plant_args` = vertex count, instance count,
// first vertex, first instance, then the staged node count).
//
// PROTOTYPE (flora lane): trees and shrubs of a kind with a grown species
// (mask in plant_args[5], bit = entry: species, then rocks from FL_ROCK_ENTRY)
// go to flora buckets instead: one
// region of `plants_out` and one indexed indirect draw per (entry, variant,
// LOD). Keep FLORA_* in step with crates/renderer/src/flora_draw.rs.

@group(3) @binding(0) var<storage, read_write> plants_out: array<Plant>;
@group(3) @binding(1) var<storage, read_write> plant_args: array<atomic<u32>>;
// Grass tufts: placed by cs_grass, counted in plant_args[9] (draw arguments
// at words 8..11).
@group(3) @binding(2) var<storage, read_write> grass_out: array<Plant>;

// Procedural plants use slots [0, FLORA_FAR_CAPACITY); flora buckets follow.
const FLORA_FAR_CAPACITY: u32 = 98304u;
const FLORA_VARIANTS: u32 = 2u;
const FLORA_LODS: u32 = 3u;
const FLORA_MASK_WORD: u32 = 5u;
const FLORA_ARGS_WORD: u32 = 16u;
// Plants rejected because their bucket (word 6) or the procedural region
// (word 7) was full; read back by flora_draw.rs (flora_overflow()).
const FLORA_OVERFLOW_WORD: u32 = 6u;
const FAR_OVERFLOW_WORD: u32 = 7u;
// Instances per bucket by LOD and their sum per (kind, variant).
const FLORA_CAP0: u32 = 1024u;
const FLORA_CAP1: u32 = 3072u;
const FLORA_CAP2: u32 = 6144u;
// LOD switch distances (m) for a 1-scale tree; shrubs switch at half. Each
// plant dithers its own switch over ±15 % so LOD changes never line up.
const FLORA_LOD0_M: f32 = 90.0;
const FLORA_LOD1_M: f32 = 300.0;
const FLORA_FAR_M: f32 = 1100.0;

fn flora_capacity(lod: u32) -> u32 {
    return select(select(FLORA_CAP2, FLORA_CAP1, lod == 1u), FLORA_CAP0, lod == 0u);
}

fn flora_base(bucket: u32) -> u32 {
    let lod = bucket % FLORA_LODS;
    let before = select(select(FLORA_CAP0 + FLORA_CAP1, FLORA_CAP0, lod == 1u), 0u, lod == 0u);
    return FLORA_FAR_CAPACITY + (bucket / FLORA_LODS) * (FLORA_CAP0 + FLORA_CAP1 + FLORA_CAP2) + before;
}

// Route a placed plant to a flora bucket; false when it stays procedural.
fn flora_route(plant: Plant) -> bool {
    let species = plant.info.x >> 8u;
    let mask = atomicLoad(&plant_args[FLORA_MASK_WORD]);
    if species == 0u || (mask & (1u << (species - 1u))) == 0u {
        return false;
    }
    let kind = species - 1u;
    // Shrubs and rocks are small: they switch LODs at half the distance.
    let reach = select(1.0, 0.5, (plant.info.x & 0xffu) >= 2u) * (0.85 + 0.3 * sc_unit(plant.info.z ^ 0x9e3779b9u));
    let d = length(plant.base.xyz) / reach;
    if d >= FLORA_FAR_M {
        return false;
    }
    let lod = select(select(2u, 1u, d < FLORA_LOD1_M), 0u, d < FLORA_LOD0_M);
    let variant = plant.info.y % FLORA_VARIANTS;
    let bucket = (kind * FLORA_VARIANTS + variant) * FLORA_LODS + lod;
    let word = FLORA_ARGS_WORD + bucket * 5u + 1u;
    let slot = atomicAdd(&plant_args[word], 1u);
    if slot >= flora_capacity(lod) {
        atomicSub(&plant_args[word], 1u);
        atomicAdd(&plant_args[FLORA_OVERFLOW_WORD], 1u);
        return true;
    }
    plants_out[flora_base(bucket) + slot] = plant;
    return true;
}

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
    if flora_route(placed.plant) {
        return;
    }
    let slot = atomicAdd(&plant_args[1], 1u);
    if slot >= min(arrayLength(&plants_out), FLORA_FAR_CAPACITY) {
        // Full: undo the count (the final count settles at the capacity).
        atomicSub(&plant_args[1], 1u);
        atomicAdd(&plant_args[FAR_OVERFLOW_WORD], 1u);
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