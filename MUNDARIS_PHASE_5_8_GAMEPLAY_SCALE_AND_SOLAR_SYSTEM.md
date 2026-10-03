# Phase 5.8 — gameplay scale and Solar System population

## Scope and acceptance

This is a **content/scale checkpoint**, not a change to the engine's supported
radius, Newtonian gravity, reference frames, terrain topology or precision path.
The ordinary development scenario is a populated gameplay Solar System. Original
Phase 3/4/5 fixtures and Earth-radius regressions remain available. Implementation,
measured evidence and outstanding visual/operator acceptance are distinguished in
[Phase 5 validation](docs/phase-5-validation.md).

## Reference scale and gravity

Earth's reference radius is **400,000 m**, diameter **800 km**, circumference
2,513.27 km and reference area 2,010,619 km². The catalog uses mean real radii and
the body-length factor `400000 / 6371000 ≈ 0.06278449223`. This factor belongs to
`SolarSystemPreset`, not coordinates, rendering, terrain selection or gravity.

Each entry independently specifies radius, reference gravity, rotation/obliquity,
orbit reference/state inputs, appearance and optional terrain identity/seed.
World continues to store independently editable positive mass and radius. Content
constructs `mu = g_reference × radius²`, then `mass_kg = mu / G` with the unchanged
CODATA simulation constant. Gameplay Earth has `mu = 1.569064e12 m³/s²`, reference
gravity **9.80665 m/s²**, escape speed **2,800.94984 m/s**. It does not retain
Earth's real mass. No gravity approximation, radius clamp or scale multiplier
enters the simulation kernel.

For the Sun the rule uses a 274 m/s² stellar reference gravity; gas/ice giants use
authored cloud-top reference gravities. These are mass-authoring reference levels,
**not walkable solid surfaces**. Real-scale mode uses the same table with unit
scales and therefore near-real masses, not a precision ephemeris or exact measured
gravitational parameters. Arbitrary mass/radius authoring remains separate from
both presets.

| Body | Gameplay radius (km) | Diameter (km) | Reference g (m/s²) | Terrain seed (hex) |
| --- | ---: | ---: | ---: | --- |
| Sun | 43,679.171 | 87,358.342 | 274 | none |
| Mercury | 153.175 | 306.351 | 3.70 | `4d455243` |
| Venus | 379.959 | 759.918 | 8.87 | `56454e55` |
| Earth | **400.000** | **800.000** | **9.80665** | `45415254` |
| Moon | 109.082 | 218.164 | 1.62 | `4d4f4f4e` |
| Mars | 212.808 | 425.616 | 3.72076 | `4d415253` |
| Jupiter | 4,389.327 | 8,778.653 | 24.79 | none |
| Saturn | 3,656.067 | 7,312.133 | 10.44 | none |
| Uranus | 1,592.340 | 3,184.681 | 8.87 | none |
| Neptune | 1,545.880 | 3,091.760 | 11.15 | none |

## Orbit and rotation authoring

`crates/app/src/solar_system.rs` centralizes the inspectable ten-body catalog and
both preset constructors. `body_radius_scale` and `orbital_distance_scale` are
independent fields. The first gameplay baseline intentionally chooses the same
numeric factor for both; no engine code requires them to be equal. Relative mean
orbital spacing is preserved, including the Moon. Earth's Sun distance is about
9.39243 billion m; Earth–Moon separation is about 24.1344 million m. There is no
additional nonuniform travel compression.

At the deterministic epoch planets have circular initial states at catalog phases
and inclinations, with `v = sqrt(G × (M_parent + M_orbiting_system) / distance)`.
The Earth/Moon barycenter follows Earth's solar initial orbit; their relative
states use the pair's actual scaled mass and separation and opposite mass-weighted
offsets. Finally all positions/velocities are shifted into the complete system's
inertial barycentric frame. Subsequent translation comes from ordinary mutual
N-body KDK at fixed **60 s**, not analytic scenery or permanent circular constraints.
Changing orbital scale reconstructs consistent speeds; it does not move bodies
inward while retaining old velocities. Initial circles are not exact multi-body
solutions, and guides are instantaneous two-body diagnostics, not predicted tracks.

Catalog orbit parents are content identity/grouping, never frame/spin ancestry.
Rotation periods are independently authored approximate real day lengths. Local
terrain +Y is the spin/latitude axis; initial orientation aligns it with the normal
to the system XY orbital plane before applying obliquity. Venus/Uranus obliquities
encode retrograde rotation without a second sign reversal. Day length is not
scaled with radius or orbital periods. Full outer moon systems need only additional
content and appropriate initial-condition composition; no moon systems are added
beyond Earth's Moon in this checkpoint.

## Terrain and appearance

Only Mercury, Venus, Earth, Moon and Mars receive world-owned V2 definitions.
Each has a distinct persistent terrain identity and seed, independent of runtime
BodyId/frame identity. Relief factors are respectively **0.35, 0.22, 1.0, 0.42,
0.76**; range wavelength factors are **0.8, 1.1, 1.0, 0.72, 1.15**.

