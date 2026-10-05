//! Immutable decorative morphology and directional evaluation shared by cached
//! backgrounds and cold catalogue authoring. These are not spatial world objects.
use glam::DVec3;

/// Tapered segment in a complex's dimensionless tangent chart.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyBranch {
    pub start: [f64; 2],
    pub end: [f64; 2],
    pub widths: [f64; 2],
    pub density: f64,
}

/// An elliptical opening; its wall is part of the absorbing structure.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyCavity {
    pub center: [f64; 2],
    pub radii: [f64; 2],
}

/// App-authored connected cloud and dust skeletons, evaluated in fixed sky axes.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyComplex {
    pub direction: DVec3,
    pub half_extent_rad: f64,
    pub roll_rad: f64,
    pub emission: [f32; 3],
    pub reflection: [f32; 3],
    pub luminosity: f32,
    pub light_position: [f64; 2],
    pub clouds: Vec<SkyBranch>,
    pub dust: Vec<SkyBranch>,
    pub cavities: Vec<SkyCavity>,
}

/// Compact diffuse disk region. Unequal, overlapping supports define a broken
/// galactic backbone rather than a uniform full-longitude stripe.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyDiskRegion {
    pub longitude_rad: f64,
    pub latitude_rad: f64,
    pub half_length_rad: f64,
    pub latitude_width_rad: f64,
    pub density: f64,
}

/// Small bounded composition, independently reproducible from app inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyMorphology {
    pub complexes: Vec<SkyComplex>,
    /// Empty retains the legacy diffuse disk for synthetic regression fixtures.
    pub disk_regions: Vec<SkyDiskRegion>,
    /// Square cached focal charts; full mip chains are renderer-owned.
    pub detail_size: u32,
}

pub(super) const COMPLEX_LIMIT: usize = 4;

impl SkyMorphology {
    pub(super) fn valid(&self) -> bool {
        let point = |p: &[f64; 2]| p.iter().all(|v| v.is_finite() && v.abs() <= 2.0);
        let branches = |list: &[SkyBranch]| {
            list.len() <= 128
                && list.iter().all(|b| {
                    point(&b.start)
                        && point(&b.end)
                        && b.widths
                            .iter()
                            .all(|w| w.is_finite() && (0.0005..=0.8).contains(w))
                        && b.density.is_finite()
                        && (0.0..=8.0).contains(&b.density)
                })
        };
        !self.complexes.is_empty()
            && self.complexes.len() <= COMPLEX_LIMIT
            && self.disk_regions.len() <= 16
            && self.disk_regions.iter().all(|region| {
                region.longitude_rad.is_finite()
                    && region.longitude_rad.abs() <= std::f64::consts::PI
                    && region.latitude_rad.is_finite()
                    && region.latitude_rad.abs() <= 0.25
                    && region.half_length_rad.is_finite()
                    && (0.05..=1.5).contains(&region.half_length_rad)
                    && region.latitude_width_rad.is_finite()
                    && (0.015..=0.2).contains(&region.latitude_width_rad)
                    && region.density.is_finite()
                    && (0.0..=1.0).contains(&region.density)
            })
            && self.detail_size.is_power_of_two()
            && (256..=2048).contains(&self.detail_size)
            && self.complexes.iter().all(|c| {
                c.direction.is_finite()
                    && (c.direction.length() - 1.0).abs() < 1e-10
                    && c.half_extent_rad.is_finite()
                    && (0.05..=0.6).contains(&c.half_extent_rad)
                    && c.roll_rad.is_finite()
                    && c.roll_rad.abs() <= std::f64::consts::TAU
                    && c.emission
                        .iter()
                        .chain(&c.reflection)
                        .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                    && c.luminosity.is_finite()
                    && (0.0..=4.0).contains(&c.luminosity)
                    && point(&c.light_position)
                    && !c.clouds.is_empty()
                    && branches(&c.clouds)
                    && branches(&c.dust)
                    && c.cavities.len() <= 16
                    && c.cavities.iter().all(|v| {
                        point(&v.center)
                            && v.radii
                                .iter()
                                .all(|r| r.is_finite() && (0.01..=0.8).contains(r))
                    })
            })
    }

    /// Static background-population extinction at the authored bearing. Foreground
    /// stars must not use this attenuation. No observer or clock enters this query.
    pub fn background_transmission(&self, seed: u64, direction: DVec3, strength: f64) -> f32 {
        let mut depth = 0.0;
        for (index, complex) in self.complexes.iter().enumerate() {
            if let Some(p) = complex.project(direction) {
                depth += shape(seed ^ index as u64, direction, p, complex).dust_depth;
            }
        }
        (-depth * strength).exp() as f32
    }

    pub(super) fn capacity_bytes(&self) -> usize {
        self.complexes.capacity() * std::mem::size_of::<SkyComplex>()
            + self.disk_regions.capacity() * std::mem::size_of::<SkyDiskRegion>()
            + self
                .complexes
                .iter()
                .map(|c| {
                    (c.clouds.capacity() + c.dust.capacity()) * std::mem::size_of::<SkyBranch>()
                        + c.cavities.capacity() * std::mem::size_of::<SkyCavity>()
                })
                .sum::<usize>()
    }

