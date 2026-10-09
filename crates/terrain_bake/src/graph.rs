//! Periodic node graph for library base terrain (`recipe.graph`).
//!
//! A graph is an ordered list of nodes; every input must name an earlier node,
//! so the list is its own topological order. Each node yields a dual number
//! (value and gradient with respect to the tile coordinates `(u, v)` in [0, 1)),
//! so slope masks are analytic. All noise is periodic over the tile, so any
//! graph tiles. Node names and semantics follow the planned Phase T node set
//! (`docs/TERRAIN_GRAPH_PRODUCER.md` §3) so the runtime graph can adopt them.
//!
//! Outputs: `height` (metres), `hardness` (clamped to [0, 0.95]) and an optional
//! `uplift` (clamped to [0, 1]) for fluvial stages.

use crate::noise::{layer_dual, layer_seed, pcg};
use crate::recipe::{NoiseKind, NoiseLayer};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const MAX_NODES: usize = 128;
/// Graph seed roles start above the legacy layer roles.
const GRAPH_ROLE_BASE: u32 = 64;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct GraphRecipe {
    pub nodes: Vec<NodeRecipe>,
    /// Node giving physical height in metres.
    pub height: String,
    /// Node giving rock hardness (clamped to [0, 0.95]).
    pub hardness: String,
    /// Node giving the fluvial uplift weight (clamped to [0, 1]); when absent,
    /// fluvial stages use the normalized height.
    #[serde(default)]
    pub uplift: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NodeRecipe {
    pub id: String,
    pub op: Op,
}

/// Coordinate warp of a noise node: p + amplitude · (x, y), in tile units.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WarpInput {
    pub x: String,
    pub y: String,
    pub amplitude: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NoiseNode {
    /// Whole cycles per tile.
    pub frequency: u32,
    pub octaves: u32,
    #[serde(default = "two")]
    pub lacunarity: u32,
    pub gain: f64,
    /// Ridge exponent (ridged only).
    #[serde(default = "two_f64")]
    pub sharpness: f64,
    #[serde(default)]
    pub rotate_octaves: bool,
    #[serde(default)]
    pub slope_damping: f64,
    #[serde(default)]
    pub domain: Option<[[i32; 2]; 2]>,
    /// Decorrelates nodes; two nodes with equal parameters and seed are identical.
    pub seed: u32,
    #[serde(default)]
    pub warp: Option<WarpInput>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CellularMode {
    /// Distance to the nearest feature point (cells, about [0, 1]).
    F1,
    /// Distance to the second nearest.
    F2,
    /// Second minus first: ridges along cell borders, 0 on them.
    F2MinusF1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CellularNode {
    pub frequency: u32,
    pub mode: CellularMode,
    /// Feature point jitter within its cell, 0..=1.
    #[serde(default = "one")]
    pub jitter: f64,
    #[serde(default)]
    pub domain: Option<[[i32; 2]; 2]>,
    pub seed: u32,
    #[serde(default)]
    pub warp: Option<WarpInput>,
}

fn two() -> u32 {
    2
}
fn two_f64() -> f64 {
    2.0
}
fn one() -> f64 {
    1.0
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Op {
    Constant {
        value: f64,
    },
    Fbm(NoiseNode),
    Ridged(NoiseNode),
    Billow(NoiseNode),
    Cellular(CellularNode),
    /// Impact craters added to `input` (see `craters.rs`).
    CraterField(crate::craters::CraterFieldNode),
    /// Sum of any number of inputs.
    Add {
        inputs: Vec<String>,
    },
    Sub {
        a: String,
        b: String,
    },
    Mul {
        a: String,
        b: String,
    },
    Min {
        a: String,
        b: String,
    },
    Max {
        a: String,
        b: String,
    },
    /// Polynomial smooth minimum with blend width `k` (input units).
    SmoothMin {
        a: String,
        b: String,
        k: f64,
    },
    SmoothMax {
        a: String,
        b: String,
        k: f64,
    },
    /// input · factor + offset.
    Scale {
        input: String,
        factor: f64,
        #[serde(default)]
        offset: f64,
    },
    /// Linear map of `from` onto `to`, optionally clamped to `to`.
    Remap {
        input: String,
        from: [f64; 2],
        to: [f64; 2],
        #[serde(default)]
        clamp: bool,
    },
    /// Monotone cubic (Fritsch–Carlson) through `points` (x strictly
    /// increasing); clamped to the end values outside.
    Curve {
        input: String,
        points: Vec<[f64; 2]>,
    },
    /// `steps` terraces over `range`: within each step the fraction f is shaped
    /// to f^exponent (flat treads, steep risers for exponent > 1).
    Terrace {
        input: String,
        steps: u32,
        exponent: f64,
        range: [f64; 2],
    },
    Abs {
        input: String,
    },
    /// max(input, 0)^exponent.
    Power {
        input: String,
        exponent: f64,
    },
    /// a + (b − a) · clamp(mask, 0, 1).
    Blend {
        a: String,
        b: String,
        mask: String,
    },
    /// smoothstep(from, to, input).
    HeightMask {
        input: String,
        from: f64,
        to: f64,
    },
    /// smoothstep(from, to, slope) with slope = |∇input| as rise over run
    /// (input in metres). The mask's own gradient is taken as zero.
    SlopeMask {
        input: String,
        from: f64,
        to: f64,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Dual {
    v: f64,
    du: f64,
    dv: f64,
}

impl Dual {
    fn constant(v: f64) -> Self {
        Self {
            v,
            du: 0.0,
            dv: 0.0,
        }
    }
    fn scale(self, k: f64) -> Self {
        Self {
            v: self.v * k,
            du: self.du * k,
            dv: self.dv * k,
        }
    }
    fn plus(self, o: Self) -> Self {
        Self {
            v: self.v + o.v,
            du: self.du + o.du,
            dv: self.dv + o.dv,
        }
    }
    /// f(self) given f(v) and f'(v).
    fn map(self, value: f64, derivative: f64) -> Self {
        Self {
            v: value,
            du: self.du * derivative,
            dv: self.dv * derivative,
        }
    }
}

fn smoothstep(from: f64, to: f64, x: f64) -> (f64, f64) {
    let width = to - from;
    let t = ((x - from) / width).clamp(0.0, 1.0);
    let value = t * t * (3.0 - 2.0 * t);
    let derivative = if t > 0.0 && t < 1.0 {
        6.0 * t * (1.0 - t) / width
    } else {
        0.0
    };
    (value, derivative)
}

#[derive(Debug, Clone)]
struct Warp {
    x: usize,
    y: usize,
    amplitude: f64,
}

#[derive(Debug, Clone)]
struct Cellular {
    frequency: u32,
    mode: CellularMode,
    jitter: f64,
    domain: [[f64; 2]; 2],
    role: u32,
}

#[derive(Debug, Clone)]
enum Compiled {
    Constant(f64),
    Noise {
        layer: NoiseLayer,
        role: u32,
        warp: Option<Warp>,
    },
    Cellular {
        cell: Cellular,
        warp: Option<Warp>,
    },
    Add(Vec<usize>),
    Sub(usize, usize),
    Mul(usize, usize),
    Min(usize, usize),
    Max(usize, usize),
    SmoothMin(usize, usize, f64),
    SmoothMax(usize, usize, f64),
    Scale(usize, f64, f64),
    Remap(usize, [f64; 2], [f64; 2], bool),
    Curve(usize, Vec<[f64; 3]>),
    Terrace(usize, u32, f64, [f64; 2]),
    Abs(usize),
    Power(usize, f64),
    Blend(usize, usize, usize),
    HeightMask(usize, f64, f64),
    SlopeMask(usize, f64, f64),
    CraterField(usize, Box<crate::craters::CraterField>),
}

/// A validated graph ready for evaluation.
#[derive(Debug, Clone)]
pub struct Program {
    nodes: Vec<Compiled>,
    height: usize,
    hardness: usize,
    uplift: Option<usize>,
    footprint_m: f64,
    recipe_seed: u64,
}

/// Base fields evaluated at pixel centres of an `n²` grid.
pub struct BaseFields {
    pub height_m: Vec<f64>,
    pub hardness: Vec<f32>,
    pub uplift: Option<Vec<f64>>,
}

fn valid_domain(domain: Option<[[i32; 2]; 2]>) -> Result<[[f64; 2]; 2]> {
    let [[a, b], [c, d]] = domain.unwrap_or([[1, 0], [0, 1]]);
    ensure!(
        [a, b, c, d].iter().all(|v| v.abs() <= 16) && a * d - b * c != 0,
        "domain must be a non-singular integer matrix with entries in -16..=16"
    );
    Ok([[a, b], [c, d]].map(|row| row.map(f64::from)))
}

/// Monotone cubic Hermite tangents (Fritsch–Carlson) for `points`.
fn curve_tangents(points: &[[f64; 2]]) -> Vec<[f64; 3]> {
    let n = points.len();
    let secants: Vec<f64> = points
        .windows(2)
        .map(|w| (w[1][1] - w[0][1]) / (w[1][0] - w[0][0]))
        .collect();
    let mut tangents = vec![0.0; n];
    tangents[0] = secants[0];
    tangents[n - 1] = secants[n - 2];
    for i in 1..n - 1 {
        tangents[i] = if secants[i - 1] * secants[i] <= 0.0 {
            0.0
        } else {
            0.5 * (secants[i - 1] + secants[i])
        };
    }
    for i in 0..n - 1 {
        if secants[i] == 0.0 {
            tangents[i] = 0.0;
            tangents[i + 1] = 0.0;
            continue;
        }
        let a = tangents[i] / secants[i];
        let b = tangents[i + 1] / secants[i];
        let length = a.hypot(b);
        if length > 3.0 {
            let t = 3.0 / length;
            tangents[i] = t * a * secants[i];
            tangents[i + 1] = t * b * secants[i];
        }
    }
    points
        .iter()
        .zip(tangents)
        .map(|(p, t)| [p[0], p[1], t])
        .collect()
}

fn curve_eval(points: &[[f64; 3]], x: f64) -> (f64, f64) {
    let last = points.len() - 1;
    if x <= points[0][0] {
        return (points[0][1], 0.0);
    }
    if x >= points[last][0] {
        return (points[last][1], 0.0);
    }
    let i = points.partition_point(|p| p[0] <= x) - 1;
    let ([x0, y0, m0], [x1, y1, m1]) = (points[i], points[i + 1]);
    let h = x1 - x0;
    let t = (x - x0) / h;
    let (t2, t3) = (t * t, t * t * t);
    let value = (2.0 * t3 - 3.0 * t2 + 1.0) * y0
        + (t3 - 2.0 * t2 + t) * h * m0
        + (-2.0 * t3 + 3.0 * t2) * y1
        + (t3 - t2) * h * m1;
    let derivative = ((6.0 * t2 - 6.0 * t) * y0
        + (3.0 * t2 - 4.0 * t + 1.0) * h * m0
        + (-6.0 * t2 + 6.0 * t) * y1
        + (3.0 * t2 - 2.0 * t) * h * m1)
        / h;
    (value, derivative)
}

/// Periodic Worley distances: (f1, f2) in cell units and their gradients with
/// respect to the lattice coordinates.
fn cellular_lattice(
    x: f64,
    y: f64,
    frequency: u32,
    jitter: f64,
    seed: u32,
) -> [(f64, f64, f64); 2] {
    let (cx, cy) = (x.floor() as i64, y.floor() as i64);
    let f = i64::from(frequency);
    let mut best = [(f64::INFINITY, 0.0, 0.0); 2];
    for j in -1..=1 {
        for i in -1..=1 {
            let (ux, uy) = (cx + i, cy + j);
            let (hx, hy) = (ux.rem_euclid(f) as u32, uy.rem_euclid(f) as u32);
            let h1 = pcg(hx ^ pcg(hy ^ pcg(seed)));
            let h2 = pcg(h1 ^ 0x68e3_1da4);
            let fx = ux as f64 + 0.5 + jitter * (f64::from(h1) / f64::from(u32::MAX) - 0.5);
            let fy = uy as f64 + 0.5 + jitter * (f64::from(h2) / f64::from(u32::MAX) - 0.5);
            let (dx, dy) = (x - fx, y - fy);
            let d = dx.hypot(dy);
            let (gx, gy) = if d > 1e-12 {
                (dx / d, dy / d)
            } else {
                (0.0, 0.0)
            };
            if d < best[0].0 {
                best[1] = best[0];
                best[0] = (d, gx, gy);
            } else if d < best[1].0 {
                best[1] = (d, gx, gy);
            }
        }
    }
    best
}

impl Program {
    pub fn compile(graph: &GraphRecipe, footprint_m: f64, recipe_seed: u64) -> Result<Self> {
        ensure!(
            !graph.nodes.is_empty() && graph.nodes.len() <= MAX_NODES,
            "graph needs 1..={MAX_NODES} nodes"
        );
        let mut index: HashMap<&str, usize> = HashMap::new();
        let mut nodes = Vec::with_capacity(graph.nodes.len());
        for (position, node) in graph.nodes.iter().enumerate() {
            let id = node.id.as_str();
            ensure!(
                !id.is_empty()
                    && id.len() <= 64
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "node id {id:?} must be 1..=64 of [a-z0-9_]"
            );
            let input = |name: &str| -> Result<usize> {
                index
                    .get(name)
                    .copied()
                    .with_context(|| format!("node {id}: input {name:?} is not an earlier node"))
            };
            let warp = |w: &Option<WarpInput>| -> Result<Option<Warp>> {
                w.as_ref()
                    .map(|w| {
                        ensure!(
                            w.amplitude.is_finite() && w.amplitude.abs() <= 1.0,
                            "node {id}: warp amplitude outside -1..=1"
                        );
                        Ok(Warp {
                            x: input(&w.x)?,
                            y: input(&w.y)?,
                            amplitude: w.amplitude,
                        })
                    })
                    .transpose()
            };
            let noise = |kind: NoiseKind, n: &NoiseNode| -> Result<Compiled> {
                let layer = NoiseLayer {
                    kind,
                    frequency: n.frequency,
                    octaves: n.octaves,
                    lacunarity: n.lacunarity,
                    gain: n.gain,
                    weight: 1.0,
                    sharpness: n.sharpness,
                    rotate_octaves: n.rotate_octaves,
                    slope_damping: n.slope_damping,
                    domain: n.domain,
                };
                crate::recipe::validate_layer(&layer).with_context(|| format!("node {id}"))?;
                Ok(Compiled::Noise {
                    layer,
                    role: GRAPH_ROLE_BASE + n.seed,
                    warp: warp(&n.warp)?,
                })
            };
            let finite = |value: f64, name: &str| -> Result<()> {
                ensure!(value.is_finite(), "node {id}: {name} is not finite");
                Ok(())
            };
            let compiled = match &node.op {
                Op::Constant { value } => {
                    finite(*value, "value")?;
                    Compiled::Constant(*value)
                }
                Op::Fbm(n) => noise(NoiseKind::Fbm, n)?,
                Op::Ridged(n) => noise(NoiseKind::Ridged, n)?,
                Op::Billow(n) => noise(NoiseKind::Billow, n)?,
                Op::Cellular(c) => {
                    ensure!(
                        (1..=4096).contains(&c.frequency),
                        "node {id}: frequency outside 1..=4096"
                    );
                    ensure!(
                        (0.0..=1.0).contains(&c.jitter),
                        "node {id}: jitter outside 0..=1"
                    );
                    Compiled::Cellular {
                        cell: Cellular {
                            frequency: c.frequency,
                            mode: c.mode,
                            jitter: c.jitter,
                            domain: valid_domain(c.domain).with_context(|| format!("node {id}"))?,
                            role: GRAPH_ROLE_BASE + c.seed,
                        },
                        warp: warp(&c.warp)?,
                    }
                }
                Op::Add { inputs } => {
                    ensure!(!inputs.is_empty(), "node {id}: add needs inputs");
                    Compiled::Add(inputs.iter().map(|i| input(i)).collect::<Result<_>>()?)
                }
                Op::Sub { a, b } => Compiled::Sub(input(a)?, input(b)?),
                Op::Mul { a, b } => Compiled::Mul(input(a)?, input(b)?),
                Op::Min { a, b } => Compiled::Min(input(a)?, input(b)?),
                Op::Max { a, b } => Compiled::Max(input(a)?, input(b)?),
                Op::SmoothMin { a, b, k } | Op::SmoothMax { a, b, k } => {
                    ensure!(k.is_finite() && *k > 0.0, "node {id}: k must be positive");
                    if matches!(node.op, Op::SmoothMin { .. }) {
                        Compiled::SmoothMin(input(a)?, input(b)?, *k)
                    } else {
                        Compiled::SmoothMax(input(a)?, input(b)?, *k)
                    }
                }
                Op::Scale {
                    input: i,
                    factor,
                    offset,
                } => {
                    finite(*factor, "factor")?;
                    finite(*offset, "offset")?;
                    Compiled::Scale(input(i)?, *factor, *offset)
                }
                Op::Remap {
                    input: i,
                    from,
                    to,
                    clamp,
                } => {
                    ensure!(
                        from.iter().chain(to).all(|v| v.is_finite()) && from[0] != from[1],
                        "node {id}: remap needs finite ranges and from[0] != from[1]"
                    );
                    Compiled::Remap(input(i)?, *from, *to, *clamp)
                }
                Op::Curve { input: i, points } => {
                    ensure!(
                        (2..=32).contains(&points.len())
                            && points.iter().flatten().all(|v| v.is_finite())
                            && points.windows(2).all(|w| w[1][0] > w[0][0]),
                        "node {id}: curve needs 2..=32 finite points with increasing x"
                    );
                    Compiled::Curve(input(i)?, curve_tangents(points))
                }
                Op::Terrace {
                    input: i,
                    steps,
                    exponent,
                    range,
                } => {
                    ensure!(
                        (1..=256).contains(steps)
                            && exponent.is_finite()
                            && (1.0..=32.0).contains(exponent)
                            && range.iter().all(|v| v.is_finite())
                            && range[1] > range[0],
                        "node {id}: terrace needs steps 1..=256, exponent 1..=32, range[1] > range[0]"
                    );
                    Compiled::Terrace(input(i)?, *steps, *exponent, *range)
                }
                Op::Abs { input: i } => Compiled::Abs(input(i)?),
                Op::Power { input: i, exponent } => {
                    ensure!(
                        exponent.is_finite() && (0.1..=16.0).contains(exponent),
                        "node {id}: exponent outside 0.1..=16"
                    );
                    Compiled::Power(input(i)?, *exponent)
                }
                Op::Blend { a, b, mask } => Compiled::Blend(input(a)?, input(b)?, input(mask)?),
                Op::CraterField(c) => Compiled::CraterField(
                    input(&c.input)?,
                    Box::new(
                        crate::craters::CraterField::new(c, footprint_m)
                            .with_context(|| format!("node {id}"))?,
                    ),
                ),
                Op::HeightMask { input: i, from, to } | Op::SlopeMask { input: i, from, to } => {
                    ensure!(
                        from.is_finite() && to.is_finite() && from != to,
                        "node {id}: mask needs finite from != to"
                    );
                    if matches!(node.op, Op::HeightMask { .. }) {
                        Compiled::HeightMask(input(i)?, *from, *to)
                    } else {
                        Compiled::SlopeMask(input(i)?, *from, *to)
                    }
                }
            };
            ensure!(
                index.insert(id, position).is_none(),
                "duplicate node id {id}"
            );
            nodes.push(compiled);
        }
        let output = |name: &str| -> Result<usize> {
            index
                .get(name)
                .copied()
                .with_context(|| format!("graph output {name:?} is not a node"))
        };
        Ok(Self {
            height: output(&graph.height)?,
            hardness: output(&graph.hardness)?,
            uplift: graph.uplift.as_deref().map(output).transpose()?,
            nodes,
            footprint_m,
            recipe_seed,
        })
    }

    /// Evaluate every node at tile coordinates `(u, v)` into `registers`.
    fn run(&self, u: f64, v: f64, registers: &mut Vec<Dual>) {
        registers.clear();
        for node in &self.nodes {
            let r = |i: usize| registers[i];
            let warped = |warp: &Option<Warp>| -> (f64, f64, [[f64; 2]; 2]) {
                match warp {
                    None => (u, v, [[1.0, 0.0], [0.0, 1.0]]),
                    Some(w) => {
                        let (x, y) = (r(w.x), r(w.y));
                        (
                            u + w.amplitude * x.v,
                            v + w.amplitude * y.v,
                            // d(p')/d(p): rows are (du', dv') over (u, v).
                            [
                                [1.0 + w.amplitude * x.du, w.amplitude * x.dv],
                                [w.amplitude * y.du, 1.0 + w.amplitude * y.dv],
                            ],
                        )
                    }
                }
            };
            // Chain rule through the warp: ∇_p n = Jᵀ ∇_p' n.
            let through = |j: [[f64; 2]; 2], value: f64, gu: f64, gv: f64| Dual {
                v: value,
                du: gu * j[0][0] + gv * j[1][0],
                dv: gu * j[0][1] + gv * j[1][1],
            };
            let out = match node {
                Compiled::Constant(value) => Dual::constant(*value),
                Compiled::Noise { layer, role, warp } => {
                    let (pu, pv, j) = warped(warp);
                    let (value, gu, gv) = layer_dual(layer, pu, pv, self.recipe_seed, *role);
                    through(j, value, gu, gv)
                }
                Compiled::Cellular { cell, warp } => {
                    let (pu, pv, j) = warped(warp);
                    let [[d00, d01], [d10, d11]] = cell.domain;
                    let f = f64::from(cell.frequency);
                    let qu = (d00 * pu + d01 * pv).rem_euclid(1.0);
                    let qv = (d10 * pu + d11 * pv).rem_euclid(1.0);
                    let seed = layer_seed(self.recipe_seed, cell.role, 0);
                    let [n1, n2] =
                        cellular_lattice(qu * f, qv * f, cell.frequency, cell.jitter, seed);
                    let (value, gx, gy) = match cell.mode {
                        CellularMode::F1 => n1,
                        CellularMode::F2 => n2,
                        CellularMode::F2MinusF1 => (n2.0 - n1.0, n2.1 - n1.1, n2.2 - n1.2),
                    };
                    let (gu, gv) = (f * (d00 * gx + d10 * gy), f * (d01 * gx + d11 * gy));
                    through(j, value, gu, gv)
                }
                Compiled::Add(inputs) => inputs
                    .iter()
                    .fold(Dual::constant(0.0), |sum, &i| sum.plus(r(i))),
                Compiled::Sub(a, b) => r(*a).plus(r(*b).scale(-1.0)),
                Compiled::Mul(a, b) => {
                    let (a, b) = (r(*a), r(*b));
                    Dual {
                        v: a.v * b.v,
                        du: a.du * b.v + a.v * b.du,
                        dv: a.dv * b.v + a.v * b.dv,
                    }
                }
                Compiled::Min(a, b) => {
                    if r(*a).v <= r(*b).v {
                        r(*a)
                    } else {
                        r(*b)
                    }
                }
                Compiled::Max(a, b) => {
                    if r(*a).v >= r(*b).v {
                        r(*a)
                    } else {
                        r(*b)
                    }
                }
                Compiled::SmoothMin(a, b, k) | Compiled::SmoothMax(a, b, k) => {
                    // smax(a, b) = −smin(−a, −b). The polynomial smin's gradient
                    // is exactly mix(∇b, ∇a, h).
                    let sign = if matches!(node, Compiled::SmoothMin(..)) {
                        1.0
                    } else {
                        -1.0
                    };
                    let (a, b) = (r(*a).scale(sign), r(*b).scale(sign));
                    let h = (0.5 + 0.5 * (b.v - a.v) / k).clamp(0.0, 1.0);
                    Dual {
                        v: b.v + h * (a.v - b.v) - k * h * (1.0 - h),
                        du: b.du + h * (a.du - b.du),
                        dv: b.dv + h * (a.dv - b.dv),
                    }
                    .scale(sign)
                }
                Compiled::Scale(i, factor, offset) => {
                    let x = r(*i).scale(*factor);
                    Dual {
                        v: x.v + offset,
                        ..x
                    }
                }
                Compiled::Remap(i, from, to, clamp) => {
                    let x = r(*i);
                    let k = (to[1] - to[0]) / (from[1] - from[0]);
                    let value = to[0] + (x.v - from[0]) * k;
                    let (lo, hi) = (to[0].min(to[1]), to[0].max(to[1]));
                    if *clamp && (value < lo || value > hi) {
                        Dual::constant(value.clamp(lo, hi))
                    } else {
                        x.map(value, k)
                    }
                }
                Compiled::Curve(i, points) => {
                    let x = r(*i);
                    let (value, derivative) = curve_eval(points, x.v);
                    x.map(value, derivative)
                }
                Compiled::Terrace(i, steps, exponent, range) => {
                    let x = r(*i);
                    let span = range[1] - range[0];
                    let s = f64::from(*steps);
                    let t = ((x.v - range[0]) / span).clamp(0.0, 1.0) * s;
                    let step = t.floor().min(s - 1.0);
                    let fraction = t - step;
                    let shaped = fraction.powf(*exponent);
                    let value = range[0] + (step + shaped) / s * span;
                    let derivative = if x.v > range[0] && x.v < range[1] {
                        exponent * fraction.powf(exponent - 1.0)
                    } else {
                        0.0
                    };
                    x.map(value, derivative)
                }
                Compiled::Abs(i) => {
                    let x = r(*i);
                    x.map(x.v.abs(), x.v.signum())
                }
                Compiled::Power(i, exponent) => {
                    let x = r(*i);
                    if x.v <= 0.0 {
                        Dual::constant(0.0)
                    } else {
                        x.map(x.v.powf(*exponent), exponent * x.v.powf(exponent - 1.0))
                    }
                }
                Compiled::Blend(a, b, mask) => {
                    let (a, b, m) = (r(*a), r(*b), r(*mask));
                    let (t, dt) = if m.v <= 0.0 {
                        (0.0, 0.0)
                    } else if m.v >= 1.0 {
                        (1.0, 0.0)
                    } else {
                        (m.v, 1.0)
                    };
                    Dual {
                        v: a.v + (b.v - a.v) * t,
                        du: a.du + (b.du - a.du) * t + (b.v - a.v) * dt * m.du,
                        dv: a.dv + (b.dv - a.dv) * t + (b.v - a.v) * dt * m.dv,
                    }
                }
                Compiled::HeightMask(i, from, to) => {
                    let x = r(*i);
                    let (value, derivative) = smoothstep(*from, *to, x.v);
                    x.map(value, derivative)
                }
                Compiled::CraterField(i, field) => {
                    let (value, gu, gv) = field.sample(u.rem_euclid(1.0), v.rem_euclid(1.0));
                    r(*i).plus(Dual {
                        v: value,
                        du: gu,
                        dv: gv,
                    })
                }
                Compiled::SlopeMask(i, from, to) => {
                    let x = r(*i);
                    let slope = x.du.hypot(x.dv) / self.footprint_m;
                    Dual::constant(smoothstep(*from, *to, slope).0)
                }
            };
            registers.push(out);
        }
    }

    /// Height (m), hardness and optional uplift at `(u, v)`.
    pub fn sample(&self, u: f64, v: f64) -> (f64, f64, Option<f64>) {
        let mut registers = Vec::with_capacity(self.nodes.len());
        self.run(u, v, &mut registers);
        self.outputs(&registers)
    }

    fn outputs(&self, registers: &[Dual]) -> (f64, f64, Option<f64>) {
        (
            registers[self.height].v,
            registers[self.hardness].v.clamp(0.0, 0.95),
            self.uplift.map(|i| registers[i].v.clamp(0.0, 1.0)),
        )
    }

    /// Height gradient (metres per tile unit) at `(u, v)`; for tests.
    pub fn height_gradient(&self, u: f64, v: f64) -> (f64, f64) {
        let mut registers = Vec::with_capacity(self.nodes.len());
        self.run(u, v, &mut registers);
        let h = registers[self.height];
        (h.du, h.dv)
    }

    /// Evaluate the outputs at pixel centres of an `n²` grid, in parallel rows.
    pub fn evaluate(&self, n: usize) -> BaseFields {
        let mut height_m = vec![0.0; n * n];
        let mut hardness = vec![0.0f32; n * n];
        let mut uplift = self.uplift.map(|_| vec![0.0; n * n]);
        let threads = std::thread::available_parallelism()
            .map_or(4, |t| t.get())
            .clamp(1, 64);
        let rows_per = n.div_ceil(threads);
        std::thread::scope(|scope| {
            let mut uplift_chunks: Vec<Option<&mut [f64]>> = match uplift.as_mut() {
                Some(u) => u.chunks_mut(rows_per * n).map(Some).collect(),
                None => (0..n.div_ceil(rows_per)).map(|_| None).collect(),
            };
            for (chunk, ((heights, hard), up)) in height_m
                .chunks_mut(rows_per * n)
                .zip(hardness.chunks_mut(rows_per * n))
                .zip(uplift_chunks.iter_mut())
                .enumerate()
            {
                let up = up.take();
                scope.spawn(move || {
                    let mut registers = Vec::with_capacity(self.nodes.len());
                    let mut up = up;
                    for (k, h) in heights.iter_mut().enumerate() {
                        let index = chunk * rows_per * n + k;
                        let (x, y) = (index % n, index / n);
                        let u = (x as f64 + 0.5) / n as f64;
                        let v = (y as f64 + 0.5) / n as f64;
                        self.run(u, v, &mut registers);
                        let (height, hardness_value, uplift_value) = self.outputs(&registers);
                        *h = height;
                        hard[k] = hardness_value as f32;
                        if let (Some(up), Some(value)) = (up.as_deref_mut(), uplift_value) {
                            up[k] = value;
                        }
                    }
                });
            }
        });
        BaseFields {
            height_m,
            hardness,
            uplift,
        }
    }
}

/// Parse and compile; for error reporting at recipe validation.
pub fn check(graph: &GraphRecipe, footprint_m: f64) -> Result<()> {
    Program::compile(graph, footprint_m, 0).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::noise::layer_value;

    fn graph(json: &str) -> GraphRecipe {
        serde_json::from_str(json).unwrap()
    }

    const SAMPLE: &str = r#"{
      "nodes": [
        {"id": "wx", "op": {"kind": "fbm", "frequency": 2, "octaves": 3, "gain": 0.5, "seed": 1}},
        {"id": "wy", "op": {"kind": "fbm", "frequency": 2, "octaves": 3, "gain": 0.5, "seed": 2}},
        {"id": "peaks", "op": {"kind": "ridged", "frequency": 3, "octaves": 5, "gain": 0.5, "seed": 3,
            "rotate_octaves": true, "domain": [[1, 1], [-1, 2]],
            "warp": {"x": "wx", "y": "wy", "amplitude": 0.05}}},
        {"id": "cells", "op": {"kind": "cellular", "frequency": 5, "mode": "f2_minus_f1", "seed": 4}},
        {"id": "hills", "op": {"kind": "billow", "frequency": 4, "octaves": 3, "gain": 0.5, "seed": 5}},
        {"id": "mask", "op": {"kind": "height_mask", "input": "peaks", "from": 0.2, "to": 0.6}},
        {"id": "mixed", "op": {"kind": "blend", "a": "hills", "b": "peaks", "mask": "mask"}},
        {"id": "shaped", "op": {"kind": "curve", "input": "mixed", "points": [[-1, -1], [0, -0.2], [0.5, 0.4], [1, 1]]}},
        {"id": "terraced", "op": {"kind": "terrace", "input": "shaped", "steps": 6, "exponent": 2, "range": [-1, 1]}},
        {"id": "rough", "op": {"kind": "smooth_max", "a": "terraced", "b": "cells", "k": 0.2}},
        {"id": "product", "op": {"kind": "mul", "a": "rough", "b": "hills"}},
        {"id": "sum", "op": {"kind": "add", "inputs": ["rough", "product"]}},
        {"id": "height", "op": {"kind": "scale", "input": "sum", "factor": 300, "offset": 10}},
        {"id": "hard", "op": {"kind": "remap", "input": "hills", "from": [-1, 1], "to": [0.2, 0.8], "clamp": true}},
        {"id": "steep", "op": {"kind": "slope_mask", "input": "height", "from": 0.2, "to": 0.8}}
      ],
      "height": "height",
      "hardness": "hard",
      "uplift": "steep"
    }"#;

    #[test]
    fn graph_is_periodic_and_gradients_match_finite_differences() {
        let program = Program::compile(&graph(SAMPLE), 4000.0, 7).unwrap();
        let h = 1.0e-6;
        let mut checked = 0;
        for k in 0..200 {
            let u = (f64::from(k) * 0.1373).fract();
            let v = (f64::from(k) * 0.2917 + 0.03).fract();
            let (a, _, _) = program.sample(u, v);
            let (b, _, _) = program.sample(u + 1.0, v - 1.0);
            assert!((a - b).abs() < 1e-7, "not periodic: {a} vs {b}");
            let (gu, gv) = program.height_gradient(u, v);
            let fd_u = (program.sample(u + h, v).0 - program.sample(u - h, v).0) / (2.0 * h);
            let fd_v = (program.sample(u, v + h).0 - program.sample(u, v - h).0) / (2.0 * h);
            // A sample landing on a kink (min/max, ridge crest, terrace step,
            // cell border) may disagree; allow a few.
            let scale = 1.0 + fd_u.hypot(fd_v);
            if (gu - fd_u).abs() / scale < 1e-3 && (gv - fd_v).abs() / scale < 1e-3 {
                checked += 1;
            }
        }
        eprintln!("gradient agreement {checked}/200");
        assert!(checked >= 195, "only {checked}/200 gradients agree");
    }

    #[test]
    fn noise_nodes_match_legacy_layers() {
        let g = graph(
            r#"{"nodes": [{"id": "r", "op": {"kind": "ridged", "frequency": 3, "octaves": 6,
                "gain": 0.5, "seed": 9, "sharpness": 2}}], "height": "r", "hardness": "r"}"#,
        );
        let program = Program::compile(&g, 4000.0, 11).unwrap();
        let layer = NoiseLayer {
            kind: NoiseKind::Ridged,
            frequency: 3,
            octaves: 6,
            lacunarity: 2,
            gain: 0.5,
            weight: 1.0,
            sharpness: 2.0,
            rotate_octaves: false,
            slope_damping: 0.0,
            domain: None,
        };
        for k in 0..20 {
            let u = f64::from(k) * 0.049;
            let expected = layer_value(&layer, u, 0.3, 11, GRAPH_ROLE_BASE + 9);
            assert_eq!(program.sample(u, 0.3).0, expected);
        }
    }

    #[test]
    fn invalid_graphs_are_rejected() {
        for bad in [
            r#"{"nodes": [{"id": "a", "op": {"kind": "add", "inputs": ["b"]}}], "height": "a", "hardness": "a"}"#,
            r#"{"nodes": [{"id": "a", "op": {"kind": "constant", "value": 1}}, {"id": "a", "op": {"kind": "constant", "value": 2}}], "height": "a", "hardness": "a"}"#,
            r#"{"nodes": [{"id": "a", "op": {"kind": "constant", "value": 1}}], "height": "zz", "hardness": "a"}"#,
            r#"{"nodes": [{"id": "a", "op": {"kind": "curve", "input": "a", "points": [[0, 0], [1, 1]]}}], "height": "a", "hardness": "a"}"#,
            r#"{"nodes": [{"id": "a", "op": {"kind": "fbm", "frequency": 0, "octaves": 3, "gain": 0.5, "seed": 1}}], "height": "a", "hardness": "a"}"#,
        ] {
            assert!(Program::compile(&graph(bad), 4000.0, 1).is_err(), "{bad}");
        }
        assert!(
            serde_json::from_str::<GraphRecipe>(
                r#"{"nodes": [{"id": "a", "op": {"kind": "constant", "value": 1, "extra": 2}}], "height": "a", "hardness": "a"}"#
            )
            .is_err()
        );
    }
}
