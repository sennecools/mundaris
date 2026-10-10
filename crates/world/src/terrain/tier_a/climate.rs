//! Tier A climate (`docs/ASTRUM_TERRAIN_PIPELINE.md` §6.5.4–6.5.6), CPU oracle:
//! temperature, three-cell wind bands, the ocean mask diffused inland and
//! moisture advection (M1), plus the M2 (Shape) rain shadow: relief smoothed
//! over about one moisture step, its slope, wind deflected around it (then
//! blurred as a vector field) and orographic rain with lee drying in the
//! moisture step, on top of M1's circulation-cell rain belts.
use super::{OCEAN_BLUR_PASSES, RELIEF_SMOOTH_PASSES, TierAInputs, WIND_BLUR_PASSES, fbm};
use crate::terrain::{
    archetype::{PlanetParams, TierAStage},
    world_map::CubeMap,
};
use glam::DVec3;

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

/// Rain-out share of the carried moisture in one step: the circulation-cell
/// rate plus orographic rain on uphill wind, reduced by lee drying on downhill
/// wind. `uphill = w'·∇h_s` (wind times smoothed slope, m/m); `None` without
/// a rain shadow (M1).
pub fn rain_rate(x: f64, uphill: Option<f64>, params: &PlanetParams) -> f64 {
    let base = params.rain * rain_modulation(x, params);
    let Some(uphill) = uphill else {
        return base;
    };
    let reference = params.orographic_slope;
    let orographic = params.orographic_rain * uphill.max(0.0) / reference;
    let lee = (params.lee_drying * (-uphill).max(0.0) / reference).min(1.0);
    ((base + orographic) * (1.0 - lee)).min(1.0)
}

/// Wind `(east, north)` deflected around relief with slope `(ge, gn)` (m/m,
/// uphill): the uphill component is turned along the relief by
/// `deflection · s`, `s = |g| / (|g| + g₀)`, and the speed scaled to
/// `|w|·(1 − slowdown · s)`.
pub fn deflect(w: (f64, f64), g: (f64, f64), params: &PlanetParams) -> (f64, f64) {
    let length = (g.0 * g.0 + g.1 * g.1).sqrt();
    if length <= 0.0 {
        return w;
    }
    let (ue, un) = (g.0 / length, g.1 / length);
    let s = length / (length + params.deflection_slope);
    let along = (w.0 * ue + w.1 * un).max(0.0) * params.wind_deflection * s;
    let (de, dn) = (w.0 - along * ue, w.1 - along * un);
    let speed = (w.0 * w.0 + w.1 * w.1).sqrt() * (1.0 - params.wind_slowdown * s);
    let deflected = (de * de + dn * dn).sqrt();
    if deflected <= 1e-12 {
        return (0.0, 0.0);
    }
    (de * speed / deflected, dn * speed / deflected)
}

/// Texel geometry shared by the climate stages.
pub struct Geometry<'a> {
    pub n: usize,
    pub radius_m: f64,
    pub pole: DVec3,
    pub directions: &'a [DVec3],
    pub frames: &'a [(DVec3, DVec3)],
}

/// One pass of the ocean-blur kernel: the texel and four bilinear samples
/// `step` radians east, west, north and south, averaged.
fn blur(geometry: &Geometry, field: &CubeMap<f32>, step: f64) -> CubeMap<f32> {
    let mut out = CubeMap::new(geometry.n, 0.0f32);
    super::par_fill(out.data_mut(), |k| {
        let d = geometry.directions[k];
        let (east, north) = geometry.frames[k];
        let mut sum = f64::from(field.data()[k]);
        for offset in [east, -east, north, -north] {
            sum += field.bilinear((d + offset * step).normalize());
        }
        (sum / 5.0) as f32
    });
    out
}

/// Step (rad) of relief smoothing and wind blur: `RELIEF_SMOOTH_PASSES`
/// passes diffuse over about one moisture step.
pub fn smoothing_step_rad(params: &PlanetParams, radius_m: f64) -> f64 {
    params.moisture_step_m / f64::from(RELIEF_SMOOTH_PASSES).sqrt() / radius_m
}

