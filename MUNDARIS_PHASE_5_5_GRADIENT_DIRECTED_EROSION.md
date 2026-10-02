# Phase 5.5 — body-fixed gradient-directed procedural erosion

## Scope and acceptance

This checkpoint implements a stateless erosion-inspired mountain field before
terrain-aware adaptive selection, displaced mixed-LOD stitching and common-refinement
morphing. Those transition systems remain out of scope. Implementation, numerical
validation, performance measurements and visual acceptance are separate gates.
The original generic stateless ridge/valley shaping is **superseded as the primary
mountain erosion model by Phase 5.5** in generator V2; V1 remains available for
deterministic A/B comparisons and historical evidence.

No separate earlier Phase 5.5 document was present. This record translates the
checkpoint requirements into the executable spherical adaptation below. It is not
a claim that unseen reference formulas were reproduced or visually accepted.

## Mathematical construction

Terrain truth is queried by normalized, body-fixed direction n. The physical
reference position is R n. Each erosion octave owns a deterministically seeded
rotation Q and a physical lattice spacing lambda. Its eight enclosing lattice
vertices own fixed anchors a=lambda*j in Q-rotated body space. A feature samples
the preceding terrain field at the physical direction Q^T normalize(a).
The origin anchor has zero influence. No face UV, patch address, renderer
coordinate, tangent-chart identity or cache slot enters the pattern.

At an anchor, project the preceding analytic derivative into the sphere tangent:

```text
s = Q (g - n dot(g,n)) / R
t = normalize(a) cross s / sqrt(dot(s,s) + epsilon^2)
fade = dot(s,s) / (dot(s,s) + epsilon^2)
u = dot(t, Q R n - a)/lambda + hashed_offset(j)
feature = -stack * fade * exp(-u^2/(2*width^2))
```

epsilon=0.0002 is a dimensionless physical slope; width=0.12 and the hashed offset
is in [-0.5,0.5). Each feature is one non-periodic straight line along the anchor
downhill direction, not an infinitely repeated sinusoidal stripe field. Gaussian
rounding smooths the bottom and ridge shoulders; no absolute-value crease or
inverse-gully denominator exists. The normalized range is [-1,0], so
incision is bounded independently of seed, slope and scale. This adapts the
normalized/straight-gully principles without transplanting a planar shader.
Physical bottom width is 0.12 lambda; independent authorable ridge/bottom
radii are not implemented.

Features blend with the tensor-product quintic partition F(t)=6t^5-15t^4+10t^3.
F' and F'' vanish at cell boundaries. Both ownership and coefficients are fixed
per feature; only transverse distance and partition vary with a query. Neighbor cells use the
same shared features. Octaves use different seeded rotations to reduce aligned
lattice bias. This is chart-free and has no tangent-frame branch or pole.
Absence of visually recognizable lattice artifacts remains a visual gate, not a
consequence of continuity alone.

## Exact analytic feedback, not frozen query orientation

The gradient of a feature coefficient is zero because its preceding field is
sampled at a **fixed anchor**, not at the query position. The runtime derivative
includes the Gaussian derivative, the quintic product-rule term and the mountain
mask product rule. Consequently no numerical Hessian, frozen query-gradient
approximation or derivative-order truncation is needed.

The anchor callback for octave k recursively evaluates the full broad field plus
octaves 0..k-1 at that anchor. Thus smaller features really orient using gradients
modified by larger erosion. A bounded, direct-mapped 256-slot stack-local memo
avoids repeated anchor evaluations within one scalar or caller-owned batch call.
Batch reuse matters especially for a coherent geometry work chunk. Collisions
recompute; they cannot alter results. Depth is at most five. There is no persistent world flow state, allocation,
erosion map, hydraulic simulation, neighbor mesh pass or renderer dependency.
Recursion is deliberately a correctness-first baseline and its CPU cost is a
known limitation.

After each previous incision e_k with amplitude A_k, the anchor evaluation carries
`stack *= 1 + 0.5 * clamp(e_k/A_k,-1,0)`. The stack remains in [0,1]. Smaller
features are attenuated in existing deep creases and preserve larger ridges.
This stack is a fixed feature coefficient, not a slope-dependent query multiplier.
Slope fading occurs at anchors: zero gradient makes their features vanish, but a
query exactly at a peak can still receive influence from nearby sloped anchors.
Pointwise peak preservation is therefore not guaranteed by the fade alone.

Macro/range seeds and existing broad landform order are retained from V1. V2 removes
the Regional ridge/valley output and consumes its existing displacement budget for
erosion. Local and Fine bands remain additional bounded relief; they do not orient
erosion. Mountain masks control strong incision and vanish exactly at coverage zero.

## Scales, filtering and certificates

