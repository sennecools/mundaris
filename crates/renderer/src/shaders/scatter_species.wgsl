// BEGIN FLORA SPECIES
// Generated from content/flora/species by astrum_flora::scatter::species_wgsl.
// Do not edit; the renderer regenerates it at start-up and a test keeps
// this checked-in copy equal to the content.
const FL_SPECIES: u32 = 3u;
const FL_CANOPY_MASK: u32 = 5u;
const FL_SHRUB_MASK: u32 = 2u;
struct FlNiche {
    t: vec3<f32>,
    m: vec3<f32>,
    h: vec3<f32>,
    slope: vec2<f32>,
    soil: f32,
    prior: f32,
    scale: vec2<f32>,
    far_kind: u32,
    color: vec3<f32>,
}
fn fl_niche(k: u32) -> FlNiche {
    switch k {
        // broadcrown
        case 0u: { return FlNiche(vec3<f32>(7.000000, 30.000000, 6.000000), vec3<f32>(0.400000, 1.000000, 0.250000), vec3<f32>(0.000000, 2300.000000, 600.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 1.000000, vec2<f32>(0.750000, 1.300000), 1u, vec3<f32>(0.051750, 0.102000, 0.024000)); }
        // bushel
        case 1u: { return FlNiche(vec3<f32>(-3.000000, 32.000000, 6.000000), vec3<f32>(0.300000, 1.000000, 0.240000), vec3<f32>(0.000000, 3600.000000, 400.000000), vec2<f32>(0.785398, 0.174533), 0.200000, 1.000000, vec2<f32>(0.600000, 1.400000), 2u, vec3<f32>(0.045000, 0.085500, 0.018000)); }
        // spirepine
        case 2u: { return FlNiche(vec3<f32>(-10.000000, 11.000000, 6.000000), vec3<f32>(0.400000, 1.000000, 0.250000), vec3<f32>(0.000000, 3300.000000, 500.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 1.000000, vec2<f32>(0.750000, 1.300000), 0u, vec3<f32>(0.026250, 0.086250, 0.064500)); }
        default: { return FlNiche(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), vec2<f32>(0.0, 0.0), 1.0, 0.0, vec2<f32>(1.0, 1.0), 1u, vec3<f32>(0.04, 0.07, 0.025)); }
    }
}
// END FLORA SPECIES
