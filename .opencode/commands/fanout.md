---
description: Decompose and orchestrate a task with concurrent Luna children while retaining the current primary model.
subagent: false
---

Orchestrate this Mundaris task in the CURRENT PRIMARY session: $ARGUMENTS

Keep the current primary agent/model unchanged. Follow AGENTS.md. The primary owns
architecture, hard reasoning, shared APIs, integration, and final review.

1. Understand the requested outcome and phase contract; map dependencies and the
   critical path. If the argument is empty, ask for a task before dispatching.
2. Identify genuine independent units. For substantial tasks aim for roughly 3–6
   concurrent read-only children where useful, without inventing work or delegating
   tightly coupled reasoning. Avoid duplicate scopes or expensive duplicated suites.
3. Give every child explicit scope, inputs, expected concise evidence/report, and
   verification budget. Dispatch luna-explore for discovery, luna-review for
   independent correctness, luna-tests for focused checks/benchmark analysis, and
   luna-worker only for isolated edits with exclusive file ownership.
4. Launch independent children before waiting: use subagent background: true if the
   runtime exposes it; otherwise issue concurrent supported tool calls or use
   scripts/opencode-parallel.ps1 for read-only tasks. Do not invent tool parameters.
    V2 supports background child sessions; use only facilities exposed by the runtime.
5. Continue useful non-overlapping critical-path work yourself. Do not poll/sleep,
   repeat children's investigations, or edit files they own. Prefer worktrees for
   larger independent edits. No overlapping editors; no child commits/pushes.
6. Gather reports, verify evidence, resolve contradictions, review important diffs,
   and integrate deliberately. Run final appropriate broad verification yourself.
7. Report outcomes, commands/results, remaining gaps, and whether concurrency was
   actually observed. All Luna children must use openai/gpt-6-luna via ChatGPT OAuth;
   stop on an unverified connection rather than using Go/OpenRouter/API billing.
