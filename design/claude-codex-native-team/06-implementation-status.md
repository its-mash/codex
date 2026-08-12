# Implementation status and evidence

This file records what is implemented in this working tree and what was observed live. It is the
handoff point between the original design and later work on richer automation policies.

## Shipped working-tree slices

| Slice | Implementation |
| --- | --- |
| Native member launch | `codex teammate` translates Claude's captured teammate argv into ordinary Codex TUI configuration. |
| Provider boundary | `codex-extension-api` owns provider-neutral agent types; `codex-external-team` owns Claude files and envelopes. |
| Incoming communication | Polling reconciliation submits `Op::InterAgentCommunication`; a successful submit is journaled for restart deduplication. |
| Outgoing communication | Existing collaboration handlers resolve local agents first and external teammates second, then append and verify a native Claude inbox envelope. |
| Lifecycle | Initial assignment, final answer, idle notification, parent-only shutdown, roster removal, and missing-team removal are handled natively. |
| Shared tasks | Claude task files are exposed through list/get/create/claim/update/complete tools; every mutation requires the observed revision. |
| Automation | A separate durable automation extension owns interval loops, UTC cron, delivery leases, manual runs, and unified-exec monitors. |
| Host `/loop` | The TUI command and experimental app-server v2 loop RPC use the same thread automation store as model tools. |
| bb-team route | The launcher selects Codex from agent frontmatter, fails closed if the native binary or Code Mode companion is absent, and has no legacy bus or keystroke fallback. |

In teammate mode, the collaboration tools use the `external_team` namespace. This avoids claiming
the platform-reserved default collaboration namespace while still executing the existing Codex
collaboration handlers. Private Codex children continue to use the same handlers and local
`AgentControl` graph.

## Live Claude-led contract proof

The safe fixture is `/home/benty/bb-team/programs/codex_native_mock`. Claude Code 2.1.228 was the
lead and implicitly created team `session-662e2b7b`; its first background `Agent` call spawned
`codex-worker` through the exact configured teammate command. The member pane ran the normal
`codex teammate` TUI and spawned the sibling `codex-code-mode-host` itself.

The lead observed, in order:

1. `NATIVE_READY` from Codex.
2. A native `PING_NATIVE` message and `PONG_NATIVE` response.
3. Task #1 claimed with revision CAS and later completed by `codex-worker`.
4. A four-second loop wake (`LOOP_FIRED`) followed by deletion.
5. A UTC cron created, manually fired (`CRON_FIRED`), and deleted.
6. A monitor attached to existing unified-exec process 91670 and matched
   `SAFE_MONITOR_READY` (`MONITOR_FIRED`).
7. A structured shutdown request from the lead and a response with the same request ID.
8. Member removal with the Codex pane, Code Mode host, and monitored process all gone.

The sanitized result is committed as
[`evidence/e2e-result-live-pass.json`](evidence/e2e-result-live-pass.json). The original fixture
result remains beside the fixture. The run prohibited network access, browser access, MCP use,
credentials, real targets, and `tmux send-keys`.

The rollout showed one `[NEW_TASK]` item, revision-bearing `task_claim` and `task_complete` calls,
loop/cron/monitor inter-agent events, external `send_message` calls, and the exact monitor marker.
The delivery journal contains the initial assignment, lead ping, and shutdown request IDs.

## Three-agent peer-messaging proof

A third bounded run (2026-08-12, team `session-bf0bf5aa`, lead role `lead-peer`) proved
member-to-member communication with Claude as lead and TWO members: `codex-worker` (native
Codex runtime, subagent type `native-codex-peer-worker`) and `claude-peer` (ordinary Claude
teammate). Both peer round trips completed directly member-to-member, never relayed by the lead:

1. Phase A: `START_PEER_PING` → claude-peer sent `PEER_PING_CLAUDE` into codex-worker's native
   inbox; codex-worker replied `PEER_PONG_CODEX` directly to claude-peer; claude-peer reported
   `PEER_PONG_SEEN:PEER_PONG_CODEX` to the lead.
2. Phase B: `START_CODEX_PEER_PING` → codex-worker sent `CODEX_PEER_PING` directly to
   claude-peer; claude-peer replied `CLAUDE_PEER_PONG` directly to codex-worker; both members
   independently reported the leg to the lead (`CODEX_PEER_PING_SEEN`,
   `PEER_REPLY_SEEN:CLAUDE_PEER_PONG`).
3. `PING_NATIVE`/`PONG_NATIVE`, the structured shutdown request/response with a matching
   auto-generated request ID, member removal, and clean claude-peer stop all succeeded.

The lead's result is committed as
[`evidence/e2e-peer-result-live-pass.json`](evidence/e2e-peer-result-live-pass.json); a
polled snapshot of the durable codex-worker inbox traffic is
[`evidence/e2e-peer-inbox-timeline.jsonl`](evidence/e2e-peer-inbox-timeline.jsonl).

Operational findings from this run, preserved deliberately:

- A fresh Codex teammate can stall on an interactive rate-limit model-switch dialog that
  consumes its initial natively delivered turn; the journaled assignment is not redelivered, so
  the lead must re-wake the member with a new message once the operator dismisses the dialog.
  Suppressing interactive rate-limit prompts in `codex teammate` mode is a candidate fork
  change.
