# Continuous camera navigation

The app owns one f64 observer. Selection is independent of focus and attachment.
No camera command changes celestial state, frame-tree origin, terrain definitions,
LOD thresholds or memory caps.

## Controls and explicit modes

Click the viewport to acquire keyboard navigation. UI widgets, labels, panels and
popups own their interactions; a panel-owned press cannot turn into a scene drag.
Releasing outside ends a gesture. Focus loss, resize/DPI changes, minimize/suspend
and excluded clock gaps cancel held keys, gestures and unfinished navigation targets.

| Mode | Pointer / wheel | Movement / attachment |
| --- | --- | --- |
| System orbit | Left drag; logarithmic pivot-distance wheel target | System overview pivot |
| Body orbit | Left drag; logarithmic complete-terrain clearance wheel target, labelled sphere fallback when unavailable | Focused body pivot |
| Surface Navigation | Right drag; scale-aware view-forward/recede wheel destination | Tangent WASD, local-up Q/E; follows body translation and rotation |
| Advanced Free Flight | Right drag; scale-aware view-forward/recede wheel destination | Camera-space WASD/QE; system-stationary, even with a translating numerical carrier |

`F` focuses selected, `I` enters Surface Navigation, `H` explicitly looks toward the
horizon, `Tab` selects next (`Shift+Tab` previous), `Home` recovers overview, `Esc`
explicitly enters Advanced Free Flight. Viewport-owned shortcuts are consumed before
UI focus traversal; focused UI widgets retain their keys. **Frame Selected** fits the
selected body independently of saved focus distance. **Body orbit** preserves the
incoming pose and retains its non-radial look offset. No altitude threshold switches
modes. Entering Surface Navigation neither approaches terrain nor resets orientation.
An incoming roll reconciles smoothly afterward, not at the entry instant.

Mode changes cancel old destinations rather than reinterpreting a radial target as
forward travel. New inward/outward wheel events retain their direction. Focus may
deliberately travel; input interrupts at the displayed pose, retaining Surface
Navigation when already in it, otherwise acquiring the requested orbit pivot.
Interruption never implicitly chooses Advanced Free Flight.

## Response, speed and protection

[Response equations and numerical gates](evidence/phase512b/RESPONSE_POLICY.md)
define projection/FOV/DPI normalization, distinct pivot/look gains, logarithmic wheel
steps, wall-time smoothing, clamps and transported heading/pitch. Pointer/wheel
deltas are consumed once, without multiplying by dt. Native deltas receive host
timestamps; integration splits at those events before advancing held motion. Paused
simulation and time warp do not change admitted editor wall time.

Wheel never adjusts FOV or the user multiplier. Use the separate **User speed
multiplier (×)** slider; hold Shift for temporary 4× boost. Diagnostics show base m/s
and source, dimensionless user and boost multipliers, and effective commanded m/s.
These are not measured simulation-frame velocities.

Base surface speed uses complete procedural clearance, not reference altitude or
coarse drawn geometry. Default Surface Navigation protects 1 m sampled radial
complete-terrain clearance, with a labelled sphere fallback. This is not collision
physics or a swept-volume guarantee. A focus recovery may escape outward from a
valid below-reference-sphere pose. Drawn mesh is measured separately and never clamps
approach; it can intersect the observer while quality is pending. **Approach (debug)**
and static fixture clearance setters are not ordinary-control route evidence.

## Diagnostics and reproducible evidence

The canonical [snapshot interface](AI_DEVELOPMENT_INTERFACE.md) is schema 2. It retains
semantic pose/frame/body associations and sphere/complete-terrain/drawn-mesh quantities.
Navigation diagnostics are optional for static captures; live controller diagnostics
include attachment policy (separate from numerical carrier), transitions, speed,
target meaning, sensitivity, safeguard and cumulative controller query counters.
Collection is observational: controller samples are reused, not regenerated for UI.

```powershell
cargo test --locked --release -p mundaris_app --test camera_navigation_512b -- --nocapture
cargo run --locked --release -p mundaris_app --features terrain-capture,surface-profile --example developer_capture -- navigation-route <fresh-directory>
```

The latter saves prepared-frame PNG/JSON checkpoints for overview → Earth orbit →
Surface Navigation → 2 m complete clearance → Moon → overview, plus route intent,
actual clearances and admitted time. Capture terrain-update budgets are separate from
navigation wall time; pending terrain does not prove settled visual quality.

For isolated native observation, set `MUNDARIS_NAVIGATION_SNAPSHOT` to a scratch JSON
path in a fresh directory before launch. This opt-in export writes each exact prepared
snapshot; it adds file I/O and is not a performance measurement mode. The native
[route recorder](evidence/phase512b/native-navigation.ps1) controls only its owned window.
Its screen images are compositor observations, not same-prepared-frame capture pairs.
Synthetic Windows messages do not establish physical-device feel or human approval.
