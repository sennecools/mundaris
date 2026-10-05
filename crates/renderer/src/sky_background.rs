use super::SkyBackground;

pub(super) struct BackgroundMip {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
struct Color {
    r: f32,
    g: f32,
    b: f32,
}

impl Color {
    fn add_scaled(&mut self, other: Self, scale: f32) {
        self.r += other.r * scale;
        self.g += other.g * scale;
        self.b += other.b * scale;
    }
}

fn hash(seed: u64, x: i32, y: i32, z: i32) -> u64 {
    let mut value = seed ^ (x as u32 as u64).wrapping_mul(0x9e37_79b1_85eb_ca87);
    value ^= (y as u32 as u64).wrapping_mul(0xc2b2_ae3d_27d4_eb4f);
    value ^= (z as u32 as u64).wrapping_mul(0x1656_67b1_9e37_79f9);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn smooth(t: f64) -> f64 {
    t * t * (3.0 - 2.0 * t)
}

pub(super) fn noise(seed: u64, p: [f64; 3], scale: f64) -> f64 {
    let q = [p[0] * scale, p[1] * scale, p[2] * scale];
    let cell = [
        q[0].floor() as i32,
        q[1].floor() as i32,
        q[2].floor() as i32,
    ];
    let f = [
        smooth(q[0] - q[0].floor()),
        smooth(q[1] - q[1].floor()),
        smooth(q[2] - q[2].floor()),
    ];
    let mut result = 0.0;
    for dz in 0..=1 {
        for dy in 0..=1 {
            for dx in 0..=1 {
                let wx = if dx == 0 { 1.0 - f[0] } else { f[0] };
                let wy = if dy == 0 { 1.0 - f[1] } else { f[1] };
                let wz = if dz == 0 { 1.0 - f[2] } else { f[2] };
                let h = hash(seed, cell[0] + dx, cell[1] + dy, cell[2] + dz);
                let value = (h >> 11) as f64 / ((1_u64 << 53) as f64);
                result += (value * 2.0 - 1.0) * wx * wy * wz;
            }
        }
    }
    result
}

fn field(
    seed: u64,
    direction: [f64; 3],
    band_width: f64,
    dust_strength: f64,
    brightness: f32,
) -> Color {
    let [x, y, z] = direction;
    let width = band_width.max(0.01);
    // This cutoff is below one display code even at maximum validated brightness.
    // It avoids expensive cold synthesis where the final directional field is black.
    if y.abs() > (width * 5.0 + 0.10).max(0.85) {
        return Color::default();
    }

    let broad = noise(seed, direction, 4.7);
    let bend = noise(seed ^ 0x6a09_e667_f3bc_c909, direction, 11.3);
    let warp = [x + 0.032 * bend, y + 0.027 * broad, z + 0.026 * bend];
    let cloud_position = [warp[0], warp[1] * 1.8, warp[2]];
    let regional = noise(seed ^ 0xbb67_ae85_84ca_a73b, cloud_position, 19.0);
    let filaments = noise(seed ^ 0x3c6e_f372_fe94_f82b, cloud_position, 47.0);
    let fine = noise(seed ^ 0xa54f_f53a_5f1d_36f1, cloud_position, 113.0);
    let grain = noise(seed ^ 0x510e_527f_ade6_82d1, cloud_position, 269.0);
    let detail = noise(seed ^ 0x9b05_688c_2b3e_6c1f, cloud_position, 593.0);
    let turbulence = 0.53 * regional + 0.28 * filaments + 0.13 * fine + 0.06 * grain;
    let irregular_latitude = y + 0.026 * broad + 0.018 * bend;
    let band = (-0.5 * (irregular_latitude / width).powi(2)).exp();
    let thin_disk = (-0.5 * (irregular_latitude / (width * 0.46)).powi(2)).exp();
    let bulge = (-(1.0 - x) / 0.075 - 0.5 * (y / 0.17).powi(2)).exp();

    // Authored cloud complexes provide large-scale composition; directional noise
    // fragments their edges. No longitude-space noise or screen-locked grain exists.
    let mut complexes = 0.0;
    let mut cool_complexes = 0.0;
    for (longitude, latitude, spread, strength, cool) in [
        (-0.44_f64, 0.045_f64, 0.09_f64, 0.85_f64, 0.2_f64),
        (-0.12, -0.05, 0.065, 1.0, 0.0),
        (0.20, 0.07, 0.09, 1.1, 0.75),
        (0.48, -0.03, 0.085, 0.8, 1.0),
        (1.05, 0.035, 0.13, 0.85, 0.65),
        (1.60, -0.06, 0.11, 1.0, 0.9),
        (2.25, 0.04, 0.13, 0.7, 0.7),
        (-1.7, 0.055, 0.12, 0.75, 0.8),
    ] {
        let centre = [
            latitude.cos() * longitude.cos(),
            latitude.sin(),
            latitude.cos() * longitude.sin(),
        ];
        let dot = x * centre[0] + y * centre[1] + z * centre[2];
        let envelope = (-(1.0 - dot).max(0.0) / (spread * spread)).exp() * strength;
        complexes += envelope;
        cool_complexes += envelope * cool;
    }
    let cloud = (3.3 * turbulence).exp();
    let star_clouds = (0.13 * band + 0.55 * thin_disk + complexes * 1.45) * cloud;

    // Variable-width warped optical-depth ridges and fragmented secondary branches
    // replace the old soft pair of uniform stripes. Absorption reveals black pockets.
    let lane_height = y + 0.029 * z + 0.030 * bend + 0.022 * regional + 0.010 * filaments;
    let lane_width = 0.019 + 0.010 * (0.5 + 0.5 * broad);
    let lane = (-0.5 * (lane_height / lane_width).powi(2)).exp();
    let branch_height = y - 0.072 * z + 0.027 * broad - 0.019 * regional + 0.011 * fine;
    let branch = (-0.5 * (branch_height / 0.012).powi(2)).exp();
    let wisps_height = y + 0.081 * z - 0.055 + 0.023 * bend + 0.015 * filaments;
    let wisps = (-0.5 * (wisps_height / 0.008).powi(2)).exp();
    let patchiness = (1.0 + 1.5 * regional + 0.6 * fine + 0.25 * grain).max(0.25);
    let dust_depth = dust_strength * (3.8 * lane + 2.0 * branch + 1.1 * wisps) * patchiness;
    let transmission = (-dust_depth).exp();
    let distant_warm = bulge * (0.8 + 0.45 * cloud);
    let cool_fraction = (0.15 + 0.5 * cool_complexes + 0.25 * regional).clamp(0.0, 0.85);
    let texture = (1.0 + 0.23 * grain + 0.12 * detail).max(0.25);
    let light = f64::from(brightness) * transmission * texture;
    // Ivory stellar light, cooler blue-white complexes and an amber core. Colour
    // depends on spatial populations, not a single brown scalar for the whole band.
    Color {
        r: (light * (star_clouds * (0.94 - 0.42 * cool_fraction) + distant_warm)) as f32,
        g: (light * (star_clouds * (0.91 - 0.19 * cool_fraction) + distant_warm * 0.69)) as f32,
        b: (light * (star_clouds * (0.89 + 0.26 * cool_fraction) + distant_warm * 0.39)) as f32,
    }
}

fn linear_to_srgb(value: f32) -> u8 {
    let value = value.clamp(0.0, 1.0);
    let encoded = if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    };
    (encoded * 255.0 + 0.5) as u8
}

#[cfg(test)]
fn srgb_to_linear(value: u8) -> f32 {
    let encoded = value as f32 / 255.0;
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

fn encode(colors: &[Color]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(colors.len() * 4);
    for color in colors {
        bytes.extend_from_slice(&[
            linear_to_srgb(color.r),
            linear_to_srgb(color.g),
            linear_to_srgb(color.b),
            255,
        ]);
    }
    bytes
}

fn sample(
    config: SkyBackground,
    morphology: Option<&super::SkyMorphology>,
    direction: [f64; 3],
) -> Color {
    if let Some(morphology) = morphology {
        let [r, g, b] = super::structure::radiance(
            morphology,
            config.seed,
            glam::DVec3::from_array(direction),
            config.brightness,
            config.dust_strength,
        );
        Color { r, g, b }
    } else {
        field(
            config.seed,
            direction,
            config.band_width_rad,
            config.dust_strength,
            config.brightness,
        )
    }
}

#[cfg(feature = "terrain-capture")]
pub(super) fn sample_linear(
    config: SkyBackground,
    morphology: Option<&super::SkyMorphology>,
    direction: [f64; 3],
) -> [f32; 3] {
    let c = sample(config, morphology, direction);
    [c.r, c.g, c.b]
}

pub(super) fn precompute(
    config: SkyBackground,
    morphology: Option<&super::SkyMorphology>,
) -> Vec<BackgroundMip> {
    let width = config.width.max(1);
    let height = config.height.max(1);
    let mut colors = Vec::with_capacity(width as usize * height as usize);
    for py in 0..height {
        let v = (py as f64 + 0.5) / height as f64;
        let latitude = std::f64::consts::PI * (0.5 - v);
        let cos_lat = latitude.cos();
        for px in 0..width {
            let u = (px as f64 + 0.5) / width as f64;
            let longitude = std::f64::consts::TAU * (u - 0.5);
            let direction = [
                cos_lat * longitude.cos(),
                latitude.sin(),
                cos_lat * longitude.sin(),
            ];
            colors.push(sample(config, morphology, direction));
        }
    }
    mip_chain(colors, width, height)
}

pub(super) fn precompute_details(
    config: SkyBackground,
    morphology: &super::SkyMorphology,
) -> Vec<Vec<BackgroundMip>> {
    morphology
        .complexes
        .iter()
        .map(|complex| {
            let size = morphology.detail_size;
            let [right, up, forward] = complex.axes();
            let extent = complex.half_extent_rad.tan();
            let mut colors = Vec::with_capacity(size as usize * size as usize);
            for y in 0..size {
                for x in 0..size {
                    let u = (2.0 * (f64::from(x) + 0.5) / f64::from(size) - 1.0) * extent;
                    let v = (1.0 - 2.0 * (f64::from(y) + 0.5) / f64::from(size)) * extent;
                    let direction = (forward + right * u + up * v).normalize();
                    colors.push(sample(config, Some(morphology), direction.to_array()));
                }
            }
            mip_chain(colors, size, size)
        })
        .collect()
}

fn mip_chain(mut colors: Vec<Color>, width: u32, height: u32) -> Vec<BackgroundMip> {
    let mut levels = Vec::new();
    let mut mip_width = width;
    let mut mip_height = height;
    loop {
        levels.push(BackgroundMip {
            width: mip_width,
            height: mip_height,
            rgba: encode(&colors),
        });
        if mip_width == 1 && mip_height == 1 {
            break;
        }
        let next_width = (mip_width / 2).max(1);
        let next_height = (mip_height / 2).max(1);
        let mut next = Vec::with_capacity(next_width as usize * next_height as usize);
        for y in 0..next_height {
            for x in 0..next_width {
                let mut average = Color::default();
                let mut count = 0.0;
                for dy in 0..2 {
                    for dx in 0..2 {
                        let sx = (x * 2 + dx).min(mip_width - 1);
                        let sy = (y * 2 + dy).min(mip_height - 1);
                        let color = colors[(sy * mip_width + sx) as usize];
                        average.add_scaled(color, 0.25);
                        count += 0.25;
                    }
                }
                if count < 1.0 {
                    average.add_scaled(average, (1.0 / count) - 1.0);
                }
                next.push(average);
            }
        }
        colors = next;
        mip_width = next_width;
        mip_height = next_height;
    }
    levels
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> SkyBackground {
        SkyBackground {
            seed: 41,
            width: 64,
            height: 32,
            band_width_rad: 0.32,
            dust_strength: 0.75,
            brightness: 0.09,
        }
    }

    #[test]
    fn generation_is_deterministic_and_has_complete_mips() {
        let config = fixture();
        let a = precompute(config, None);
        let b = precompute(config, None);
        assert_eq!(a.len(), 7);
        assert!(
            a.iter()
                .zip(&b)
                .all(|(x, y)| x.width == y.width && x.height == y.height && x.rgba == y.rgba)
        );
        assert_eq!((a.last().unwrap().width, a.last().unwrap().height), (1, 1));
    }

    #[test]
    fn directional_field_is_continuous_across_longitude_wrap() {
        let epsilon = 1.0e-7_f64;
        let left = [-epsilon.cos(), 0.17, -epsilon.sin()];
        let right = [-epsilon.cos(), 0.17, epsilon.sin()];
        let a = field(7, left, 0.32, 0.7, 0.09);
        let b = field(7, right, 0.32, 0.7, 0.09);
        assert!((a.r - b.r).abs() < 1.0e-5);
        assert!((a.g - b.g).abs() < 1.0e-5);
        assert!((a.b - b.b).abs() < 1.0e-5);
    }

    #[test]
    fn band_has_structure_and_dust_contrast_over_faint_away_sky() {
        let config = fixture();
        let mips = precompute(config, None);
        let image = &mips[0];
        let mut band = Vec::new();
        let mut away = Vec::new();
        for y in 0..image.height {
            let latitude = std::f64::consts::PI * (0.5 - (y as f64 + 0.5) / image.height as f64);
            for x in 0..image.width {
                let offset = ((y * image.width + x) * 4) as usize;
                let value = srgb_to_linear(image.rgba[offset])
                    + srgb_to_linear(image.rgba[offset + 1])
                    + srgb_to_linear(image.rgba[offset + 2]);
                if latitude.abs() < 0.24 {
                    band.push(value);
                } else if latitude.abs() > 0.85 {
                    away.push(value);
                }
            }
        }
        let band_mean = band.iter().sum::<f32>() / band.len() as f32;
        let away_mean = away.iter().sum::<f32>() / away.len() as f32;
        let band_variance =
            band.iter().map(|v| (v - band_mean).powi(2)).sum::<f32>() / band.len() as f32;
        assert!(band_mean > away_mean * 10.0);
        assert!(band_variance > 1.0e-5);
        assert!(band.iter().any(|v| *v < band_mean * 0.7));
        assert!(band.iter().any(|v| *v > band_mean * 1.3));
    }

    #[test]
    fn dust_absorbs_light_and_cloud_complexes_have_distinct_colour() {
        let seed = 0x6d75_6e64_6172_6973;
        let mut cool = 0;
        let mut warm = 0;
        let mut dark_pockets = 0;
        let mut bright_clouds = 0;
        for longitude_index in -40..=40 {
            let longitude = f64::from(longitude_index) * 0.015;
            for latitude_index in -15..=15 {
                let latitude = f64::from(latitude_index) * 0.01;
                let direction = [
                    latitude.cos() * longitude.cos(),
                    latitude.sin(),
                    latitude.cos() * longitude.sin(),
                ];
                let on = field(seed, direction, 0.11, 0.9, 0.13);
                let off = field(seed, direction, 0.11, 0.0, 0.13);
                assert!(on.r <= off.r && on.g <= off.g && on.b <= off.b);
                if on.b > on.r * 1.15 {
                    cool += 1;
                }
                if on.r > on.b * 1.3 {
                    warm += 1;
                }
                if on.r + on.g + on.b < 0.025 {
                    dark_pockets += 1;
                }
                if on.r + on.g + on.b > 0.5 {
                    bright_clouds += 1;
                }
            }
        }
        assert!(cool > 20 && warm > 20, "colour populations {cool}/{warm}");
        assert!(
            dark_pockets > 20 && bright_clouds > 20,
            "contrast {dark_pockets}/{bright_clouds}"
        );
    }

    #[test]
    fn polar_cutoff_is_below_one_display_code_for_validated_widths() {
        for width in [0.01_f64, 0.11, 0.17] {
            let cutoff = (width * 5.0 + 0.10).max(0.85);
            if cutoff >= 1.0 {
                continue;
            }
            let latitude_sine = cutoff - 1e-8;
            let cosine = (1.0 - latitude_sine * latitude_sine).sqrt();
            for longitude_index in 0..32 {
                let longitude = f64::from(longitude_index) * std::f64::consts::TAU / 32.0;
                let direction = [
                    cosine * longitude.cos(),
                    latitude_sine,
                    cosine * longitude.sin(),
                ];
                let color = field(7, direction, width, 0.0, 1.0);
                assert_eq!([color.r, color.g, color.b].map(linear_to_srgb), [0; 3]);
            }
        }
    }
}