/// Smoothed relief `h_s` (passes of the ocean-blur kernel on `max(h, 0)`) and
/// its slope east and north (m/m, central differences one step apart).
pub fn relief_slope(
    geometry: &Geometry,
    elevation: &CubeMap<f32>,
    params: &PlanetParams,
) -> (CubeMap<f32>, CubeMap<f32>) {
    let step = smoothing_step_rad(params, geometry.radius_m);
    let mut smooth = CubeMap::new(geometry.n, 0.0f32);
    for (s, h) in smooth.data_mut().iter_mut().zip(elevation.data()) {
        *s = h.max(0.0);
    }
    for _ in 0..RELIEF_SMOOTH_PASSES {
        smooth = blur(geometry, &smooth, step);
    }
    let mut east_slope = CubeMap::new(geometry.n, 0.0f32);
    let mut north_slope = CubeMap::new(geometry.n, 0.0f32);
    let scale = 1.0 / (2.0 * step * geometry.radius_m);
    let slope_of = |k: usize, axis: DVec3| {
        let d = geometry.directions[k];
        (smooth.bilinear((d + axis * step).normalize())
            - smooth.bilinear((d - axis * step).normalize()))
            * scale
    };
    super::par_fill(east_slope.data_mut(), |k| {
        slope_of(k, geometry.frames[k].0) as f32
    });
    super::par_fill(north_slope.data_mut(), |k| {
        slope_of(k, geometry.frames[k].1) as f32
    });
    (east_slope, north_slope)
}

/// Ocean mask (`h < 0`) diffused inland for temperature moderation.
pub fn ocean_mask(
    geometry: &Geometry,
    elevation: &CubeMap<f32>,
    params: &PlanetParams,
) -> CubeMap<f32> {
    let mut ocean = CubeMap::new(geometry.n, 0.0f32);
    for (o, h) in ocean.data_mut().iter_mut().zip(elevation.data()) {
        *o = if *h < 0.0 { 1.0 } else { 0.0 };
    }
    if params.ocean_blur_m > 0.0 {
        let step = params.ocean_blur_m / f64::from(OCEAN_BLUR_PASSES).sqrt() / geometry.radius_m;
        for _ in 0..OCEAN_BLUR_PASSES {
            ocean = blur(geometry, &ocean, step);
        }
    }
    ocean
}

/// Temperature from latitude, lapse rate over `elevation`, ocean moderation
/// and noise (constant mid value without the temperature stage).
pub fn temperature(
    geometry: &Geometry,
    elevation: &CubeMap<f32>,
    ocean: &CubeMap<f32>,
    inputs: &TierAInputs,
) -> CubeMap<f32> {
    let p = &inputs.params;
    let middle = 0.5 * (p.equator_c + p.pole_c);
    let mut temperature = CubeMap::new(geometry.n, middle as f32);
    if inputs.has(TierAStage::Temperature) {
        let noise_frequency = geometry.radius_m / p.temperature_noise_wavelength_m;
        super::par_fill(temperature.data_mut(), |k| {
            let d = geometry.directions[k];
            let x = d.dot(geometry.pole);
            let h = f64::from(elevation.data()[k]);
            let t = base_temperature(x, p) - p.lapse_c_per_km * h.max(0.0) / 1000.0;
            let moderated =
                middle + (t - middle) * (1.0 - p.ocean_moderation * f64::from(ocean.data()[k]));
            let noise =
                p.temperature_noise_c * fbm(d, noise_frequency, 3, 2.0, 0.5, p.temperature_seed);
            (moderated + noise) as f32
        });
    }
    temperature
}

/// Three-cell wind bands (zero without the wind stage).
pub fn wind_bands(geometry: &Geometry, inputs: &TierAInputs) -> (CubeMap<f32>, CubeMap<f32>) {
    let mut east = CubeMap::new(geometry.n, 0.0f32);
    let mut north = CubeMap::new(geometry.n, 0.0f32);
    if inputs.has(TierAStage::Wind) {
        for (k, d) in geometry.directions.iter().enumerate() {
            let (e, n) = wind(d.dot(geometry.pole), &inputs.params);
            east.data_mut()[k] = e as f32;
            north.data_mut()[k] = n as f32;
        }
    }
    (east, north)
}

