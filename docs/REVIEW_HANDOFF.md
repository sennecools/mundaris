# Implementation-agent review handoff

Use the headings below when finishing an agreed task. Keep it focused; mark irrelevant
sections `N/A` and unavailable validation `UNTESTED` or `BLOCKED`, not passed.
Do not change acceptance criteria or hide failures to obtain a PASS.

## Goal

What was requested, the agreed scope, and the acceptance authority/criteria.

## Result

**PASS / PARTIAL / FAIL**, against those criteria. Name which gates passed and which
remain open. Implemented code or passing unit tests alone is not complete acceptance.

## Architecture changes

What actually changed, including ownership, shared APIs, precision boundaries,
resource/data-flow changes, and any deviations. State `None` if appropriate.

## Files changed

Important paths and their purpose. Separate task edits from pre-existing user work.

## Tests

Exact commands, exit status/results, platform, and relevant limitations. Say which
tests were not run and why. Distinguish environmental blockers from test failures.

## Measurements

Only when relevant: comparable before/after values, units, fixture, build/profile,
hardware, revision/dirty state, sampling method, and raw evidence paths. Separate
CPU preparation, GPU work, wall time, and memory; do not call speculation a speedup.

## Captures

Only when relevant: paths, reproduction command/settings, revision/dirty state,
settled/readiness/quality state, overlays, and paired diagnostic snapshot. Offscreen
captures do not alone prove native UX; report native interaction evidence separately.

## Known failures

Every known failing/partial gate, regression, risk, missing evidence, or blocked
prerequisite. Do not bury these in the success summary.

## Evidence

Paths to raw results, manifests, snapshots, capture indices, and reports. Identify
which evidence corresponds to this implementation versus historical/intermediate runs.

## Git state

Branch, HEAD, `git status --short`, and task commit if one was explicitly authorized.
If uncommitted, say so; HEAD alone does not identify a dirty build. Never commit/push
just to complete the handoff. Record enough input state to reproduce evidence.

## Reviewer follow-up

The reviewer inspects the actual diff/source and the relevant underlying evidence,
uses [REVIEW_CHECKLIST.md](REVIEW_CHECKLIST.md), and accepts/rejects claims with
limitations. A handoff summary is an index, not proof. Discuss the next step with the
user rather than automatically starting another phase.
