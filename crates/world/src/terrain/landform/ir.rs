//! Validated recipe IR: a typed DAG [`Program`] in topological order
//! (M2 Shape design §2–§4).
//!
//! Recipe nodes have one of two types. *Octaves* (Fbm, RidgedFbm, Billow and
//! the stack modifiers Warp and SlopeDamped) describe a band-limited octave
//! stack that can still be modified; *Scalar* is a value with a gradient.
//! ErosionFilter turns a stack into a scalar, and every combinator coerces a
//! stack to a scalar. Lowering folds a stack and its modifiers into one
//! [`StackOp`]; all other nodes become scalar [`Op`]s whose operands are
//! indices of earlier ops. The last op is the output.
//!
//! Every relief, warp and gully octave sits on a dyadic ladder
//! (`surface::ladder`): wavelengths snap to `{1, 1.25, 1.5, 1.75}·2^k` and
//! halve per octave (lacunarity 2).
//!
//! Nesting rule (no pop): whatever an octave depends on is fully resolved
//! whenever that octave is visible at all. Validation guarantees it for the
//! warp (finest warp wavelength ≥ 2 × the stack's base wavelength) and the
//! erosion filter (base gully wavelength ≤ half the stack's base); the
//! evaluator steers damping and gullies only by octaves of at most half their
//! frequency. Consequently a texel's band-limited value differs from the full
//! function only by the faded octaves themselves (`bounds.rs`).
//!
//! [`Program::structure_identity`] hashes what generated code depends on (op
//! kinds, wiring, field ids, which modifiers exist, curve point counts);
//! [`Program::constants`] lists every number, which a constants buffer
//! carries, so parameter edits never rebuild a pipeline.
use super::LandformError;
use super::eval::{gully_mean, shape_mean};
use super::schema::{Anisotropy, Input, Node, RecipeField, RecipeFile};
use crate::terrain::surface::ladder::{LadderOctave, snap_wavelength};
use std::collections::HashMap;

/// Recipe schema version.
pub const RECIPE_SCHEMA: u32 = 1;
/// Octave limit of one stack.
pub const MAX_STACK_OCTAVES: u32 = 24;
/// Octave limit of a warp or an erosion filter.
pub const MAX_MODIFIER_OCTAVES: u32 = 8;
/// Shortest and longest wavelengths any octave may have.
pub const MIN_WAVELENGTH_M: f64 = 0.25;
pub const MAX_WAVELENGTH_M: f64 = 524_288.0;
/// Anisotropic octaves: `f·kappa·clamp ≤` this many 4th-coordinate cells.
pub const MAX_ANISOTROPY_CELLS: f64 = 256.0;
/// Warped octaves: `f·strength ≤` this many lattice cells of displacement.
pub const MAX_WARP_CELLS: f64 = 64.0;
/// Curve point limit.
pub const MAX_CURVE_POINTS: usize = 16;
/// Largest magnitude of authored scalar constants.
const MAX_CONSTANT: f64 = 1.0e6;
/// Relative slack for comparing snapped wavelengths.
const SLACK: f64 = 1.0e-9;

/// Per-octave shaping of a stack.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NoiseKind {
    /// `n`.
    Fbm,
    /// `max(1 − |n|, 0)^sharpness`.
    Ridged { sharpness: f64 },
    /// `|n|`.
    Billow,
}

impl NoiseKind {
    pub(crate) fn tag(self) -> u64 {
        match self {
            Self::Fbm => 1,
            Self::Ridged { .. } => 2,
            Self::Billow => 3,
        }
    }
}

/// Anisotropy of the first `octaves` octaves (design §3): the 3D lattice of
/// octave `k` runs at level `level_k + stretch_log2`, the 4th coordinate is
/// `q_w = f_k·kappa·clamp(δ, ±clamp_m)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnisotropyOp {
    pub stretch_log2: u32,
    pub kappa: f64,
    pub octaves: u32,
    pub clamp_m: f64,
}

/// Domain warp: displacement component `c` is `strength_m` times a
/// unit-normalised gain-½ fBm of `octaves` octaves from `base`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WarpOp {
    pub strength_m: f64,
    pub base: LadderOctave,
    pub octaves: u32,
    pub salt: u32,
}

