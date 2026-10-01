# ADR 0003: Authoritative celestial state, time and disposable frame projection

Status: accepted for Phase 2 implementation.

## Decision

`SimulationInstant` lives in `mundaris_math`: a checked dimensioned value in seconds
relative to a local working epoch. This matches math's metres/seconds/radians
charter. Playback rate, pause and requested-time control belong in simulation.
World depends on math; simulation currently depends only on math and can later
depend on world without a cycle. A world-owned instant was also viable; duplicating
types, raw seconds throughout world, and moving a clock into core were rejected.

`CelestialSystem` owns append-only bodies, opaque caller-namespaced `BodyId`,
positive mass/reference radius, system-space center/velocity, unit orientation,
system-axis angular velocity, one sample instant and a checked revision.
Names are display metadata (1–128 UTF-8 bytes); no category or primary hierarchy
controls physics. Validated types make invalid numeric states unrepresentable.
Authoring edits and simulation batches share transactional mutation foundations.
Advancing the sample instant requires all bodies; subsets can edit the same instant.
Caller ordering is unrestricted; reusable dense duplicate flags keep batches O(U).

The world-owned projection contains a generic Phase 1 tree and dense associations.
Each body has a system-root translating anchor and its own rotating child. The
projection is reconstructable and never supplies domain identity. Tree namespaces
are supplied explicitly and must be fresh for each build, as in Phase 1; no global
atomic allocator is introduced. Publishing appends mappings without replacing old
ones and records a revision only after coherent frame publication succeeds.

The app samples a bounded analytic fixture, explicitly commits world state, then
publishes frames. Ordinary immutable borrowing provides coherent inspection and
rendering. The renderer remains domain-agnostic. No gravity/integrator, persistent
identity, deletion, initial/current duplication, snapshot framework or seek policy
for future integrated simulations is implied.
