# OpenCode orchestration

## Reviewer / Plan mode

The user normally talks to **Plan** as Mundaris's reviewer, technical director, and
architect, then hands an agreed task to a separate **Build**/implementation session.
`AGENTS.md` defines both roles, user authority, evidence hierarchy, claim vocabulary,
and discussion-first behavior. The reviewer is a technical partner, not just a prompt
generator. A discussion does not authorize source changes.

Project-local `.opencode/agents/plan.md` overrides the built-in `plan` ID using the
documented OpenCode V2 Markdown agent mechanism. Selecting **Plan** in this repository
loads its reviewer prompt and permissions automatically; no invented mode hook,
`instructions` setting, or additional reviewer alias is used. It does **not** switch
an existing Build session to Plan, change the default agent, or choose a model.
Select Plan explicitly (or use `opencode run --agent plan` for a CLI session).

The profile denies actions by default, then allows reads/search, discussion questions,
skills/web research, Git inspection, and public OpenCode metadata inspection for
authentication verification. Edits (including built-in plan-file exceptions), editing
children, arbitrary shell commands, and unconfigured mutation/MCP tools remain denied.
`luna-explore` discovery is allowed; `luna-review` / `luna-tests` require approval since
focused trusted checks can execute code and write build artifacts. Verify the existing
ChatGPT OAuth connection before delegation; stop delegation on ambiguity. No editing
worker or general child is allowed from Plan. Read-only does not imply an OS sandbox:
Git allowlists must be used only for inspection, without write flags/redirection or
external helpers, and child permissions are their own profiles.

Reviewer startup: read `AGENTS.md`, then [REVIEWER_CONTEXT.md](REVIEWER_CONTEXT.md),
check repository status, inspect a relevant latest report only when useful, read
mechanics-reference sections/source as needed, then talk to the user. Do not load all
large evidence files every message. Use [REVIEW_HANDOFF.md](REVIEW_HANDOFF.md) for an
implementation report and relevant portions of [REVIEW_CHECKLIST.md](REVIEW_CHECKLIST.md)
for verification. Context is a dated orientation index, not acceptance authority.

Example: user asks "Shouldn't the GPU do this?" -> reviewer checks ownership and
current measurements, discusses suitable GPU/CPU work -> user agrees to one narrow
task -> separate coding agent implements and provides evidence -> reviewer inspects
the diff, snapshots, captures, and timings, then discusses the result/next step.

