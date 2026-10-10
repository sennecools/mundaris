//! Tier A world-map bake, CPU test oracle (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6).
//!
//! Under ADR 0023 the GPU bake (`renderer::tier_a`) produces the fields the
//! terrain uses; this f64 bake defines the same functions and checks the GPU
//! within tolerance. M1 (World map and planet editor) stages: continents with a
//! domain warp, sea level from an area-weighted histogram (exact ocean
//! coverage), a continental shelf, temperature (insolation by latitude and
//! axial tilt, lapse rate, ocean moderation), three-cell wind bands and moisture
//! advection without an orographic term (no rain shadow yet).
//!
//! Every stage is per texel, except the histogram and the iterated passes
//! (ocean diffusion and moisture), which sample the previous iteration through
//! [`CubeMap::bilinear`] so face edges need no special cases.
use super::{
    TerrainError,
    archetype::{PlanetParams, TierAStage},
    noise::gradient_noise,
    world_map::{CubeMap, texel_direction},
};
use glam::DVec3;

/// Sea-level histogram resolution over the normalised noise range `[-1, 1]`.
pub const HISTOGRAM_BINS: usize = 4096;
/// Integer area weight of the largest (face-centre) texel.
pub const AREA_WEIGHT_UNITS: f64 = 1024.0;
/// Diffusion passes spreading the ocean mask; each pass steps
/// `ocean_blur_m / sqrt(OCEAN_BLUR_PASSES)` so the diffusion length is the
/// authored distance at any resolution.
pub const OCEAN_BLUR_PASSES: u32 = 16;

/// Inputs of one body's bake.
#[derive(Debug, Clone, PartialEq)]
pub struct TierAInputs {
    pub params: PlanetParams,
    pub stages: Vec<TierAStage>,
    pub radius_m: f64,
    /// Body-fixed rotation axis (latitude reference).
    pub pole: DVec3,
    pub face_cells: usize,
}

/// Baked fields. Elevation is metres relative to sea level, temperature °C,
/// moisture 0..1, wind east/north components in the local tangent frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TierAFields {
    pub elevation: CubeMap<f32>,
    pub temperature: CubeMap<f32>,
    pub moisture: CubeMap<f32>,
    pub wind_east: CubeMap<f32>,
    pub wind_north: CubeMap<f32>,
    /// Normalised noise values at sea level and at the 0.1 % and 99.9 %
    /// area percentiles (the deepest ocean and highest land references).
    pub sea_level: f64,
    pub noise_low: f64,
    pub noise_high: f64,
    /// Area-weighted fraction of texels below sea level.
    pub ocean_fraction: f64,
}

/// Area weight of a texel at chart position `(u, v)`: its solid angle relative
/// to a face-centre texel, quantised so CPU and GPU histograms agree.
pub fn area_weight(u: f64, v: f64) -> u32 {
    (AREA_WEIGHT_UNITS / (1.0 + u * u + v * v).powf(1.5)).round() as u32
}

/// Chart coordinates of texel (i, j) on an `n`-cell face.
pub fn texel_uv(i: usize, j: usize, n: usize) -> (f64, f64) {
    (
        -1.0 + (2.0 * i as f64 + 1.0) / n as f64,
        -1.0 + (2.0 * j as f64 + 1.0) / n as f64,
    )
}

/// Normalised fBm in about `[-0.87, 0.87]`: octave `k` has frequency
/// `frequency * lacunarity^k`, amplitude `gain^k` and seed `seed + k`.
pub fn fbm(p: DVec3, frequency: f64, octaves: u32, lacunarity: f64, gain: f64, seed: u32) -> f64 {
    let (mut sum, mut norm, mut amplitude, mut f) = (0.0, 0.0, 1.0, frequency);
    for k in 0..octaves {
        sum += amplitude * gradient_noise(p * f, seed.wrapping_add(k)).0;
        norm += amplitude;
        amplitude *= gain;
        f *= lacunarity;
    }
    sum / norm
}

