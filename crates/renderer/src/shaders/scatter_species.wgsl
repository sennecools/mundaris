// BEGIN FLORA SPECIES
// Generated from content/flora/species by astrum_flora::scatter::species_wgsl.
// Do not edit; the renderer regenerates it at start-up and a test keeps
// this checked-in copy equal to the content.
const FL_SPECIES: u32 = 10u;
const FL_CANOPY_MASK: u32 = 145u;
const FL_SHRUB_MASK: u32 = 878u;
// Planet foliage style (grass clumps follow it).
const FL_STYLISED: bool = true;
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
    // Expected crown area (m2) of one plant from above and from the side.
    crown: vec2<f32>,
}
fn fl_niche(k: u32) -> FlNiche {
    switch k {
        // niolivaeth-broadcrown
        case 0u: { return FlNiche(vec3<f32>(7.000000, 30.000000, 6.000000), vec3<f32>(0.400000, 1.000000, 0.250000), vec3<f32>(0.000000, 2300.000000, 600.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 1.000000, vec2<f32>(0.750000, 1.300000), 1u, vec3<f32>(0.579533, 0.078429, 0.000011), vec2<f32>(127.485573, 102.280724)); }
        // quimoul-bushel
        case 1u: { return FlNiche(vec3<f32>(-3.000000, 32.000000, 6.000000), vec3<f32>(0.300000, 1.000000, 0.240000), vec3<f32>(0.000000, 3600.000000, 400.000000), vec2<f32>(0.785398, 0.174533), 0.200000, 1.000000, vec2<f32>(0.600000, 1.400000), 2u, vec3<f32>(0.593004, 0.066994, 0.262654), vec2<f32>(19.052147, 12.990774)); }
        // susykil-flower
        case 2u: { return FlNiche(vec3<f32>(7.555458, 32.555458, 6.000000), vec3<f32>(0.300000, 0.900000, 0.150000), vec3<f32>(0.000000, 3640.682373, 400.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 1.101142, vec2<f32>(0.700000, 1.300000), 2u, vec3<f32>(0.550471, 0.133770, 0.125523), vec2<f32>(0.448071, 0.226547)); }
        // dredril-coral
        case 3u: { return FlNiche(vec3<f32>(8.199363, 32.199364, 6.000000), vec3<f32>(0.500000, 1.000000, 0.150000), vec3<f32>(0.000000, 2502.369873, 400.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 0.832513, vec2<f32>(0.700000, 1.300000), 2u, vec3<f32>(0.314052, 0.093261, 0.506236), vec2<f32>(0.126023, 0.206359)); }
        // somaen-crystal
        case 4u: { return FlNiche(vec3<f32>(-19.731379, 10.268622, 6.000000), vec3<f32>(0.000000, 1.000000, 0.150000), vec3<f32>(0.000000, 3410.156250, 400.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 1.128984, vec2<f32>(0.700000, 1.300000), 0u, vec3<f32>(0.178016, 0.159130, 0.803378), vec2<f32>(35.808365, 38.198502)); }
        // thoudrin-anemone
        case 5u: { return FlNiche(vec3<f32>(6.380127, 32.380127, 6.000000), vec3<f32>(0.600000, 1.000000, 0.150000), vec3<f32>(0.000000, 3527.624268, 400.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 0.994208, vec2<f32>(0.700000, 1.300000), 2u, vec3<f32>(0.059956, 0.156873, 0.623918), vec2<f32>(1.195008, 0.808693)); }
        // bave-frond
        case 6u: { return FlNiche(vec3<f32>(15.206469, 38.206467, 6.000000), vec3<f32>(0.450000, 1.000000, 0.150000), vec3<f32>(0.000000, 3273.054688, 400.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 1.001872, vec2<f32>(0.700000, 1.300000), 2u, vec3<f32>(0.133601, 0.119272, 0.554915), vec2<f32>(1.621435, 0.891497)); }
        // lytaethaeth-cap
        case 7u: { return FlNiche(vec3<f32>(3.825644, 29.825644, 6.000000), vec3<f32>(0.550000, 1.000000, 0.150000), vec3<f32>(0.000000, 3611.176758, 400.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 0.766218, vec2<f32>(0.700000, 1.300000), 1u, vec3<f32>(0.148212, 0.161400, 0.614960), vec2<f32>(70.714546, 46.430367)); }
        // naeshoulyth-gourd
        case 8u: { return FlNiche(vec3<f32>(16.113712, 41.113712, 6.000000), vec3<f32>(0.100000, 0.500000, 0.150000), vec3<f32>(0.000000, 2670.316162, 400.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 1.287122, vec2<f32>(0.700000, 1.300000), 2u, vec3<f32>(0.470251, 0.095011, 0.348037), vec2<f32>(1.354195, 1.782947)); }
        // shouvoushoun-bladder
        case 9u: { return FlNiche(vec3<f32>(3.373740, 28.373739, 6.000000), vec3<f32>(0.650000, 1.000000, 0.150000), vec3<f32>(0.000000, 2697.083252, 400.000000), vec2<f32>(0.698132, 0.139626), 0.000000, 0.761665, vec2<f32>(0.700000, 1.300000), 2u, vec3<f32>(0.702172, 0.115466, 0.041911), vec2<f32>(2.152643, 2.720985)); }
        default: { return FlNiche(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(0.0, 0.0, 1.0), vec2<f32>(0.0, 0.0), 1.0, 0.0, vec2<f32>(1.0, 1.0), 1u, vec3<f32>(0.04, 0.07, 0.025), vec2<f32>(0.0, 0.0)); }
    }
}
const FL_ROCKS: u32 = 2u;
// Bucket entry of rock 0 (after the MAX_SPECIES plant entries).
const FL_ROCK_ENTRY: u32 = 12u;
// Bedrock hardness envelope (min, max, falloff) of rock archetype `r`.
fn fl_rock(r: u32) -> vec3<f32> {
    switch r {
        // granite
        case 0u: { return vec3<f32>(0.550000, 1.000000, 0.100000); }
        // sandstone
        case 1u: { return vec3<f32>(0.000000, 0.500000, 0.100000); }
        default: { return vec3<f32>(0.0, 0.0, 1.0); }
    }
}
// END FLORA SPECIES
