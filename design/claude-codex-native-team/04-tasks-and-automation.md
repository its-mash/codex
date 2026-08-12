# Tasks and automation

Communication, scheduled work, and process monitors converge on one native Codex wakeup primitive:
`Op::InterAgentCommunication`. They do not type into a terminal and do not create fake user turns.

See [unified-wakeup.puml](diagrams/unified-wakeup.puml).

## Shared Claude tasks

Claude's task directory remains authoritative. The external-team extension exposes:

```text
task_list
task_get
task_create
task_claim
task_update
task_complete
```

Every returned task includes a SHA-256 revision of the exact file bytes. Claim, update, and
completion require that observed revision and execute under the task-store lock.

| Operation | Required state transition |
| --- | --- |
| Create | new numeric ID, `pending`, no owner |
| Claim | observed revision matches and current state is `pending`; set owner to this member and state to `in_progress` |
| Update | observed revision matches; patch only supplied fields |
| Complete | observed revision matches, state is `in_progress`, and owner is this member; set `completed` |

This is compare-and-swap, not a read followed by an unlocked write. A concurrent-claim test runs two
claims against the same revision and asserts exactly one succeeds. Codex does not mirror the task
into a private store. Task assignment itself arrives through Claude's initial assignment or normal
message contract; this MVP does not watch arbitrary task-file changes and generate automatic turns.

## Durable automation store

`codex-automation` is independent of the Claude adapter. A Claude teammate keys its store by
`external--<team>--<member>`; an ordinary Codex thread keys it by thread ID. The same runtime is
used by model tools, app-server loop methods, and the TUI `/loop` command.

The versioned JSON state contains:

```text
jobs[]:
  id, name, prompt, kind, schedule, enabled
  created_at, next_due_at, last_run_at
  pending_fire_at, delivery_owner, delivery_lease_expires_at

monitors[]:
  id, name, process_id, prompt, contains
  poll_seconds, once, enabled, created_at
```

Writes use an advisory lock and atomic replacement. The state file is limited to 8 MiB and 1,000
combined jobs/monitors. Automation prompts are capped at approximately 4,000 tokens, and
model-facing task/automation tool results are capped at approximately 8,000 tokens (or the turn's
smaller truncation budget).

## Loops

Model tools:

```text
loop_create
loop_list
loop_stop
```

`loop_create` accepts exactly one of `every_seconds` or `every_minutes` plus a prompt. An interval
must be at least one second. `loop_stop` deletes by stable ID.

The TUI is a host surface over the same records:

```text
/loop 30s Re-evaluate the active work and continue the highest-value lane.
/loop list
/loop stop <id>
```

The TUI accepts `s`, `m`, `h`, and `d` units. App-server v2 exposes experimental
`loop/create`, `loop/list`, and `loop/delete`; list is cursor-paginated and the TUI follows all
pages.

## UTC cron

Model tools:

```text
cron_create
cron_list
cron_update
cron_delete
cron_run
```

Five-field expressions are normalized to second zero. Six- and seven-field expressions are passed
to the cron parser. All evaluation is UTC. `cron_run` submits an immediate native event without
altering the next scheduled occurrence.

The current scheduler intentionally implements one clear policy:

- a due occurrence becomes one `pending_fire_at` value;
- one runtime claims it with a random owner ID and a 30-second lease;
- another runtime cannot claim before lease expiry;
- only the current owner can acknowledge delivery;
- acknowledgement computes the next occurrence and clears the lease;
- after a job has delivered during an active turn, that job is excluded until the thread becomes
  idle, preventing repeated mailbox ticks from accumulating during a long turn.

Misfire catch-up queues, overlapping executions, selectable policies, and non-UTC time zones are
future extensions, not current behavior.

## Existing-process monitors

Model tools:

```text
monitor_start
monitor_list
monitor_stop
```

`monitor_start` attaches to an existing unified-exec process ID. It never starts a command. The
definition can match a substring, poll every 1–3600 seconds, and either fire once or continue. A
matching non-empty output chunk, process exit, or attachment failure becomes a bounded
`InterAgentCommunication` event.

The core polling seam requests at most 4096 output tokens and retains at most 64 KiB for each poll.
Polling selects over the monitor cancellation token and the whole automation-runtime cancellation
token, so shutdown does not wait for the polling timeout. Natural completion removes the runtime
cancel handle and disables one-shot or ended monitors in durable state.

The bb-team Postgres peer listener is supported by starting the maintained listener through unified
exec and attaching a monitor to its returned process ID. This proves the native integration without
adding Postgres logic to the monitor service.

WebSocket, file/directory, and HTTP sources are deferred. `monitor_read` is not part of this MVP;
the event includes the bounded matching output and the original process remains available through
unified exec when further output is needed.

## Unified delivery

External messages use authors under `/root/external/...`; automation events use
`/root/automation`. Both target `/root` and set `trigger_turn=true`.

```text
event arrives
  -> CodexThread.submit(Op::InterAgentCommunication)
  -> active thread: enqueue at the native mailbox boundary
  -> idle thread: start a native turn
  -> wait_agent: observe mailbox activity through the existing input queue
```

The scheduled prompt and monitor output are bounded before they become model-visible. No component
uses a shell sleep loop as its scheduler, a prompt hook as transport, or tmux keystrokes as wakeup.

## Relationship to other primitives

- Goal extension: persistent objective and automatic continuation semantics; unchanged.
- Queue extension: durable user-message queue; automation events are not user messages.
- `clock.sleep`: bounded waiting inside one active turn; not a durable schedule.
- Unified exec: owns command/background-process lifecycle and retained output.
- External-team extension: owns Claude roster, messaging, lifecycle, and tasks.
- Automation extension: owns loops, cron, delivery leases, and monitor definitions.