/// Normalised continent noise at unit direction `d` (before sea level).
pub fn continent_noise(d: DVec3, params: &PlanetParams, radius_m: f64) -> f64 {
    let warp_frequency = radius_m / params.warp_wavelength_m;
    let warp = DVec3::new(
        fbm(d, warp_frequency, 3, 2.0, 0.5, params.warp_seed),
        fbm(
            d,
            warp_frequency,
            3,
            2.0,
            0.5,
            params.warp_seed.wrapping_add(16),
        ),
        fbm(
            d,
            warp_frequency,
            3,
            2.0,
            0.5,
            params.warp_seed.wrapping_add(32),
        ),
    );
    // Displacement in radians, proportional to the continent wavelength.
    let warped =
        (d + warp * (params.warp_strength * params.continent_wavelength_m / radius_m)).normalize();
    fbm(
        warped,
        radius_m / params.continent_wavelength_m,
        params.continent_octaves,
        params.continent_lacunarity,
        params.continent_gain,
        params.continent_seed,
    )
}

/// Sea level: the normalised noise value below which `coverage` of the
/// area-weighted histogram lies (linear within the crossing bin).
pub fn sea_level_from_histogram(histogram: &[u64], coverage: f64) -> f64 {
    let total: u64 = histogram.iter().sum();
    let target = coverage * total as f64;
    let mut below = 0.0;
    for (bin, &count) in histogram.iter().enumerate() {
        let next = below + count as f64;
        if next >= target && count > 0 {
            let fraction = (target - below) / count as f64;
            return -1.0 + 2.0 * (bin as f64 + fraction) / histogram.len() as f64;
        }
        below = next;
    }
    1.0
}

pub fn histogram_bin(r: f64) -> usize {
    (((r + 1.0) * 0.5 * HISTOGRAM_BINS as f64).floor().max(0.0) as usize).min(HISTOGRAM_BINS - 1)
}

/// Area percentiles that set the elevation scale: land reaches its full height
/// at the 99.9 % noise value and the ocean its full depth at the 0.1 % value.
pub const RELIEF_PERCENTILES: [f64; 2] = [0.001, 0.999];

/// Elevation (m, sea level 0) from normalised noise `r`, sea level `s` and the
/// relief percentiles `low`, `high`.
pub fn shape_elevation(
    r: f64,
    s: f64,
    low: f64,
    high: f64,
    params: &PlanetParams,
    shelf: bool,
) -> f64 {
    if r >= s {
        let t = ((r - s) / (high - s).max(1e-9)).clamp(0.0, 1.0);
        return params.land_height_m * t.powf(params.land_exponent);
    }
    let t = ((s - r) / (s - low).max(1e-9)).clamp(0.0, 1.0);
    let deep = |u: f64| 1.0 - (1.0 - u.clamp(0.0, 1.0)).powi(3);
    let depth = if shelf && params.shelf_fraction > 0.0 {
        if t < params.shelf_fraction {
            params.shelf_depth_m * t / params.shelf_fraction
        } else {
            let u = (t - params.shelf_fraction) / (1.0 - params.shelf_fraction);
            params.shelf_depth_m + (params.ocean_depth_m - params.shelf_depth_m).max(0.0) * deep(u)
        }
    } else {
        params.ocean_depth_m * deep(t)
    };
    -depth
}

/// Annual-mean insolation shape at `x = sin(latitude)` for axial tilt `tilt`
/// (North 1975): `1 + s2 P2(x)`, `s2 = -5/8 P2(cos tilt)`.
pub fn insolation(x: f64, tilt_rad: f64) -> f64 {
    let p2 = |y: f64| 0.5 * (3.0 * y * y - 1.0);
    1.0 - 0.625 * p2(tilt_rad.cos()) * p2(x)
}

/// Sea-level temperature before moderation and noise.
pub fn base_temperature(x: f64, params: &PlanetParams) -> f64 {
    let tilt = params.axial_tilt_deg.to_radians();
    let (equator, pole) = (insolation(0.0, tilt), insolation(1.0, tilt));
    let span = equator - pole;
    let t = if span.abs() < 1e-9 {
        0.5
    } else {
        (insolation(x, tilt) - pole) / span
    };
    params.pole_c + (params.equator_c - params.pole_c) * t
}

