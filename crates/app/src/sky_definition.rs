//! Immutable decorative distant sky content for the renderer.
//!
//! These generated points are finite-parallax presentation content, not an
//! astronomical catalog and not addressable systems.

use std::sync::Arc;

use glam::{DQuat, DVec3};
use astrum_renderer::RenderPreparationError;
use astrum_renderer::sky::{
    SkyBackground, SkyBranch, SkyCavity, SkyComplex, SkyDefinition, SkyDiskRegion, SkyIdentity,
    SkyMorphology, SkyStar,
};

pub const PRESET_ID: &str = "astrum.decorative-galactic-sky";
pub const PRESET_VERSION: u32 = 4;
pub const PRESET_SEED: u64 = 0x6d75_6e64_6172_6973;
pub const FINITE_STAR_MIN_DISTANCE_M: f64 = 1.0e18;
pub const FINITE_STAR_MAX_DISTANCE_M: f64 = 2.0e19;
pub const SUPPORTED_OBSERVER_RADIUS_M: f64 = 1.0e14;
pub const CLASSIFICATION: &str = "decorative; non-addressable; not an astronomical catalog";

const STAR_COUNT: usize = 131_072;
const GALACTIC_CLOUD_LONGITUDES: [f64; 4] = [-0.24, 0.28, 1.5, -1.75];
const GALACTIC_CLOUD_LATITUDES: [f64; 4] = [0.035, 0.44, -0.50, 0.32];

/// Builds the deterministic versioned default sky; retain it for the session.
///
/// The origin is an inertial system anchor. The authored galactic plane has
/// normal +Y before a fixed cinematic tilt. A broken disk backbone lies around
/// X/Z, while nearby decorative nebulae and clusters extend outside that plane.
pub fn default_sky() -> Result<Arc<SkyDefinition>, RenderPreparationError> {
    let orientation = DQuat::from_rotation_z(0.4) * DQuat::from_rotation_x(0.7);
    let morphology = cinematic_morphology();
    let mut random = PresetRng(PRESET_SEED);
    let mut stars = Vec::with_capacity(STAR_COUNT);
    for _ in 0..STAR_COUNT {
        let population = random.unit();
        let (longitude, latitude, cloud) = if population < 0.60 {
            // Shared authored regions vary the disk's density, width and centre.
            // The low-density gaps remain populated by the isotropic foreground,
            // not another bright, full-longitude stripe.
            let region = choose_disk_region(&morphology.disk_regions, &mut random);
            (
                region.longitude_rad + random.normal() * region.half_length_rad * 0.36,
                region.latitude_rad + random.normal() * region.latitude_width_rad,
                None,
            )
        } else if population < 0.70 {
            // A modest nearby population follows the three off-plane structures.
            // Wider unequal groups avoid concentrating most stars into four balls.
            let cloud = 1 + (random.next() as usize) % 3;
            (
                GALACTIC_CLOUD_LONGITUDES[cloud] + random.normal() * [0.0, 0.10, 0.13, 0.08][cloud],
                GALACTIC_CLOUD_LATITUDES[cloud]
                    + random.normal() * [0.0, 0.065, 0.08, 0.055][cloud],
                Some(cloud),
            )
        } else {
            (
                random.range(-std::f64::consts::PI, std::f64::consts::PI),
                random.range(-1.0, 1.0).asin(),
                None,
            )
        };
        let foreground = random.unit() < 0.24;
        let (minimum, maximum) = if foreground {
            (FINITE_STAR_MIN_DISTANCE_M, 3.0e18)
        } else {
            (6.0e18, FINITE_STAR_MAX_DISTANCE_M)
        };
        let distance = minimum * (maximum / minimum).powf(random.unit());
        let latitude_cos = latitude.cos();
        let galactic_position = DVec3::new(
            distance * latitude_cos * longitude.cos(),
            distance * latitude.sin(),
            distance * latitude_cos * longitude.sin(),
        );
        // Renderer applies the authored galactic-to-system orientation once.
        let position_m = galactic_position;

        // A steep luminosity distribution keeps the field fine-grained, with a
        // sparse tail of brighter points and restrained stellar color variation.
        let intrinsic_flux = (0.035 / (1.0 - random.unit() * 0.999)).min(28.0) as f32;
        let transmission = if foreground {
            1.0
        } else {
            morphology.background_transmission(PRESET_SEED, galactic_position / distance, 0.9)
        };
        let flux = intrinsic_flux * transmission;
        let tint = random.unit() as f32;
        let local_blue = matches!(cloud, Some(1 | 2));
        let color = if tint < if local_blue { 0.58 } else { 0.25 } {
            [0.55 + tint * 0.6, 0.72 + tint * 0.45, 1.0]
        } else if tint > 0.90 {
            [1.0, 0.65 + (1.0 - tint) * 1.5, 0.35 + (1.0 - tint) * 4.5]
        } else {
            [1.0, 0.94, 0.80]
        };
        // Faint populations keep subpixel cores. Brightness does not inflate every
        // point into a glowing dot; rare bright cores remain below one pixel sigma.
        let radius_pixels =
            (0.35 + random.unit() * 0.16 + if intrinsic_flux > 2.0 { 0.12 } else { 0.0 }) as f32;
        stars.push(SkyStar {
            position_m,
            color,
            flux,
            radius_pixels,
        });
    }

    SkyDefinition::try_new(
        SkyIdentity {
            preset: PRESET_ID,
            version: PRESET_VERSION,
            seed: PRESET_SEED,
        },
        DVec3::ZERO,
        orientation,
        stars,
        SkyBackground {
            seed: PRESET_SEED,
            width: 2048,
            height: 1024,
            band_width_rad: 0.11,
            dust_strength: 0.9,
            brightness: 0.13,
        },
    )
    .and_then(|definition| definition.with_morphology(morphology))
    .map(Arc::new)
}

