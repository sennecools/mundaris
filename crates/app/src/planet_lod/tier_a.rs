//! Converts world Tier A bake inputs (f64, metres) into the renderer's GPU bake
//! inputs (f32, radians and cycles per unit direction), narrowed once.
use astrum_renderer::{
    AtlasWorldSurface,
    tier_a::{TierABakeInputs, stage},
};
use astrum_world::terrain::{
    archetype::TierAStage,
    tier_a::{OCEAN_BLUR_PASSES, TierAInputs},
    world_field::WorldLook,
};

pub fn bake_inputs(inputs: &TierAInputs) -> TierABakeInputs {
    let p = &inputs.params;
    let r = inputs.radius_m;
    let stages = inputs
        .stages
        .iter()
        .map(|s| match s {
            TierAStage::Continents => stage::CONTINENTS,
            TierAStage::SeaLevel => stage::SEA_LEVEL,
            TierAStage::Shelf => stage::SHELF,
            TierAStage::Temperature => stage::TEMPERATURE,
            TierAStage::Wind => stage::WIND,
            TierAStage::Moisture => stage::MOISTURE,
        })
        .fold(0, |a, b| a | b);
    TierABakeInputs {
        face_cells: inputs.face_cells as u32,
        stages,
        radius_m: r as f32,
        continent_frequency: (r / p.continent_wavelength_m) as f32,
        warp_frequency: (r / p.warp_wavelength_m) as f32,
        warp_scale: (p.warp_strength * p.continent_wavelength_m / r) as f32,
        octaves: p.continent_octaves,
        lacunarity: p.continent_lacunarity as f32,
        gain: p.continent_gain as f32,
        continent_seed: p.continent_seed,
        warp_seed: p.warp_seed,
        temperature_seed: p.temperature_seed,
        ocean_coverage: p.ocean_coverage as f32,
        land_height_m: p.land_height_m as f32,
        land_exponent: p.land_exponent as f32,
        ocean_depth_m: p.ocean_depth_m as f32,
        shelf_depth_m: p.shelf_depth_m as f32,
        shelf_fraction: p.shelf_fraction as f32,
        equator_c: p.equator_c as f32,
        pole_c: p.pole_c as f32,
        axial_tilt_rad: p.axial_tilt_deg.to_radians() as f32,
        lapse_c_per_km: p.lapse_c_per_km as f32,
        ocean_moderation: p.ocean_moderation as f32,
        ocean_blur_step_rad: (p.ocean_blur_m / f64::from(OCEAN_BLUR_PASSES).sqrt() / r) as f32,
        temperature_noise_c: p.temperature_noise_c as f32,
        temperature_noise_frequency: (r / p.temperature_noise_wavelength_m) as f32,
        wind_cells: p.wind_cells as f32,
        wind_meridional: p.wind_meridional as f32,
        evaporation: p.evaporation as f32,
        rain: p.rain as f32,
        moisture_iterations: p.moisture_iterations,
        moisture_step_rad: (p.moisture_step_m / r) as f32,
        moisture_spread: p.moisture_spread as f32,
        precipitation_scale: p.precipitation_scale as f32,
        pole: inputs.pole.as_vec3().to_array(),
    }
}

/// Colour constants of a world-map body for the GPU producer, narrowed once.
pub fn surface(look: &WorldLook) -> AtlasWorldSurface {
    let v3 = |c: [f64; 3]| c.map(|v| v as f32);
    AtlasWorldSurface {
        lut_size: look.lut.size,
        lut_srgb: look.lut.srgb.to_vec(),
        temperature_c: [
            look.lut.temperature_c.0 as f32,
            look.lut.temperature_c.1 as f32,
        ],
        moisture: [look.lut.moisture.0 as f32, look.lut.moisture.1 as f32],
        snow_temperature_c: look.snow_temperature_c as f32,
        snow_blend_c: look.snow_blend_c as f32,
        snow_slope_rad: [look.snow_slope_rad.0 as f32, look.snow_slope_rad.1 as f32],
        snow_albedo: v3(look.snow_albedo),
        water_shallow: v3(look.water_shallow),
        water_deep: v3(look.water_deep),
        water_depth_scale_m: look.water_depth_scale_m as f32,
        climate_temperature_c: look
            .climate
            .as_ref()
            .map_or(0.0, |c| c.temperature_c as f32),
        climate_moisture: look.climate.as_ref().map_or(0.0, |c| c.moisture as f32),
    }
}