/// Local east and north unit vectors at `d` for rotation axis `pole`.
pub fn tangent_frame(d: DVec3, pole: DVec3) -> (DVec3, DVec3) {
    let east = pole.cross(d);
    let east = if east.length_squared() < 1e-18 {
        d.any_orthonormal_vector()
    } else {
        east.normalize()
    };
    (east, d.cross(east))
}

/// Three-cell-style wind at `x = sin(latitude)`: `(east, north)` components.
pub fn wind(x: f64, params: &PlanetParams) -> (f64, f64) {
    let latitude = x.clamp(-1.0, 1.0).asin();
    let a = latitude.abs() / std::f64::consts::FRAC_PI_2;
    let s = (f64::from(params.wind_cells) * std::f64::consts::PI * a).sin();
    (-s, -latitude.signum() * params.wind_meridional * s)
}

/// Rain scale at `x = sin(latitude)` from the circulation cells: the cells'
/// meridional wind `-m · sin(2 · cells · |lat|)` converges (air rises, more
/// rain) where `cos(2 · cells · |lat|) > 0` and diverges (air sinks, deserts)
/// where it is negative: the equator and ~60° are wet, ~30° and the poles dry
/// for three cells.
pub fn rain_modulation(x: f64, params: &PlanetParams) -> f64 {
    let latitude = x.clamp(-1.0, 1.0).asin().abs();
    1.0 + params.rain_convergence * (2.0 * f64::from(params.wind_cells) * latitude).cos()
}

/// Fraction of rain-out moisture that evaporates from the ocean at `t_c`.
pub fn evaporation_factor(t_c: f64) -> f64 {
    ((t_c + 10.0) / 40.0).clamp(0.1, 1.0)
}