fn choose_disk_region<'a>(
    regions: &'a [SkyDiskRegion],
    random: &mut PresetRng,
) -> &'a SkyDiskRegion {
    let weight = |r: &SkyDiskRegion| r.density * r.half_length_rad * r.latitude_width_rad;
    let mut selection = random.unit() * regions.iter().map(weight).sum::<f64>();
    // The app authors a nonempty positive-weight list. The final region absorbs
    // roundoff at the upper boundary without unbounded rejection sampling.
    let mut chosen = &regions[0];
    for region in regions {
        chosen = region;
        selection -= weight(region);
        if selection <= 0.0 {
            break;
        }
    }
    chosen
}

fn cinematic_morphology() -> SkyMorphology {
    let mut random = PresetRng(PRESET_SEED ^ 0x5a13_c0de);
    let mut complexes = Vec::new();
    // Distinct skeletons: broken arch, branching pillar, split fan and reflection
    // crescent. Seeded perturbations vary contours, not connectivity.
    let cloud_paths: &[&[[f64; 2]]] = &[
        &[
            [-0.78, -0.32],
            [-0.48, 0.10],
            [-0.20, 0.40],
            [0.12, 0.38],
            [0.52, 0.10],
            [0.72, -0.22],
        ],
        &[
            [-0.67, -0.44],
            [-0.31, -0.30],
            [-0.06, -0.04],
            [0.10, 0.28],
            [0.40, 0.60],
        ],
        &[
            [-0.65, -0.30],
            [-0.25, -0.12],
            [0.05, 0.12],
            [0.45, 0.15],
            [0.72, 0.41],
        ],
        &[
            [-0.60, -0.40],
            [-0.67, 0.00],
            [-0.35, 0.42],
            [0.05, 0.52],
            [0.47, 0.31],
            [0.63, -0.12],
        ],
    ];
    for (index, path) in cloud_paths.iter().enumerate() {
        let longitude = GALACTIC_CLOUD_LONGITUDES[index];
        let latitude = GALACTIC_CLOUD_LATITUDES[index];
        let direction = DVec3::new(
            latitude.cos() * longitude.cos(),
            latitude.sin(),
            latitude.cos() * longitude.sin(),
        );
        let mut clouds = Vec::new();
        let mut dust = Vec::new();
        for (segment, pair) in path.windows(2).enumerate() {
            clouds.push(SkyBranch {
                start: pair[0],
                end: pair[1],
                widths: [0.15 + random.unit() * 0.07, 0.12 + random.unit() * 0.10],
                density: 1.0,
            });
            let a = [pair[0][0] + 0.035, pair[0][1] - 0.085];
            let b = [pair[1][0] + 0.025, pair[1][1] - 0.075];
            dust.push(SkyBranch {
                start: a,
                end: b,
                widths: [0.035 + random.unit() * 0.03, 0.022 + random.unit() * 0.02],
                density: 2.5,
            });
            if segment == 0 {
                continue;
            }
            let sign = if (segment + index) % 2 == 0 {
                1.0
            } else {
                -1.0
            };
            let endpoint = [
                b[0] + sign * random.range(0.16, 0.28),
                (b[1] + random.range(0.18, 0.32) * if index == 0 { -1.0 } else { 1.0 })
                    .clamp(-0.78, 0.78),
            ];
            clouds.push(SkyBranch {
                start: pair[1],
                end: endpoint,
                widths: [0.13, 0.05],
                density: 1.0,
            });
            grow_dust(&mut dust, &mut random, b, endpoint, 0.032, 3);
        }
        let cavities = if index == 0 {
            vec![
                SkyCavity {
                    center: [-0.08, 0.18],
                    radii: [0.18, 0.15],
                },
                SkyCavity {
                    center: [0.45, 0.05],
                    radii: [0.075, 0.09],
                },
            ]
        } else if index == 1 {
            vec![SkyCavity {
                center: [0.11, 0.17],
                radii: [0.06, 0.085],
            }]
        } else {
            vec![SkyCavity {
                center: [-0.23, 0.25],
                radii: [0.11, 0.13],
            }]
        };
        let (emission, reflection, luminosity) = match index {
            0 => ([1.0, 0.12, 0.19], [0.22, 0.52, 1.0], 0.88),
            1 => ([0.51, 0.17, 0.47], [0.22, 0.57, 1.0], 0.73),
            2 => ([0.24, 0.39, 0.52], [0.30, 0.69, 1.0], 0.66),
            _ => ([0.82, 0.48, 0.22], [0.27, 0.48, 1.0], 0.47),
        };
        complexes.push(SkyComplex {
            direction,
            half_extent_rad: [0.24, 0.23, 0.21, 0.19][index],
            roll_rad: [0.15, -0.68, 0.58, -0.92][index],
            emission,
            reflection,
            luminosity,
            light_position: [-0.23 + index as f64 * 0.12, 0.36],
            clouds,
            dust,
            cavities,
        });
    }
    SkyMorphology {
        complexes,
        disk_regions: [
            (-0.42, 0.015, 0.72, 0.075, 1.0),
            (0.58, -0.045, 0.46, 0.095, 0.48),
            (1.44, 0.035, 0.65, 0.065, 0.82),
            (2.43, -0.025, 0.36, 0.045, 0.24),
            (-2.65, 0.055, 0.46, 0.11, 0.38),
            (-1.70, -0.035, 0.50, 0.065, 0.62),
        ]
        .into_iter()
        .map(
            |(longitude_rad, latitude_rad, half_length_rad, latitude_width_rad, density)| {
                SkyDiskRegion {
                    longitude_rad,
                    latitude_rad,
                    half_length_rad,
                    latitude_width_rad,
                    density,
                }
            },
        )
        .collect(),
        detail_size: 1024,
    }
}