Supported mechanism: [V2 agents](https://opencode.ai/v2/docs/agents) and
[V2 permissions](https://opencode.ai/v2/docs/permissions). Verify effective discovery
with `opencode debug agents` after loading/reloading project configuration. Behavioral
instructions steer conversation; permission inspection alone does not prove every
future conversational response or native runtime outcome.

## Responsibilities and models

`AGENTS.md` supplies persistent project rules. In an implementation session, the
current primary owns architecture, dependency ordering, shared APIs, difficult
numerical/debugging decisions, integration, and final correctness within agreed scope.
In Plan, it owns technical discussion and independent review, not source edits.
For substantial tasks, delegate practical independent work within the active role;
prefer concurrent read-only investigations, usually 3–6 when genuinely useful.
Never create duplicate work merely to increase concurrency.

The primary model remains user-selected; neither project commands nor configuration
set a primary/default model. Select a strong model interactively, normally Sol High,
through the existing ChatGPT connection.

Every `.opencode/agents/luna-*.md` explicitly sets `openai/gpt-6-luna`.
On the verified setup, provider `openai` maps to the OpenAI integration's OAuth
connection and `https://chatgpt.com/backend-api/codex`, not an API-key connection.
Go and OpenRouter are separate providers and are not used by this workflow.
Model pinning alone does not guarantee authentication on another machine: verify
the connection before dispatch, and stop on ambiguity rather than substituting
another provider. Never export credentials or include account identifiers in reports.

## Agents and permissions

| Agent | Scope | Allowed operations |
| --- | --- | --- |
| `luna-explore` | Repository/specification mapping and focused discovery | Read, glob, grep; no shell or edits |
| `luna-review` | Independent Rust, invariants, regression, precision review | Discovery, Git inspection, focused Cargo test/check; no edits |
| `luna-tests` | Validation, failures, Rustdoc, benchmark evidence | Discovery, Git inspection, Cargo test/check/Clippy/doc/bench, check-only formatting; no edits |
| `luna-worker` | Isolated implementation or mechanical/docs changes | Discovery, edit tools, Git inspection, focused checks; explicit exclusive ownership required |

Agents use V2 ordered `permissions` rules with default deny. All deny nested
delegation and external-directory access; shell access is allowlisted and excludes
Git mutations, commits, and pushes. Sensitive environment-file reads are denied
except examples. Prompts prohibit credential inspection, source provenance claims,
architecture drift, and unassigned work. These are workflow controls, not an OS
sandbox: tests/build scripts execute code, and an editing worker's ownership list
is a parent-enforced contract, not a dynamically generated path permission list.
Only run trusted commands and review results. Docs/benchmark specialists are not
added because the core four already cover those distinct delegated scopes.

## Commands

```text
/fanout audit Phase 4 topology implementation before GPU integration
/parallel-review review the completed milestone against its phase contract
```

Both commands accept `$ARGUMENTS` and orchestrate in the current primary session.
They do not select an agent or model. `/fanout` maps dependencies, assigns distinct
scopes and budgets, launches appropriate Luna children, keeps useful critical-path
work in the primary, then synthesizes, reviews, integrates, and verifies.
`/parallel-review` supplies complementary invariants, Rust/API, precision, and
regression checks for a completed milestone; scale down if those overlap.

V2 supports foreground/background children through the `subagent` tool. When
exposed, launch independent children with `background: true` before waiting and
continue non-overlapping primary work. Completion notifications return results;
do not poll or duplicate running investigations. No special startup flag is required
by the verified v2.0.22 runtime. If a client does not expose background operation,
use supported concurrent tool calls or the CLI fallback below. Command discovery
does not prove execution concurrency; verify timings when troubleshooting.

For a major phase, useful independent scopes include existing API mapping,
regression discovery, numerical review, focused baseline tests, and an isolated
module or test harness. Surface-phase scopes can include adjacency/stitch indices,
LOD math, culling, precision paths, benchmark evidence, and documentation consistency.
Do not split tightly coupled reasoning, shared APIs, or integration for appearances.
Avoid simultaneous giant workspace suites; shared Cargo target locks may serialize
checks. Delegate focused locked commands; the primary owns final broad validation
using the README and CI workflow, including platform limitations.

## Windows hard-parallel fallback

Requires the installed OpenCode V2 CLI and Windows PowerShell 5.1 or PowerShell 7.
Use a task array, not repeated `-Task` parameters:

```powershell
./scripts/opencode-parallel.ps1 -Task 'Identify workspace crates', 'Identify latest ADR' -WhatIf
./scripts/opencode-parallel.ps1 -Task 'Identify workspace crates', 'Identify latest ADR'
./scripts/opencode-parallel.ps1 -Agent luna-tests -Task 'Run one explicitly scoped locked check', 'Inspect benchmark evidence without running benchmarks'
```

The helper accepts 1–6 tasks and only the three read-only profiles. It checks public
provider/integration metadata for a single OAuth connection on the ChatGPT route
and confirms the selected agent's model and denied edit permissions. It stops if
the connection is ambiguous, missing, or changed. It never changes global settings.
`-WhatIf` prints the plan without contacting the provider or starting jobs.

Each task starts a separate PowerShell job invoking
`opencode run --model openai/gpt-6-luna --agent <profile> --format json --file <task>`.
All jobs start before any wait. Prompts are attached as UTF-8 files, not interpolated
into shell commands. No auto-approval or shared session is used. Separate prompts,
JSON output, stderr, exit codes, and UTC start/end timings go under ignored
`.opencode/results/<run>/`; inspect the per-task files and `summary.json`.
Run as a script, not dot-sourced: any nonzero child exit returns failure. Retained
results may contain repository content, absolute paths, and local session metadata;
do not commit or share them unredacted. Remove individual completed run directories
when no longer needed. CLI exit success alone does not establish review correctness
or billing identity; verify assistant message model metadata and the OAuth route.

## Editing safety and worktrees

Never run overlapping editing workers, including overlap with the primary. Give
each worker explicit file ownership, inputs, acceptance criteria, and a verification
budget. Preserve pre-existing changes; return architecture/shared-API questions to
the primary. Workers report changed files and exact tests, and the primary reviews
their diffs rather than trusting summaries. No commits/pushes unless the user
explicitly requests a workflow requiring them; the worker profile intentionally
does not allow those operations.

For larger independent edits, prefer separately named Git worktrees from an agreed
baseline. Remember a worktree does not contain uncommitted changes from the main
checkout. Explicitly arrange any necessary inputs and copy project tooling if it
has not yet been committed. Assign one owner per worktree, review each diff, and
integrate deliberately. Remove only the specific worktrees after preserving useful
work; never force-clean unknown changes. A helper is deferred: no blind merging,
automatic commits, pushes, persistent worktrees, or branch cleanup is automated.

## Setup verification

This workflow was checked with OpenCode v2.0.22. Existing legacy agent permissions
were translated by that version; these definitions now use native V2 syntax.
Use `opencode debug agents` to inspect effective model/mode/permissions. The
`/api/command` endpoint must be queried with the repository location rather than
the server's default location to verify project command discovery. Root `AGENTS.md`
is the instruction mechanism; no JSON `instructions` setting is needed.

During setup, two read-only background children ran concurrently, and two separate
CLI fallback tasks also overlapped and exited successfully. Assistant message
metadata confirmed `openai/gpt-6-luna`; public provider/integration metadata confirmed
the ChatGPT OAuth route. This verifies the tested runtime, not every future client's
scheduler or subscription availability. Recheck after switching connections or
upgrading OpenCode. No engine work or Cargo dependency change is part of this setup.
