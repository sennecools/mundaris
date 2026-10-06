# Codex orchestration

## Instructions and roles

Codex reads the existing root `AGENTS.md`; no rename to `agent.md` or duplicate
instruction file is needed. Start reviewer sessions with `AGENTS.md`, then
`docs/REVIEWER_CONTEXT.md`, then Git status and relevant source/evidence. Select
Plan for discussion/review and use a separate implementation session for agreed
edits. OpenCode's `.opencode/agents/plan.md` permissions are not a Codex profile.
Codex mode selection does not itself prove a read-only OS sandbox.

`.codex/config.toml` sets child defaults to `gpt-6-luna`, high reasoning, and at
most six concurrent children. It does not select the primary model. Actual tool
availability and session concurrency limits can be lower. Trusted projects load
project config; start a new chat/session to load newly added configuration.

Each `.codex/agents/luna-*.toml` is a standalone custom agent with `name`,
`description`, `developer_instructions`, explicit model/effort and sandbox defaults:

| Role | Assignment | Default sandbox |
| --- | --- | --- |
| `luna-explore` | Repository/specification discovery | `read-only` |
| `luna-review` | Independent correctness/regression review | `read-only` |
| `luna-tests` | Focused locked checks, failures, benchmark evidence | `workspace-write` for build/evidence artifacts; no source edits |
| `luna-worker` | Agreed edits with exclusive file ownership | `workspace-write` |

Children disable nested delegation and require ChatGPT login. Existing project
authority, evidence vocabulary, ownership and no-commit/no-push rules still apply.
Codex runtime permission overrides can supersede custom-agent sandbox defaults.
Ownership and command-budget instructions do not enforce path-level permissions;
these files do not reproduce OpenCode's per-tool ordered permission allowlists.
The parent must assign narrow scope and inspect actual results/diffs.

Verify `codex login status` reports `Logged in using ChatGPT` before delegation.
Never inspect/export credentials, silently substitute a model/provider, or use API
billing. Use custom roles when the spawn interface exposes them. If the desktop
interface instead exposes only a model override, select `gpt-6-luna`, high reasoning,
and pass the matching role's instructions with a fresh/scoped context. The native
tool's actual signature governs; do not invent a custom-role parameter.

Example request in a new Codex chat:

```text
Review this focused change. Use luna-explore to map the changed ownership and
luna-review to check invariants independently. Give them distinct scopes, launch
both before waiting, and verify their evidence. Keep the primary model unchanged.
```

OpenCode slash commands `/fanout` and `/parallel-review` are not imported into
Codex. Plain requests and the native subagent tools provide orchestration.

## Windows parallel launcher

`scripts/codex-parallel.ps1` supports Windows PowerShell 5.1 and PowerShell 7 with
the installed `codex.exe` on PATH (shell shims are rejected). It accepts 1–6 tasks, one of the three non-editing roles,
and a per-process timeout (default 10 minutes). Run as a script, not dot-sourced.

```powershell
./scripts/codex-parallel.ps1 -Task 'Identify workspace crates', 'Identify latest ADR' -WhatIf
./scripts/codex-parallel.ps1 -Task 'Identify workspace crates', 'Identify latest ADR'
./scripts/codex-parallel.ps1 -Agent luna-review -Task 'Review one specified invariant', 'Review one separate precision boundary'
```

`-WhatIf` prints tasks without starting Codex, contacting a provider or writing
results. For real runs, the launcher checks public ChatGPT login status, loads the
selected role's literal instructions, and supplies them explicitly to `codex exec`.
There is no assumed `codex --agent` flag. Every process pins `gpt-6-luna`, high
reasoning, the OpenAI provider, ChatGPT login, read-only sandbox, and no fresh
approvals or nested agents. It ignores user configuration and execpolicy rules to
avoid inheriting provider/plugin/hook/approval customizations; authentication still
uses the existing Codex home. Root `AGENTS.md` continues to load.

The helper's sandbox is **read-only for every role**, including `luna-tests`.
Checks requiring build writes must be returned as blocked; assign those to a
native validation child with an appropriate parent permission mode instead. The
launcher never accepts `luna-worker` or grants additional writable directories.

It starts all owned Codex processes before waiting, passes UTF-8 prompt bytes
through stdin without shell interpolation, drains stdout/stderr asynchronously,
and attempts to terminate its owned process trees on timeout or cancellation.
Cleanup errors do not skip later children and are retained in `launcher-errors.txt`.
A timeout,
nonzero child exit or missing final response fails the run. Results are saved as
UTF-8 under ignored `.codex/results/<run>/`: prompts, JSONL events, stderr, final
responses and a summary with exit codes and UTC process timings. Runs use
`--ephemeral` so separate CLI sessions are not retained in session history.
Process overlap does not prove simultaneous model inference or useful work.

Codex's protected runtime and result directory can require approval when this
launcher is invoked inside another restricted Codex session. A sandbox failure is
not a reason to disable the child sandbox. Logs may contain repository text and
local session metadata; review before sharing and do not commit raw results.

## Setup evidence

2026-10-05: baseline `ef40ed3c81c2a4b66f3cd359c508c8944cc5183d` on `main`, with
pre-existing untracked `experiments/`; setup changes are uncommitted. Installed
CLI: `codex-cli 0.160.0`. Public login status reports ChatGPT and the bundled model
catalog includes `gpt-6-luna`. A native Luna child independently reviewed launcher
requirements without editing. Two launcher smoke tasks returned exact Unicode
responses (`café ✓`) and exit 0 with final output, overlapping from
15:10:00.775 to 15:10:04.674 UTC. Local raw evidence:
`.codex/results/71a5d3f01b2246c7adc2bbd88ba48a6e/summary.json` and paired logs/finals.
This verifies the tested launch/encoding/concurrency path, not every future client,
subscription availability, editing-worker behavior, or engine acceptance.

VERIFIED separately: Windows PowerShell 5.1 returned the same exact Unicode reply
with exit 0 (`.codex/results/61ec1dbb07ff41cbbe5e86fc84d8d4c5/`). A strict-config
native role-selection smoke test reported that this CLI session exposes model
selection but no named custom-role control; neither requested child was spawned.
Its runtime header confirms `gpt-6-luna`, provider `openai`, high reasoning and
read-only sandbox; retained `native-roles.*` files are in the first run directory.
Use the explicit-model/instruction fallback above for this interface. Do not claim
named-role dispatch or per-role sandbox defaults were exercised by that test.
Timeout/cancellation fault injection and editing workers remain UNTESTED. No engine
source changed; Cargo, engine capture and engine performance validation are N/A.

VERIFIED after the cleanup fix: both PowerShell 7 smoke tasks again exited 0,
returned the exact Unicode replies and overlapped; final local evidence is
`.codex/results/8fbc9085956d45a9ab56c177a9e20fd1/`. Process starts were
15:19:02.074/15:19:02.086 UTC, finishes 15:19:05.763/15:19:06.788 UTC.

Relevant official references: [AGENTS.md discovery](https://learn.chatgpt.com/docs/agent-configuration/agents-md),
[custom subagents](https://learn.chatgpt.com/docs/agent-configuration/subagents), and
[configuration keys](https://learn.chatgpt.com/docs/config-file/config-reference).
OpenCode remains available via its [existing workflow](opencode-workflow.md).