fn grow_dust(
    branches: &mut Vec<SkyBranch>,
    random: &mut PresetRng,
    start: [f64; 2],
    end: [f64; 2],
    width: f64,
    depth: u32,
) {
    let midpoint = [
        (start[0] + end[0]) * 0.5 + random.range(-0.024, 0.024),
        (start[1] + end[1]) * 0.5 + random.range(-0.024, 0.024),
    ];
    branches.push(SkyBranch {
        start,
        end: midpoint,
        widths: [width, width * 0.75],
        density: 2.2,
    });
    branches.push(SkyBranch {
        start: midpoint,
        end,
        widths: [width * 0.75, width * 0.32],
        density: 1.7,
    });
    if depth > 0 {
        let delta = [end[0] - start[0], end[1] - start[1]];
        let sign = if random.unit() < 0.5 { 1.0 } else { -1.0 };
        let fork = [
            (midpoint[0] + delta[0] * 0.28 - delta[1] * 0.52 * sign).clamp(-0.82, 0.82),
            (midpoint[1] + delta[1] * 0.28 + delta[0] * 0.52 * sign).clamp(-0.82, 0.82),
        ];
        grow_dust(branches, random, midpoint, fork, width * 0.56, depth - 1);
    }
}

/// Small fixed integer generator; no ambient entropy or runtime state is used.
struct PresetRng(u64);

