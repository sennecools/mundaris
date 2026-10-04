---
description: Independently review a completed milestone across invariants, Rust APIs, precision, and regression evidence.
subagent: false
---

Review this completed Mundaris milestone in the CURRENT PRIMARY session: $ARGUMENTS

Keep the primary agent/model unchanged. Establish the milestone's phase contract,
diff/base, and acceptance criteria. Ask for scope if missing. Follow AGENTS.md.

When genuinely independent, launch simultaneous read-only investigations:
- luna-review: architecture, authoritative/derived ownership, and phase invariants;
- luna-review: Rust/API correctness, transactionality, and bounded resources;
- luna-review: numerical precision, observer-relative paths, and LOD/topology math;
- luna-tests: focused tests/regressions and validation evidence, with exact commands.

Scale down for small/coupled changes; add discovery only if needed. Assign explicit
distinct scopes and command budgets; do not run several complete workspace suites.
Use subagent background: true when supported, concurrent tool calls otherwise, or the
read-only CLI fallback. Launch independent children before waiting and perform a
non-overlapping primary check while they run. No editing, commits, or pushes.

All children use openai/gpt-6-luna through ChatGPT OAuth. Review evidence, resolve
contradictions, discard unsupported claims, and synthesize findings by severity
with file/line references. Report clean reviews honestly, exact verification results,
remaining native/platform gaps, and observed concurrency. The primary owns the
final acceptance decision; workers' answers alone do not establish correctness.
