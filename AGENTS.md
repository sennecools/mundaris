# Mundaris repository guidance

Follow the relevant phase specification, `docs/architecture.md`,
`docs/engine-invariants.md`, `docs/coding-standards.md`, and applicable ADRs.
Preserve existing user changes. Distinguish implementation from validation evidence.
Use focused, locked Cargo checks; final quality commands are documented in the README
and `.github/workflows/ci.yml`. Commit or push only when explicitly requested.
Do not mention model/tool/generated-code provenance in source, documentation,
commits, or metadata; development-tool configuration and usage are legitimate topics.

## Subagent orchestration

- For substantial work, first consider decomposition into independent discovery,
  review, tests, benchmarks, documentation checks, or isolated implementation.
  Delegate practical routine work to the configured `luna-*` subagents. The primary
  owns architecture, dependency ordering, shared APIs, difficult debugging/numerical
  reasoning, integration, and final correctness; use its strongest selected model
  for genuinely difficult decisions.
- Prefer concurrent read-only delegation aggressively: roughly 3–6 children when
  that many useful independent questions exist, never manufactured parallelism.
  Give each child a distinct scope, inputs, expected report, and verification budget.
  Launch independent children before waiting, using subagent `background: true` when
  available, or simultaneous tool calls/the CLI fallback. Continue non-overlapping
  primary work; do not poll or duplicate running investigations.
- Use `luna-explore` for repository/specification mapping, `luna-review` for
  correctness/invariants, `luna-tests` for focused validation and benchmark analysis,
  and `luna-worker` for scoped edits (including mechanical/documentation work).
  Delegate different focused checks; avoid duplicated full-workspace suites.
- Editing children need explicit disjoint file ownership. Never let two agents edit
  overlapping files concurrently, including the primary. Prefer isolated Git
  worktrees for larger independent edits; the primary reviews and integrates them.
  Workers follow existing architecture and return architectural questions to the
  primary. No child commits or pushes without an explicitly requested workflow.
- Review important results and edits, resolve contradictions, and run final
  verification. Keep tightly coupled critical-path reasoning in the primary;
  correctness takes priority over concurrency. Request concise actionable reports.
- Keep the current primary model user-selected. All `luna-*` agents use
  `openai/gpt-6-luna` through the existing **ChatGPT OAuth subscription**, never Go,
  OpenRouter, or API-key billing. Stop if that connection cannot be verified.

Examples of useful independent surface-phase questions: adjacency/stitch indices,
LOD-error math, culling, precision paths, benchmark evidence, and documentation
consistency. Choose only scopes that the task's dependencies genuinely permit.
See [OpenCode workflow](docs/opencode-workflow.md) for commands and execution details.