/// Gradient-aligned gully octaves (`eval.rs` documents the kernel).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ErosionOp {
    pub strength: f64,
    pub base: LadderOctave,
    pub octaves: u32,
    pub salt: u32,
    /// Measured mean of the gully kernel, subtracted per octave.
    pub mean: f64,
}

/// One octave stack with its modifiers.
#[derive(Debug, Clone, PartialEq)]
pub struct StackOp {
    pub kind: NoiseKind,
    pub base: LadderOctave,
    pub octaves: u32,
    pub gain: f64,
    pub salt: u32,
    pub anisotropy: Option<AnisotropyOp>,
    pub warp: Option<WarpOp>,
    /// SlopeDamped coefficient.
    pub damp: Option<f64>,
    pub erosion: Option<ErosionOp>,
    /// Measured mean of the per-octave shape for 3D and 4D (anisotropic)
    /// octaves; 0 for Fbm.
    pub mean3: f64,
    pub mean4: f64,
}

impl StackOp {
    /// Octave `k` (wavelength halves per octave).
    pub fn octave(&self, k: u32) -> LadderOctave {
        LadderOctave {
            ladder: self.base.ladder,
            level: self.base.level - k as i32,
        }
    }

    /// Normalised amplitude `gain^k / Σ gain^j` (amplitudes sum to 1).
    pub fn amplitude(&self, k: u32) -> f64 {
        let total: f64 = (0..self.octaves).map(|j| self.gain.powi(j as i32)).sum();
        self.gain.powi(k as i32) / total
    }

    pub fn is_anisotropic(&self, k: u32) -> bool {
        self.anisotropy.is_some_and(|a| k < a.octaves)
    }

    /// Highest spatial frequency of octave `k`, used for band limiting: an
    /// anisotropic octave varies at `f·√(stretch⁻² + kappa²)` across iso-δ
    /// (|∇δ| = 1).
    pub fn effective_frequency(&self, k: u32) -> f64 {
        let f = self.octave(k).frequency_per_m();
        match self.anisotropy {
            Some(a) if k < a.octaves => {
                let s = f64::from(1u32 << a.stretch_log2);
                f * (1.0 / (s * s) + a.kappa * a.kappa).sqrt()
            }
            _ => f,
        }
    }

    pub fn mean(&self, k: u32) -> f64 {
        if self.is_anisotropic(k) {
            self.mean4
        } else {
            self.mean3
        }
    }
}

/// Monotone cubic (Fritsch–Carlson) curve, flat outside `[x_0, x_n]`.
#[derive(Debug, Clone, PartialEq)]
pub struct CurveOp {
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
    /// Hermite tangents dy/dx at the points.
    pub tangents: Vec<f64>,
    /// Largest |dy/dx| (bounds).
    pub lipschitz: f64,
}

impl CurveOp {
    fn new(points: &[(f64, f64)]) -> Self {
        let xs: Vec<f64> = points.iter().map(|p| p.0).collect();
        let ys: Vec<f64> = points.iter().map(|p| p.1).collect();
        let n = xs.len();
        let secant: Vec<f64> = (0..n - 1)
            .map(|i| (ys[i + 1] - ys[i]) / (xs[i + 1] - xs[i]))
            .collect();
        let mut m = vec![0.0; n];
        m[0] = secant[0];
        m[n - 1] = secant[n - 2];
        for i in 1..n - 1 {
            m[i] = if secant[i - 1] * secant[i] > 0.0 {
                0.5 * (secant[i - 1] + secant[i])
            } else {
                0.0
            };
        }
        for i in 0..n - 1 {
            if secant[i] == 0.0 {
                m[i] = 0.0;
                m[i + 1] = 0.0;
                continue;
            }
            let a = m[i] / secant[i];
            let b = m[i + 1] / secant[i];
            let r = a * a + b * b;
            if r > 9.0 {
                let t = 3.0 / r.sqrt();
                m[i] = t * a * secant[i];
                m[i + 1] = t * b * secant[i];
            }
        }
        let mut curve = Self {
            xs,
            ys,
            tangents: m,
            lipschitz: 0.0,
        };
        // dy/dx is quadratic in t on each segment: check the ends and vertex.
        let mut lipschitz = 0.0f64;
        for i in 0..n - 1 {
            let h = curve.xs[i + 1] - curve.xs[i];
            for t in [0.0, 0.25, 0.5, 0.75, 1.0] {
                lipschitz = lipschitz.max(curve.evaluate(curve.xs[i] + t * h).1.abs());
            }
            let (a, b, c) = curve.slope_coefficients(i);
            if a != 0.0 {
                let t = (-b / (2.0 * a)).clamp(0.0, 1.0);
                lipschitz = lipschitz.max((a * t * t + b * t + c).abs());
            }
        }
        curve.lipschitz = lipschitz;
        curve
    }

