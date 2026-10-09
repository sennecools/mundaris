//! Lunar crater production and chronology (Neukum, Ivanov & Hartmann 2001).
//!
//! - Production function: log10 N(>D) = Σ aₖ (log10 D)^k per km² for a surface
//!   with N(1 km) = 10^a₀, valid for D ≈ 0.01–300 km.
//! - Chronology: N(1 km)(T) = 5.44e-14 (e^{6.93 T} − 1) + 8.38e-4 T per km², with
//!   T the surface age in Ga.

const PRODUCTION: [f64; 12] = [
    -3.0876, -3.557528, 0.781027, 1.021521, -0.156012, -0.444058, 0.019977, 0.086850, -0.005874,
    -0.006809, 8.25e-4, 5.54e-5,
];

/// Cumulative craters ≥ 1 km per km² on a surface of age `t_ga`.
pub fn n1(t_ga: f64) -> f64 {
    5.44e-14 * ((6.93 * t_ga).exp() - 1.0) + 8.38e-4 * t_ga
}

/// Cumulative craters ≥ `d_km` per km² on a surface of age `t_ga`.
pub fn cumulative(d_km: f64, t_ga: f64) -> f64 {
    let x = d_km.log10();
    let log_n: f64 = PRODUCTION
        .iter()
        .enumerate()
        .map(|(k, a)| a * x.powi(k as i32))
        .sum();
    10f64.powf(log_n - PRODUCTION[0]) * n1(t_ga)
}

/// Diameter (km) with cumulative fraction `q` ∈ [0, 1] of the population
/// between `d_min_km` and `d_max_km` (q = 0 → d_min, q = 1 → d_max).
pub fn diameter_quantile(q: f64, d_min_km: f64, d_max_km: f64) -> f64 {
    let (n_min, n_max) = (cumulative(d_min_km, 1.0), cumulative(d_max_km, 1.0));
    let target = n_min - q * (n_min - n_max);
    let (mut lo, mut hi) = (d_min_km.ln(), d_max_km.ln());
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if cumulative(mid.exp(), 1.0) > target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    (0.5 * (lo + hi)).exp()
}

/// Formation age (Ga before present) with cumulative fraction `q` of all
/// craters formed on a surface of age `surface_ga` (q = 0 → now, 1 → oldest).
pub fn age_quantile(q: f64, surface_ga: f64) -> f64 {
    let target = q * n1(surface_ga);
    let (mut lo, mut hi) = (0.0, surface_ga);
    for _ in 0..80 {
        let mid = 0.5 * (lo + hi);
        if n1(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    0.5 * (lo + hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_published_anchor_values() {
        // N(1 km) at 3.0 Ga: 2.572e-3 /km² (same as E3 neukum.js).
        assert!(
            (n1(3.0) - 2.5722e-3).abs() / 2.5722e-3 < 1e-3,
            "{}",
            n1(3.0)
        );
        // The production function is normalised so N(1 km) equals the chronology.
        assert!((cumulative(1.0, 3.0) - n1(3.0)).abs() < 1e-12);
        let d = diameter_quantile(0.5, 3.0, 60.0);
        assert!(
            d > 3.0 && d < 6.0,
            "median of a steep population is near d_min: {d}"
        );
        let t = age_quantile(0.5, 4.1);
        assert!(
            t > 3.8 && t < 4.1,
            "half of all craters are older than ~3.9 Ga: {t}"
        );
    }
}
