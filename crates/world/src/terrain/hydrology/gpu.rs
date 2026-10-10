//! PROTOTYPE (M3 Water): the hydrology packed into one u32 storage buffer for
//! the Tier B producer (`renderer/src/shaders/river_carve.wgsl`). Positions
//! and metres narrow to f32 here, once (no f64 on the GPU).
//!
//! Layout in words:
//! - header (16): vertex count, hash cells per face edge, hash start offset,
//!   hash segment offset, lake offset, lake cells per face edge (0 without
//!   lakes), vertex offset, enabled flag, then f32: radius, valley factor,
//!   valley min, valley max, floodplain slope, valley side slope, 2 unused;
//! - vertices (8 words each): position xyz, width, depth, incision, bed (f32),
//!   downstream index (u32, `u32::MAX` at a mouth);
//! - hash: `6·cells² + 1` row starts, then the segment (upstream vertex) list;
//! - lakes: `6·n²` water levels (f32, `f32::MIN` without a lake), dilated.
use super::Hydrology;

pub const HEADER_WORDS: usize = 16;
pub const VERTEX_WORDS: usize = 8;

/// Pack `hydrology` for the GPU.
pub fn pack(hydrology: &Hydrology) -> Vec<u32> {
    let graph = &hydrology.rivers;
    let hash = &hydrology.hash;
    let lakes = hydrology.fill.lake_level_map();
    let has_lakes = !hydrology.fill.lakes.is_empty();
    let vertex_offset = HEADER_WORDS;
    let start_offset = vertex_offset + VERTEX_WORDS * graph.vertices.len();
    let segment_offset = start_offset + hash.start.len();
    let lake_offset = segment_offset + hash.segments.len();
    let lake_words = if has_lakes { lakes.data().len() } else { 0 };
    let mut out = Vec::with_capacity(lake_offset + lake_words);
    let p = &hydrology.params;
    let f = |x: f64| (x as f32).to_bits();
    out.extend_from_slice(&[
        graph.vertices.len() as u32,
        hash.cells as u32,
        start_offset as u32,
        segment_offset as u32,
        lake_offset as u32,
        if has_lakes { lakes.n() as u32 } else { 0 },
        vertex_offset as u32,
        1,
        f(hydrology.radius_m),
        f(p.valley_factor),
        f(p.valley_min_m),
        f(p.valley_max_m),
        f(p.floodplain_slope),
        f(p.valley_side_slope),
        0,
        0,
    ]);
    for v in &graph.vertices {
        let pos = v.pos.as_vec3();
        out.extend_from_slice(&[
            pos.x.to_bits(),
            pos.y.to_bits(),
            pos.z.to_bits(),
            f(v.width_m),
            f(v.depth_m),
            f(v.incision_m),
            f(v.bed_m),
            v.downstream,
        ]);
    }
    out.extend_from_slice(&hash.start);
    out.extend_from_slice(&hash.segments);
    if has_lakes {
        out.extend(lakes.data().iter().map(|l| l.to_bits()));
    }
    out
}

/// A disabled buffer (no carving, no lakes) of the header size.
pub fn disabled() -> Vec<u32> {
    vec![0; HEADER_WORDS]
}
