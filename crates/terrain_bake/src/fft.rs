//! Radix-2 FFT for periodic n² grids (n a power of two), used to apply
//! real, isotropic frequency-domain filters such as scale-dependent diffusion.
//! Rows run on all cores; columns are done as rows of the transpose.

use std::f64::consts::TAU;

/// In-place complex FFT of one row (length a power of two). The inverse is
/// normalised by 1/n.
fn fft(re: &mut [f64], im: &mut [f64], inverse: bool) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let angle = if inverse { TAU } else { -TAU } / len as f64;
        let (wr, wi) = (angle.cos(), angle.sin());
        for start in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (a, b) = (start + k, start + k + len / 2);
                let tr = re[b] * cr - im[b] * ci;
                let ti = re[b] * ci + im[b] * cr;
                re[b] = re[a] - tr;
                im[b] = im[a] - ti;
                re[a] += tr;
                im[a] += ti;
                let next = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = next;
            }
        }
        len <<= 1;
    }
    if inverse {
        let scale = 1.0 / n as f64;
        re.iter_mut().for_each(|v| *v *= scale);
        im.iter_mut().for_each(|v| *v *= scale);
    }
}

fn rows(re: &mut [f64], im: &mut [f64], n: usize, inverse: bool) {
    let threads = std::thread::available_parallelism()
        .map_or(4, |t| t.get())
        .clamp(1, 64);
    let per = n.div_ceil(threads).max(1) * n;
    std::thread::scope(|scope| {
        for (r, i) in re.chunks_mut(per).zip(im.chunks_mut(per)) {
            scope.spawn(move || {
                for (rr, ii) in r.chunks_mut(n).zip(i.chunks_mut(n)) {
                    fft(rr, ii, inverse);
                }
            });
        }
    });
}

fn transpose(v: &mut [f64], n: usize) {
    for y in 0..n {
        for x in y + 1..n {
            v.swap(y * n + x, x * n + y);
        }
    }
}

fn fft2(re: &mut [f64], im: &mut [f64], n: usize, inverse: bool) {
    rows(re, im, n, inverse);
    transpose(re, n);
    transpose(im, n);
    rows(re, im, n, inverse);
    transpose(re, n);
    transpose(im, n);
}

/// Angular wavenumber |k| (rad/m) of every bin of an n² grid with cell size
/// `cell_m`, in FFT order.
pub fn wavenumbers(n: usize, cell_m: f64) -> Vec<f64> {
    let l = n as f64 * cell_m;
    let axis = |i: usize| {
        let i = if i <= n / 2 {
            i as f64
        } else {
            i as f64 - n as f64
        };
        i * TAU / l
    };
    (0..n * n).map(|i| axis(i % n).hypot(axis(i / n))).collect()
}

/// Multiply the spectrum of the periodic grid `values` by the real `gain`
/// (one value per bin, FFT order, symmetric in ±k so the result stays real).
pub fn filter(values: &mut [f64], n: usize, gain: &[f64]) {
    assert!(n.is_power_of_two() && values.len() == n * n && gain.len() == n * n);
    let mut im = vec![0.0; n * n];
    fft2(values, &mut im, n, false);
    for ((r, i), g) in values.iter_mut().zip(im.iter_mut()).zip(gain) {
        *r *= g;
        *i *= g;
    }
    fft2(values, &mut im, n, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_kernel_decays_a_sinusoid_exactly() {
        // Constant diffusion multiplies mode k by exp(−κt·k²).
        let (n, cell, kt) = (64, 2.0, 30.0);
        let k = 3.0 * TAU / (n as f64 * cell);
        let wave = |x: usize, y: usize| (k * (x as f64 * cell) + 0.3).cos() + 0.0 * y as f64;
        let mut v: Vec<f64> = (0..n * n).map(|i| wave(i % n, i / n)).collect();
        let gain: Vec<f64> = wavenumbers(n, cell)
            .iter()
            .map(|k| (-kt * k * k).exp())
            .collect();
        filter(&mut v, n, &gain);
        let expected = (-kt * k * k).exp();
        for (i, value) in v.iter().enumerate() {
            assert!((value - expected * wave(i % n, i / n)).abs() < 1e-9);
        }
    }
}
