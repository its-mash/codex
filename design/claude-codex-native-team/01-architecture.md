# Architecture

## System boundary

Claude owns the external team. Codex owns one teammate runtime and any private descendants that
teammate chooses to spawn.

```text
Claude team
├── team-lead                         Claude, authoritative parent
├── recon-challenger                  Claude teammate
└── recon-driver                      native Codex teammate
    ├── /root/js_analysis             private Codex child
    └── /root/api_review              private Codex child
```

Claude does not need to understand the private Codex graph. Codex does not mirror the entire
Claude team into `AgentControl` as fake local threads.

See [system-context.puml](diagrams/system-context.puml) and
[runtime-components.puml](diagrams/runtime-components.puml).

## Evaluated integration shapes

| Shape | CLI/TUI experience | Codex input semantics | Fork footprint | Decision |
| --- | --- | --- | --- | --- |
| Terminal bridge using tmux or hooks | Normal-looking but controlled by keystrokes | Claude traffic becomes fake operator input | Small initially, brittle permanently | Reject |
| Standalone bridge driving app-server RPC | Requires a separate TUI client or dual-process ownership | Can preserve structured input only after adding an RPC contract | Medium, plus bridge lifecycle and reconnection state | Keep only as a diagnostic option |
| Copy-only sidecar between Claude files and a Codex outbox | TUI can remain normal | Still needs a native Codex inbox/outbox contract | Duplicates retries, wakeup, and lifecycle outside Codex | Reject as the product architecture |
| In-process external-team extension | Ordinary TUI | Uses the existing agent-message mailbox | Small generic core seam plus isolated provider | Select |

The standalone app-server idea is technically viable, but it solves the wrong boundary for this
use case. `codex teammate` starts the normal TUI/runtime and the extension registry is installed in
that runtime's app-server host. This preserves the CLI experience and direct operator input while
avoiding a second integration process that must rediscover thread ownership. Current Codex models
also require the packaged `codex-code-mode-host` companion beside the `codex` executable; the
bb-team launcher checks that prerequisite before joining the team. The provider boundary still
allows a future out-of-process transport if Claude publishes a supported socket or RPC API.

## Existing mechanisms this design reuses

This is not a new parallel agent runtime. It composes mechanisms already present in this fork:

| Existing mechanism | Current location | Reuse |
| --- | --- | --- |
| Structured agent messages | `codex-rs/protocol/src/protocol.rs` | Preserve `InterAgentCommunication` provenance |
| Native mailbox admission | `codex-rs/core/src/session/handlers.rs` | Wake idle threads or steer active turns |
| Collaboration tools | `codex-rs/core/src/tools/handlers/multi_agents_v2/` | Route the same tools to local or external targets |
| Agent resolution | `codex-rs/core/src/agent/agent_resolver.rs` | Add a local-or-external result, preserving local precedence |
| Extension interfaces and thread data | `codex-rs/ext/extension-api/` | Carry a generic provider handle without Claude types in core |
| Thread wakeup from an extension | `codex-rs/ext/queue/src/service.rs` | Follow the weak-`ThreadManager` to `CodexThread` ownership pattern |
| Extension installation | `codex-rs/app-server/src/extensions.rs` | Install dormant external-team and automation extensions |
| Normal embedded app-server TUI | `codex-rs/tui/src/lib.rs` and `app_server_session.rs` | Keep the foreground interactive Codex experience |
| Code Mode companion | `codex-rs/code-mode-host` | Execute native tool orchestration for current models |

The current `/home/benty/bb-team/.claude/teammate-cc.sh` and
`/home/benty/bb-team/.claude/codex-teammate/codex-teammate.sh` implement the launch route using the
observed Claude argv and filesystem layout. Their Codex branch now contains no mailbox MCP, inbox
hook, scheduler sidecar, TUI-state detection, or tmux wakeup. It fails closed instead of silently
falling back to Claude when the native launcher or Code Mode companion is missing.

## Launch shape

The existing Claude launch interception remains the entry point:

```text
CLAUDE_CODE_TEAMMATE_COMMAND=/home/benty/bb-team/.claude/teammate-cc.sh
```

The shim keeps only environment preparation and engine selection. For a Codex model it executes:

```text
codex teammate \
  --team-name <team> \
  --agent-name <name> \
  --agent-id <id> \
  --agent-type <role> \
  --model <native-model> \
  --effort <effort>
```

`codex teammate` launches the ordinary Codex TUI with an external-team configuration attached to
its underlying thread. It does not launch a second renderer or require a standalone app-server.

## Generic external-team API

Add object-safe types to `codex-extension-api`. They define what Codex needs without exposing any
Claude paths or JSON formats:

