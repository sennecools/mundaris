//! Derived semantic channels computed in f64 on the CPU from the final eroded
//! height. All neighbourhoods wrap, so derived channels tile like the height.

use crate::recipe::DerivedRecipe;

/// Normalized fields at full height resolution, each in [0, 1].
pub struct DerivedFields {
    pub flow: Vec<f64>,
    pub wetness: Vec<f64>,
    pub exposure: Vec<f64>,
    pub spawn: Vec<f64>,
}

const NEIGHBOURS: [(i64, i64, f64); 8] = [
    (-1, 0, 1.0),
    (1, 0, 1.0),
    (0, -1, 1.0),
    (0, 1, 1.0),
    (-1, -1, std::f64::consts::SQRT_2),
    (1, -1, std::f64::consts::SQRT_2),
    (-1, 1, std::f64::consts::SQRT_2),
    (1, 1, std::f64::consts::SQRT_2),
];

fn at(n: usize, x: i64, y: i64) -> usize {
    let n = n as i64;
    (y.rem_euclid(n) * n + x.rem_euclid(n)) as usize
}

/// Slope as rise over run from periodic central differences.
pub fn slope_tangent(height_m: &[f64], n: usize, cell_m: f64) -> Vec<f64> {
    let mut out = vec![0.0; n * n];
    for y in 0..n as i64 {
        for x in 0..n as i64 {
            let gx = (height_m[at(n, x + 1, y)] - height_m[at(n, x - 1, y)]) / (2.0 * cell_m);
            let gy = (height_m[at(n, x, y + 1)] - height_m[at(n, x, y - 1)]) / (2.0 * cell_m);
            out[at(n, x, y)] = (gx * gx + gy * gy).sqrt();
        }
    }
    out
}

fn laplacian(height_m: &[f64], n: usize, cell_m: f64) -> Vec<f64> {
    let mut out = vec![0.0; n * n];
    for y in 0..n as i64 {
        for x in 0..n as i64 {
            let centre = height_m[at(n, x, y)];
            let sum = height_m[at(n, x - 1, y)]
                + height_m[at(n, x + 1, y)]
                + height_m[at(n, x, y - 1)]
                + height_m[at(n, x, y + 1)];
            out[at(n, x, y)] = (sum - 4.0 * centre) / (cell_m * cell_m);
        }
    }
    out
}

/// Multiple-flow-direction accumulation (Freeman/Quinn) in cell areas.
/// Cells are processed from highest to lowest; ties break by index so the
/// order, and therefore the result, is deterministic. Pits retain their flow.
pub fn flow_accumulation(height_m: &[f64], n: usize, exponent: f64) -> Vec<f64> {
    let mut order: Vec<u32> = (0..(n * n) as u32).collect();
    order.sort_unstable_by(|&a, &b| {
        height_m[b as usize]
            .total_cmp(&height_m[a as usize])
            .then(a.cmp(&b))
    });
    let mut accumulation = vec![1.0; n * n];
    for &cell in &order {
        let i = cell as usize;
        let (x, y) = ((i % n) as i64, (i / n) as i64);
        let h = height_m[i];
        let mut weights = [0.0; 8];
        let mut total = 0.0;
        for (k, &(dx, dy, distance)) in NEIGHBOURS.iter().enumerate() {
            let drop = (h - height_m[at(n, x + dx, y + dy)]) / distance;
            if drop > 0.0 {
                weights[k] = drop.powf(exponent);
                total += weights[k];
            }
        }
        if total > 0.0 {
            let share = accumulation[i] / total;
            for (k, &(dx, dy, _)) in NEIGHBOURS.iter().enumerate() {
                if weights[k] > 0.0 {
                    accumulation[at(n, x + dx, y + dy)] += share * weights[k];
                }
            }
        }
    }
    accumulation
}

/// Periodic separable box mean with the given radius in cells.
pub fn box_blur(values: &[f64], n: usize, radius: usize) -> Vec<f64> {
    let width = (2 * radius + 1) as f64;
    let mut horizontal = vec![0.0; n * n];
    for y in 0..n {
        let row = &values[y * n..(y + 1) * n];
        let mut sum: f64 = (0..=2 * radius).map(|k| row[(k + n - radius) % n]).sum();
        for x in 0..n {
            horizontal[y * n + x] = sum / width;
            sum += row[(x + radius + 1) % n] - row[(x + n - radius) % n];
        }
    }
    let mut out = vec![0.0; n * n];
    for x in 0..n {
        let column = |y: usize| horizontal[(y % n) * n + x];
        let mut sum: f64 = (0..=2 * radius).map(|k| column(k + n - radius)).sum();
        for y in 0..n {
            out[y * n + x] = sum / width;
            sum += column(y + radius + 1) - column(y + n - radius);
        }
    }
    out
}

