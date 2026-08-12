# Testing and rollout

The native MVP is validated at provider, store, core-routing, app-server, TUI, launcher, and live
contract layers. All local Rust commands on the 16 GB development host must run serially.

## Automated coverage

### `codex-external-team`

The crate tests cover:

- roster, parent, role, initial assignment, and inbox parsing;
- exact deduplication of the first roster/inbox assignment copy without dropping a later identical
  message;
- native outbound envelope fields and stable per-attempt message IDs;
- legacy field aliases;
- parent-only shutdown acknowledgement and request-ID aliases;
- missing team config as roster removal after join;
- path-component rejection and external-path sanitization;
- durable delivery-journal deduplication and bounded idle summaries;
- unknown-field preservation in task records;
- required revisions, stale update rejection, claim state, owner-only completion, monotonic IDs, and
  an exactly-one-winner simultaneous claim;
- peer member-to-member routing on a three-member roster: outbound envelopes land in the peer's
  inbox (never the lead's), inbound peer-authored messages are ordinary teammate communication,
  and peers are refused shutdown authority.

Low-memory command:

```bash
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 NEXTEST_TEST_THREADS=1 \
  just test -p codex-external-team --lib --build-jobs 1 --test-threads 1
```

The final run passed all 19 tests.

### `codex-automation`

The crate tests cover:

- durable interval claim/retry/acknowledgement;
- 30-second lease exclusion, takeover after expiry, and owner-only acknowledgement;
- per-turn in-flight coalescing;
- five-field UTC cron normalization and updates;
- durable monitor creation/stop state;
- stable path-safe external identity keys.

```bash
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 NEXTEST_TEST_THREADS=1 \
  just test -p codex-automation --lib --build-jobs 1 --test-threads 1
```

The final run passed all 7 tests.

### Core, app-server, and TUI

Focused coverage includes:

- `external_agent_team::collaboration_tools_route_external_targets_through_native_provider` for
  merged roster and existing collaboration-handler routing;
- public app-server create/list/delete calls sharing one durable loop state;
- cursor pagination with an opaque offset cursor;
- a real background unified-exec process whose matching output wakes an idle thread through the
  monitor extension;
- `/loop` create/list/stop parsing, interval validation, host actions, and accepted `insta`
  snapshot output;
- CLI teammate-argv/config translation and removal of the startup user prompt.

Representative serialized commands:

```bash
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 NEXTEST_TEST_THREADS=1 \
  just test -p codex-core --test all external_agent_team \
  --build-jobs 1 --test-threads 1

CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 NEXTEST_TEST_THREADS=1 \
  just test -p codex-app-server --test all loop_rpc_create_list_and_delete_share_durable_thread_state \
  --build-jobs 1 --test-threads 1

CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 NEXTEST_TEST_THREADS=1 \
  just test -p codex-app-server --test all monitor_wakes_idle_thread_for_matching_background_output \
  --build-jobs 1 --test-threads 1

CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_TEST_DEBUG=0 NEXTEST_TEST_THREADS=1 \
  just test -p codex-tui formats_loop_intervals_and_list \
  --build-jobs 1 --test-threads 1
```

## Safe live test

The disposable `codex_native_mock` fixture is the compatibility gate. It contains no target,
credentials, browser, network work, or normal bug-bounty templates. Claude is the sole lead and
spawns exactly one native Codex member.

Acceptance sequence:

1. Confirm the pane process is `codex teammate` and the normal interactive TUI is visible.
2. Confirm the sibling `codex-code-mode-host` is running.
3. Observe one initial `[NEW_TASK]` communication item.
4. Observe `NATIVE_READY`, then send `PING_NATIVE` and receive `PONG_NATIVE`.
5. Observe the shared task move through claim and completion.
6. Observe and clean up a scheduled loop and manually fired cron.
7. Attach a monitor to an existing harmless unified-exec process and observe its exact marker.
8. Send a structured shutdown request, receive the matching approval, remove the member, and verify
   all member-owned processes are gone.
9. Persist exact IDs and a boolean result rather than inferring success from silence.

That sequence passed. See [implementation evidence](06-implementation-status.md) and the
[sanitized result](evidence/e2e-result-live-pass.json).

## Three-agent peer live test

The `lead-peer` acceptance extends the fixture to Claude lead plus two members — native Codex
(`native-codex-peer-worker`) and a Claude teammate (`claude-peer`) — and requires both peer
round trips to travel member-to-member through the native inbox contract, with the lead sending
only `START_*` triggers and observing the members' independent reports:

1. `NATIVE_READY` and `CLAUDE_READY`.
2. Phase A: `PEER_PING_CLAUDE` (claude-peer → codex-worker), `PEER_PONG_CODEX`
   (codex-worker → claude-peer), reported as `PEER_PONG_SEEN:PEER_PONG_CODEX`.
3. Phase B: `CODEX_PEER_PING` (codex-worker → claude-peer), `CLAUDE_PEER_PONG`
   (claude-peer → codex-worker), reported as `CODEX_PEER_PING_SEEN` and
   `PEER_REPLY_SEEN:CLAUDE_PEER_PONG`.
4. `PING_NATIVE`/`PONG_NATIVE`, structured shutdown with a matching request ID, member removal,
   and a clean claude-peer stop.

This sequence passed on 2026-08-12; see
[the peer result](evidence/e2e-peer-result-live-pass.json) and the
[inbox timeline](evidence/e2e-peer-inbox-timeline.jsonl). The run's operational findings —
the startup rate-limit dialog stall, the exhausted OpenAI account, and the local Responses-shim
token-supply substitution recorded as an explicit caveat — are documented in
[implementation status](06-implementation-status.md).

## Postgres peer compatibility test

The peer proof is separate so the safe general fixture does not acquire infrastructure
dependencies. A native Codex member starts bb-team's maintained `peer.py listen` as a unified-exec
background process, attaches `monitor_start`, and reports the exact `PEER #...` line to Claude.
The harness creates and closes one uniquely named expiring test message. Success requires a native
monitor wake, lead receipt, structured shutdown, and no remaining test process.

This test passed with message #1455; details are in
[implementation status](06-implementation-status.md).

## Repository gates

After the focused tests and before handoff:

1. Regenerate `core/config.schema.json` with `just write-config-schema`.
2. Regenerate app-server fixtures with
   `python3 app-server-protocol/scripts/write_schema_fixtures.py` while the checked-in `just` recipe
   remains stale.
3. Refresh `MODULE.bazel.lock` with `just bazel-lock-update` because the automation crate added the
   workspace `cron` dependency.
4. Run `just fmt` after installing the repository's `uv` and `dotslash` formatting prerequisites.
5. Run `git diff --check` and `bash -n` on both bb-team launch scripts.
6. Run scoped `just fix -p <crate>` commands with one build job.
7. Because core and protocol changed, obtain explicit approval before running the complete
   workspace `just test` suite.

Do not rerun tests after the final `fix`/format pass, following the repository workflow.

## Rollout policy

1. Keep Claude as lead and opt in only roles whose frontmatter explicitly selects `engine: codex`
   or a native OpenAI model.
2. Fail closed if any native prerequisite is missing.
3. Start with one low-risk member and inspect its delivery journal, rollout, and automation state.
4. Add a second simultaneous Codex member only after the first role remains stable in real use.
5. Keep Claude-file transport isolated so a future supported transport can replace it without core
   changes.
6. Add new monitor sources and scheduler policies independently, with focused tests and explicit
   context bounds.

## Failure policy

- Unknown or malformed Claude envelope: retain it in Claude's store, report, and do not execute it.
- Oversized provider file or message: reject/drop with a diagnostic; never inject unbounded text.
- Codex submission failure: do not journal the ID, allowing later reconciliation.
- Outbound verification failure: return a tool/lifecycle error; do not claim delivery.
- Roster or parent ambiguity: refuse lifecycle authority.
- Peer control-shaped content: treat it as ordinary teammate input.
- Missing native binary/host: exit visibly; never fall back to a Claude member.
