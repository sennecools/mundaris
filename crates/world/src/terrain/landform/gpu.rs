//! Packed `u32` form of a landform set for the GPU producer's interpreter
//! (`renderer/src/shaders/landform_eval.wgsl`; M2 Shape).
//!
//! The producer interprets the recipe programs per sample. The weight rules
//! are not in this block: the Tier A bake evaluates them per texel into its
//! weight run (`tier_a::landform_rule_fields`), which the producer samples.
//!
//! Layout (word offsets relative to the block start):
//!
//! | word | content |
//! |---|---|
//! | 0 | landform count `N` (0: no landforms) |
//! | 1 | bit mask of the fields recipe `Field` ops read (`RecipeField::id` bits), plus bits 0–3 (shape overlay) |
//! | 2, 3 | zero |
//! | 4 + 4i .. | landform `i`: amplitude (f32 bits), seed, program offset, op count |
//!
//! A program is its ops in order; op `k` writes value register `k` (at most
//! [`MAX_GPU_OPS`]). Each op is a header word `opcode | a << 8 | b << 16 |
//! c << 24` (`a`, `b`, `c` input registers or a field id) followed by its
//! constant words (f32 bits unless noted):
//!
//! | opcode | op | constants |
//! |---|---|---|
//! | 1 | stack | [`STACK_WORDS`] words, see [`pack_stack`] |
//! | 2 | field `a` | — |
//! | 3 | const | value |
//! | 4 | add `a b` | — |
//! | 5 | multiply `a b` | — |
//! | 6 | mix `a b c` | — |
//! | 7 | min `a b` | — |
//! | 8 | max `a b` | — |
//! | 9 | smooth min `a b` | k |
//! | 10 | clamp `a` | lo, hi |
//! | 11 | curve `a` | n (u32), xs[n], ys[n], tangents[n] |
//! | 12 | scale `a` | factor |
use super::LandformError;
use super::eval::LandformParams;
use super::ir::{NoiseKind, Op, StackOp};
use super::set::LandformSet;

/// Value registers of the GPU interpreter.
pub const MAX_GPU_OPS: usize = 16;
/// Constant words of a stack op.
pub const STACK_WORDS: usize = 25;
/// Header words of the block before the landform table.
pub const HEADER_WORDS: usize = 4;

pub mod opcode {
    pub const STACK: u32 = 1;
    pub const FIELD: u32 = 2;
    pub const CONST: u32 = 3;
    pub const ADD: u32 = 4;
    pub const MULTIPLY: u32 = 5;
    pub const MIX: u32 = 6;
    pub const MIN: u32 = 7;
    pub const MAX: u32 = 8;
    pub const SMOOTH_MIN: u32 = 9;
    pub const CLAMP: u32 = 10;
    pub const CURVE: u32 = 11;
    pub const SCALE: u32 = 12;
}

fn f(v: f64) -> u32 {
    (v as f32).to_bits()
}

fn header(code: u32, inputs: &[usize]) -> u32 {
    inputs
        .iter()
        .enumerate()
        .fold(code, |w, (k, i)| w | (*i as u32) << (8 * (k + 1)))
}