    /// dy/dx on segment `i` as `a·t² + b·t + c`.
    fn slope_coefficients(&self, i: usize) -> (f64, f64, f64) {
        let h = self.xs[i + 1] - self.xs[i];
        let dy = (self.ys[i + 1] - self.ys[i]) / h;
        let (m0, m1) = (self.tangents[i], self.tangents[i + 1]);
        (
            -6.0 * dy + 3.0 * m0 + 3.0 * m1,
            6.0 * dy - 4.0 * m0 - 2.0 * m1,
            m0,
        )
    }

    /// Value and dy/dx at `x`.
    pub fn evaluate(&self, x: f64) -> (f64, f64) {
        let n = self.xs.len();
        if x <= self.xs[0] {
            return (self.ys[0], 0.0);
        }
        if x >= self.xs[n - 1] {
            return (self.ys[n - 1], 0.0);
        }
        let i = self
            .xs
            .partition_point(|&xi| xi <= x)
            .saturating_sub(1)
            .min(n - 2);
        let h = self.xs[i + 1] - self.xs[i];
        let t = (x - self.xs[i]) / h;
        let (t2, t3) = (t * t, t * t * t);
        let value = (2.0 * t3 - 3.0 * t2 + 1.0) * self.ys[i]
            + (t3 - 2.0 * t2 + t) * h * self.tangents[i]
            + (-2.0 * t3 + 3.0 * t2) * self.ys[i + 1]
            + (t3 - t2) * h * self.tangents[i + 1];
        let (a, b, c) = self.slope_coefficients(i);
        (value, a * t2 + b * t + c)
    }
}

/// One scalar op; operands index earlier ops.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    Stack(Box<StackOp>),
    Field(RecipeField),
    Const(f64),
    Add(usize, usize),
    Multiply(usize, usize),
    /// `a + (b − a)·t`.
    Mix(usize, usize, usize),
    Min(usize, usize),
    Max(usize, usize),
    SmoothMin(usize, usize, f64),
    Clamp(usize, f64, f64),
    Curve(usize, CurveOp),
    Scale(usize, f64),
}

/// Compile-time context of a recipe.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CompileOptions {
    /// Longest base wavelength a relief stack may have (set file).
    pub relief_band_edge_m: f64,
}

/// A validated landform recipe. Its output is in units of the landform
/// amplitude.
#[derive(Debug, Clone, PartialEq)]
pub struct Program {
    name: String,
    ops: Vec<Op>,
    labels: Vec<String>,
}

