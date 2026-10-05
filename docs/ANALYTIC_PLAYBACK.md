# Default analytic playback — Phase 5.13B

Normal launch, `--solar-system` and `--real-solar-system` select prescribed analytic
motion. `--gravity-orbits` selects the original Newtonian hierarchy; its circular
oracle remains selectable. Loading replaces the session, not a live conversion
of histories. Visual content, terrain definitions and navigation defaults remain.

## Approved content policy

The old solar initialization had globally barycentric position/velocity and an
Earth–Moon inner wobble. One ellipse per body cannot reproduce that compound motion
exactly. **Approved on 2026-10-05:** hold the Sun stationary at its previous epoch
position; planets use catalogue circles about the Sun, and the Moon circles Earth.
This removes solar reflex motion and Earth's inner wobble. At epoch Earth and Moon
move together by 292,894.815 m gameplay / 4,665,082.161 m real-scale. Other relative
catalogue positions remain unchanged. Zero COM/momentum conservation is not a
prescribed-mode gate. This is approximate authored content, not a new ephemeris.

Real-scale periods below freeze the original setup's circular pacing, including
Moon mass in Earth's setup period. They are stored numbers, not recomputed from
mutable masses/radii. Preset authoring scales them by
`orbital_distance_scale^1.5 / body_radius_scale`; gameplay retains its original
~0.250568 factor relative to real-scale periods. Subsequent property edits do not
change periods, plane, phase, hierarchy or spin.

| Body | Reference | Base period (s) | Epoch mean anomaly (rad) | Inclination (rad) |
| --- | --- | ---: | ---: | ---: |
| Sun | Stationary | N/A | N/A | N/A |
| Mercury | Sun | 7603291.579640426 | 0.15 | 0.122 |
| Venus | Sun | 19421516.906539556 | 2.1 | 0.059 |
| Earth | Sun | 31569669.97644 | 1.2 | 0 |
| Moon | Earth | 2359041.4475321984 | 0.73 | 0.08979719001510825 |
| Mars | Sun | 59376327.09925439 | 3.4 | 0.0322885911618951 |
| Jupiter | Sun | 374654349.4712371 | 4.2 | 0.022741640153486113 |
| Saturn | Sun | 936313251.3685672 | 5.0 | 0.04337143191205909 |
| Uranus | Sun | 2656215444.7528477 | 0.9 | 0.013491395117916168 |
| Neptune | Sun | 5199727113.931417 | 2.7 | 0.030892327760299633 |

All eccentricities are zero; working epoch zero is not a calendar date. The plane
is an X-axis rotation by inclination, +X periapsis. References contribute
translation/velocity, never spin. Spin keeps catalogue rotation periods/obliquities:
local +Y axis, epoch X-rotation by `pi/2 + obliquity`, positive
`TAU / rotation_period_s`. Tilts above 90° encode retrograde without a second
negative sign. Terrain remains body-fixed.

## Time, publication and editing

- Initially paused at 1000×. Changing rate alone does not resume. Finite signed
  rates, zero, pause/resume and fractional/negative seeks are direct. Single ±
  samples 60 seconds away, **not** an integration step. Reset returns to the
  authored epoch while retaining IDs/properties/focus.
- The foundation envelope remains: absolute instant/epoch and elapsed interval
  ≤2^36 seconds; ≤2^32 cycles. No clamp or gravity fallback. Failed requests pause
  and retain authoritative state/time/revision; target and last successfully
  published frame time stay distinct. Seek a supported time or Reset to recover.
- Unchanged valid paused/zero-rate times are not resampled. Hidden time is excluded;
  gaps above 250 ms cancel advancement/navigation and require Resume.
- Complete world sample → coherent frames → camera/render preparation.
  Frame-publication failure retains complete authority, pauses and suppresses
  drawing until rebuilt/coherent. Body-fixed observers inherit spin; translating
  attachments do not. Shared-ancestry f64 precision remains upstream of narrowing.
- Name/mass/radius edits use atomic world APIs and explicitly reconstruct bindings
  from unchanged immutable definitions. Definition validation precedes mutation;
  property/metadata edits cannot change compatible namespace/topology. Failed
  edits retain authority. External revisions are not automatically adopted.
  Velocity editing is visibly disabled in prescribed mode: velocity is the
  trajectory derivative. It remains available in Newtonian mode.

## Guides and history

Analytic dashed guides use authored ellipses at the reference's current published
translating anchor, with existing 64–512/1024 bounded tessellation and clipping.
Mismatched alternate references have no conic and an explanatory diagnostic.
Newtonian osculating guides are unchanged.

Analytic trails record one synchronized all-body sample per successful changing
publication, independent of Newtonian tick stride. Existing 8192-sample / 8 MiB
caps remain. Seek/reset/load/direction discontinuities clear/reseed. Large seeks
never generate intermediate history. Inertial and simultaneous body-relative
display use those actual samples; display changes do not rewrite history.

## Reproducible checks

```powershell
cargo test --locked -p mundaris_app --test analytic_playback_513b --test analytic_failure_513b
cargo test --locked --release -p mundaris_app --test analytic_playback_513b --test analytic_failure_513b
./scripts/ai-check.ps1 -OutputDirectory <fresh-directory>
cargo run --locked --release -p mundaris_app --features terrain-capture,surface-profile --example developer_capture -- analytic-playback <fresh-directory>
cargo run --locked --release -p mundaris_app --features terrain-capture,surface-profile --example developer_capture -- earth-orbit -31557600000.25 <fresh-directory>
cargo run --locked --release -p mundaris_app --example analytic_playback_profile -- <fresh-json-file>
./scripts/validate.ps1 -OutputDirectory <fresh-directory> -IncludeGpu
```

Captures share default solar authoring and motion sessions. [Schema 3](AI_DEVELOPMENT_INTERFACE.md)
separates requested/published time and mode-specific diagnostics. The profile retains
30 repeats per preset/time for complete sample/world commit, frame publication and
app `update`. It excludes render-time terrain admission, GPU, presentation and
native FPS; it is not an equal-model Newtonian solver speedup.

The opt-in `MUNDARIS_ANALYTIC_VALIDATE=1` native route uses ordinary commands for
playback, reverse, ±1000-year seeks and Earth/Moon attachment. Its
[runner](evidence/phase513b/native-playback.ps1) retains observational screen images
and snapshots, not guaranteed same-raster-frame pairs or physical-input acceptance.
Human control/visual approval and prior terrain/camera/UI gates remain separate.
No Phase 5.13C work is authorized by this integration.
