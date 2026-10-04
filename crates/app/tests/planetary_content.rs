use mundaris_app::solar_system::{SolarBody, planetary_config};
use mundaris_renderer::PlanetLandProfile;

#[test]
fn planetary_defaults_are_body_relative_and_content_gated() {
    let gameplay_radius = 400_000.0;
    let earth = planetary_config(SolarBody::Earth, gameplay_radius)
        .unwrap()
        .unwrap();
    assert_eq!(earth.land, PlanetLandProfile::Earth);
    assert!(earth.ocean_enabled && earth.clouds_enabled && earth.atmosphere_enabled);
    assert_eq!(earth.sea_datum_m, 350.0);
    assert_eq!(earth.cloud_altitude_m, gameplay_radius * 0.012);
    assert_eq!(earth.atmosphere_height_m, gameplay_radius * 0.025);

    let real_earth = planetary_config(SolarBody::Earth, 6_371_000.0)
        .unwrap()
        .unwrap();
    assert_eq!(real_earth.atmosphere_height_m, 100_000.0);
    assert_eq!(real_earth.cloud_altitude_m, 12_000.0);
    assert_eq!(
        planetary_config(SolarBody::Earth, 800_000.0)
            .unwrap()
            .unwrap()
            .cloud_altitude_m,
        9_600.0
    );
    assert!(planetary_config(SolarBody::Sun, 1.0).unwrap().is_none());
}

#[test]
fn non_earth_content_does_not_gain_earth_layers() {
    for body in [SolarBody::Moon, SolarBody::Mercury, SolarBody::Venus] {
        let config = planetary_config(body, 400_000.0).unwrap().unwrap();
        assert_eq!(config.land, PlanetLandProfile::Rock);
        assert!(!config.ocean_enabled && !config.clouds_enabled && !config.atmosphere_enabled);
    }
    let mars = planetary_config(SolarBody::Mars, 400_000.0)
        .unwrap()
        .unwrap();
    assert_eq!(mars.land, PlanetLandProfile::Mars);
    assert!(!mars.ocean_enabled && !mars.clouds_enabled);
    assert!(!mars.atmosphere_enabled);
}

#[test]
fn planetary_dimensions_reject_invalid_radii() {
    for radius in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        assert!(planetary_config(SolarBody::Earth, radius).is_err());
    }
}
