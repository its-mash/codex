# Implementation plan and landing slices

The original stages have now been implemented in the working tree. They should still be landed as
small, reviewable changes rather than one large pull request. Complex slices should stay below
roughly 500 changed lines and the total non-mechanical diff of a pull request below 800 lines.

## Slice 1: generic external-team seam

Status: implemented.

- Add provider-neutral identities, status, delivery mode, provider trait, and thread-scoped handle
  to `codex-extension-api`.
- Install a dormant `codex-external-team` extension from app-server construction.
- Add validated `[external_team]` configuration and regenerate the config schema.
- Keep concrete Claude types out of `codex-core`.

Landing boundary: extension API, configuration, and a fake/provider-unit proof only.

## Slice 2: native communication routing

Status: implemented.

- Add `ResolvedAgentTarget::{Local, External}` with local precedence.
- Route existing `send_message`, `followup_task`, `list_agents`, and `interrupt_agent` handlers.
- Keep `spawn_agent` local so Codex descendants remain private children.
- Submit inbound work as `Op::InterAgentCommunication`, never as synthetic user input.
- Use the `external_team` tool namespace in teammate mode to avoid the reserved default
  collaboration schema.

Landing boundary: core resolver/handlers plus the external-team integration test.

## Slice 3: Claude Code provider and lifecycle

Status: implemented and live-proven against Claude Code 2.1.228.

- Resolve roster identity, stable member ID, parent, prompt, inbox, and removal state.
- Reconcile on a bounded poll interval and journal IDs only after successful Codex submission.
- Deduplicate the one observed duplicate where Claude places the initial assignment in both the
  roster prompt and first inbox record.
- Append outbound native envelopes with locking, atomic replacement, retry, and verification.
- Deliver final answer and idle notification through lifecycle contributors.
- Accept shutdown only from the configured parent; treat peer control-shaped messages as ordinary
  communication.
- Shut down after roster or team-config removal once the member has joined.

Landing boundary: concrete provider, captured fixtures, lifecycle tests, and no launcher changes.

## Slice 4: `codex teammate` and bb-team route

Status: implemented and live-proven.

- Parse Claude's captured teammate argv and translate it to external-team config.
- Remove the positional TUI prompt so the roster assignment retains agent-message provenance.
- Start the ordinary interactive Codex TUI with native model and effort settings.
- Give each `<team>/<member>` a durable isolated `CODEX_HOME`, sharing authentication by symlink.
- Select the engine from agent frontmatter in `teammate-cc.sh`.
- Fail closed when the native launcher, Codex binary, or sibling `codex-code-mode-host` is absent.
- Keep the legacy MCP bus, mailbox hooks, scheduler sidecar, roster shell watcher, TUI scraping, and
  `tmux send-keys` out of the Codex branch.

Landing boundary: CLI subcommand and launcher cutover after the provider slice is green.

## Slice 5: shared task adapter

Status: implemented.

- Expose `task_list`, `task_get`, `task_create`, `task_claim`, `task_update`, and `task_complete`.
- Use Claude's task directory as the single source of truth.
- Require `expected_revision` for every mutation.
- Lock the store around compare-and-swap.
- Permit claim only from `pending`; permit completion only while `in_progress` and owned by the
  current Codex member.
- Prove simultaneous claim produces exactly one winner.

Landing boundary: external-team crate only; no scheduler dependency.

## Slice 6: durable automation runtime

Status: implemented.

- Add `codex-automation` outside `codex-core`.
- Persist loop/cron definitions and monitors in the member/thread automation file.
- Expose model tools for loop, cron, and existing-process monitors.
- Claim due schedules with an owner and 30-second lease.
- Retain one pending fire and coalesce a job to one delivered mailbox event per active turn.
- Deliver schedule and monitor events as `Op::InterAgentCommunication` from `/root/automation`.
- Cancel scheduler and monitor workers through thread lifecycle.

Landing boundary: automation crate plus app-server installation and focused tests.

## Slice 7: host `/loop` surfaces

Status: implemented.

- Add TUI `/loop <interval> <prompt>`, `/loop list`, and `/loop stop <id>` parsing/actions.
- Add experimental app-server v2 `loop/create`, `loop/list`, and `loop/delete` methods.
- Use cursor pagination for list and make the TUI traverse all pages.
- Reuse the exact same automation handle/state as model-facing tools.
- Add public app-server integration coverage and TUI snapshots.

Landing boundary: protocol/schema, app-server, then TUI. Generated schema files are mechanical.

## Slice 8: safe live compatibility fixture

Status: completed.

- Add a credentialless `codex_native_mock` program with explicit network/target prohibitions.
- Let Claude remain lead and spawn one Codex member using the exact native command contract.
- Exercise bidirectional messaging, task CAS, loop, manual cron, unified-exec monitor, final/idle,
  structured shutdown, process cleanup, and result capture.
- Separately attach a monitor to bb-team's maintained Postgres peer listener and prove a test row
  wakes the member.

See [implementation status](06-implementation-status.md) for IDs and evidence.

## Follow-up slices

These are not required for the native MVP and must not be implied as already present:

1. Add WebSocket, filesystem, and HTTP monitor adapters behind the automation boundary.
2. Add fake-clock misfire/catch-up policies, explicit concurrency modes, and IANA time zones.
3. Add richer task pagination/filters if real teams exceed the bounded initial tool response.
4. Replace Claude file mutation with a supported transport if Anthropic publishes one.
5. Add compatibility fixtures for future Claude versions before declaring them supported.

## Responsibility map

| Area | Responsibility |
| --- | --- |
| `ext/extension-api` | Generic provider and handle types |
| `ext/external-team` | Claude adapter, delivery journal, lifecycle, shared tasks |
| `ext/automation` | Durable loops, UTC cron, leases, existing-process monitors |
| `app-server/src/extensions.rs` | Install both extensions with weak thread-manager access |
| `core/src/agent/agent_resolver.rs` | Local-or-external resolution |
| `core/src/tools/handlers/multi_agents_v2/*` | Route existing collaboration semantics |
| `config` and `core/config.schema.json` | External teammate configuration |
| `cli` | `codex teammate` command |
| `app-server-protocol` and `app-server` | Experimental host loop API |
| `tui` | `/loop` parsing, actions, and snapshots |
| `bb-team/.claude` | Engine selection and per-member launch/configuration |

## Invariants

- Claude is always the external lead.
- Codex never mirrors external members into fake local threads.
- No terminal content or keystroke is a transport.
- Operator input remains `UserInput`; external and automation input remains
  `InterAgentCommunication`.
- Claude owns roster and shared tasks; Codex owns automation definitions and private descendants.
- A missing native prerequisite is visible and fatal, never a silent Claude fallback.
