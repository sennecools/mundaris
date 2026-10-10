# astrum_simulation: motion over time

Loaded automatically when you work in this crate. Map note:
`D:/Astrum/Astrum/systems/Solar system and orbits.md`.

## Owns
Bounded analytic motion sampling (`analytic_motion.rs`, `orbital_elements.rs`),
serial Newtonian gravity with a kick-drift-kick integrator on integer fixed
ticks (`gravity.rs`, `integrator.rs`, `runner.rs`, `time.rs`), bounded
snapshots and replay (`history.rs`) and numerical diagnostics (`diagnostics.rs`).
Depends on `astrum_math` and `astrum_world`.

## Never
- No graphics or windowing dependencies.
- Never publish partial state: a failed batch (solver, time or arithmetic error,
  stale binding) leaves revision and instant unchanged.
- Analytic motion and integrated gravity are separate producers; do not merge them.

## Rules that bite
- Inertial SI f64 state, independent of reference radius.
- Analytic orbits have authored periods independent of mass; orbit references
  contribute centre position/velocity, not spin.
- Fixed integer ticks: this is the future network tick, keep it deterministic.

## Test
`cargo test -p astrum_simulation`. Benches: `gravity`, `fixed_steps`,
`analytic_motion`. Long-orbit checks live in `scripts/validate.ps1 -Only long-orbits`.

## Contracts
ADRs 0003, 0004, 0007; `docs/ANALYTIC_PLAYBACK.md`; `docs/engine-invariants.md`
("Independent systems and representations").