/// Bake every M1 stage the inputs request.
pub fn bake(inputs: &TierAInputs) -> Result<TierAFields, TerrainError> {
    let n = inputs.face_cells;
    let p = &inputs.params;
    if !(4..=4096).contains(&n)
        || !inputs.radius_m.is_finite()
        || inputs.radius_m <= 0.0
        || !inputs.pole.is_finite()
        || (inputs.pole.length() - 1.0).abs() > 1e-6
    {
        return Err(TerrainError::InvalidConfig);
    }
    let has = |stage| inputs.stages.contains(&stage);
    let texels = 6 * n * n;
    let directions: Vec<DVec3> = (0..6)
        .flat_map(|face| (0..n).flat_map(move |j| (0..n).map(move |i| (face, i, j))))
        .map(|(face, i, j)| texel_direction(face, i, j, n))
        .collect();
    let weights: Vec<u32> = (0..texels)
        .map(|k| {
            let (i, j) = (k % n, (k / n) % n);
            let (u, v) = texel_uv(i, j, n);
            area_weight(u, v)
        })
        .collect();

    // Continents and sea level.
    let raw: Vec<f64> = directions
        .iter()
        .map(|d| {
            if has(TierAStage::Continents) {
                continent_noise(*d, p, inputs.radius_m)
            } else {
                0.0
            }
        })
        .collect();
    let mut histogram = vec![0u64; HISTOGRAM_BINS];
    for (r, w) in raw.iter().zip(&weights) {
        histogram[histogram_bin(*r)] += u64::from(*w);
    }
    let sea_level = if has(TierAStage::SeaLevel) {
        sea_level_from_histogram(&histogram, p.ocean_coverage)
    } else {
        -1.0
    };
    let noise_low = sea_level_from_histogram(&histogram, RELIEF_PERCENTILES[0]);
    let noise_high = sea_level_from_histogram(&histogram, RELIEF_PERCENTILES[1]);
    let shelf = has(TierAStage::Shelf);
    let mut elevation = CubeMap::new(n, 0.0f32);
    for (k, r) in raw.iter().enumerate() {
        elevation.data_mut()[k] =
            shape_elevation(*r, sea_level, noise_low, noise_high, p, shelf) as f32;
    }
    let total_weight: u64 = weights.iter().map(|w| u64::from(*w)).sum();
    let ocean_weight: u64 = elevation
        .data()
        .iter()
        .zip(&weights)
        .filter(|(h, _)| **h < 0.0)
        .map(|(_, w)| u64::from(*w))
        .sum();

    // Ocean mask diffused inland for temperature moderation.
    let mut ocean = CubeMap::new(n, 0.0f32);
    for (k, h) in elevation.data().iter().enumerate() {
        ocean.data_mut()[k] = if *h < 0.0 { 1.0 } else { 0.0 };
    }
    let frames: Vec<(DVec3, DVec3)> = directions
        .iter()
        .map(|d| tangent_frame(*d, inputs.pole))
        .collect();
    let blur_step = p.ocean_blur_m / f64::from(OCEAN_BLUR_PASSES).sqrt() / inputs.radius_m;
    for _ in 0..if p.ocean_blur_m > 0.0 {
        OCEAN_BLUR_PASSES
    } else {
        0
    } {
        let previous = ocean.clone();
        for (k, d) in directions.iter().enumerate() {
            let (east, north) = frames[k];
            let mut sum = f64::from(previous.data()[k]);
            for offset in [east, -east, north, -north] {
                sum += previous.bilinear((*d + offset * blur_step).normalize());
            }
            ocean.data_mut()[k] = (sum / 5.0) as f32;
        }
    }

    // Temperature.
    let middle = 0.5 * (p.equator_c + p.pole_c);
    let noise_frequency = inputs.radius_m / p.temperature_noise_wavelength_m;
    let mut temperature = CubeMap::new(n, middle as f32);
    if has(TierAStage::Temperature) {
        for (k, d) in directions.iter().enumerate() {
            let x = d.dot(inputs.pole);
            let h = f64::from(elevation.data()[k]);
            let t = base_temperature(x, p) - p.lapse_c_per_km * h.max(0.0) / 1000.0;
            let moderated =
                middle + (t - middle) * (1.0 - p.ocean_moderation * f64::from(ocean.data()[k]));
            let noise =
                p.temperature_noise_c * fbm(*d, noise_frequency, 3, 2.0, 0.5, p.temperature_seed);
            temperature.data_mut()[k] = (moderated + noise) as f32;
        }
    }

    // Wind.
    let mut wind_east = CubeMap::new(n, 0.0f32);
    let mut wind_north = CubeMap::new(n, 0.0f32);
    if has(TierAStage::Wind) {
        for (k, d) in directions.iter().enumerate() {
            let (e, nn) = wind(d.dot(inputs.pole), p);
            wind_east.data_mut()[k] = e as f32;
            wind_north.data_mut()[k] = nn as f32;
        }
    }

    // Moisture: semi-Lagrangian advection, ocean evaporation, uniform rain.
    let mut moisture = CubeMap::new(n, 0.5f32);
    if has(TierAStage::Moisture) {
        let step = p.moisture_step_m / inputs.radius_m;
        let mut carried = CubeMap::new(n, 0.0f32);
        let mut precipitation = vec![0.0f64; texels];
        for _ in 0..p.moisture_iterations {
            let previous = carried.clone();
            for (k, d) in directions.iter().enumerate() {
                let (east, north) = frames[k];
                let flow =
                    east * f64::from(wind_east.data()[k]) + north * f64::from(wind_north.data()[k]);
                let source = *d - flow * step;
                let mut upwind = previous.bilinear(source.normalize());
                if p.moisture_spread > 0.0 {
                    let side = 0.5 * step;
                    let lateral: f64 = [east, -east, north, -north]
                        .iter()
                        .map(|offset| previous.bilinear((source + *offset * side).normalize()))
                        .sum();
                    upwind =
                        (1.0 - p.moisture_spread) * upwind + p.moisture_spread * 0.25 * lateral;
                }
                let evaporate = if elevation.data()[k] < 0.0 {
                    p.evaporation * evaporation_factor(f64::from(temperature.data()[k]))
                } else {
                    0.0
                };
                let rain = upwind * p.rain * rain_modulation(d.dot(inputs.pole), p);
                carried.data_mut()[k] = (upwind + evaporate - rain) as f32;
                precipitation[k] += rain;
            }
        }
        // Over the sea too: rain falls there as well, and a fixed 1.0 on ocean
        // texels would bleed into coastal land wherever colour interpolates
        // between world-map texels (wet rims along every coast).
        for (value, rain) in moisture.data_mut().iter_mut().zip(&precipitation) {
            *value = (1.0 - (-rain / p.precipitation_scale).exp()) as f32;
        }
    }

    Ok(TierAFields {
        elevation,
        temperature,
        moisture,
        wind_east,
        wind_north,
        sea_level,
        noise_low,
        noise_high,
        ocean_fraction: ocean_weight as f64 / total_weight as f64,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::archetype::tests::terra;

    fn inputs(n: usize, seed: u64) -> TierAInputs {
        let archetype = terra();
        TierAInputs {
            params: archetype.sample(seed),
            stages: archetype.stages.clone(),
            radius_m: 338_950.0,
            pole: DVec3::Y,
            face_cells: n,
        }
    }

    fn area_mean(fields: &CubeMap<f32>, n: usize, keep: impl Fn(usize) -> bool) -> f64 {
        let (mut sum, mut weight) = (0.0, 0.0);
        for (k, v) in fields.data().iter().enumerate() {
            if keep(k) {
                let (u, w) = texel_uv(k % n, (k / n) % n, n);
                let a = f64::from(area_weight(u, w));
                sum += f64::from(*v) * a;
                weight += a;
            }
        }
        sum / weight
    }

    #[test]
    fn ocean_coverage_is_exact_and_bake_is_deterministic() {
        let input = inputs(64, 7);
        let fields = bake(&input).unwrap();
        // §19.4: ocean coverage within ±1 % of the target.
        assert!(
            (fields.ocean_fraction - input.params.ocean_coverage).abs() < 0.01,
            "{} vs {}",
            fields.ocean_fraction,
            input.params.ocean_coverage
        );
        assert_eq!(fields, bake(&input).unwrap());
        let other = bake(&inputs(64, 8)).unwrap();
        assert_ne!(fields.elevation, other.elevation);
        let (low, high) = fields
            .elevation
            .data()
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), h| (a.min(*h), b.max(*h)));
        assert!(low < -1000.0 && high > 500.0, "{low} {high}");
        assert!(f64::from(high) <= input.params.land_height_m + 1e-3);
        assert!(f64::from(low) >= -input.params.ocean_depth_m - 1e-3);
    }

    #[test]
    fn temperature_falls_with_latitude_and_altitude() {
        let n = 64;
        let input = inputs(n, 7);
        let fields = bake(&input).unwrap();
        let directions: Vec<DVec3> = (0..6 * n * n)
            .map(|k| texel_direction(k / (n * n), k % n, (k / n) % n, n))
            .collect();
        let band = |lo: f64, hi: f64| {
            area_mean(&fields.temperature, n, |k| {
                let x = directions[k].dot(DVec3::Y).abs();
                (lo..hi).contains(&x)
            })
        };
        let (tropics, mid, polar) = (band(0.0, 0.3), band(0.4, 0.7), band(0.85, 1.0));
        assert!(tropics > mid && mid > polar, "{tropics} {mid} {polar}");
        assert!(tropics - polar > 25.0);
        // Altitude, on the baked field: without the lapse rate every land
        // texel is warmer by lapse · height, scaled by the ocean moderation
        // factor 1 - moderation · mask (mask in 0..1); oceans are unchanged.
        let mut flat = input.clone();
        flat.params.lapse_c_per_km = 0.0;
        let without = bake(&flat).unwrap();
        let lapse = input.params.lapse_c_per_km;
        let mut high_land = 0;
        for k in 0..6 * n * n {
            let h = f64::from(fields.elevation.data()[k]);
            let cooling =
                f64::from(without.temperature.data()[k]) - f64::from(fields.temperature.data()[k]);
            let full = lapse * h.max(0.0) / 1000.0;
            let least = full * (1.0 - input.params.ocean_moderation);
            assert!(
                cooling <= full + 1e-4 && cooling >= least - 1e-4,
                "texel {k}: cooled {cooling}, expected within [{least}, {full}]"
            );
            if h > 1000.0 {
                high_land += 1;
            }
        }
        assert!(high_land > 0, "the bake has land above 1 km");
    }

    #[test]
    fn circulation_cells_give_a_wet_equator_and_dry_subtropics() {
        let n = 64;
        let base = inputs(n, 7);
        let directions: Vec<DVec3> = (0..6 * n * n)
            .map(|k| texel_direction(k / (n * n), k % n, (k / n) % n, n))
            .collect();
        // Mean moisture over all texels in a latitude band (degrees).
        let band = |fields: &TierAFields, lo: f64, hi: f64| {
            area_mean(&fields.moisture, n, |k| {
                let latitude = directions[k].dot(base.pole).abs().asin().to_degrees();
                (lo..hi).contains(&latitude)
            })
        };
        let with = bake(&base).unwrap();
        let mut flat = base.clone();
        flat.params.rain_convergence = 0.0;
        let without = bake(&flat).unwrap();
        let gap = |f: &TierAFields| band(f, 0.0, 10.0) - band(f, 25.0, 35.0);
        assert!(gap(&with) > 0.0, "equator wetter than the subtropics");
        assert!(
            gap(&with) > gap(&without) + 0.05,
            "convergence widens the gap: {} vs {}",
            gap(&with),
            gap(&without)
        );
        assert_eq!(with.elevation, without.elevation, "moisture only");
    }

    #[test]
    fn coasts_are_wetter_than_continental_interiors() {
        let n = 64;
        let fields = bake(&inputs(n, 7)).unwrap();
        let land = |k: usize| fields.elevation.data()[k] >= 0.0;
        let ocean = |k: usize| fields.elevation.data()[k] < 0.0;
        let neighbours = |k: usize| crate::terrain::world_map::neighbours(k, n);
        let coastal: Vec<usize> = (0..6 * n * n)
            .filter(|&k| land(k) && neighbours(k).iter().any(|&m| ocean(m)))
            .collect();
        // Interior: land whose 2-ring has no ocean.
        let interior: Vec<usize> = (0..6 * n * n)
            .filter(|&k| {
                land(k)
                    && neighbours(k)
                        .iter()
                        .all(|&m| land(m) && neighbours(m).iter().all(|&q| land(q)))
            })
            .collect();
        assert!(!coastal.is_empty() && !interior.is_empty());
        let mean = |set: &[usize]| {
            set.iter()
                .map(|&k| f64::from(fields.moisture.data()[k]))
                .sum::<f64>()
                / set.len() as f64
        };
        assert!(
            mean(&coastal) > mean(&interior) + 0.05,
            "{} {}",
            mean(&coastal),
            mean(&interior)
        );
        assert!(
            fields
                .moisture
                .data()
                .iter()
                .all(|m| (0.0..=1.0).contains(m))
        );
    }

    #[test]
    fn stage_seeds_are_independent() {
        let base = inputs(32, 7);
        let mut wetter = base.clone();
        wetter.params.set("rain", base.params.rain * 0.5).unwrap();
        let a = bake(&base).unwrap();
        let b = bake(&wetter).unwrap();
        assert_eq!(a.elevation, b.elevation);
        assert_eq!(a.temperature, b.temperature);
        assert_ne!(a.moisture, b.moisture);
    }

    #[test]
    fn wind_bands_follow_three_cell_circulation() {
        let params = inputs(4, 1).params;
        let at = |deg: f64| wind(deg.to_radians().sin(), &params);
        assert!(at(15.0).0 < 0.0 && at(15.0).1 < 0.0); // trades: easterly, equatorward
        assert!(at(45.0).0 > 0.0 && at(45.0).1 > 0.0); // westerlies, poleward
        assert!(at(75.0).0 < 0.0); // polar easterlies
        assert!(at(-15.0).1 > 0.0); // southern trades blow north
        assert!(insolation(0.0, 23.45_f64.to_radians()) > insolation(1.0, 23.45_f64.to_radians()));
    }
}