impl Program {
    /// Validate and lower `recipe`.
    pub fn compile(
        name: &str,
        recipe: &RecipeFile,
        options: &CompileOptions,
    ) -> Result<Self, LandformError> {
        if recipe.schema != RECIPE_SCHEMA {
            return Err(LandformError::Recipe(format!(
                "schema {} is not {RECIPE_SCHEMA}",
                recipe.schema
            )));
        }
        let mut index = HashMap::new();
        for (i, (node_name, _)) in recipe.graph.iter().enumerate() {
            if node_name.is_empty() || index.insert(node_name.as_str(), i).is_some() {
                return Err(LandformError::Recipe(format!(
                    "node name '{node_name}' is empty or repeated"
                )));
            }
        }
        if !index.contains_key(recipe.output.as_str()) {
            return Err(LandformError::Recipe(format!(
                "output '{}' is not a node",
                recipe.output
            )));
        }
        let mut lowerer = Lowerer {
            recipe,
            index,
            state: vec![State::Unvisited; recipe.graph.len()],
            coerced: HashMap::new(),
            options: *options,
            ops: Vec::new(),
            labels: Vec::new(),
        };
        // Dependencies are emitted first, so the output op is the last one.
        let out = lowerer.scalar_input(&Input::Ref(recipe.output.clone()), &recipe.output)?;
        debug_assert_eq!(out + 1, lowerer.ops.len());
        if let Some(unused) =
            (0..recipe.graph.len()).find(|&i| lowerer.state[i] == State::Unvisited)
        {
            // Lower it anyway so authoring errors surface first.
            lowerer.named(unused)?;
            return Err(LandformError::Node {
                node: recipe.graph[unused].0.clone(),
                message: "not used by the output".into(),
            });
        }
        Ok(Self {
            name: name.to_string(),
            ops: lowerer.ops,
            labels: lowerer.labels,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Ops in topological order; the last is the output.
    pub fn ops(&self) -> &[Op] {
        &self.ops
    }

    /// Recipe node label of each op (inline nodes as `parent.slot`).
    pub fn labels(&self) -> &[String] {
        &self.labels
    }

    /// Hash of everything generated code depends on.
    pub fn structure_identity(&self) -> u64 {
        let mut words = vec![0x4c46_5354_0000_0001u64, self.ops.len() as u64];
        for op in &self.ops {
            match op {
                Op::Stack(s) => words.extend([
                    1,
                    s.kind.tag(),
                    u64::from(s.anisotropy.is_some())
                        | u64::from(s.warp.is_some()) << 1
                        | u64::from(s.damp.is_some()) << 2
                        | u64::from(s.erosion.is_some()) << 3,
                ]),
                Op::Field(f) => words.extend([2, u64::from(f.id())]),
                Op::Const(_) => words.push(3),
                Op::Add(a, b) => words.extend([4, *a as u64, *b as u64]),
                Op::Multiply(a, b) => words.extend([5, *a as u64, *b as u64]),
                Op::Mix(a, b, t) => words.extend([6, *a as u64, *b as u64, *t as u64]),
                Op::Min(a, b) => words.extend([7, *a as u64, *b as u64]),
                Op::Max(a, b) => words.extend([8, *a as u64, *b as u64]),
                Op::SmoothMin(a, b, _) => words.extend([9, *a as u64, *b as u64]),
                Op::Clamp(x, _, _) => words.extend([10, *x as u64]),
                Op::Curve(x, c) => words.extend([11, *x as u64, c.xs.len() as u64]),
                Op::Scale(x, _) => words.extend([12, *x as u64]),
            }
        }
        fold_hash(words)
    }

    /// Every number of the program in op order (constants buffer content):
    /// Stack: base ladder, base level, octaves, gain, salt, sharpness (0 if
    /// not ridged), mean3, mean4, then if present anisotropy (stretch_log2,
    /// kappa, octaves, clamp_m), warp (strength_m, ladder, level, octaves,
    /// salt), damp, erosion (strength, ladder, level, octaves, salt, mean);
    /// Const: value; SmoothMin: k; Clamp: min, max; Curve: xs, ys, tangents;
    /// Scale: factor.
    pub fn constants(&self) -> Vec<f64> {
        let mut out = Vec::new();
        for op in &self.ops {
            match op {
                Op::Stack(s) => {
                    out.extend([
                        f64::from(s.base.ladder),
                        f64::from(s.base.level),
                        f64::from(s.octaves),
                        s.gain,
                        f64::from(s.salt),
                        match s.kind {
                            NoiseKind::Ridged { sharpness } => sharpness,
                            _ => 0.0,
                        },
                        s.mean3,
                        s.mean4,
                    ]);
                    if let Some(a) = s.anisotropy {
                        out.extend([
                            f64::from(a.stretch_log2),
                            a.kappa,
                            f64::from(a.octaves),
                            a.clamp_m,
                        ]);
                    }
                    if let Some(w) = s.warp {
                        out.extend([
                            w.strength_m,
                            f64::from(w.base.ladder),
                            f64::from(w.base.level),
                            f64::from(w.octaves),
                            f64::from(w.salt),
                        ]);
                    }
                    if let Some(d) = s.damp {
                        out.push(d);
                    }
                    if let Some(e) = s.erosion {
                        out.extend([
                            e.strength,
                            f64::from(e.base.ladder),
                            f64::from(e.base.level),
                            f64::from(e.octaves),
                            f64::from(e.salt),
                            e.mean,
                        ]);
                    }
                }
                Op::Const(v) | Op::SmoothMin(_, _, v) | Op::Scale(_, v) => out.push(*v),
                Op::Clamp(_, lo, hi) => out.extend([*lo, *hi]),
                Op::Curve(_, c) => {
                    out.extend(&c.xs);
                    out.extend(&c.ys);
                    out.extend(&c.tangents);
                }
                _ => {}
            }
        }
        out
    }

    /// Hash of [`Self::constants`].
    pub fn constants_identity(&self) -> u64 {
        fold_hash(
            [0x4c46_434f_0000_0001u64]
                .into_iter()
                .chain(self.constants().into_iter().map(f64::to_bits)),
        )
    }

    /// Complete identity (structure and constants).
    pub fn identity(&self) -> u64 {
        fold_hash([self.structure_identity(), self.constants_identity()])
    }
}

pub(crate) fn fold_hash(words: impl IntoIterator<Item = u64>) -> u64 {
    words
        .into_iter()
        .fold(0x4c41_4e44_464f_524d, |hash, word| splitmix(hash ^ word))
}

fn splitmix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

// --------------------------------------------------------------- lowering

#[derive(Debug, Clone, PartialEq)]
enum State {
    Unvisited,
    Visiting,
    Done(Value),
}

/// Lowered value of a node: a stack not yet emitted, or an emitted op.
#[derive(Debug, Clone, PartialEq)]
enum Value {
    Stack(Box<StackOp>),
    Scalar(usize),
}

struct Lowerer<'a> {
    recipe: &'a RecipeFile,
    index: HashMap<&'a str, usize>,
    state: Vec<State>,
    /// Emitted scalar op of each named node used as a scalar.
    coerced: HashMap<usize, usize>,
    options: CompileOptions,
    ops: Vec<Op>,
    labels: Vec<String>,
}

fn node_error(label: &str, message: impl Into<String>) -> LandformError {
    LandformError::Node {
        node: label.to_string(),
        message: message.into(),
    }
}

impl Lowerer<'_> {
    fn push(&mut self, op: Op, label: String) -> usize {
        self.ops.push(op);
        self.labels.push(label);
        self.ops.len() - 1
    }

    fn named(&mut self, i: usize) -> Result<Value, LandformError> {
        let (name, node) = &self.recipe.graph[i];
        match &self.state[i] {
            State::Done(value) => return Ok(value.clone()),
            State::Visiting => return Err(node_error(name, "is part of a cycle")),
            State::Unvisited => {}
        }
        self.state[i] = State::Visiting;
        let value = self.node(node, name)?;
        // A shared stack is emitted once, on first scalar use.
        self.state[i] = State::Done(value.clone());
        Ok(value)
    }

    fn lookup(&self, name: &str, label: &str) -> Result<usize, LandformError> {
        self.index
            .get(name)
            .copied()
            .ok_or_else(|| node_error(label, format!("refers to unknown node '{name}'")))
    }

    fn input(&mut self, input: &Input, label: &str) -> Result<Value, LandformError> {
        match input {
            Input::Ref(name) => {
                let i = self.lookup(name, label)?;
                self.named(i)
            }
            Input::Node(node) => self.node(node, label),
        }
    }

    /// Lower an input that must be a scalar (stacks are emitted). A named
    /// stack is emitted once and shared by every scalar use.
    fn scalar_input(&mut self, input: &Input, label: &str) -> Result<usize, LandformError> {
        match input {
            Input::Ref(name) => {
                let i = self.lookup(name, label)?;
                if let Some(&op) = self.coerced.get(&i) {
                    return Ok(op);
                }
                let value = self.named(i)?;
                let op = self.scalar(value, name)?;
                self.coerced.insert(i, op);
                Ok(op)
            }
            Input::Node(node) => {
                let value = self.node(node, label)?;
                self.scalar(value, label)
            }
        }
    }

    fn scalar(&mut self, value: Value, label: &str) -> Result<usize, LandformError> {
        match value {
            Value::Scalar(op) => Ok(op),
            Value::Stack(stack) => {
                let stack = self.finish_stack(*stack, label)?;
                Ok(self.push(Op::Stack(Box::new(stack)), label.to_string()))
            }
        }
    }

    fn stack_input(&mut self, input: &Input, label: &str) -> Result<StackOp, LandformError> {
        match self.input(input, label)? {
            Value::Stack(stack) => Ok(*stack),
            Value::Scalar(_) => Err(node_error(
                label,
                "input must be a noise stack (Fbm, RidgedFbm, Billow, Warp or SlopeDamped)",
            )),
        }
    }

    fn node(&mut self, node: &Node, label: &str) -> Result<Value, LandformError> {
        let sub = |slot: &str| format!("{label}.{slot}");
        Ok(match node {
            Node::Fbm {
                base_wavelength_m,
                min_wavelength_m,
                gain,
                salt,
                anisotropy,
            } => Value::Stack(Box::new(stack(
                label,
                NoiseKind::Fbm,
                *base_wavelength_m,
                *min_wavelength_m,
                *gain,
                *salt,
                anisotropy,
            )?)),
            Node::RidgedFbm {
                base_wavelength_m,
                min_wavelength_m,
                gain,
                sharpness,
                salt,
                anisotropy,
            } => {
                if !(0.5..=8.0).contains(sharpness) {
                    return Err(node_error(label, "sharpness must be in 0.5..=8"));
                }
                Value::Stack(Box::new(stack(
                    label,
                    NoiseKind::Ridged {
                        sharpness: *sharpness,
                    },
                    *base_wavelength_m,
                    *min_wavelength_m,
                    *gain,
                    *salt,
                    anisotropy,
                )?))
            }
            Node::Billow {
                base_wavelength_m,
                min_wavelength_m,
                gain,
                salt,
                anisotropy,
            } => Value::Stack(Box::new(stack(
                label,
                NoiseKind::Billow,
                *base_wavelength_m,
                *min_wavelength_m,
                *gain,
                *salt,
                anisotropy,
            )?)),
            Node::Warp {
                input,
                strength_m,
                base_wavelength_m,
                octaves,
                salt,
            } => {
                let mut stack = self.stack_input(input, &sub("input"))?;
                if stack.warp.is_some() {
                    return Err(node_error(label, "the stack is already warped"));
                }
                if !(0.0..=1.0e5).contains(strength_m) {
                    return Err(node_error(label, "strength_m must be in 0..=100000"));
                }
                if !(1..=MAX_MODIFIER_OCTAVES).contains(octaves) {
                    return Err(node_error(label, "octaves must be in 1..=8"));
                }
                stack.warp = Some(WarpOp {
                    strength_m: *strength_m,
                    base: snapped(label, *base_wavelength_m)?,
                    octaves: *octaves,
                    salt: *salt,
                });
                Value::Stack(Box::new(stack))
            }
            Node::SlopeDamped { input, damp } => {
                let mut stack = self.stack_input(input, &sub("input"))?;
                if stack.damp.is_some() {
                    return Err(node_error(label, "the stack is already damped"));
                }
                if !(0.0..=100.0).contains(damp) {
                    return Err(node_error(label, "damp must be in 0..=100"));
                }
                stack.damp = Some(*damp);
                Value::Stack(Box::new(stack))
            }
            Node::ErosionFilter {
                input,
                strength,
                base_wavelength_m,
                octaves,
                salt,
            } => {
                let mut stack = self.stack_input(input, &sub("input"))?;
                if !(0.0..=4.0).contains(strength) {
                    return Err(node_error(label, "strength must be in 0..=4"));
                }
                if !(1..=MAX_MODIFIER_OCTAVES).contains(octaves) {
                    return Err(node_error(label, "octaves must be in 1..=8"));
                }
                stack.erosion = Some(ErosionOp {
                    strength: *strength,
                    base: snapped(label, *base_wavelength_m)?,
                    octaves: *octaves,
                    salt: *salt,
                    mean: gully_mean(),
                });
                let stack = self.finish_stack(stack, label)?;
                Value::Scalar(self.push(Op::Stack(Box::new(stack)), label.to_string()))
            }
            Node::Field(field) => Value::Scalar(self.push(Op::Field(*field), label.to_string())),
            Node::Const(v) => {
                constant(label, *v)?;
                Value::Scalar(self.push(Op::Const(*v), label.to_string()))
            }
            Node::Add { a, b }
            | Node::Multiply { a, b }
            | Node::Min { a, b }
            | Node::Max { a, b } => {
                let a = self.scalar_input(a, &sub("a"))?;
                let b = self.scalar_input(b, &sub("b"))?;
                let op = match node {
                    Node::Add { .. } => Op::Add(a, b),
                    Node::Multiply { .. } => Op::Multiply(a, b),
                    Node::Min { .. } => Op::Min(a, b),
                    _ => Op::Max(a, b),
                };
                Value::Scalar(self.push(op, label.to_string()))
            }
            Node::Mix { a, b, t } => {
                let a = self.scalar_input(a, &sub("a"))?;
                let b = self.scalar_input(b, &sub("b"))?;
                let t = self.scalar_input(t, &sub("t"))?;
                Value::Scalar(self.push(Op::Mix(a, b, t), label.to_string()))
            }
            Node::SmoothMin { a, b, k } => {
                if !(k.is_finite() && *k > 0.0 && *k <= MAX_CONSTANT) {
                    return Err(node_error(label, "k must be positive and finite"));
                }
                let a = self.scalar_input(a, &sub("a"))?;
                let b = self.scalar_input(b, &sub("b"))?;
                Value::Scalar(self.push(Op::SmoothMin(a, b, *k), label.to_string()))
            }
            Node::Clamp { input, min, max } => {
                constant(label, *min)?;
                constant(label, *max)?;
                if min > max {
                    return Err(node_error(label, "min exceeds max"));
                }
                let x = self.scalar_input(input, &sub("input"))?;
                Value::Scalar(self.push(Op::Clamp(x, *min, *max), label.to_string()))
            }
            Node::Curve { input, points } => {
                if !(2..=MAX_CURVE_POINTS).contains(&points.len())
                    || points
                        .iter()
                        .any(|p| constant(label, p.0).is_err() || constant(label, p.1).is_err())
                    || points.windows(2).any(|w| w[1].0 <= w[0].0)
                {
                    return Err(node_error(
                        label,
                        "needs 2..=16 finite points with strictly increasing x",
                    ));
                }
                let x = self.scalar_input(input, &sub("input"))?;
                Value::Scalar(self.push(Op::Curve(x, CurveOp::new(points)), label.to_string()))
            }
            Node::Scale { input, factor } => {
                constant(label, *factor)?;
                let x = self.scalar_input(input, &sub("input"))?;
                Value::Scalar(self.push(Op::Scale(x, *factor), label.to_string()))
            }
        })
    }

    /// Cross-checks of a complete stack and its measured means.
    fn finish_stack(&self, mut s: StackOp, label: &str) -> Result<StackOp, LandformError> {
        let base_m = s.base.wavelength_m();
        let edge = self.options.relief_band_edge_m;
        if base_m > edge * (1.0 + SLACK) {
            return Err(node_error(
                label,
                format!(
                    "base wavelength {base_m} m exceeds the relief band edge {edge} m (Tier A owns it)"
                ),
            ));
        }
        let finest = s.octave(s.octaves - 1);
        if let Some(a) = s.anisotropy {
            for k in 0..a.octaves {
                let cells = s.octave(k).frequency_per_m() * a.kappa * a.clamp_m;
                if cells > MAX_ANISOTROPY_CELLS * (1.0 + SLACK) {
                    return Err(node_error(
                        label,
                        format!(
                            "anisotropic octave {k}: f·kappa·clamp = {cells:.1} exceeds {MAX_ANISOTROPY_CELLS}"
                        ),
                    ));
                }
            }
        }
        if let Some(w) = s.warp {
            let warp_finest = LadderOctave {
                ladder: w.base.ladder,
                level: w.base.level - (w.octaves as i32 - 1),
            };
            if warp_finest.wavelength_m() < 2.0 * base_m * (1.0 - SLACK) {
                return Err(node_error(
                    label,
                    "the finest warp wavelength must be at least twice the stack's base wavelength",
                ));
            }
            let mut finest_f = finest.frequency_per_m();
            if let Some(e) = s.erosion {
                finest_f = finest_f.max(
                    LadderOctave {
                        ladder: e.base.ladder,
                        level: e.base.level - (e.octaves as i32 - 1),
                    }
                    .frequency_per_m(),
                );
            }
            let cells = finest_f * w.strength_m;
            if cells > MAX_WARP_CELLS * (1.0 + SLACK) {
                return Err(node_error(
                    label,
                    format!("f·warp strength = {cells:.1} exceeds {MAX_WARP_CELLS}"),
                ));
            }
        }
        if let Some(e) = s.erosion {
            let e_finest = e.base.level - (e.octaves as i32 - 1);
            let e_finest = LadderOctave {
                ladder: e.base.ladder,
                level: e_finest,
            };
            if e.base.wavelength_m() > 0.5 * base_m * (1.0 + SLACK)
                || e_finest.wavelength_m() < MIN_WAVELENGTH_M
            {
                return Err(node_error(
                    label,
                    "gully wavelengths must be at most half the stack's base wavelength and at least 0.25 m",
                ));
            }
        }
        let four_d = s.anisotropy.is_some();
        s.mean3 = shape_mean(s.kind, false);
        s.mean4 = if four_d {
            shape_mean(s.kind, true)
        } else {
            0.0
        };
        Ok(s)
    }
}

fn constant(label: &str, v: f64) -> Result<(), LandformError> {
    if v.is_finite() && v.abs() <= MAX_CONSTANT {
        Ok(())
    } else {
        Err(node_error(
            label,
            format!("{v} is not a finite constant within ±1e6"),
        ))
    }
}

fn snapped(label: &str, wavelength_m: f64) -> Result<LadderOctave, LandformError> {
    if !(MIN_WAVELENGTH_M..=MAX_WAVELENGTH_M).contains(&wavelength_m) {
        return Err(node_error(
            label,
            format!("wavelength {wavelength_m} m is outside 0.25..=524288 m"),
        ));
    }
    Ok(snap_wavelength(wavelength_m))
}

fn stack(
    label: &str,
    kind: NoiseKind,
    base_wavelength_m: f64,
    min_wavelength_m: f64,
    gain: f64,
    salt: u32,
    anisotropy: &Option<Anisotropy>,
) -> Result<StackOp, LandformError> {
    let base = snapped(label, base_wavelength_m)?;
    if !(MIN_WAVELENGTH_M..=base_wavelength_m).contains(&min_wavelength_m) {
        return Err(node_error(label, "min_wavelength_m must be in 0.25..=base"));
    }
    if !(gain > 0.0 && gain < 1.0) {
        return Err(node_error(label, "gain must be in (0, 1)"));
    }
    let mut octaves = 0u32;
    while octaves <= MAX_STACK_OCTAVES
        && base.wavelength_m() / 2f64.powi(octaves as i32) >= min_wavelength_m * (1.0 - SLACK)
    {
        octaves += 1;
    }
    if !(1..=MAX_STACK_OCTAVES).contains(&octaves) {
        return Err(node_error(
            label,
            format!("{octaves} octaves after snapping; need 1..={MAX_STACK_OCTAVES}"),
        ));
    }
    let anisotropy = match anisotropy {
        None => None,
        Some(a) => {
            let kappa_log2 = a.kappa.log2();
            if !a.stretch.is_power_of_two()
                || a.stretch > 16
                || !(a.kappa.is_finite() && kappa_log2 == kappa_log2.round())
                || !(-4.0..=4.0).contains(&kappa_log2)
                || !(1..=octaves).contains(&a.octaves)
                || !(a.clamp_km.is_finite() && a.clamp_km > 0.0 && a.clamp_km <= 300.0)
            {
                return Err(node_error(
                    label,
                    "anisotropy needs stretch a power of two ≤ 16, kappa a power of two in 1/16..=16, 1..=octaves anisotropic octaves and clamp_km in (0, 300]",
                ));
            }
            if base.ladder != 0 {
                return Err(node_error(
                    label,
                    "an anisotropic stack needs a power-of-two base wavelength (f·kappa dyadic)",
                ));
            }
            Some(AnisotropyOp {
                stretch_log2: a.stretch.trailing_zeros(),
                kappa: a.kappa,
                octaves: a.octaves,
                clamp_m: 1000.0 * a.clamp_km,
            })
        }
    };
    Ok(StackOp {
        kind,
        base,
        octaves,
        gain,
        salt,
        anisotropy,
        warp: None,
        damp: None,
        erosion: None,
        mean3: 0.0,
        mean4: 0.0,
    })
}