/// Periodic box downsample by an integer factor.
pub fn downsample(values: &[f64], n: usize, factor: usize) -> Vec<f64> {
    let m = n / factor;
    let scale = 1.0 / (factor * factor) as f64;
    let mut out = vec![0.0; m * m];
    for y in 0..n {
        for x in 0..n {
            out[(y / factor) * m + x / factor] += values[y * n + x] * scale;
        }
    }
    out
}

/// Value at quantile `q` of `|values|` (robust normalization scale).
pub fn abs_quantile(values: &[f64], q: f64) -> f64 {
    let mut magnitudes: Vec<f64> = values.iter().map(|v| v.abs()).collect();
    let k = ((magnitudes.len() - 1) as f64 * q).round() as usize;
    let (_, value, _) = magnitudes.select_nth_unstable_by(k, f64::total_cmp);
    *value
}

fn scaled(value: f64, scale: f64) -> f64 {
    if scale > 0.0 { value / scale } else { 0.0 }
}

pub fn derive(
    height_m: &[f64],
    erosion_delta_m: &[f64],
    n: usize,
    cell_m: f64,
    recipe: &DerivedRecipe,
) -> DerivedFields {
    let accumulation = flow_accumulation(height_m, n, recipe.flow_exponent);
    let log_scale = abs_quantile(&accumulation, 0.999).ln().max(1.0e-9);
    let flow: Vec<f64> = accumulation
        .iter()
        .map(|a| (a.ln() / log_scale).clamp(0.0, 1.0))
        .collect();

    let radius = ((recipe.relief_radius_m / cell_m).round() as usize).clamp(1, n / 4);
    let blurred = box_blur(height_m, n, radius);
    let relief: Vec<f64> = height_m.iter().zip(&blurred).map(|(h, b)| h - b).collect();
    let relief_scale = abs_quantile(&relief, 0.98);
    let curvature = laplacian(height_m, n, cell_m);
    let curvature_scale = abs_quantile(&curvature, 0.98);
    let slope = slope_tangent(height_m, n, cell_m);

    let deposition: Vec<f64> = erosion_delta_m.iter().map(|d| d.max(0.0)).collect();
    let incision: Vec<f64> = erosion_delta_m.iter().map(|d| (-d).max(0.0)).collect();
    let deposition_scale = abs_quantile(&deposition, 0.99);
    let incision_scale = abs_quantile(&incision, 0.99);

    let (w, e, s) = (recipe.wetness, recipe.exposure, recipe.spawn);
    let mut wetness = vec![0.0; n * n];
    let mut exposure = vec![0.0; n * n];
    let mut spawn = vec![0.0; n * n];
    for i in 0..n * n {
        let relief_n = scaled(relief[i], relief_scale).clamp(-1.0, 1.0);
        let convex_n = scaled(-curvature[i], curvature_scale).clamp(-1.0, 1.0);
        let deposition_n = scaled(deposition[i], deposition_scale).min(1.0);
        let incision_n = scaled(incision[i], incision_scale).min(1.0);
        let steep_n = (slope[i] / s.steep_tangent).min(1.0);
        exposure[i] =
            (e.bias + 0.5 * e.relief * relief_n + 0.5 * e.convexity * convex_n).clamp(0.0, 1.0);
        wetness[i] = (w.bias
            + w.flow * flow[i]
            + w.deposition * deposition_n
            + w.shelter * (-relief_n).max(0.0))
        .clamp(0.0, 1.0);
        spawn[i] = (s.bias + s.flow * flow[i] + s.incision * incision_n + s.steepness * steep_n)
            .clamp(0.0, 1.0);
    }
    DerivedFields {
        flow,
        wetness,
        exposure,
        spawn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flow_is_conserved_on_a_slope_with_one_pit() {
        // A cone with its pit at the centre collects every cell's unit area.
        let n = 16;
        let mut height = vec![0.0; n * n];
        for y in 0..n {
            for x in 0..n {
                let dx = x as f64 - 8.0;
                let dy = y as f64 - 8.0;
                height[y * n + x] = (dx * dx + dy * dy).sqrt();
            }
        }
        let accumulation = flow_accumulation(&height, n, 1.1);
        let total: f64 = accumulation
            .iter()
            .enumerate()
            .filter(|&(i, _)| {
                let h = height[i];
                let (x, y) = ((i % n) as i64, (i / n) as i64);
                NEIGHBOURS
                    .iter()
                    .all(|&(dx, dy, _)| height[at(n, x + dx, y + dy)] >= h)
            })
            .map(|(_, a)| a)
            .sum();
        assert!((total - (n * n) as f64).abs() < 1e-6, "pits hold {total}");
    }

    #[test]
    fn box_blur_preserves_mean_and_wraps() {
        let n = 8;
        let values: Vec<f64> = (0..n * n).map(|i| (i % 5) as f64).collect();
        let blurred = box_blur(&values, n, 2);
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;
        assert!((mean(&values) - mean(&blurred)).abs() < 1e-12);
        let constant = box_blur(&vec![3.0; n * n], n, 3);
        assert!(constant.iter().all(|v| (v - 3.0).abs() < 1e-12));
    }
}