The regional band's metre wavelength defines lambda_0; subsequent spacings are
lambda_0/4^k. Every selected spacing must be at least 8 m. Configurations support
one through five octaves and strength [0,1]; the initial production default is
three. The benchmark hierarchy is 64/16/4/1/0.25 km; live default uses
128/32/8 km. Nothing depends on patch level. Physical amplitudes are inherited
from the existing terrain authoring bands; their preset radius scaling is not
a new erosion frequency rule.

Let B be the Regional band envelope. A_k=B*strength*2^(-k-1); their sum is strictly
less than B. Mask, stack and slope fade are in [0,1]. Positive erosion displacement
is zero, negative displacement is bounded by the amplitude sum. Existing symmetric
global intervals remain conservative and intentionally loose.

For unit-amplitude patterns, sum |Dw| <= 3.75 sqrt(3)/lambda and |Dfeature| <=
exp(-1/2)/(0.12 lambda). Thus G<=16/lambda. The partition Hessian row-sum is
<=40.125/lambda²; product and feature curvature terms add less than 136/lambda².
H<=192/lambda²
is conservative. Compose the smooth mountain mask with the existing analytic
product bounds; convert metre derivatives to unit-direction derivatives with R
and R². Certificate arithmetic widens outward.

For the partition derivative estimate, set v=t(1-t) in [0,1/4]. Then
F'^2/[F(1-F)]=900v/(10+15v+36v²)<=225/16 (with endpoint limits).
Weighted Cauchy–Schwarz over the tensor partition gives the stated
sum |Dw|<=3.75 sqrt(3)/lambda. This controls the product Hessian terms,
not just each independent Cartesian derivative.

The derivative-aware effective scale is R/max(G,R/lambda). Existing 4–8 footprint
activation applies. Exactly-zero weights skip all feature/anchor evaluation.
Feature orientations use the **complete unfiltered preceding field**, ensuring
that a footprint changes only each emitted octave's scalar weight. Therefore
`sum A_k*(1-w_k)` bounds omitted erosion exactly without an unaccounted feedback
residual. Active features still require their larger-scale anchor dependencies;
operation diagnostics count these actual operations. At coarse footprints with
all octaves suppressed there are zero erosion features. Spatial analytic
derivatives hold the footprint constant, as in the existing query contract.

## Identity, geometry and scheduling

V2 is a distinct immutable generator version. Configured octave count and strength
are part of exact TerrainConfig equality and hence geometry/cache identity. V1's
algorithm remains unchanged. Geometry generation continues to call the world query
once; f64 radial-graph normals remain authoritative. No per-patch erosion metadata
or payload is added. The cache's existing invalidation/LRU/pins/byte limits remain.

The uniform ready-cover display remains restricted to levels 0–4. The native
preview defaults to V2 when explicitly enabled. Set `MUNDARIS_PHASE55_AB=legacy`
for V1 with the same seed and existing validation route. The live update is reduced
to 64 vertices maximum below a 5 km footprint, while coarse work retains the
1,156-vertex ceiling to avoid starving uniform-cover replacement. Both use
8-vertex chunks and the existing 2 ms cutoff between chunks. This is not a hard
real-time deadline; a chunk can overshoot it.

## Evidence tools and remaining gates

`cargo bench --locked -p mundaris_world --bench terrain_erosion` compares scalar,
batch32 and batch289 V1/one-to-five V2 octave cases and six physical footprints.
`cargo bench --locked -p mundaris_app --bench planet_terrain_generation` includes
complete Grid16 misses, 8/16/32/64 chunks at coarse/fine profiles, and 4096-entry
cache residency. Value-only V2 is not supported. Setup is outside Criterion where
the existing harness uses batched setup; output and cache destruction remain timed.
Concurrent exploratory runs must not be presented as final isolated CPU evidence.

`cargo bench --locked -p mundaris_app --bench planet_terrain_erosion` separately
measures the exact live preset: scalar/batch32/batch289 footprints, complete Grid16
misses and coarse/fine chunk sizes. Its seed, band counts and physical erosion
scales differ from the historical fixture; validation tables distinguish them.

`cargo run --locked --release -p mundaris_app --example erosion_capture` creates
deterministic CPU inspection BMPs in `target/phase55-captures`. A/B use the same
seed, center and sampling footprint. Lighting deliberately exaggerates analytic
relief by 20x: these are inspection images, **not live Vulkan screenshots**.
Optional `erosion_diagnostics` exposes contribution, mask, crease, tangent gradient
and operation counts without adding permanent renderer dependencies.

Results and unresolved acceptance are recorded in `docs/phase-5-validation.md`.
Visually recognizable branching, no obvious cell repetition/axis bias, peak/valley
quality and live matched-camera mountain inspection must be established by actual
captures, not inferred solely from gradient feedback tests. Mixed-LOD stitching,
common-refinement morphing, replacement popping, coarse horizon and incomplete
terrain-aware adaptive selection retain their previous status.
