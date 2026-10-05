//! Deterministic real-Solar-System content and initial conditions.
use glam::DVec3;
use mundaris_math::*;
use mundaris_simulation::GRAVITATIONAL_CONSTANT_M3_KG_S2 as G;
use mundaris_world::{terrain::*, *};
use std::{f64::consts::TAU, num::NonZeroU64};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SolarBody {
    Sun,
    Mercury,
    Venus,
    Earth,
    Moon,
    Mars,
    Jupiter,
    Saturn,
    Uranus,
    Neptune,
}

/// Catalogue values are authoring facts, independent of transient world body IDs.
#[derive(Debug, Clone, Copy)]
pub struct SolarBodyContent {
    pub identity: SolarBody,
    pub name: &'static str,
    pub real_mean_radius_m: f64,
    /// Reference surface gravity; gas giants use cloud-top reference gravity.
    pub reference_gravity_m_s2: f64,
    pub rotation_period_s: f64,
    pub axial_tilt_rad: f64,
    /// Descriptive orbital grouping only; not frame or integration ancestry.
    pub orbit_parent: Option<SolarBody>,
    pub real_orbital_distance_m: f64,
    pub orbital_phase_rad: f64,
    pub orbital_inclination_rad: f64,
    pub color: [f32; 4],
    /// Earth retains the checkpoint elevation diagnostic; other rocky bodies use
    /// their identifying sphere color until final materials exist.
    pub terrain_elevation_diagnostic: bool,
    pub rocky_terrain_seed: Option<u64>,
}

pub const SOLAR_SYSTEM_CONTENT: [SolarBodyContent; 10] = [
    entry(
        SolarBody::Sun,
        "Sun",
        6.957e8,
        274.0,
        25.05 * 86400.0,
        7.25_f64.to_radians(),
        None,
        0.0,
        0.0,
        0.0,
        [1.0, 0.68, 0.24, 1.0],
        None,
    ),
    entry(
        SolarBody::Mercury,
        "Mercury",
        2.4397e6,
        3.70,
        58.646 * 86400.0,
        0.034_f64.to_radians(),
        Some(SolarBody::Sun),
        5.7909e10,
        0.15,
        0.122,
        [0.58, 0.55, 0.52, 1.0],
        Some(0x4d455243),
    ),
    entry(
        SolarBody::Venus,
        "Venus",
        6.0518e6,
        8.87,
        243.025 * 86400.0,
        177.36_f64.to_radians(),
        Some(SolarBody::Sun),
        1.0821e11,
        2.1,
        0.059,
        [0.91, 0.68, 0.37, 1.0],
        Some(0x56454e55),
    ),
    entry(
        SolarBody::Earth,
        "Earth",
        6.371e6,
        9.80665,
        86164.0905,
        23.439_f64.to_radians(),
        Some(SolarBody::Sun),
        1.495978707e11,
        1.2,
        0.0,
        [0.20, 0.48, 0.88, 1.0],
        Some(0x45415254),
    ),
    entry(
        SolarBody::Moon,
        "Moon",
        1.7374e6,
        1.62,
        27.321661 * 86400.0,
        6.68_f64.to_radians(),
        Some(SolarBody::Earth),
        3.844e8,
        0.73,
        5.145_f64.to_radians(),
        [0.68, 0.67, 0.64, 1.0],
        Some(0x4d4f4f4e),
    ),
    entry(
        SolarBody::Mars,
        "Mars",
        3.3895e6,
        3.72076,
        88642.6848,
        25.19_f64.to_radians(),
        Some(SolarBody::Sun),
        2.2794e11,
        3.4,
        1.850_f64.to_radians(),
        [0.78, 0.31, 0.19, 1.0],
        Some(0x4d415253),
    ),
    entry(
        SolarBody::Jupiter,
        "Jupiter",
        6.9911e7,
        24.79,
        9.925 * 3600.0,
        3.13_f64.to_radians(),
        Some(SolarBody::Sun),
        7.7857e11,
        4.2,
        1.303_f64.to_radians(),
        [0.75, 0.58, 0.43, 1.0],
        None,
    ),
    entry(
        SolarBody::Saturn,
        "Saturn",
        5.8232e7,
        10.44,
        10.656 * 3600.0,
        26.73_f64.to_radians(),
        Some(SolarBody::Sun),
        1.4335e12,
        5.0,
        2.485_f64.to_radians(),
        [0.83, 0.74, 0.53, 1.0],
        None,
    ),
    entry(
        SolarBody::Uranus,
        "Uranus",
        2.5362e7,
        8.87,
        17.24 * 3600.0,
        97.77_f64.to_radians(),
        Some(SolarBody::Sun),
        2.8725e12,
        0.9,
        0.773_f64.to_radians(),
        [0.42, 0.76, 0.80, 1.0],
        None,
    ),
    entry(
        SolarBody::Neptune,
        "Neptune",
        2.4622e7,
        11.15,
        16.11 * 3600.0,
        28.32_f64.to_radians(),
        Some(SolarBody::Sun),
        4.4951e12,
        2.7,
        1.770_f64.to_radians(),
        [0.23, 0.34, 0.82, 1.0],
        None,
    ),
];

