//! Converts world Tier A bake inputs (f64, metres) into the renderer's GPU bake
//! inputs (f32, radians and cycles per unit direction), narrowed once.
use astrum_renderer::{
    AtlasWorldSurface,
    tier_a::{TIER_A_MAX_PLATES, TierABakeInputs, stage},
};
use astrum_world::terrain::{
    archetype::TierAStage,
    landform,
    tier_a::{
        OCEAN_BLUR_PASSES, TierAInputs, climate::smoothing_step_rad, erosion, height_bound_m,
        tectonics,
    },
    world_field::WorldField,
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
            TierAStage::Tectonics => stage::TECTONICS,
            TierAStage::RainShadow => stage::RAIN_SHADOW,
            TierAStage::Erosion => stage::EROSION,
        })
        .fold(0, |a, b| a | b);
    let tectonics_on = inputs.has(TierAStage::Tectonics);
    let set = tectonics::plates(p, p.tectonic_seed);
    let mut plates = [[0.0f32; 16]; TIER_A_MAX_PLATES];
    for (slot, plate) in plates
        .iter_mut()
        .zip(set.packed().chunks(16).take(TIER_A_MAX_PLATES))
    {
        slot.copy_from_slice(plate);
    }
    let erosion = erosion::ErosionConstants::new(p);
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
        rain_convergence: p.rain_convergence as f32,
        pole: inputs.pole.as_vec3().to_array(),
        height_bound_m: height_bound_m(p, &inputs.stages) as f32,
        flow_normaliser: erosion::flow_normaliser(r) as f32,
        crust_weight: if tectonics_on {
            p.crust_weight as f32
        } else {
            0.0
        },
        tectonic_seed: p.tectonic_seed,
        hardness_seed: p.hardness_seed,
        plate_count: set.plates.len().min(TIER_A_MAX_PLATES) as u32,
        plates,
        plate_warp_frequency: (r / p.plate_warp_wavelength_m) as f32,
        plate_warp_scale: (p.plate_warp_strength * p.plate_warp_wavelength_m / r) as f32,
        boundary_softness_m: p.boundary_softness_m as f32,
        boundary_clamp_m: p.boundary_clamp_m as f32,
        collision_height_m: p.collision_height_m as f32,
        arc_height_m: p.arc_height_m as f32,
        trench_depth_m: p.trench_depth_m as f32,
        ridge_height_m: p.ridge_height_m as f32,
        rift_depth_m: p.rift_depth_m as f32,
        transform_height_m: p.transform_height_m as f32,
        orogen_roughness: p.orogen_roughness as f32,
        roughness_frequency: (r / p.orogen_roughness_wavelength_m) as f32,
        orogen_width_m: p.orogen_width_m as f32,
        arc_width_m: p.arc_width_m as f32,
        arc_offset_m: p.arc_offset_m as f32,
        trench_width_m: p.trench_width_m as f32,
        ridge_width_m: p.ridge_width_m as f32,
        rift_width_m: p.rift_width_m as f32,
        transform_width_m: p.transform_width_m as f32,
        crust_width_m: p.crust_width_m as f32,
        hardness_noise: p.hardness_noise as f32,
        hardness_noise_frequency: (r / p.hardness_noise_wavelength_m) as f32,
        junction_blend_m: p.junction_blend_m as f32,
        smoothing_step_rad: smoothing_step_rad(p, r) as f32,
        wind_deflection: p.wind_deflection as f32,
        deflection_slope: p.deflection_slope as f32,
        wind_slowdown: p.wind_slowdown as f32,
        orographic_rain: p.orographic_rain as f32,
        lee_drying: p.lee_drying as f32,
        orographic_slope: p.orographic_slope as f32,
        erosion_strength: erosion.strength as f32,
        erosion_uplift_m: erosion.uplift_m as f32,
        tan_talus: erosion.tan_talus as f32,
        deposition: erosion.deposition as f32,
        erosion_capacity: erosion.capacity as f32,
        erosion_area_exponent: erosion.m as f32,
        erosion_slope_exponent: erosion.n as f32,
        erosion_flow_exponent: erosion.p as f32,
        thermal_rate: erosion.thermal as f32,
        sediment_depth_m: p.sediment_depth_m as f32,
        erosion_cascade: p
            .erosion_cascade
            .map(|(divisor, iterations)| [divisor, iterations]),
        landform_rules: inputs.landform_rules.clone(),
    }
}

/// Colour constants and the packed landform set of a world-map body for the
/// GPU producer, narrowed once.
pub fn surface(field: &WorldField) -> anyhow::Result<AtlasWorldSurface> {
    let look = field.look();
    let landforms = match field.landforms() {
        Some(l) => landform::gpu::pack_set(&l.set, &l.params)?,
        None => Vec::new(),
    };
    let v3 = |c: [f64; 3]| c.map(|v| v as f32);
    Ok(AtlasWorldSurface {
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
        landforms,
    })
}