    fn disk_density(&self, longitude: f64, latitude: f64, perturbation: f64) -> f64 {
        if self.disk_regions.is_empty() {
            return (-0.5 * (latitude / 0.072).powi(2)).exp();
        }
        let mut density = 0.0_f64;
        for region in &self.disk_regions {
            let delta = (longitude - region.longitude_rad + std::f64::consts::PI)
                .rem_euclid(std::f64::consts::TAU)
                - std::f64::consts::PI;
            let support = 1.0 - smoothstep(0.25, 1.0, delta.abs() / region.half_length_rad);
            let centre = region.latitude_rad + 0.025 * perturbation;
            let width = region.latitude_width_rad * (1.0 + 0.3 * perturbation);
            density = density.max(
                region.density * support * (-0.5 * ((latitude - centre) / width).powi(2)).exp(),
            );
        }
        density
    }
}

impl SkyComplex {
    pub(super) fn axes(&self) -> [DVec3; 3] {
        let reference = if self.direction.y.abs() < 0.9 {
            DVec3::Y
        } else {
            DVec3::Z
        };
        let right = reference.cross(self.direction).normalize();
        let up = self.direction.cross(right);
        let (s, c) = self.roll_rad.sin_cos();
        [right * c + up * s, up * c - right * s, self.direction]
    }

    fn project(&self, direction: DVec3) -> Option<[f64; 2]> {
        // Cone rejection avoids evaluating branch graphs over the whole sphere.
        if direction.dot(self.direction) < (self.half_extent_rad * 1.42).cos() {
            return None;
        }
        let [right, up, forward] = self.axes();
        let denominator = direction.dot(forward) * self.half_extent_rad.tan();
        let p = [
            direction.dot(right) / denominator,
            direction.dot(up) / denominator,
        ];
        (p[0].abs() <= 1.04 && p[1].abs() <= 1.04).then_some(p)
    }
}