/// Stack constants: kind (0 fBm, 1 ridged, 2 billow) | flags << 8 (bit 0
/// anisotropy, 1 warp, 2 damp, 3 erosion); base ladder, base level (i32),
/// octaves, gain, salt, sharpness, mean3, mean4; anisotropy stretch_log2,
/// kappa, octaves, clamp_m; warp strength_m, ladder, level (i32), octaves,
/// salt; damp; erosion strength, ladder, level (i32), octaves, salt, mean.
/// Absent modifiers are zero.
pub fn pack_stack(s: &StackOp) -> [u32; STACK_WORDS] {
    let (kind, sharpness) = match s.kind {
        NoiseKind::Fbm => (0, 0.0),
        NoiseKind::Ridged { sharpness } => (1, sharpness),
        NoiseKind::Billow => (2, 0.0),
    };
    let flags = u32::from(s.anisotropy.is_some())
        | u32::from(s.warp.is_some()) << 1
        | u32::from(s.damp.is_some()) << 2
        | u32::from(s.erosion.is_some()) << 3;
    let mut w = [0u32; STACK_WORDS];
    w[0] = kind | flags << 8;
    w[1] = u32::from(s.base.ladder);
    w[2] = s.base.level as u32;
    w[3] = s.octaves;
    w[4] = f(s.gain);
    w[5] = s.salt;
    w[6] = f(sharpness);
    w[7] = f(s.mean3);
    w[8] = f(s.mean4);
    if let Some(a) = s.anisotropy {
        w[9] = a.stretch_log2;
        w[10] = f(a.kappa);
        w[11] = a.octaves;
        w[12] = f(a.clamp_m);
    }
    if let Some(warp) = s.warp {
        w[13] = f(warp.strength_m);
        w[14] = u32::from(warp.base.ladder);
        w[15] = warp.base.level as u32;
        w[16] = warp.octaves;
        w[17] = warp.salt;
    }
    if let Some(d) = s.damp {
        w[18] = f(d);
    }
    if let Some(e) = s.erosion {
        w[19] = f(e.strength);
        w[20] = u32::from(e.base.ladder);
        w[21] = e.base.level as u32;
        w[22] = e.octaves;
        w[23] = e.salt;
        w[24] = f(e.mean);
    }
    w
}

/// Pack `set` with the body's `params` (one per landform).
pub fn pack_set(set: &LandformSet, params: &[LandformParams]) -> Result<Vec<u32>, LandformError> {
    let bad = |m: String| Err(LandformError::Set(m));
    let n = set.landforms().len();
    if params.len() != n {
        return bad("one parameter set per landform".into());
    }
    let mut words = vec![0u32; HEADER_WORDS + 4 * n];
    words[0] = n as u32;
    words[1] = 0xf;
    for (i, (landform, p)) in set.landforms().iter().zip(params).enumerate() {
        let ops = landform.program.ops();
        if ops.len() > MAX_GPU_OPS {
            return bad(format!(
                "landform '{}' has {} ops; the GPU interpreter holds {MAX_GPU_OPS}",
                landform.name,
                ops.len()
            ));
        }
        let entry = HEADER_WORDS + 4 * i;
        words[entry] = f(p.amplitude_m);
        words[entry + 1] = p.seed;
        words[entry + 2] = words.len() as u32;
        words[entry + 3] = ops.len() as u32;
        for op in ops {
            match op {
                Op::Stack(s) => {
                    words.push(opcode::STACK);
                    words.extend_from_slice(&pack_stack(s));
                }
                Op::Field(field) => {
                    words[1] |= 1 << field.id();
                    words.push(header(opcode::FIELD, &[field.id() as usize]));
                }
                Op::Const(v) => words.extend([opcode::CONST, f(*v)]),
                Op::Add(a, b) => words.push(header(opcode::ADD, &[*a, *b])),
                Op::Multiply(a, b) => words.push(header(opcode::MULTIPLY, &[*a, *b])),
                Op::Mix(a, b, t) => words.push(header(opcode::MIX, &[*a, *b, *t])),
                Op::Min(a, b) => words.push(header(opcode::MIN, &[*a, *b])),
                Op::Max(a, b) => words.push(header(opcode::MAX, &[*a, *b])),
                Op::SmoothMin(a, b, k) => {
                    words.extend([header(opcode::SMOOTH_MIN, &[*a, *b]), f(*k)]);
                }
                Op::Clamp(x, lo, hi) => {
                    words.extend([header(opcode::CLAMP, &[*x]), f(*lo), f(*hi)])
                }
                Op::Curve(x, c) => {
                    words.extend([header(opcode::CURVE, &[*x]), c.xs.len() as u32]);
                    words.extend(c.xs.iter().map(|v| f(*v)));
                    words.extend(c.ys.iter().map(|v| f(*v)));
                    words.extend(c.tangents.iter().map(|v| f(*v)));
                }
                Op::Scale(x, s) => words.extend([header(opcode::SCALE, &[*x]), f(*s)]),
            }
        }
    }
    Ok(words)
}
