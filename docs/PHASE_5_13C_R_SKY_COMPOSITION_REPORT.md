# Phase 5.13C-R — off-band composition revision

## Goal

User-approved follow-up: retain a believable galactic backbone but break its
uniformity, distribute nearby nebulae/clusters outside it, and preserve quiet dark
directions. This revises composition, not the camera, terrain, simulation or UI.

## Result

**DONE FOR NOW — visual/composition scope accepted by the user on 2026-10-05.**
The user reviewed the candidate and said "looks good enough for now" and requested
a commit and completion marking. This closes the requested visual iteration, not
the remaining engineering gates. Overall Phase 5.13C-R validation remains **PARTIAL**:
native foreground/display blockers, warm targets and the remaining precision/
latency gates remain explicitly open. The final full Windows quality matrix now
passes. No unavailable measurement is promoted to a pass.

Version 4 places three of four focal nebulae at galactic latitudes **+25.2°,
−28.6° and +18.3°**, retaining the red main structure near the disk. Their angular
sizes and orientations vary. Six compact, unequal app-authored diffuse regions
replace the full-longitude background stripe, varying its centre, width and density.

Stellar authoring changes from 78% disk / 22% isotropic to **60% disk / 10% local
off-band clusters / 30% isotropic**. The disk stars follow the same authored regions,
and local groups follow the off-band complexes. Exact deterministic catalogue
counts: **82,084** stars within ±10° of the plane, **41,933** outside ±15°, and
**661** inside the named quiet-sky 15° cone, out of 131,072 total. These are catalogue
distribution checks, not brightness measurements or visual acceptance scores.

OBSERVED: inspected toward/along/off-band/quiet captures and native-pixel detail
crops show spatially separated coloured structures with dark space around them.
The user's acceptance above is the authority for the current visual composition.

## Architecture changes

App-owned immutable `SkyMorphology` now includes at most 16 validated
`SkyDiskRegion` inputs. Diffuse evaluation uses periodic longitude and compact
support; empty lists preserve legacy synthetic-fixture behaviour. CPU definition
capacity includes the region allocation. The fixed renderer representations remain
one 2048×1024 base and four 1024×1024 filtered/mipped charts.

No changes to shader, GPU upload/draw implementation, catalogue admission cap,
finite distance range, observer envelope, extinction ownership or clock behaviour.
Star count remains 131,072; the two draws and 786,432 requested star vertices are
unchanged. No new dependencies. Existing presentation-only ownership is preserved.

## Files changed

Five source paths changed relative to this follow-up's retained starting manifest:

- `crates/app/src/sky_definition.rs`: preset version 4, off-plane bearings, mixed
  star populations, shared diffuse-region recipe and focused admission/composition tests.
- `crates/renderer/src/sky_structure.rs`: bounded region inputs, periodic diffuse
  evaluation, memory accounting and gap/wrap tests.
- `crates/renderer/src/sky.rs`: re-export the new decorative input type.
- `crates/renderer/tests/sky_capture_513c.rs`: explicitly retain legacy diffuse
  behaviour in the existing synthetic focal fixture.
- `crates/app/src/sky_capture.rs`: five fixed-bearing diagnostic views and unit test.

Phase evidence, this report, the evidence index and reviewer context are updated.
Pre-existing user work is preserved; source scope is checked against fingerprints.
Commit preparation also adds a `.gitattributes` rule to retain captured evidence
byte-for-byte rather than normalizing its original line endings. This preserves
the stored SHA-256 records; it does not change production source or rendering.

## Tests

Windows / AMD Radeon RX 9070 XT / Vulkan. Nine focused locked commands, exact
arguments, exit codes and stdout/stderr are retained in
`evidence/phase513cr/composition-20261005/checks-final/commands.json`:

- Eleven renderer sky unit tests pass in debug/release.
- Eight app sky unit tests pass in debug/release, including directional captures,
  off-band distribution, dark gaps, repeatability, field contrast and invalid inputs.
- Both explicitly selected adapter-required sky GPU regressions pass: finite-star
  centroid/cache/depth/toggle/invalid-state plus cached-mip seam/pole/return fixtures.
- Explicit same-foreground-state day/night GPU test passes.
- App/renderer all-target/all-feature Clippy with warnings denied, formatting and
  locked release all-feature app build pass.
- `scripts/ai-check.ps1` passes; paired final Earth PNG/JSON inspected. Its terrain
  remains `quality_pending=true`, `settled=false`; it is not settled terrain evidence.
- Evidence verifier checks **166** matched settings/PNG-size pairs, identical
  return/initial-motion images, one static upload and unchanged GPU payload.