```rust
pub struct ExternalAgent {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    pub status: ExternalAgentStatus,
}

pub enum ExternalMessageDelivery {
    Queue,
    Wake,
}

pub trait ExternalTeamProvider: Send + Sync {
    fn identity(&self) -> ExternalAgent;
    fn parent(&self) -> ExternalAgent;
    fn resolve_agent<'a>(
        &'a self,
        target: &'a str,
    ) -> ExternalTeamFuture<'a, Option<ExternalAgent>>;
    fn list_agents(&self) -> ExternalTeamFuture<'_, Vec<ExternalAgent>>;
    fn send_message<'a>(
        &'a self,
        target: &'a ExternalAgent,
        content: &'a str,
        delivery: ExternalMessageDelivery,
    ) -> ExternalTeamFuture<'a, Result<(), ExternalTeamError>>;
    fn interrupt<'a>(
        &'a self,
        target: &'a ExternalAgent,
        reason: &'a str,
    ) -> ExternalTeamFuture<'a, Result<(), ExternalTeamError>>;
}

pub struct ExternalTeamHandle {
    pub provider: Arc<dyn ExternalTeamProvider>,
}
```

The actual signatures should follow repository conventions for boxed, sendable extension futures.
The public trait requires documentation explaining provider ordering, delivery guarantees, and
identity stability.

## External-team extension

Create `codex-rs/ext/external-team`. It owns:

- provider installation and configuration;
- the Claude Code provider;
- inbound event watching and delivery journal;
- external roster resolution;
- outbound mailbox delivery;
- final-result and lifecycle forwarding;
- task tools in a later stage.

The app-server extension registry installs it alongside queue, goal, memory, automation, and other
extensions. The external-team extension is dormant unless the thread has an external-team
configuration.

At thread start it:

1. Parses the thread ID from the thread-scoped extension store.
2. Constructs the configured provider.
3. Stores `ExternalTeamHandle` in thread extension data.
4. Starts bounded polling reconciliation for the provider inbox and roster.
5. Resolves the `CodexThread` through the weak `ThreadManager`.
6. Submits inbound work as `Op::InterAgentCommunication`.

This follows the existing queue extension's ownership pattern and avoids adding Claude state to
`Session` or `AgentControl`.

## Existing collaboration-tool integration

Introduce a resolver result local to the collaboration implementation:

```rust
enum ResolvedAgentTarget {
    Local(ThreadId),
    External(ExternalAgent),
}
```

Update the existing handlers rather than adding parallel `bb_*` tools:

| Existing tool | External behavior |
| --- | --- |
| `send_message` | Provider `send_message(..., Queue)` |
| `followup_task` | Provider `send_message(..., Wake)` |
| `list_agents` | Merge local `AgentControl` agents and provider roster |
| `interrupt_agent` | Provider interrupt when supported; otherwise a clear unsupported result |
| `wait_agent` | No change; inbound provider events already signal the native mailbox |
| `spawn_agent` | No change; spawns a private Codex child |

Target resolution is defined in [target-resolution.puml](diagrams/target-resolution.puml).

In teammate mode these collaboration tools are exposed under the `external_team` namespace. The
namespace avoids colliding with the platform-reserved default collaboration schema; routing still
uses the existing Codex handlers rather than a provider-specific bus.

## Addressing

Do not change `InterAgentCommunication` from `AgentPath` to a broader type in the first stage. That
would create a large protocol and persistence migration.

Map external identities into a reserved valid path:

```text
Claude name              Codex communication author
team-lead                /root/external/team_lead
recon-challenger         /root/external/recon_challenger
```

Tool-facing references remain readable:

```text
team-lead
external:team-lead
```

Resolution precedence:

1. Thread UUID: local.
2. `/root/...`: local canonical path.
3. `external:<name>`: external exact match.
4. Bare name: local exact match, then external fallback.

This prevents external names from stealing canonical local paths while allowing Claude's hyphenated
names.

## Configuration

The implementation uses a nested configuration shape rather than several environment-only
switches:

```toml
[external_team]
provider = "claudeCode"
team_name = "github"
agent_name = "recon-driver"
agent_id = "recon-driver@github"
agent_role = "recon-driver"
parent_name = "team-lead"
```

`codex teammate` builds this configuration from Claude's argv. Credentials and program secrets stay
in the process environment and are not serialized into rollout metadata.

Changing `ConfigToml` requires regenerating `codex-rs/core/config.schema.json`.

## Dependency direction

The intended dependency direction is:

```text
codex-extension-api
        ^
        |
codex-external-team ----> codex-core public CodexThread/ThreadManager API
        ^
        |
codex-app-server installs external-team and automation extensions

codex-automation ----> codex-core public CodexThread/ThreadManager API

codex-core collaboration handlers ----> external-team types in extension-api only
```

`codex-core` must not depend on the concrete external-team extension crate because that would invert
the extension dependency and risk a cycle.

## Sources of truth

| State | Source of truth |
| --- | --- |
| Team name and membership | Claude team config |
| External parent | Claude `leadAgentId` plus configured parent name |
| Codex thread and history | Codex rollout/thread store |
| Local Codex descendants | Codex `AgentControl` and agent graph store |
| Claude shared tasks | Claude task store |
| Codex schedule definitions | Codex automation store |
| Monitor definitions and state | Codex automation store |
| Cross-runtime delivery dedupe | External-team delivery journal |

The automation store is keyed by the external team/member identity for teammates and by thread ID
for ordinary Codex sessions. `/loop`, app-server loop RPCs, and model tools all resolve that same
store.

No state is silently mirrored as an independent second source of truth.
