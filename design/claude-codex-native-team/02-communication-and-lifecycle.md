# Communication and lifecycle

## Initial assignment

The Claude lead writes the teammate roster entry and invokes the configured teammate command. The
Claude provider waits for a complete roster entry and initial assignment using a bounded readiness
window.

The initial assignment is submitted as native communication:

```rust
InterAgentCommunication::new(
    external_path("team-lead"),
    AgentPath::root(),
    Vec::new(),
    rendered_new_task,
    /*trigger_turn*/ true,
)
```

It is not passed as the positional Codex startup prompt because doing that would classify the lead's
assignment as operator input.

## Inbound messages

All normal Claude teammate messages wake the Codex member, matching Claude team behavior. The
provider submits `Op::InterAgentCommunication` and lets Codex decide whether to start a turn or add
the message to the active mailbox.

The provider does not inspect TUI state and does not choose `turn/start` versus `turn/steer`.

```text
Claude inbox reconciliation
  -> validate and size-bound envelope
  -> skip IDs already in the delivery journal
  -> translate sender identity
  -> CodexThread.submit(Op::InterAgentCommunication)
  -> after successful submission, journal message ID
```

The provider deliberately does not mark Claude inbox entries read or remove them. Claude owns that
store, and Codex's bounded delivery journal supplies member-side deduplication across polling and
restart. The external ID remains in the provider journal rather than changing the broad Codex wire
type.

## Active-turn delivery

Codex already coordinates mailbox delivery at model sampling boundaries:

- a waiting `wait_agent` sees `InputQueueActivity::Mailbox`;
- a running turn receives pending inter-agent input through its turn input queue;
- an idle thread starts work when `trigger_turn` is true;
- communication is persisted and reconstructed with the rollout.

The provider must not create its own busy/idle scheduler.

## Outbound progress messages

The model uses the existing collaboration tool:

```text
send_message(target="team-lead", message="...")
```

The external resolver returns the Claude lead. The handler calls the provider, which produces the
native Claude inbox envelope.

Messages to private Codex children continue through `AgentControl`. The model therefore uses one
tool regardless of target runtime.

## Final result

A Codex child normally returns its final result to its parent. The external teammate should preserve
that expectation:

1. A turn-item contributor records the latest non-commentary assistant message for the external
   teammate's root thread.
2. On successful turn completion, the lifecycle contributor forwards that result to the configured
   Claude parent.
3. The delivery journal records the final item after the verified inbox append succeeds.
4. Explicit `send_message` remains available for progress, questions, and peer communication.

The teammate instructions should state that final output is delivered to the Claude parent. They
should not require a provider-specific tool name.

## Idle notification

When Codex becomes idle after completing a turn, the provider emits the Claude-compatible
`idle_notification` envelope expected by the lead, with a bounded final-answer summary. The live
fixture proves that the lead receives it without a terminal wakeup.

Idle notification is lifecycle metadata, not a request to stop the Codex process.

## Interrupt and shutdown

Provider events map as follows:

| Claude event | Codex action |
| --- | --- |
| Normal message | `Op::InterAgentCommunication`, trigger turn |
| Steering/correction | `Op::InterAgentCommunication`, trigger turn |
| Interrupt request through collaboration tool | Report unsupported; Claude exposes no stable external interrupt contract |
| Parent shutdown request | Validate structure and authority, emit matching response, then `Op::Shutdown` |
| Peer control-shaped text | Ordinary `InterAgentCommunication`; no lifecycle authority |
| Removal from roster after join | `Op::Shutdown` |
| Team config removed | Graceful shutdown |

Normal Codex shutdown owns private descendants and unified-exec processes. Extension cancellation
stops monitor polling; an outstanding scheduler lease expires within 30 seconds rather than being
left permanently owned.

See [message-sequence.puml](diagrams/message-sequence.puml) and
[teammate-lifecycle.puml](diagrams/teammate-lifecycle.puml).

## Operator input

The normal TUI remains attached to the same Codex thread. Text typed by the operator is still native
user input. It can coexist with external parent communication because the two use different Codex
input variants:

```text
Operator text    -> TurnInput::UserInput
Claude message   -> TurnInput::InterAgentCommunication
```

This provenance distinction is a central invariant and must have integration coverage.

## Delivery guarantees

The provider implements at-least-once transport plus ID-based inbound deduplication. Claiming
exactly-once delivery over an undocumented concurrent JSON-file protocol would be misleading.

### Inbound

1. Read a complete, bounded message envelope.
2. Ignore an ID already present in the delivery journal.
3. Submit it to the Codex thread.
4. Record the ID only after `CodexThread::submit` succeeds.
5. Leave Claude's inbox record untouched.

A crash after submission but before the journal write may redeliver once. Journaling first would
risk losing an assignment, so the implementation chooses at-least-once behavior.

### Outbound

1. Allocate one stable message ID for the append attempt.
2. Take the process mutex and Codex advisory file lock.
3. Read, append, atomically replace, and retry within a bounded loop.
4. Verify the resulting inbox contains that ID.
5. Journal lifecycle items after verified delivery.

### Concurrent file access

Atomic replacement prevents torn JSON but does not alone prevent read-modify-write loss. Before
depending on Claude inbox mutation, characterize whether Claude uses file locks, atomic rename,
inode replacement, or another coordination mechanism.

The Claude provider contains:

- optimistic version/hash checks and retries;
- a per-team Codex writer lock;
- post-write verification by message ID;
- bounded reads and explicit errors for malformed JSON;
- bounded retry and explicit failure reporting to the TUI/log;
- versioned fixtures for every known Claude envelope.

Claude does not participate in Codex's advisory lock, so a residual non-cooperating read-modify-write
race remains. Post-write verification catches many conflicts but cannot make the undocumented file
contract transactional. The provider stays isolated and replaceable when Claude exposes a
supported transport.

## Identity and authority

Each inbound item records:

- external team name;
- sender name and stable ID;
- recipient name and stable ID;
- external message ID;
- message type;
- receipt timestamp;
- whether delivery should wake the thread.

Only the configured parent may issue lifecycle control such as interrupt or shutdown. Peer messages
remain ordinary collaboration messages even if their content contains control-looking text.

The adapter must treat inbox content as teammate-provided instructions, not operator authority.