impl PresetRng {
    fn next(&mut self) -> u64 {
        // SplitMix64, with wrapping arithmetic defined by the integer type.
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.0;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) as f64) * (1.0 / ((1_u64 << 53) as f64))
    }

    fn range(&mut self, minimum: f64, maximum: f64) -> f64 {
        minimum + (maximum - minimum) * self.unit()
    }

    fn normal(&mut self) -> f64 {
        // Irwin-Hall approximation avoids platform-sensitive transcendental RNG.
        (0..12).map(|_| self.unit()).sum::<f64>() - 6.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_is_repeatable_and_has_bounded_finite_stars() {
        let first = default_sky().unwrap();
        let second = default_sky().unwrap();
        assert_eq!(first.identity().preset, PRESET_ID);
        assert_eq!(first.identity().version, PRESET_VERSION);
        assert_eq!(first.identity().seed, PRESET_SEED);
        assert_eq!(first.anchor_m(), DVec3::ZERO);
        assert_ne!(first.galactic_to_system(), DQuat::IDENTITY);
        assert_eq!(first.stars().len(), STAR_COUNT);
        assert_eq!(first.morphology(), second.morphology());
        let mut hash_a = 0xcbf2_9ce4_8422_2325_u64;
        let mut hash_b = hash_a;
        for (a, b) in first.stars().iter().zip(second.stars()) {
            assert_eq!(a.position_m, b.position_m);
            assert_eq!(a.color.map(f32::to_bits), b.color.map(f32::to_bits));
            assert_eq!(a.flux.to_bits(), b.flux.to_bits());
            assert_eq!(a.radius_pixels.to_bits(), b.radius_pixels.to_bits());
            let distance = a.position_m.length();
            assert!((FINITE_STAR_MIN_DISTANCE_M..=FINITE_STAR_MAX_DISTANCE_M).contains(&distance));
            for bits in [
                a.position_m.x.to_bits(),
                a.position_m.y.to_bits(),
                a.position_m.z.to_bits(),
            ] {
                hash_a = (hash_a ^ bits).wrapping_mul(0x100_0000_01b3);
            }
            for bits in [
                b.position_m.x.to_bits(),
                b.position_m.y.to_bits(),
                b.position_m.z.to_bits(),
            ] {
                hash_b = (hash_b ^ bits).wrapping_mul(0x100_0000_01b3);
            }
        }
        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn preset_has_a_faint_majority_and_few_bright_stars() {
        let sky = default_sky().unwrap();
        let stars = sky.stars();
        let faint = stars.iter().filter(|star| star.flux < 0.15).count();
        let bright = stars.iter().filter(|star| star.flux > 1.0).count();
        assert!(faint > STAR_COUNT * 75 / 100 && faint < STAR_COUNT * 95 / 100);
        assert!(bright > 0 && bright < STAR_COUNT * 4 / 100);
        assert!(
            stars
                .iter()
                .all(|star| (0.35..=0.64).contains(&star.radius_pixels))
        );
        assert!(CLASSIFICATION.contains("non-addressable"));
        assert!(stars.iter().any(|star| star.flux > 6.0));
    }

    #[test]
    fn preset_has_clustered_disk_and_distinct_stellar_colors() {
        let sky = default_sky().unwrap();
        let clustered = sky
            .stars()
            .iter()
            .filter(|star| {
                let longitude = star.position_m.z.atan2(star.position_m.x);
                let latitude = (star.position_m.y / star.position_m.length()).asin();
                GALACTIC_CLOUD_LONGITUDES
                    .iter()
                    .zip(GALACTIC_CLOUD_LATITUDES)
                    .any(|(center, offset)| {
                        let delta = (longitude - center + std::f64::consts::PI)
                            .rem_euclid(std::f64::consts::TAU)
                            - std::f64::consts::PI;
                        delta.abs() < 0.22 && (latitude - offset).abs() < 0.10
                    })
            })
            .count();
        let blue = sky
            .stars()
            .iter()
            .filter(|star| star.color[2] > star.color[1] + 0.08)
            .count();
        let warm = sky
            .stars()
            .iter()
            .filter(|star| star.color[2] < 0.7)
            .count();
        assert!(clustered > STAR_COUNT / 12);
        assert!(blue > STAR_COUNT / 5);
        assert!(warm > 0 && warm < STAR_COUNT / 8);
    }

    #[test]
    fn composition_is_bounded_and_invalid_graphs_cannot_publish() {
        let sky = default_sky().unwrap();
        let morphology = sky.morphology().unwrap();
        assert_eq!(morphology.complexes.len(), 4);
        assert_eq!(morphology.detail_size, 1024);
        assert!(
            morphology
                .complexes
                .iter()
                .all(|c| c.dust.len() > c.clouds.len())
        );
        assert!(morphology.complexes.iter().all(|c| !c.cavities.is_empty()));
        assert_eq!(morphology.disk_regions.len(), 6);
        for invalid in 0..10 {
            let mut candidate = morphology.clone();
            match invalid {
                0 => candidate.detail_size = 4096,
                1 => candidate.complexes[0].clouds[0].start[0] = f64::NAN,
                2 => candidate.complexes[0].cavities[0].radii[0] = -0.1,
                3 => candidate.complexes[0].direction = DVec3::ZERO,
                4 => candidate.complexes.push(candidate.complexes[0].clone()),
                5 => candidate.disk_regions[0].longitude_rad = f64::NAN,
                6 => candidate.disk_regions[0].latitude_width_rad = 0.0,
                7 => candidate.disk_regions[0].density = 1.01,
                8 => candidate.disk_regions[0].half_length_rad = f64::INFINITY,
                _ => candidate
                    .disk_regions
                    .resize(17, candidate.disk_regions[0].clone()),
            }
            assert!(sky.as_ref().clone().with_morphology(candidate).is_err());
        }
    }

    #[test]
    fn composition_extends_outside_disk_without_filling_dark_sky() {
        let sky = default_sky().unwrap();
        let morphology = sky.morphology().unwrap();
        let offband = morphology
            .complexes
            .iter()
            .filter(|c| c.direction.y.asin().abs() > 15_f64.to_radians())
            .count();
        assert_eq!(offband, 3);
        assert_eq!(morphology.detail_size, 1024);
        let mut plane_stars = 0;
        let mut offband_stars = 0;
        let mut quiet_cone_stars = 0;
        let dark_direction = DVec3::new(
            0.6_f64.cos() * 2.65_f64.cos(),
            0.6_f64.sin(),
            0.6_f64.cos() * 2.65_f64.sin(),
        );
        for star in sky.stars() {
            let direction = star.position_m.normalize();
            let latitude = direction.y.asin().abs();
            plane_stars += usize::from(latitude < 10_f64.to_radians());
            offband_stars += usize::from(latitude > 15_f64.to_radians());
            quiet_cone_stars +=
                usize::from(direction.dot(dark_direction) > 15_f64.to_radians().cos());
        }
        assert!(
            (STAR_COUNT * 50 / 100..STAR_COUNT * 70 / 100).contains(&plane_stars),
            "plane population {plane_stars}"
        );
        assert!(
            (STAR_COUNT * 28 / 100..STAR_COUNT * 40 / 100).contains(&offband_stars),
            "offband population {offband_stars}"
        );
        assert!(
            (STAR_COUNT / 400..STAR_COUNT / 100).contains(&quiet_cone_stars),
            "quiet cone population {quiet_cone_stars}"
        );
        println!(
            "Composition populations: plane (<10deg) {plane_stars}, offband (>15deg) {offband_stars}, quiet 15deg cone {quiet_cone_stars}; total {STAR_COUNT}"
        );
    }

    #[cfg(feature = "terrain-capture")]
    #[test]
    fn generated_field_has_offband_focal_light_and_quiet_directions() {
        use astrum_renderer::sky::sample_background;
        let sky = default_sky().unwrap();
        let luminosity = |direction| {
            sample_background(&sky, direction)
                .unwrap()
                .into_iter()
                .map(f64::from)
                .sum::<f64>()
        };
        let dark = luminosity(DVec3::Y);
        // The diagnostic boundary admits unit-vector roundoff at the poles.
        assert_eq!(luminosity(DVec3::Y * (1.0 + 1e-12)), dark);
        for complex in &sky.morphology().unwrap().complexes {
            // Sample the chart in a neighbourhood, not a centre which may be an
            // intentionally absorbing cavity or branching-dust intersection.
            let reference = DVec3::Y;
            let right = reference.cross(complex.direction).normalize();
            let up = complex.direction.cross(right);
            let mut maximum = 0.0_f64;
            for y in -4..=4 {
                for x in -4..=4 {
                    maximum = maximum.max(luminosity(
                        (complex.direction
                            + right * (f64::from(x) * 0.025)
                            + up * (f64::from(y) * 0.025))
                            .normalize(),
                    ));
                }
            }
            assert!(
                maximum > 0.04 && maximum > dark * 100.0,
                "focal {maximum} vs dark {dark}"
            );
        }
        let gap = DVec3::new(3.0_f64.cos(), 0.0, 3.0_f64.sin());
        assert!(luminosity(gap) < 1e-6, "backbone gap must remain dark");
    }

    #[test]
    fn foreground_is_unextinguished_but_background_contains_obscured_populations() {
        let sky = default_sky().unwrap();
        let foreground: Vec<_> = sky
            .stars()
            .iter()
            .filter(|s| s.position_m.length() < 3.01e18)
            .collect();
        let background: Vec<_> = sky
            .stars()
            .iter()
            .filter(|s| s.position_m.length() > 5.99e18)
            .collect();
        assert!(foreground.len() > STAR_COUNT / 5);
        assert!(foreground.iter().all(|s| s.flux >= 0.035));
        assert!(background.iter().any(|s| s.flux < 0.005));
    }
}