/// Wind deflected around the smoothed relief, then `WIND_BLUR_PASSES` vector
/// blur passes: each neighbour sample is taken as a 3-D tangent vector in its
/// own east/north frame, averaged and projected onto the texel's frame.
pub fn deflect_wind(
    geometry: &Geometry,
    wind_east: &CubeMap<f32>,
    wind_north: &CubeMap<f32>,
    slope_east: &CubeMap<f32>,
    slope_north: &CubeMap<f32>,
    params: &PlanetParams,
) -> (CubeMap<f32>, CubeMap<f32>) {
    let n = geometry.n;
    let mut east = CubeMap::new(n, 0.0f32);
    let mut north = CubeMap::new(n, 0.0f32);
    for k in 0..6 * n * n {
        let w = (
            f64::from(wind_east.data()[k]),
            f64::from(wind_north.data()[k]),
        );
        let g = (
            f64::from(slope_east.data()[k]),
            f64::from(slope_north.data()[k]),
        );
        let (e, nn) = deflect(w, g, params);
        east.data_mut()[k] = e as f32;
        north.data_mut()[k] = nn as f32;
    }
    let step = smoothing_step_rad(params, geometry.radius_m);
    for _ in 0..WIND_BLUR_PASSES {
        let mut both = vec![(0.0f32, 0.0f32); 6 * n * n];
        super::par_fill(&mut both, |k| {
            let d = geometry.directions[k];
            let (fe, fn_) = geometry.frames[k];
            let mut sum = fe * f64::from(east.data()[k]) + fn_ * f64::from(north.data()[k]);
            for offset in [fe, -fe, fn_, -fn_] {
                let s = (d + offset * step).normalize();
                let (se, sn) = tangent_frame(s, geometry.pole);
                sum += se * east.bilinear(s) + sn * north.bilinear(s);
            }
            let v = sum / 5.0;
            (v.dot(fe) as f32, v.dot(fn_) as f32)
        });
        for (k, (e, nn)) in both.into_iter().enumerate() {
            east.data_mut()[k] = e;
            north.data_mut()[k] = nn;
        }
    }
    (east, north)
}

/// Fields the moisture advection reads.
pub struct MoistureInputs<'a> {
    pub elevation: &'a CubeMap<f32>,
    pub temperature: &'a CubeMap<f32>,
    pub wind_east: &'a CubeMap<f32>,
    pub wind_north: &'a CubeMap<f32>,
    /// Smoothed relief slope east/north for the rain shadow.
    pub slope: Option<(&'a CubeMap<f32>, &'a CubeMap<f32>)>,
}