Earth's continental band uses two base cycles per body, 4,200 m amplitude and three
octaves. Range organization uses `2.2 × radius × body_range_factor`, 2,200 m
amplitude and three octaves (Earth: 880 km base scale, with smaller octave scales).
Regional/local/fine bands keep physical base scales **25 km / 2 km / 160 m** and
amplitudes **900 / 220 / 35 m**, respectively. Amplitudes are multiplied by the
body's relief factor and `min(radius / 400000, 1)` for small-body envelope safety.
These are initial deterministic presets, not accepted continent/erosion morphology
or a biome system. Local detail remains queryable but resource-constrained live
geometry is not guaranteed to reach metre resolution.

Sun and giants remain on the existing physical sphere/far-body path, with no
rocky terrain definition or generated terrain budgets. Sun is an unlit simple
stellar placeholder. The catalog supplies grey/yellow/blue/red/tan/gold/cyan/deep
blue debug colors; final planetary/stellar materials, oceans, clouds and rings are
not implemented. Native terrain lighting can derive its body-fixed directional
light from the actual central star; the existing Phase 5.6 lighting presets remain
available for directed inspection. No atmosphere height is inferred from radius.

## Population admission, ownership and memory

`TerrainPopulation` is app composition used by native rendering and headless
population validation/captures. It refreshes projected terrain-aware far error
without patch generation, rejects bodies whose expanded sphere is outside the
frustum, and admits the rocky body with greatest error above the inherited
0.05-pixel prewarm threshold. One observer-local displaced cover shares the existing
cache and aggregate accounting. Other rocky bodies remain far-only; all gas/ice
giants and the Sun are far-only. This admission policy is not a physical draw-size
change and is not support for multiple simultaneously nearby terrain surfaces.

Far geometry remains the sole opaque owner until six generated roots establish
complete displaced coverage. Changing admission first prepares the source far
replacement, cancels its queued/partial generation, unpins its raw geometry and
releases its stitched/morph/selector state. Destination roots activate under the
same readiness rules. Unpinned raw entries remain reusable until ordinary LRU
pressure evicts them; BodyId, full terrain definition/revision and radius remain
cache identity. Inactive planets have no queued generation merely because they exist.

No worker generation, GPU generation, allocator, stitch topology or morph
optimization redesign is introduced. Live generation remains at most **64 samples**,
eight-sample microbatches and a **2 ms between-batch cutoff**; selector, stitching
and morph construction remain outside that cutoff. The **128 MiB aggregate ceiling
is unchanged**. Connected admission retains the prior conservative 16 MiB headroom
(112 MiB cache/admission quota), including retained cache/derived capacity and the
existing 72 MiB renderer staging reservation. CPU accounting is not process RSS;
GPU geometry retains its existing separate cap. There are ten persistent world
bodies and five lightweight surface handoff sessions, not five full terrain caches.

## Validation and retained limitations

Focused tests cover catalog identity/seed separation, size/gravity/rotation rules,
independent orbit scaling, Earth/Moon circular initialization, deterministic states,
resolved Newtonian advancement, far-only zero work, Earth→Moon→Mars→Earth transfer,
cache reuse and source-centred centimetre detail at actual system positions and a
shared `1e16 m` translation. Checkpoint terrain tests exercise 50/100/400/1000/6371/
12742 km radii; real-field transition regression adds 400 km without replacing
10 km or Earth-radius cases. Existing terrain-domain radius validation is unchanged;
these tests do not assert infinite numerical range.

Evidence, capture interpretation, diameter comparisons, performance and quality
command results are recorded in [validation](docs/phase-5-validation.md) and the
[capture index](docs/evidence/phase58/README.md). An 800 km default is not changed
merely to conceal performance costs or without qualitative evidence.

Final retained system view performs zero terrain generation. High orbit has
690 ready/690 visible patches; low orbit 693/219; ground 696/399. Steady population
update medians are respectively 3.9777/5.3620/5.0645 ms, with render preparation
10.6493/3.3697/6.3432 ms. These are directed headless host measurements, not native
FPS; all terrain scenes remain quality-pending. Ground peak aggregate accounting
is 117,440,115 bytes under the unchanged quota/ceiling. The early active morph
capture still required a 203.1621 ms worst construction. Different quotas, fields,
cameras and viewports make these captures unsuitable as like-for-like Phase 5.7
optimization evidence. The 600/800/1000 km diameter images demonstrate physical
size differences but do not establish preferred gameplay travel/morphology.

Phase 5.7 debt remains open: full-cover stitching previously **11–14.7 ms median**,
production morph construction **152.8 ms median / 736.4 ms worst**, heavy-motion
worst **169.11 ms**, incomplete requested-quality convergence. Smaller radii do not
solve those costs. Morphology/seed acceptance, displaced-horizon certification,
terrain-aware collision/walking/navigation, operator control feel, high-DPI/sleep
recovery, Linux native and current remote CI are still open. Stronger gameplay-world
curvature must not be artificially flattened. Biomes, atmosphere, final materials,
stellar rendering, terrain self-shadowing, rings/belts/minor bodies and all
stitch/morph/generation performance architecture changes remain deferred.