- After user approval, `scripts/validate.ps1 -IncludeGpu` completes all **11**
  locked Windows quality commands with exit 0, including workspace debug/release
  tests, both Clippy configurations, rustdoc, long orbits and the three explicit
  adapter-required integration checks. Exact commands/results are retained in
  `full-validation-final/validation.json`.
- Separate post-approval sky GPU and day/night commands also pass, retaining raw
  stdout/stderr and exact timestamps in `post-approval-sky-gpu/commands.json`.
  Their named errors remain 0.002139788 physical px and 0.001413084 linear channel;
  day/night on/off differences remain 1/255 and 240/255. These are not warm profiles.

The final focused run follows a pole-domain roundoff guard added to the diagnostic
field evaluation. That clamp does not change the generated base/chart directions
in these captures; first-look PNGs precede that guard, and their recipe/appearance
inputs otherwise match the final source. No visual claim is inferred from tests.

## Measurements

Single cold captures only; not controlled profiling or first-visible latency:

| 1440p fixture | Version 3 | Version 4 |
| --- | ---: | ---: |
| Generation | 2227.7919 ms | 2457.6315 ms |
| Upload API | 7.1274 ms | 6.9781 ms |
| Owned GPU payload | 37,749,052 B | 37,749,052 B |
| CPU definition capacity | 6,305,880 B | 6,306,144 B |

4K cold generation is 2261.6295 → 2637.3337 ms. The separately calculated owned
transient generation-payload bound remains **53,477,372 B**, not peak RSS/VRAM.
This change does not demonstrate a performance improvement: generation is slower
in these observations. Warm CPU ≤0.1 ms p95 / GPU ≤0.5 ms p95 targets remain
unchanged and UNTESTED, deferred beyond this accepted-for-now visual checkpoint,
with ≥30 warmups/≥200 raw retained
on/off samples still required. Submission/readback and first-visible latency
remain unmeasured here.

## Captures

Open the [before/after viewer](evidence/phase513cr/composition-20261005/review.html).
It labels overview scaling and offers physical-pixel inspection/original PNG links.

Version-3 and version-4 directories each contain 23 stills and 60 paired raw motion
frames at 2560×1440 and 3840×2160, with identical poses, FOV and appearance controls.
New fixed views inspect upper/lower/warm off-band structures, quiet sky and the main
nebula. Existing toward/along/away, return, day/night and airless Moon remain.
`crops/` retain identical 768×512 physical-pixel rectangles without exposure,
resizing or sharpening. `field/` exports the actual base/focal pixels and raw linear
field samples. The sequence is 30 FPS / 0.08° / 1e12 m per frame; viewer playback
and selected-frame inspection do not prove native temporal stability.

## Known failures

No focused test failures in this revision. Native foreground capture and native
4K display limitations from the previous checkpoint remain BLOCKED, not repaired.
No additional native attempts were made. The user accepted the current visual
composition for now; native motion/UX, warm profiling, full precision matrix, cold
first-visible latency and the complete current-sky native route remain UNTESTED.
The final Windows quality matrix passes, but Linux/remote CI remains UNTESTED;
nothing here closes unrelated terrain/camera/UI acceptance gates.

The first post-approval full validation attempt exceeded an initial 120-second
command timeout during the debug suite after four passing checks; it is retained
as incomplete, not as a failed assertion. The retry removed that execution timeout
and passed; the debug suite took 432.89 s and release validation took 70.35 s.
The strict whole-checkpoint whitespace check also reports original whitespace in
raw logs, captured patches and the imported transcript. Those bytes are preserved
instead of rewriting historical evidence; the source/documentation check excluding
those inputs passes. See `commit-review/` for both results and evidence-byte checks.
An initial closeout verifier miscounted JSON arrays under Windows PowerShell;
direct array parsing corrected it without changing tests or their results. The
pre-correction summary is retained beside the final 11-command/two-command record.

## Evidence

[Revision evidence index](evidence/phase513cr/composition-20261005/README.md), source
fingerprints, matched-settings and resource JSON, raw captures/crops, focused logs,
paired fast checks and reproduction scripts. Version-3 baseline is built in an
isolated copied workspace with an independent target directory; only its capture
helper is refreshed so the new named views match. No baseline recipe is substituted.

## Git state

Implementation/validation baseline: HEAD `d81bb2a17ad4f8c1d06eaa1619b6281b946641c3`
with substantial pre-existing dirty/untracked work. The user explicitly authorized
committing the **whole current checkpoint**, including prerequisite phases and
retained documentation/evidence. The commit introducing this acceptance record
identifies that checkpoint. No push or unrelated cleanup is authorized.

## Reviewer follow-up

Visual/composition review is closed for now at the user's request, with the full
Windows quality matrix and separate sky GPU tests passed. Preserve the accepted
candidate; track native blockers and remaining performance/precision/latency gates
separately. Do not advance to another engine phase automatically.