/// Moisture: semi-Lagrangian advection, ocean evaporation, rain by the
/// circulation cells plus the rain shadow; `1 − exp(−precipitation / scale)`
/// (0.5 without the moisture stage).
pub fn moisture(
    geometry: &Geometry,
    inputs: &TierAInputs,
    fields: &MoistureInputs,
) -> CubeMap<f32> {
    let n = geometry.n;
    let p = &inputs.params;
    let mut moisture = CubeMap::new(n, 0.5f32);
    if !inputs.has(TierAStage::Moisture) {
        return moisture;
    }
    let step = p.moisture_step_m / geometry.radius_m;
    let mut carried = CubeMap::new(n, 0.0f32);
    let mut precipitation = vec![0.0f64; 6 * n * n];
    for _ in 0..p.moisture_iterations {
        let previous = carried.clone();
        let mut step_out = vec![(0.0f32, 0.0f64); 6 * n * n];
        super::par_fill(&mut step_out, |k| {
            let d = geometry.directions[k];
            let (east, north) = geometry.frames[k];
            let (we, wn) = (
                f64::from(fields.wind_east.data()[k]),
                f64::from(fields.wind_north.data()[k]),
            );
            let flow = east * we + north * wn;
            let source = d - flow * step;
            let mut upwind = previous.bilinear(source.normalize());
            if p.moisture_spread > 0.0 {
                let side = 0.5 * step;
                let lateral: f64 = [east, -east, north, -north]
                    .iter()
                    .map(|offset| previous.bilinear((source + *offset * side).normalize()))
                    .sum();
                upwind = (1.0 - p.moisture_spread) * upwind + p.moisture_spread * 0.25 * lateral;
            }
            let evaporate = if fields.elevation.data()[k] < 0.0 {
                p.evaporation * evaporation_factor(f64::from(fields.temperature.data()[k]))
            } else {
                0.0
            };
            let uphill = fields
                .slope
                .map(|(se, sn)| we * f64::from(se.data()[k]) + wn * f64::from(sn.data()[k]));
            let rain = upwind * rain_rate(d.dot(geometry.pole), uphill, p);
            ((upwind + evaporate - rain) as f32, rain)
        });
        for (k, (value, rain)) in step_out.into_iter().enumerate() {
            carried.data_mut()[k] = value;
            precipitation[k] += rain;
        }
    }
    // Over the sea too: rain falls there as well, and a fixed 1.0 on ocean
    // texels would bleed into coastal land wherever colour interpolates
    // between world-map texels (wet rims along every coast).
    for (value, rain) in moisture.data_mut().iter_mut().zip(&precipitation) {
        *value = (1.0 - (-rain / p.precipitation_scale).exp()) as f32;
    }
    moisture
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::tier_a::{
        bake_full,
        tests::{area_mean, inputs},
    };

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

    #[test]
    fn deflection_turns_uphill_wind_and_keeps_downhill_wind() {
        let params = inputs(4, 1).params;
        // Wind straight up a steep slope: turned and slowed.
        let (e, n) = deflect((1.0, 0.0), (0.2, 0.0), &params);
        assert!(e < 1.0 && (e * e + n * n).sqrt() < 1.0);
        // Downhill wind keeps its direction.
        let (e, n) = deflect((-1.0, 0.0), (0.2, 0.0), &params);
        assert!(e < 0.0 && n.abs() < 1e-12);
        // Flat ground: unchanged.
        assert_eq!(deflect((0.3, -0.4), (0.0, 0.0), &params), (0.3, -0.4));
    }

    /// Rain shadow (§19.4, M2 design §6): on high-uplift land, texels whose
    /// wind blows downhill (lee) are drier than those whose wind blows uphill
    /// (windward), and the gap shrinks by at least 80 % without orographic
    /// rain and lee drying.
    #[test]
    fn rain_shadow_dries_the_lee_of_ranges() {
        let n = 64;
        let base = inputs(n, 7);
        let gap = |input: &TierAInputs, reference: &crate::terrain::tier_a::TierAOutput| {
            let output = bake_full(input).unwrap();
            let d = &reference.diagnostics;
            let f = &reference.fields;
            let uphill = |k: usize| {
                f64::from(f.wind_east.data()[k]) * f64::from(d.slope_east.data()[k])
                    + f64::from(f.wind_north.data()[k]) * f64::from(d.slope_north.data()[k])
            };
            let flank = |k: usize| d.uplift.data()[k] > 0.5 && d.pre_erosion.data()[k] > 0.0;
            let windward = area_mean(&output.fields.moisture, n, |k| flank(k) && uphill(k) > 1e-3);
            let lee = area_mean(&output.fields.moisture, n, |k| {
                flank(k) && uphill(k) < -1e-3
            });
            (windward, lee, windward - lee)
        };
        let reference = bake_full(&base).unwrap();
        let with = gap(&base, &reference);
        let mut flat = base.clone();
        flat.params.orographic_rain = 0.0;
        flat.params.lee_drying = 0.0;
        let without = gap(&flat, &reference);
        println!("rain shadow: windward/lee/gap with {with:?}, without {without:?}");
        assert!(with.1 < with.0, "lee drier than windward");
        assert!(
            without.2.abs() <= 0.2 * with.2,
            "gap shrinks by 80 %: {} vs {}",
            without.2,
            with.2
        );
    }
}
