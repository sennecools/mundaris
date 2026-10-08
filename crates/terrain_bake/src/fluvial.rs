//! Stream-power fluvial incision with tectonic uplift (CPU, f64).
//!
//! Each step routes flow by steepest descent over a depression-filled surface,
//! accumulates drainage area along the receiver tree, and solves
//! dh/dt = U − K·A^m·S implicitly in receiver order (Braun & Willett 2013, n = 1),
//! then limits each cell to its talus slope above its receiver. Talus and
//! erodibility vary with rock hardness, and receiver choice carries a fixed
//! per-cell jitter, so hillslopes do not settle into grid-aligned planes.
//! Depressions are filled each step (lakes instantly infilled), so every cell
//! drains to the fixed base-level outlet. Drainage-organized relief grows where
//! the uplift field is high (Cordonnier et al. 2016).

use crate::drainage::fill_from_outlets;
use crate::noise::pcg;
use crate::recipe::FluvialRecipe;

const SQRT2: f64 = std::f64::consts::SQRT_2;
const OFFSETS: [(i64, i64, f64); 8] = [
    (-1, 0, 1.0),
    (1, 0, 1.0),
    (0, -1, 1.0),
    (0, 1, 1.0),
    (-1, -1, SQRT2),
    (1, -1, SQRT2),
    (-1, 1, SQRT2),
    (1, 1, SQRT2),
];

pub struct FluvialInputs<'a> {
    pub n: usize,
    pub cell_m: f64,
    /// Uplift weight per cell in [0, 1].
    pub uplift: &'a [f64],
    /// Cells marked true are fixed base level.
    pub outlet: &'a [bool],
    pub iterations: u32,
    /// Per-cell stable slope (rise over run).
    pub talus: &'a [f64],
    /// Per-cell erodibility multiplier.
    pub erodibility_scale: &'a [f64],
    pub fill_slope: f64,
    pub seed: u32,
}

/// Fixed per-cell routing weight in [1 − jitter/2, 1 + jitter/2].
fn jitter_weight(index: usize, seed: u32, jitter: f64) -> f64 {
    let hash = pcg(index as u32 ^ pcg(seed));
    1.0 + jitter * (f64::from(hash) / f64::from(u32::MAX) - 0.5)
}

/// Receiver of every cell (itself for outlets) and the distance to it in cells.
/// Candidate slopes are weighted by a fixed per-neighbour jitter.
fn receivers(
    height: &[f64],
    n: usize,
    outlet: &[bool],
    seed: u32,
    jitter: f64,
) -> (Vec<u32>, Vec<f64>) {
    let mut receiver = vec![0u32; n * n];
    let mut distance = vec![0.0; n * n];
    for i in 0..n * n {
        receiver[i] = i as u32;
        if outlet[i] {
            continue;
        }
        let (x, y) = ((i % n) as i64, (i / n) as i64);
        let mut best = 0.0;
        for (k, &(dx, dy, d)) in OFFSETS.iter().enumerate() {
            let j =
                (y + dy).rem_euclid(n as i64) as usize * n + (x + dx).rem_euclid(n as i64) as usize;
            let slope = (height[i] - height[j]) / d * jitter_weight(i * 8 + k, seed, jitter);
            if slope > best {
                best = slope;
                receiver[i] = j as u32;
                distance[i] = d;
            }
        }
    }
    (receiver, distance)
}

/// Cells ordered so every receiver precedes its donors (base level first).
fn stack_order(receiver: &[u32]) -> Vec<u32> {
    let count = receiver.len();
    let mut donor_count = vec![0u32; count + 1];
    for (i, &r) in receiver.iter().enumerate() {
        if r as usize != i {
            donor_count[r as usize + 1] += 1;
        }
    }
    for i in 0..count {
        donor_count[i + 1] += donor_count[i];
    }
    let offsets = donor_count;
    let mut fill = offsets.clone();
    let mut donors = vec![0u32; offsets[count] as usize];
    for (i, &r) in receiver.iter().enumerate() {
        if r as usize != i {
            donors[fill[r as usize] as usize] = i as u32;
            fill[r as usize] += 1;
        }
    }
    let mut stack = Vec::with_capacity(count);
    let mut pending = Vec::new();
    for (i, &r) in receiver.iter().enumerate() {
        if r as usize == i {
            pending.push(i as u32);
            while let Some(cell) = pending.pop() {
                stack.push(cell);
                let c = cell as usize;
                pending.extend_from_slice(&donors[offsets[c] as usize..offsets[c + 1] as usize]);
            }
        }
    }
    stack
}

/// Run fluvial steps in place on `height` (metres).
pub fn erode(height: &mut [f64], inputs: &FluvialInputs<'_>, recipe: &FluvialRecipe) {
    let n = inputs.n;
    let cell_area = inputs.cell_m * inputs.cell_m;
    let mut area = vec![0.0; n * n];
    for _ in 0..inputs.iterations {
        // Base-level cells keep their height; all others receive uplift.
        for ((h, &outlet), &uplift) in height.iter_mut().zip(inputs.outlet).zip(inputs.uplift) {
            if !outlet {
                *h += recipe.dt * recipe.uplift_m_per_step * uplift;
            }
        }
        fill_from_outlets(height, n, inputs.outlet, inputs.fill_slope * inputs.cell_m);
        let (receiver, distance) =
            receivers(height, n, inputs.outlet, inputs.seed, recipe.routing_jitter);
        let stack = stack_order(&receiver);
        area.fill(cell_area);
        for &cell in stack.iter().rev() {
            let i = cell as usize;
            let r = receiver[i] as usize;
            if r != i {
                area[r] += area[i];
            }
        }
        for &cell in &stack {
            let i = cell as usize;
            let r = receiver[i] as usize;
            if r == i {
                continue;
            }
            let run_m = distance[i] * inputs.cell_m;
            let factor = recipe.erodibility
                * inputs.erodibility_scale[i]
                * recipe.dt
                * area[i].powf(recipe.area_exponent)
                / run_m;
            let solved = (height[i] + factor * height[r]) / (1.0 + factor);
            height[i] = solved.min(height[r] + inputs.talus[i] * run_m);
        }
        diffuse(
            height,
            n,
            inputs.outlet,
            recipe.dt * recipe.diffusion_m2_per_step / cell_area,
        );
    }
}

/// Explicit periodic linear diffusion with total coefficient `amount` (D·dt/Δx²),
/// split into stable substeps. Base-level cells stay fixed.
fn diffuse(height: &mut [f64], n: usize, outlet: &[bool], amount: f64) {
    if amount <= 0.0 {
        return;
    }
    let substeps = (amount / 0.2).ceil() as usize;
    let k = amount / substeps as f64;
    let mut next = height.to_vec();
    for _ in 0..substeps {
        for y in 0..n {
            for x in 0..n {
                let i = y * n + x;
                if outlet[i] {
                    continue;
                }
                let sum = height[y * n + (x + n - 1) % n]
                    + height[y * n + (x + 1) % n]
                    + height[((y + n - 1) % n) * n + x]
                    + height[((y + 1) % n) * n + x];
                next[i] = height[i] + k * (sum - 4.0 * height[i]);
            }
        }
        height.copy_from_slice(&next);
    }
}