fn smoothstep(low: f64, high: f64, x: f64) -> f64 {
    let t = ((x - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn branch_distance(p: [f64; 2], branch: &SkyBranch) -> (f64, f64) {
    let delta = [
        branch.end[0] - branch.start[0],
        branch.end[1] - branch.start[1],
    ];
    let length_sq = delta[0] * delta[0] + delta[1] * delta[1];
    let t = (((p[0] - branch.start[0]) * delta[0] + (p[1] - branch.start[1]) * delta[1])
        / length_sq.max(1e-12))
    .clamp(0.0, 1.0);
    let distance =
        (p[0] - branch.start[0] - delta[0] * t).hypot(p[1] - branch.start[1] - delta[1] * t);
    let width = branch.widths[0] * (1.0 - t) + branch.widths[1] * t;
    (distance - width, width)
}

struct Shape {
    cloud: f64,
    dust_depth: f64,
    rim: f64,
    cavity_rim: f64,
    texture: f64,
}

fn shape(seed: u64, direction: DVec3, p: [f64; 2], complex: &SkyComplex) -> Shape {
    let noise = |salt, scale| super::background::noise(seed ^ salt, direction.to_array(), scale);
    let broad = noise(0x1259, 37.0);
    let middle = noise(0x51a3, 119.0);
    let fine = noise(0xe01f, 397.0);
    let grain = noise(0x23bc, 1187.0);
    let q = [
        p[0] + 0.035 * broad + 0.012 * middle,
        p[1] + 0.042 * middle + 0.008 * fine,
    ];
    let mut cloud_distance = 10.0_f64;
    for branch in &complex.clouds {
        let (distance, width) = branch_distance(q, branch);
        cloud_distance = cloud_distance
            .min(distance - width * (0.30 * broad + 0.25 * middle) + 0.014 * fine + 0.005 * grain);
    }
    // Branch unions, not radial cloud envelopes, define the connected silhouette.
    let cloud = (1.0 - smoothstep(-0.025, 0.095, cloud_distance))
        + 0.16 * (-cloud_distance.max(0.0) * 16.0).exp();
    let mut dust_depth = 0.0_f64;
    let mut nearest_dust = 10.0_f64;
    for branch in &complex.dust {
        let (distance, width) = branch_distance(q, branch);
        let edge = distance + 0.009 * fine + 0.0035 * grain;
        nearest_dust = nearest_dust.min(edge.abs());
        let column = (1.0 - smoothstep(-width * 0.45, 0.014, edge)) * branch.density;
        // Connected branches overlap into knots; optical depth, not additive black.
        dust_depth += column * (1.0 + 0.16 * middle + 0.13 * fine);
    }
    let mut cavity_mask = 1.0;
    let mut cavity_rim = 0.0_f64;
    for cavity in &complex.cavities {
        let radius = ((q[0] - cavity.center[0]) / cavity.radii[0])
            .hypot((q[1] - cavity.center[1]) / cavity.radii[1]);
        let irregular_radius = radius + 0.11 * middle + 0.035 * fine;
        cavity_mask *= smoothstep(0.78, 1.08, irregular_radius);
        cavity_rim = cavity_rim.max((-(irregular_radius - 1.05).abs() * 16.0).exp());
        dust_depth += (1.0 - smoothstep(0.68, 0.94, irregular_radius)) * 2.0;
    }
    let boundary_fade = 1.0 - smoothstep(0.78, 1.02, p[0].abs().max(p[1].abs()));
    Shape {
        cloud: cloud * cavity_mask * boundary_fade,
        dust_depth: dust_depth * boundary_fade,
        rim: (-nearest_dust * 46.0).exp() * boundary_fade,
        cavity_rim: cavity_rim * boundary_fade,
        texture: (0.82
            + 0.43 * middle
            + 0.25 * fine
            + 0.14 * grain
            + 0.16 * (1.0 - fine.abs() * 2.0))
            .max(0.1)
            * (0.7 * middle + 0.45 * fine + 0.25 * grain).exp(),
    }
}

pub(super) fn radiance(
    morphology: &SkyMorphology,
    seed: u64,
    direction: DVec3,
    brightness: f32,
    dust: f64,
) -> [f32; 3] {
    // Subdued aggregate disk light supports the isolated focal structures. Its
    // grains are unresolved populations, never individually identified stars.
    let regional = super::background::noise(seed ^ 0x9001, direction.to_array(), 23.0);
    let disk = morphology.disk_density(
        direction.z.atan2(direction.x),
        direction.y.clamp(-1.0, 1.0).asin(),
        regional,
    );
    let grain = super::background::noise(seed ^ 0xf831, direction.to_array(), 431.0);
    let aggregate = 0.032 * disk * (0.7 + 0.6 * regional + 0.16 * grain).max(0.08);
    let mut color = [aggregate * 0.76, aggregate * 0.81, aggregate * 0.95];
    let mut total_depth = 0.0;
    for (index, complex) in morphology.complexes.iter().enumerate() {
        let Some(p) = complex.project(direction) else {
            continue;
        };
        let shape = shape(seed ^ index as u64, direction, p, complex);
        let distance_to_light =
            (p[0] - complex.light_position[0]).hypot(p[1] - complex.light_position[1]);
        let illumination = 1.0 / (0.7 + 2.2 * distance_to_light);
        let transmission = (-shape.dust_depth * dust).exp();
        // Blue reflection skins lie around absorbing silhouettes. Red emission
        // fills connected gas; light position breaks symmetry and supplies depth.
        let emission = shape.cloud * shape.texture * (0.65 + 0.7 * illumination) * transmission;
        let reflection =
            (shape.cloud * 0.24 + shape.rim * 0.65 * shape.cloud + shape.cavity_rim * 0.35)
                * illumination
                * transmission.sqrt();
        for (channel, value) in color.iter_mut().enumerate() {
            *value += f64::from(complex.luminosity)
                * (f64::from(complex.emission[channel]) * emission
                    + f64::from(complex.reflection[channel]) * reflection);
        }
        total_depth += shape.dust_depth * dust;
    }
    let aggregate_transmission = (-total_depth).exp();
    for (channel, value) in color.iter_mut().enumerate() {
        let unabsorbed = [0.76, 0.81, 0.95][channel] * aggregate;
        *value -= unabsorbed * (1.0 - aggregate_transmission);
    }
    color.map(|v| (v.max(0.0) * f64::from(brightness)) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_regions_have_dark_gaps_and_periodic_longitude() {
        let morphology = SkyMorphology {
            complexes: vec![],
            detail_size: 1024,
            disk_regions: vec![SkyDiskRegion {
                longitude_rad: 3.0,
                latitude_rad: 0.03,
                half_length_rad: 0.55,
                latitude_width_rad: 0.07,
                density: 0.8,
            }],
        };
        assert_eq!(morphology.disk_density(3.0, 0.03, 0.0), 0.8);
        assert_eq!(morphology.disk_density(0.0, 0.0, 0.0), 0.0);
        assert!(morphology.disk_density(3.0, 0.5, 0.0) < 1e-8);
        for latitude in [-0.1, 0.0, 0.1] {
            let left = morphology.disk_density(-std::f64::consts::PI, latitude, 0.2);
            let right = morphology.disk_density(std::f64::consts::PI, latitude, 0.2);
            assert!((left - right).abs() < 1e-14);
        }
    }

    #[test]
    fn tapered_branches_have_connected_variable_width_support() {
        let branch = SkyBranch {
            start: [-0.5, 0.0],
            end: [0.5, 0.0],
            widths: [0.2, 0.02],
            density: 1.0,
        };
        assert!(branch_distance([-0.45, 0.1], &branch).0 < 0.0);
        assert!(branch_distance([0.45, 0.1], &branch).0 > 0.0);
        assert!(branch_distance([0.0, 0.0], &branch).0 < 0.0);
    }
}
