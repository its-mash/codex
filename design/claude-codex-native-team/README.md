# Claude-led, Codex-native agent teams

This design and implementation make a native Codex session a member of a Claude Code agent team
while Claude remains the only team lead.

The integration deliberately does not emulate a user typing into Codex. Claude teammate messages
and assignments, scheduled work, and monitor events enter the Codex runtime through its existing
`InterAgentCommunication` mailbox. Shared task mutations use native extension tools against
Claude's authoritative task store. Codex continues to use its normal TUI, model context, tool
runtime, persistence, and private Codex subagent graph.

## Fixed requirements

- Claude is always the external team lead.
- Codex is always a top-level Claude teammate, never the Claude team lead.
- Selecting a native OpenAI model in a Claude teammate definition launches native Codex.
- The ordinary Codex TUI remains visible and accepts direct operator input.
- No `tmux send-keys`, terminal scraping, synthetic keypresses, or prompt hooks are transport.
- Claude messages are not represented as operator/user messages.
- Codex's existing collaboration tools and mailbox drive communication.
- Claude-specific file formats remain isolated behind a provider boundary.
- Shared tasks, cron, `/loop`, and unified-exec monitors use native Codex tools and state.
- Fork changes stay generic where practical and small enough to land in reviewable stages.

## What "native" means

| Capability | Native owner | Integration behavior |
| --- | --- | --- |
| Claude roster and parent identity | Claude Code | Read through the Claude provider |
| Teammate launch and removal | Claude Code | Existing teammate command starts/stops `codex teammate` |
| Model execution and context | Codex | Unchanged Codex thread and rollout |
| Visible interactive UI | Codex TUI | Normal foreground TUI, not a custom renderer |
| Incoming team message | Codex mailbox | `Op::InterAgentCommunication` |
| Outgoing team message | Codex collaboration tool | Existing `send_message` with external-target resolution |
| Waiting for messages | Codex input queue | Existing `wait_agent` behavior |
| Private child agents | Codex `AgentControl` | Remain local descendants of the Codex teammate |
| Shared tasks | Claude task store | Native Codex task tools backed by the provider |
| Cron and `/loop` | Codex scheduler extension | Durable events injected into the Codex mailbox |
| Background-terminal monitors | Codex automation extension | Bounded unified-exec polling and event-driven mailbox wakeup |

## Implemented and proven

The working tree now contains the native MVP, not only the proposal:

| Capability | Status | Live evidence |
| --- | --- | --- |
| Claude-selected Codex member | Implemented | Claude 2.1.228 spawned `codex teammate` as `codex-worker` |
| Lead/member messaging | Implemented | `NATIVE_READY` → `PING_NATIVE` → `PONG_NATIVE` |
| Member↔member (peer) messaging | Implemented and proven both directions | `PEER_PING_CLAUDE`/`PEER_PONG_CODEX` and `CODEX_PEER_PING`/`CLAUDE_PEER_PONG` exchanged directly between a Claude member and the Codex member, never relayed by the lead |
| Shared tasks | Implemented with required revision CAS | task #1 moved `pending` → `in_progress` → `completed` |
| Loop and cron | Implemented with durable one-owner leases | scheduled loop and manual cron both woke the active native turn |
| Monitor | Implemented for existing unified-exec processes | local marker and bb-team Postgres peer listener both woke Codex |
| Shutdown | Implemented with parent-only authority | matching structured request/response, pane and child host exited |

The full Claude-led single-member run is recorded in
[`evidence/e2e-result-live-pass.json`](evidence/e2e-result-live-pass.json), and the three-agent
peer-messaging run (Claude lead, Codex member, Claude member) in
[`evidence/e2e-peer-result-live-pass.json`](evidence/e2e-peer-result-live-pass.json). The
Postgres peer proof, the peer-run operational findings, and the memory-safe build procedure are
summarized in [`06-implementation-status.md`](06-implementation-status.md).

WebSocket, filesystem, and HTTP monitor sources are deliberately outside this MVP. They can be
added behind the same automation interface without changing Claude's team contract.

## Documents

- [Architecture](01-architecture.md)
- [Communication and lifecycle](02-communication-and-lifecycle.md)
- [Implementation plan](03-implementation-plan.md)
- [Tasks and automation](04-tasks-and-automation.md)
- [Testing and rollout](05-testing-and-rollout.md)
- [Implementation status, evidence, and low-memory builds](06-implementation-status.md)

## Diagrams

- [System context](diagrams/system-context.puml)
- [Runtime components](diagrams/runtime-components.puml)
- [Message sequence](diagrams/message-sequence.puml)
- [Target resolution](diagrams/target-resolution.puml)
- [Unified wakeup path](diagrams/unified-wakeup.puml)
- [Teammate lifecycle](diagrams/teammate-lifecycle.puml)

## Primary decision

Implement an in-process external-team extension, not a terminal bridge and not a standalone
app-server client.

The extension uses the same pattern already used by other Codex extensions: it receives a weak
`ThreadManager`, obtains the appropriate `CodexThread`, and submits a native `Op`. A small resolver
addition lets the existing collaboration handlers target provider-owned agents. Claude-specific
roster, mailbox, shutdown, and task behavior lives in the provider implementation.

The bb-team launch shim now executes the native path. The older bus, hook, scheduler, and roster
watcher files remain characterization material only and are absent from the Codex launch path.