- The OpenAI account hit an account-wide usage limit mid-window, so the member's token supply
  was re-pointed at a Claude-backed model (`cc/claude-sonnet-5`) through a local
  OpenAI-compatible router. Everything else — the `codex teammate` runtime, TUI, mailbox
  reconciliation, collaboration tools, task CAS, and shutdown authority — stayed the native
  fork. The lead recorded this substitution as an explicit caveat in the result file, which is
  the honest reading: the run certifies the peer transport contract, not OpenAI model identity.
- The fork speaks only `wire_api = "responses"`, so the launcher gained a
  `cc/*`-model route that fails closed unless the local translation shim
  (`bb-team/.claude/codex-teammate/responses_shim.py`, systemd user unit `bb-responses-shim`)
  is answering. The shim translates namespace tool declarations (flattened as
  `<namespace>__<tool>` and reconstructed on the way back), `agent_message` input items (team
  communications must reach the model as user-side input, or the Claude backend rejects the
  conversation for ending on an assistant message), and absorbs the router's transient
  403 rotation windows with parsed-backoff retries, because Codex's own retry budget is too
  small to survive them.

## Postgres peer-listener proof

A second bounded run proved that bb-team's maintained Postgres peer listener works without a new
monitor source. Codex started the existing local command
`env -u BBTEAM_PROGRAM_DIR BB_PEER_ROOT=1 python3 .claude/lib/peer.py listen` through unified exec,
then attached `monitor_start` to process 89334. A single expiring self-addressed test message caused
the monitor to deliver the exact line:

```text
PEER #1455 from rog [inform] CODEX_NATIVE_MONITOR_E2E_20260812_0615
```

The Codex member sent `PEER_MONITOR_ARMED` and `PEER_MONITOR_SEEN` to the Claude lead, the test row
was closed, structured shutdown succeeded, and all test child processes exited. No existing peer
messages or listeners were modified.

## Implemented bounds and policies

- The provider reconciles at 50–5000 ms and keeps a bounded 10,000-ID delivery journal.
- Incoming Claude messages are limited to approximately 8,000 tokens before native submission.
- Outbound inbox updates use a per-process mutex, an advisory file lock, atomic replacement,
  retry, and message-ID post-write verification. A non-cooperating Claude writer can still race
  the read-modify-write sequence; the adapter remains replaceable when Claude exposes a supported
  transport.
- Task mutations are lock-protected CAS operations. Claim only accepts `pending`; completion only
  accepts `in_progress` owned by the current member.
- Schedules use UTC, persist to the member state directory, claim with a 30-second owner lease,
  retain one pending fire, and deliver at most one event for a job during a Codex turn. The
  in-flight set clears when that turn becomes idle.
- Monitor polling is limited to 1–3600 seconds. A monitor attaches only to an existing unified-exec
  process; it cannot launch commands. Output is bounded by the unified-exec poll contract at 4096
  tokens and 64 KiB before delivery. Local cancellation and thread shutdown interrupt polling.
- Automation prompts are limited to approximately 4,000 tokens; task and automation tool results
  are rejected before model delivery when they exceed the smaller of the turn policy and an
  8,000-token ceiling.
- App-server loop listing uses opaque cursor pagination (default 100, maximum 1000).

## Deliberately deferred

- WebSocket, filesystem, directory, and HTTP monitor sources.
- Catch-up queues, overlap policies, and non-UTC scheduler time zones.
- A supported Claude socket/RPC transport if Anthropic publishes one.
- Automatic task-change events beyond the initial assignment/message contract.
- Promoting Codex to lead or spawning Claude agents from Codex.

## Memory-safe local builds (16 GB host)

Do not run broad Cargo builds with the default parallelism on this machine. Use one compiler job
and one nextest thread:

```bash
cd /home/benty/codex/codex-rs
export CARGO_BUILD_JOBS=1
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_TEST_DEBUG=0
export NEXTEST_TEST_THREADS=1

cargo build -p codex-cli --bin codex --profile dev-small --jobs 1
cargo build -p codex-code-mode-host --bin codex-code-mode-host --profile dev-small --jobs 1
just test -p codex-external-team --build-jobs 1 --test-threads 1
```

`dev-small` disables development debug information and strips symbols, which materially lowers
link memory. Disabling test debug info and incremental compilation kept the rebuilt debug/test
tree to about 5.6 GiB; the previous symbol-heavy incremental tree reached roughly 94 GiB and filled
the filesystem. For a persistent machine-local default, put this in the user's Cargo config rather
than changing the repository-wide config:

```toml
[build]
jobs = 1

[profile.test]
debug = 0
incremental = false
```

The Code Mode companion pulls a `rusty_v8` archive. During this implementation the default sandbox
artifact URL returned 404, so the build used the matching Codex release archive and binding from
`target/rusty-v8-v150.4.0`. That artifact failure was distinct from the earlier memory exhaustion.

The checked-in `just write-app-server-schema` recipe currently names a removed Cargo binary. Until
that upstream recipe is repaired, the maintained generator used here is:

```bash
CARGO_BUILD_JOBS=1 python3 app-server-protocol/scripts/write_schema_fixtures.py
```
