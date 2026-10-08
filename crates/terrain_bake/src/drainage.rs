//! Depression filling so every cell drains to the base-level outlet.
//!
//! Priority-Flood with a gradient (Barnes, Lehman & Mulla 2014) on a periodic
//! 8-connected grid: closed basins become gently sloped floors toward their
//! spill points, so erosion carves valleys instead of filling lakes flat.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

struct Entry {
    height: f64,
    index: u32,
}

impl PartialEq for Entry {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Entry {}
impl PartialOrd for Entry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Entry {
    // Reversed for a min-heap; ties break by index for a deterministic order.
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .height
            .total_cmp(&self.height)
            .then(other.index.cmp(&self.index))
    }
}

/// Raise depressions in `height` (any unit) so that every cell reaches a cell
/// at or below `outlet_level` through neighbours descending by at least
/// `fill_slope × distance`. Returns the number of raised cells.
pub fn fill_depressions(height: &mut [f64], n: usize, outlet_level: f64, fill_slope: f64) -> usize {
    let outlet: Vec<bool> = height.iter().map(|&h| h <= outlet_level).collect();
    fill_from_outlets(height, n, &outlet, fill_slope)
}

/// As [`fill_depressions`], seeded by an explicit outlet mask whose cells keep
/// their height (a fixed base level).
pub fn fill_from_outlets(height: &mut [f64], n: usize, outlet: &[bool], fill_slope: f64) -> usize {
    let mut closed = outlet.to_vec();
    let mut heap = BinaryHeap::new();
    for (index, &h) in height.iter().enumerate() {
        if outlet[index] {
            heap.push(Entry {
                height: h,
                index: index as u32,
            });
        }
    }
    let mut raised = 0;
    let wrap = |v: i64| v.rem_euclid(n as i64) as usize;
    while let Some(Entry { height: h, index }) = heap.pop() {
        let (x, y) = ((index as usize % n) as i64, (index as usize / n) as i64);
        for dy in -1..=1i64 {
            for dx in -1..=1i64 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let neighbour = wrap(y + dy) * n + wrap(x + dx);
                if closed[neighbour] {
                    continue;
                }
                closed[neighbour] = true;
                let distance = if dx != 0 && dy != 0 {
                    std::f64::consts::SQRT_2
                } else {
                    1.0
                };
                let minimum = h + fill_slope * distance;
                if height[neighbour] < minimum {
                    height[neighbour] = minimum;
                    raised += 1;
                }
                heap.push(Entry {
                    height: height[neighbour],
                    index: neighbour as u32,
                });
            }
        }
    }
    raised
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filled_grid_drains_everywhere() {
        let n = 32;
        let mut height: Vec<f64> = (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f64, (i / n) as f64);
                (x * 0.7).sin() * (y * 0.5).cos() * 10.0 + (x * 0.2).cos() * 4.0
            })
            .collect();
        let outlet = height.iter().copied().fold(f64::INFINITY, f64::min) + 0.5;
        let raised = fill_depressions(&mut height, n, outlet, 0.01);
        assert!(raised > 0);
        for i in 0..n * n {
            if height[i] <= outlet {
                continue;
            }
            let (x, y) = ((i % n) as i64, (i / n) as i64);
            let has_lower = (-1..=1i64).any(|dy| {
                (-1..=1i64).any(|dx| {
                    let j = (y + dy).rem_euclid(n as i64) as usize * n
                        + (x + dx).rem_euclid(n as i64) as usize;
                    j != i && height[j] < height[i]
                })
            });
            assert!(has_lower, "cell {i} is a pit after filling");
        }
    }
}
