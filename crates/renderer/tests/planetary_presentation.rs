use mundaris_renderer::{PlanetLandProfile, PlanetaryConfig};

#[test]
fn planetary_defaults_are_bounded_and_valid() {
    let config = PlanetaryConfig::default();
    assert_eq!(config.land, PlanetLandProfile::Earth);
    assert!(config.ocean_enabled && config.clouds_enabled && config.atmosphere_enabled);
    assert_eq!(config.try_validate().unwrap(), config);
}

#[test]
fn planetary_config_rejects_non_finite_and_unbounded_layers() {
    for config in [
        PlanetaryConfig {
            sea_datum_m: f64::NAN,
            ..Default::default()
        },
        PlanetaryConfig {
            cloud_altitude_m: 1.0e9,
            ..Default::default()
        },
        PlanetaryConfig {
            cloud_coverage: 2.0,
            ..Default::default()
        },
        PlanetaryConfig {
            atmosphere_height_m: 0.0,
            ..Default::default()
        },
        PlanetaryConfig {
            ocean_roughness: f32::NAN,
            ..Default::default()
        },
        PlanetaryConfig {
            rayleigh_optical_depth: [0.035, f32::INFINITY, 0.19],
            ..Default::default()
        },
        PlanetaryConfig {
            sun_intensity: -1.0,
            ..Default::default()
        },
    ] {
        assert!(config.try_validate().is_err());
    }
}

#[test]
fn planetary_shell_shader_is_valid_wgsl() {
    let module =
        naga::front::wgsl::parse_str(include_str!("../src/shaders/planetary.wgsl")).unwrap();
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    )
    .validate(&module)
    .unwrap();
}