#[allow(clippy::too_many_arguments)]
const fn entry(
    identity: SolarBody,
    name: &'static str,
    real_mean_radius_m: f64,
    reference_gravity_m_s2: f64,
    rotation_period_s: f64,
    axial_tilt_rad: f64,
    orbit_parent: Option<SolarBody>,
    real_orbital_distance_m: f64,
    orbital_phase_rad: f64,
    orbital_inclination_rad: f64,
    color: [f32; 4],
    rocky_terrain_seed: Option<u64>,
) -> SolarBodyContent {
    SolarBodyContent {
        identity,
        name,
        real_mean_radius_m,
        reference_gravity_m_s2,
        rotation_period_s,
        axial_tilt_rad,
        orbit_parent,
        real_orbital_distance_m,
        orbital_phase_rad,
        orbital_inclination_rad,
        color,
        terrain_elevation_diagnostic: matches!(identity, SolarBody::Earth),
        rocky_terrain_seed,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SolarSystemPreset {
    pub body_radius_scale: f64,
    pub orbital_distance_scale: f64,
}

impl SolarSystemPreset {
    /// 400 km Earth radius and the same uniform baseline factor for orbital lengths.
    pub const fn gameplay() -> Self {
        let scale = 400_000.0 / 6_371_000.0;
        Self {
            body_radius_scale: scale,
            orbital_distance_scale: scale,
        }
    }
    /// Real mean radii and approximate real orbital lengths, not an ephemeris.
    pub const fn real_scale() -> Self {
        Self {
            body_radius_scale: 1.0,
            orbital_distance_scale: 1.0,
        }
    }

    /// Prescribed periods retain the original Newtonian preset's setup pacing.
    /// Base values are frozen from the original catalogue gravity/radius inputs;
    /// preset scaling follows the same radius/distance scaling as the setup orbit.
    pub fn orbital_period_s(self, body: SolarBody) -> f64 {
        let base_period_s = match body {
            SolarBody::Sun => 0.0,
            SolarBody::Mercury => 7_603_291.579_640_426,
            SolarBody::Venus => 19_421_516.906_539_556,
            SolarBody::Earth => 31_569_669.976_44,
            SolarBody::Moon => 2_359_041.447_532_198_4,
            SolarBody::Mars => 59_376_327.099_254_39,
            SolarBody::Jupiter => 374_654_349.471_237_1,
            SolarBody::Saturn => 936_313_251.368_567_2,
            SolarBody::Uranus => 2_656_215_444.752_847_7,
            SolarBody::Neptune => 5_199_727_113.931_417,
        };
        base_period_s * self.orbital_distance_scale.powf(1.5) / self.body_radius_scale
    }

    /// Construct complete prescribed motion without changing the legacy Newtonian
    /// initialization path or its state/terrain/spin authoring.
    pub fn create_analytic(
        self,
        namespace: NonZeroU64,
    ) -> anyhow::Result<(CelestialSystem, CelestialMotionDefinition)> {
        let system = self.create(namespace)?;
        let mut definitions = Vec::with_capacity(system.body_count());
        for (i, (id, body)) in system.bodies().enumerate() {
            let content = &SOLAR_SYSTEM_CONTENT[i];
            let translation = if content.identity == SolarBody::Sun {
                CelestialTranslation::Stationary(body.state().center_in_system())
            } else {
                let reference = system
                    .bodies()
                    .nth(index(content.orbit_parent.ok_or_else(|| {
                        anyhow::anyhow!("non-central body is missing its orbit reference")
                    })?))
                    .map(|(reference, _)| reference)
                    .ok_or_else(|| anyhow::anyhow!("orbit reference is missing"))?;
                let plane = UnitRotation::from_axis_angle(
                    Direction3::try_new(DVec3::X)?,
                    content.orbital_inclination_rad,
                )?;
                CelestialTranslation::Elliptic(EllipticOrbit::new(
                    reference,
                    content.real_orbital_distance_m * self.orbital_distance_scale,
                    0.0,
                    plane,
                    self.orbital_period_s(content.identity),
                    content.orbital_phase_rad,
                    SimulationInstant::ZERO,
                )?)
            };
            let tilt = UnitRotation::from_axis_angle(
                Direction3::try_new(DVec3::X)?,
                std::f64::consts::FRAC_PI_2 + content.axial_tilt_rad,
            )?;
            definitions.push(BodyMotion {
                body: id,
                translation,
                spin: AxialSpin::new(
                    tilt,
                    Direction3::try_new(DVec3::Y)?,
                    TAU / content.rotation_period_s,
                    SimulationInstant::ZERO,
                )?,
            });
        }
        let definition = CelestialMotionDefinition::new(&system, &definitions)?;
        Ok((system, definition))
    }

    pub fn create(self, namespace: NonZeroU64) -> anyhow::Result<CelestialSystem> {
        anyhow::ensure!(
            self.body_radius_scale.is_finite()
                && self.body_radius_scale > 0.0
                && self.orbital_distance_scale.is_finite()
                && self.orbital_distance_scale > 0.0,
            "solar-system scales must be finite and positive"
        );
        let radii: Vec<_> = SOLAR_SYSTEM_CONTENT
            .iter()
            .map(|b| b.real_mean_radius_m * self.body_radius_scale)
            .collect();
        anyhow::ensure!(
            radii.iter().all(|r| r.is_finite() && *r > 0.0),
            "scaled radius is not representable"
        );
        // Every body, including the Sun and gas giants, preserves its authored
        // surface/cloud-top reference gravity under either radius preset.
        let masses: Vec<_> = SOLAR_SYSTEM_CONTENT
            .iter()
            .zip(&radii)
            .map(|(b, r)| b.reference_gravity_m_s2 * r * r / G)
            .collect();
        anyhow::ensure!(
            masses.iter().all(|m| m.is_finite() && *m > 0.0),
            "body mass is not representable"
        );
        let mut positions = vec![DVec3::ZERO; SOLAR_SYSTEM_CONTENT.len()];
        let mut velocities = vec![DVec3::ZERO; SOLAR_SYSTEM_CONTENT.len()];

        // First author every Sun-centered planet orbit, treating Earth's satellite
        // as part of Earth's orbiting system mass. Orbit parent values do not create
        // hierarchy or constraints in the subsequent N-body simulation.
        for (i, body) in SOLAR_SYSTEM_CONTENT.iter().enumerate().skip(1) {
            if body.identity == SolarBody::Moon {
                continue;
            }
            let parent = index(body.orbit_parent.ok_or_else(|| {
                anyhow::anyhow!("non-central body is missing its orbit reference")
            })?);
            let distance = body.real_orbital_distance_m * self.orbital_distance_scale;
            anyhow::ensure!(
                distance.is_finite() && distance > 0.0,
                "scaled orbital distance is not representable"
            );
            let (radial, tangent) = orbital_axes(body);
            let system_mass = if body.identity == SolarBody::Earth {
                masses[i] + masses[index(SolarBody::Moon)]
            } else {
                masses[i]
            };
            let speed = (G * (masses[parent] + system_mass) / distance).sqrt();
            positions[i] = positions[parent] + radial * distance;
            velocities[i] = velocities[parent] + tangent * speed;
        }

        // Resolve the Earth/Moon relative circular orbit around the inner barycenter.
        let earth = index(SolarBody::Earth);
        let moon = index(SolarBody::Moon);
        let moon_content = &SOLAR_SYSTEM_CONTENT[moon];
        let moon_distance = moon_content.real_orbital_distance_m * self.orbital_distance_scale;
        anyhow::ensure!(
            moon_distance.is_finite() && moon_distance > 0.0,
            "scaled lunar distance is not representable"
        );
        let (moon_radial, moon_tangent) = orbital_axes(moon_content);
        let relative_speed = (G * (masses[earth] + masses[moon]) / moon_distance).sqrt();
        let inner_mass = masses[earth] + masses[moon];
        let barycenter_position = positions[earth];
        let barycenter_velocity = velocities[earth];
        positions[earth] =
            barycenter_position - moon_radial * (masses[moon] / inner_mass * moon_distance);
        velocities[earth] =
            barycenter_velocity - moon_tangent * (masses[moon] / inner_mass * relative_speed);
        positions[moon] =
            barycenter_position + moon_radial * (masses[earth] / inner_mass * moon_distance);
        velocities[moon] =
            barycenter_velocity + moon_tangent * (masses[earth] / inner_mass * relative_speed);

        // Translate all initial states to the global inertial barycentric frame.
        let total_mass: f64 = masses.iter().sum();
        let center = positions
            .iter()
            .zip(&masses)
            .fold(DVec3::ZERO, |sum, (p, m)| sum + *p * *m)
            / total_mass;
        let motion = velocities
            .iter()
            .zip(&masses)
            .fold(DVec3::ZERO, |sum, (v, m)| sum + *v * *m)
            / total_mass;
        for p in &mut positions {
            *p -= center;
        }
        for v in &mut velocities {
            *v -= motion;
        }

        let mut system = CelestialSystem::new(namespace, SimulationInstant::ZERO);
        for (i, body) in SOLAR_SYSTEM_CONTENT.iter().enumerate() {
            // The orbit plane is system XY. Local +Y is the terrain latitude/spin
            // axis, so first align it with system +Z, then apply authored obliquity.
            let tilt = UnitRotation::from_axis_angle(
                Direction3::try_new(DVec3::X)?,
                std::f64::consts::FRAC_PI_2 + body.axial_tilt_rad,
            )?;
            // A tilt above 90 degrees encodes retrograde spin without a second sign.
            let axis = tilt
                .rotate_direction(Direction3::try_new(DVec3::Y)?)?
                .unit();
            let omega = axis * (TAU / body.rotation_period_s);
            let id = system.insert_body(
                body.name,
                BodyProperties::new(masses[i], radii[i])?,
                BodyState::new(
                    LocalPosition::try_metres(positions[i])?,
                    LinearVelocity3::try_metres_per_second(velocities[i])?,
                    tilt,
                    AngularVelocity3::try_radians_per_second(omega)?,
                ),
            )?;
            if body.rocky_terrain_seed.is_some() {
                system.edit_terrain(id, terrain_definition(body.identity, radii[i])?)?;
            }
        }
        Ok(system)
    }
}

fn index(body: SolarBody) -> usize {
    body as usize
}
fn orbital_axes(body: &SolarBodyContent) -> (DVec3, DVec3) {
    let (sin_phase, cos_phase) = body.orbital_phase_rad.sin_cos();
    let (sin_inclination, cos_inclination) = body.orbital_inclination_rad.sin_cos();
    (
        DVec3::new(
            cos_phase,
            sin_phase * cos_inclination,
            sin_phase * sin_inclination,
        ),
        DVec3::new(
            -sin_phase,
            cos_phase * cos_inclination,
            cos_phase * sin_inclination,
        ),
    )
}

pub fn content(body: SolarBody) -> &'static SolarBodyContent {
    &SOLAR_SYSTEM_CONTENT[index(body)]
}
pub fn body_color(body: SolarBody) -> [f32; 4] {
    content(body).color
}

/// Natural material defaults for catalogue bodies; all dimensions are authored
/// relative to the supplied (possibly gameplay-scaled) reference radius.
pub fn planetary_config(
    body: SolarBody,
    radius_m: f64,
) -> anyhow::Result<Option<mundaris_renderer::PlanetaryConfig>> {
    if !radius_m.is_finite() || radius_m <= 0.0 {
        anyhow::bail!("planetary radius must be finite and positive");
    }
    use mundaris_renderer::{PlanetLandProfile as Land, PlanetaryConfig};
    let mut config = match body {
        SolarBody::Sun
        | SolarBody::Jupiter
        | SolarBody::Saturn
        | SolarBody::Uranus
        | SolarBody::Neptune => return Ok(None),
        SolarBody::Earth => PlanetaryConfig {
            land: Land::Earth,
            sea_datum_m: GAMEPLAY_EARTH_SEA_LEVEL_M,
            ..Default::default()
        },
        SolarBody::Mars => PlanetaryConfig {
            land: Land::Mars,
            ocean_enabled: false,
            clouds_enabled: false,
            atmosphere_enabled: false,
            ..Default::default()
        },
        SolarBody::Moon | SolarBody::Mercury | SolarBody::Venus => PlanetaryConfig {
            land: Land::Rock,
            ocean_enabled: false,
            clouds_enabled: false,
            atmosphere_enabled: false,
            ..Default::default()
        },
    };
    if body == SolarBody::Earth {
        config.cloud_altitude_m = (0.012 * radius_m).min(12_000.0);
        config.atmosphere_height_m = (0.025 * radius_m).min(100_000.0);
    }
    Ok(Some(config.try_validate()?))
}

/// Reference datum for diagnostic basin colouring and the render-only ocean.
/// Kept outside terrain definitions so display changes never invalidate raw terrain.
pub const GAMEPLAY_EARTH_SEA_LEVEL_M: f64 = 350.0;

/// Only Earth uses the tuned diagnostic ocean level; airless bodies use zero.
pub fn reference_sea_level_m(body: SolarBody) -> Option<f64> {
    content(body).rocky_terrain_seed.map(|_| {
        if body == SolarBody::Earth {
            GAMEPLAY_EARTH_SEA_LEVEL_M
        } else {
            0.0
        }
    })
}

/// Content-authored readability thresholds scaled to each rocky preset's relief.
/// Blue on airless bodies means below the reference datum, not liquid water.
pub fn terrain_readability_config(
    body: SolarBody,
    radius_m: f64,
) -> anyhow::Result<Option<mundaris_renderer::planet_surface::TerrainReadability>> {
    terrain_readability_config_with_sea_level(
        body,
        radius_m,
        reference_sea_level_m(body).unwrap_or(0.0),
    )
}

/// Debug/content override of the datum without modifying procedural geometry.
pub fn terrain_readability_config_with_sea_level(
    body: SolarBody,
    radius_m: f64,
    sea_level_m: f64,
) -> anyhow::Result<Option<mundaris_renderer::planet_surface::TerrainReadability>> {
    if content(body).rocky_terrain_seed.is_none() {
        return Ok(None);
    }
    anyhow::ensure!(
        radius_m.is_finite() && radius_m > 0.0,
        "invalid palette radius"
    );
    let relief = match body {
        SolarBody::Mercury => 0.35,
        SolarBody::Venus => 0.22,
        SolarBody::Earth => 1.0,
        SolarBody::Moon => 0.42,
        SolarBody::Mars => 0.76,
        _ => 0.5,
    };
    let scale = relief * (radius_m / 400_000.0).min(1.0);
    Ok(Some(
        mundaris_renderer::planet_surface::TerrainReadability::try_new(
            sea_level_m,
            sea_level_m + 60.0 * scale,
            sea_level_m + 250.0 * scale,
            sea_level_m + 350.0 * scale,
            sea_level_m + 650.0 * scale,
            8.0,
            16.0,
        )?,
    ))
}

/// Build rocky V2 definitions. Macro relief wavelengths scale with the body;
/// regional and local wavelengths remain physical, and heights shrink for small worlds.
pub fn terrain_definition(
    body: SolarBody,
    radius_m: f64,
) -> anyhow::Result<Option<TerrainDefinition>> {
    let Some(seed) = content(body).rocky_terrain_seed else {
        return Ok(None);
    };
    anyhow::ensure!(
        radius_m.is_finite() && radius_m > 0.0,
        "terrain radius must be finite and positive"
    );
    let (relief, macro_factor) = match body {
        SolarBody::Mercury => (0.35, 0.8),
        SolarBody::Venus => (0.22, 1.1),
        SolarBody::Earth => (1.0, 1.0),
        SolarBody::Moon => (0.42, 0.72),
        SolarBody::Mars => (0.76, 1.15),
        _ => (0.5, 1.0),
    };
    let height_scale = relief * (radius_m / 400_000.0).min(1.0);
    let macro_scale = (radius_m * 2.2 * macro_factor).max(32.0);
    let bands = [
        TerrainBandConfig::new(
            4200.0 * height_scale,
            TerrainScale::Angular {
                lowest_cycles_per_body: 2.0,
            },
            3,
        )?,
        TerrainBandConfig::new(
            2200.0 * height_scale,
            TerrainScale::Metres {
                longest_wavelength_m: macro_scale,
            },
            3,
        )?,
        TerrainBandConfig::new(
            900.0 * height_scale,
            TerrainScale::Metres {
                longest_wavelength_m: 25_000.0,
            },
            3,
        )?,
        TerrainBandConfig::new(
            220.0 * height_scale,
            TerrainScale::Metres {
                longest_wavelength_m: 2_000.0,
            },
            3,
        )?,
        TerrainBandConfig::new(
            35.0 * height_scale,
            TerrainScale::Metres {
                longest_wavelength_m: 160.0,
            },
            2,
        )?,
    ];
    let controls = TerrainControls::new(0.0, 1.0, 0.45, 0.62, 0.2, 0.4)?;
    let config = TerrainConfig::new(bands, controls)?;
    Ok(Some(TerrainDefinition::new(
        TerrainIdentity(seed),
        TerrainSeed(seed),
        TerrainGeneratorVersion::V2,
        config,
    )))
}
