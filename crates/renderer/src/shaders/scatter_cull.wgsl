// PROTOTYPE (M5 Life): GPU culling of the plant candidates. One thread per
// (drawn node, slot); survivors are appended to `plants_out` and counted in
// the indirect draw arguments (`plant_args` = vertex count, instance count,
// first vertex, first instance, then the staged node count).
//
// PROTOTYPE (flora lane): plants of a grown species or rock (mask in
// plant_args[5], bit = entry) go to flora buckets instead, one per
// (entry, variant, tier) with tiers LOD0 mesh, LOD1 mesh, impostor. Inside
// a distance band around each tier change a plant goes to both tiers with
// complementary dither (cross-fade). No bucket has a fixed size: cs_scatter
// appends flora plants to a staging region and counts per bucket,
// cs_flora_prefix turns counts into bucket bases (header) and draw
// arguments, cs_flora_scatter copies each plant to its bucket. Keep FLORA_*
// in step with crates/renderer/src/flora_draw.rs.

@group(3) @binding(0) var<storage, read_write> plants_out: array<Plant>;
@group(3) @binding(1) var<storage, read_write> plant_args: array<atomic<u32>>;
// Grass tufts: placed by cs_grass, counted in plant_args[9] (draw arguments
// at words 8..11).
@group(3) @binding(2) var<storage, read_write> grass_out: array<Plant>;

// plants_out layout: procedural fallback [0, FAR), bucket-base header
// [FAR, FAR + HEADER), staging, then sorted buckets.
const FLORA_FAR_CAPACITY: u32 = 32768u;
const FLORA_HEADER: u32 = 32u;
const FLORA_STAGE: u32 = 114656u;
const FLORA_STAGE_START: u32 = 32800u;
const FLORA_SORTED_START: u32 = 147456u;
const FLORA_VARIANTS: u32 = 2u;
const FLORA_TIERS: u32 = 3u;
const FLORA_ENTRIES: u32 = 14u;
const FLORA_BUCKETS: u32 = 84u;
const FLORA_MASK_WORD: u32 = 5u;
// Plants dropped because staging (word 6) or the procedural region (word
// 7) was full; read back by flora_draw.rs (flora_overflow()).
const FLORA_OVERFLOW_WORD: u32 = 6u;
const FAR_OVERFLOW_WORD: u32 = 7u;
const FLORA_STAGE_WORD: u32 = 12u;
const FLORA_ARGS_WORD: u32 = 16u;
const FLORA_CURSOR_WORD: u32 = 448u;
// Tier bands (m, 1-scale tree; shrubs and rocks at half): cross-fade LOD0 →
// LOD1 over [A0, A1], LOD1 → impostor over [B0, B1]. Each plant jitters its
// bands by ±15 % so no band lines up.
const FLORA_A0: f32 = 70.0;
const FLORA_A1: f32 = 110.0;
const FLORA_B0: f32 = 250.0;
const FLORA_B1: f32 = 350.0;

fn flora_stage(p_in: Plant, bucket: u32, split: f32) {
    var p = p_in;
    p.info.x = (p.info.x & 0xffffu) | (bucket << 16u);
    p.up.w = split;
    let slot = atomicAdd(&plant_args[FLORA_STAGE_WORD], 1u);
    if slot >= FLORA_STAGE {
        atomicAdd(&plant_args[FLORA_OVERFLOW_WORD], 1u);
        return;
    }
    plants_out[FLORA_STAGE_START + slot] = p;
    atomicAdd(&plant_args[FLORA_ARGS_WORD + bucket * 5u + 1u], 1u);
}

// Route a placed plant to flora buckets; false when it stays procedural.
// `up.w` carries the tier split: s > 0 keeps dither < s, s < 0 keeps
// dither ≥ 1 + s (the complementary half of a cross-fade).
fn flora_route(plant: Plant) -> bool {
    let species = (plant.info.x >> 8u) & 0xffu;
    let mask = atomicLoad(&plant_args[FLORA_MASK_WORD]);
    if species == 0u || species > FLORA_ENTRIES || (mask & (1u << (species - 1u))) == 0u {
        return false;
    }
    let entry = species - 1u;
    // Shrubs and rocks are small: they switch tiers at half the distance.
    let reach = select(1.0, 0.5, (plant.info.x & 0xffu) >= 2u) * (0.85 + 0.3 * sc_unit(plant.info.z ^ 0x9e3779b9u));
    let d = length(plant.base.xyz) / reach;
    let variant = plant.info.y % FLORA_VARIANTS;
    let first = (entry * FLORA_VARIANTS + variant) * FLORA_TIERS;
    if d < FLORA_A0 {
        flora_stage(plant, first, 1.0);
    } else if d < FLORA_A1 {
        let t = (d - FLORA_A0) / (FLORA_A1 - FLORA_A0);
        flora_stage(plant, first, 1.0 - t);
        flora_stage(plant, first + 1u, -t);
    } else if d < FLORA_B0 {
        flora_stage(plant, first + 1u, 1.0);
    } else if d < FLORA_B1 {
        let t = (d - FLORA_B0) / (FLORA_B1 - FLORA_B0);
        flora_stage(plant, first + 1u, 1.0 - t);
        flora_stage(plant, first + 2u, -t);
    } else {
        flora_stage(plant, first + 2u, 1.0);
    }
    return true;
}

// Bucket bases and instance counts from the staged counts (one thread).
@compute @workgroup_size(1)
fn cs_flora_prefix() {
    var sum = 0u;
    let sorted_cap = arrayLength(&plants_out) - FLORA_SORTED_START;
    for (var b = 0u; b < FLORA_BUCKETS; b = b + 1u) {
        let word = FLORA_ARGS_WORD + b * 5u + 1u;
        let wanted = atomicLoad(&plant_args[word]);
        let count = min(wanted, sorted_cap - sum);
        if count < wanted {
            atomicAdd(&plant_args[FLORA_OVERFLOW_WORD], wanted - count);
        }
        atomicStore(&plant_args[word], count);
        atomicStore(&plant_args[FLORA_CURSOR_WORD + b], 0u);
        let base = FLORA_SORTED_START + sum;
        let h = FLORA_FAR_CAPACITY + b / 4u;
        switch b % 4u {
            case 0u: { plants_out[h].info.x = base; }
            case 1u: { plants_out[h].info.y = base; }
            case 2u: { plants_out[h].info.z = base; }
            default: { plants_out[h].info.w = base; }
        }
        sum += count;
    }
}

// Copy each staged plant into its bucket.
@compute @workgroup_size(64)
fn cs_flora_scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x + id.y * 65535u * 64u;
    if i >= min(atomicLoad(&plant_args[FLORA_STAGE_WORD]), FLORA_STAGE) {
        return;
    }
    let p = plants_out[FLORA_STAGE_START + i];
    let b = p.info.x >> 16u;
    let slot = atomicAdd(&plant_args[FLORA_CURSOR_WORD + b], 1u);
    if slot >= atomicLoad(&plant_args[FLORA_ARGS_WORD + b * 5u + 1u]) {
        return;
    }
    let h = plants_out[FLORA_FAR_CAPACITY + b / 4u].info;
    let base = select(select(select(h.w, h.z, b % 4u == 2u), h.y, b % 4u == 1u), h.x, b % 4u == 0u);
    plants_out[base + slot] = p;
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