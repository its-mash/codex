//! Fork-owned: external agent-team support for the MultiAgentV2 tools.
//!
//! When this thread runs as a native Codex teammate inside a team whose control
//! plane belongs to another runtime (Claude Code), the collaboration tools must
//! be able to (a) resolve teammate names that are not local Codex agents,
//! (b) deliver messages / interrupts through the external provider, and
//! (c) send `message` arguments in plaintext, since the runtime has to read them
//! to forward them off-process.
//!
//! Everything lives here so the upstream handler files only carry one-line hooks.

use super::message_tool::MessageDeliveryMode;
use super::*;
use crate::agent::control::ListedAgent;
use crate::config::Config;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolCallSource;
use codex_extension_api::ExternalAgent;
use codex_extension_api::ExternalAgentStatus;
use codex_extension_api::ExternalMessageDelivery;
use codex_extension_api::ExternalTeamHandle;
use codex_protocol::ThreadId;
use codex_tools::ToolSpec;
use futures::future::BoxFuture;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Target resolution
// ---------------------------------------------------------------------------

pub(crate) enum ResolvedAgentTarget {
    Local(ThreadId),
    External {
        handle: Arc<ExternalTeamHandle>,
        agent: ExternalAgent,
    },
}

/// Resolves a tool-facing agent target to a local thread or an external teammate.
///
/// Local resolution is delegated to upstream's [`resolve_agent_target`]; external
/// resolution kicks in for an explicit `external:<name>` prefix, or as a fallback
/// when a bare (non-path) name is not a local agent but the provider knows it.
pub(crate) async fn resolve_agent_target_or_external(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    target: &str,
) -> Result<ResolvedAgentTarget, FunctionCallError> {
    let external_handle = session
        .services
        .thread_extension_data
        .get::<ExternalTeamHandle>();
    if let Some(external_name) = target.strip_prefix("external:") {
        let Some(handle) = external_handle else {
            return Err(FunctionCallError::RespondToModel(
                "this thread is not attached to an external agent team".to_string(),
            ));
        };
        return resolve_external_target(handle, external_name).await;
    }

    let local_error = match resolve_agent_target(session, turn, target).await {
        Ok(thread_id) => return Ok(ResolvedAgentTarget::Local(thread_id)),
        Err(error) => error,
    };
    if target.starts_with('/') {
        return Err(local_error);
    }
    if let Some(handle) = external_handle
        && let Some(agent) = handle
            .provider()
            .resolve_agent(target)
            .await
            .map_err(FunctionCallError::RespondToModel)?
    {
        return Ok(ResolvedAgentTarget::External { handle, agent });
    }
    Err(local_error)
}

async fn resolve_external_target(
    handle: Arc<ExternalTeamHandle>,
    target: &str,
) -> Result<ResolvedAgentTarget, FunctionCallError> {
    if target.trim().is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "external agent target must not be empty".to_string(),
        ));
    }
    let agent = handle
        .provider()
        .resolve_agent(target)
        .await
        .map_err(FunctionCallError::RespondToModel)?
        .ok_or_else(|| {
            FunctionCallError::RespondToModel(format!("external agent `{target}` was not found"))
        })?;
    Ok(ResolvedAgentTarget::External { handle, agent })
}

// ---------------------------------------------------------------------------
// Delivery helpers used by the upstream handlers' `External` match arms
// ---------------------------------------------------------------------------

/// `send_message` / `followup_task` delivery to an external teammate.
pub(crate) async fn deliver_message(
    handle: Arc<ExternalTeamHandle>,
    agent: ExternalAgent,
    message: &str,
    mode: MessageDeliveryMode,
) -> Result<FunctionToolOutput, FunctionCallError> {
    let delivery = match mode {
        MessageDeliveryMode::QueueOnly => ExternalMessageDelivery::Queue,
        MessageDeliveryMode::TriggerTurn => ExternalMessageDelivery::Wake,
    };
    handle
        .provider()
        .send_message(&agent, message, delivery)
        .await
        .map_err(FunctionCallError::RespondToModel)?;
    Ok(FunctionToolOutput::from_text(String::new(), Some(true)))
}

/// `interrupt_agent` against an external teammate; returns the teammate's prior status.
pub(crate) async fn interrupt_agent(
    handle: Arc<ExternalTeamHandle>,
    agent: ExternalAgent,
) -> Result<AgentStatus, FunctionCallError> {
    let previous_status = agent_status(agent.status);
    handle
        .provider()
        .interrupt(&agent, "interrupt requested by a Codex teammate")
        .await
        .map_err(FunctionCallError::RespondToModel)?;
    Ok(previous_status)
}

/// Appends the external team's roster to a `list_agents` result when the requested
/// `path_prefix` does not exclude external agents.
pub(crate) async fn extend_with_external_agents(
    session: &Arc<Session>,
    path_prefix: Option<&str>,
    agents: &mut Vec<ListedAgent>,
) -> Result<(), FunctionCallError> {
    let external_agents_visible =
        path_prefix.is_none_or(|prefix| prefix == "external" || prefix.starts_with("external:"));
    if !external_agents_visible {
        return Ok(());
    }
    let Some(handle) = session
        .services
        .thread_extension_data
        .get::<ExternalTeamHandle>()
    else {
        return Ok(());
    };
    let external_agents = handle
        .provider()
        .list_agents()
        .await
        .map_err(FunctionCallError::RespondToModel)?;
    agents.extend(external_agents.into_iter().map(|agent| ListedAgent {
        agent_name: agent.name,
        agent_status: agent_status(agent.status),
    }));
    Ok(())
}

fn agent_status(status: ExternalAgentStatus) -> AgentStatus {
    match status {
        ExternalAgentStatus::Active => AgentStatus::Running,
        ExternalAgentStatus::Idle | ExternalAgentStatus::Unknown => AgentStatus::PendingInit,
        ExternalAgentStatus::Stopped => AgentStatus::Shutdown,
    }
}

// ---------------------------------------------------------------------------
// Plaintext message wrapper
// ---------------------------------------------------------------------------

/// Wraps `send_message` / `followup_task` so that, on an external-team thread, the
/// `message` argument is requested and handled in plaintext instead of the
/// Responses-API encrypted channel (the runtime must read it to forward it).
///
/// Off an external-team thread the wrapper is a transparent pass-through, so
/// upstream behavior is unchanged there.
pub(crate) struct ExternalTeamMessageTool<H> {
    inner: H,
    plaintext: bool,
}

/// Wraps `handler` in plaintext mode iff `config.external_team` is set.
pub(crate) fn plaintext_when_external<H: CoreToolRuntime>(
    handler: H,
    config: &Config,
) -> ExternalTeamMessageTool<H> {
    ExternalTeamMessageTool::new(handler, config.external_team.is_some())
}

impl<H: CoreToolRuntime> ExternalTeamMessageTool<H> {
    pub(crate) fn new(inner: H, plaintext: bool) -> Self {
        Self { inner, plaintext }
    }
}

impl<H: CoreToolRuntime> ToolExecutor<ToolInvocation> for ExternalTeamMessageTool<H> {
    fn tool_name(&self) -> ToolName {
        self.inner.tool_name()
    }

    fn spec(&self) -> ToolSpec {
        let spec = self.inner.spec();
        if !self.plaintext {
            return spec;
        }
        match spec {
            ToolSpec::Function(mut tool) => {
                if let Some(properties) = tool.parameters.properties.as_mut()
                    && let Some(message) = properties.get_mut("message")
                {
                    message.encrypted = None;
                }
                ToolSpec::Function(tool)
            }
            spec => spec,
        }
    }

    fn exposure(&self) -> codex_tools::ToolExposure {
        self.inner.exposure()
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        self.inner.supports_parallel_tool_calls()
    }

    fn search_info(&self) -> Option<codex_tools::ToolSearchInfo> {
        self.inner.search_info()
    }

    fn handle(&self, mut invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        if self.plaintext && matches!(invocation.source, ToolCallSource::Direct) {
            // Upstream's message flow only skips encryption for
            // `DirectPlaintextMessage`; the model was told (via `spec`) to send
            // plaintext, so label the call accordingly.
            invocation.source = ToolCallSource::DirectPlaintextMessage;
        }
        self.inner.handle(invocation)
    }
}

impl<H: CoreToolRuntime> CoreToolRuntime for ExternalTeamMessageTool<H> {
    fn wait_until_ready<'a>(&'a self, session: &'a Arc<Session>) -> Option<BoxFuture<'a, ()>> {
        self.inner.wait_until_ready(session)
    }

    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        self.inner.matches_kind(payload)
    }

    fn create_diff_consumer(
        &self,
    ) -> Option<Box<dyn crate::tools::registry::ToolArgumentDiffConsumer>> {
        self.inner.create_diff_consumer()
    }
}

#[cfg(test)]
mod tests {
    use super::super::message_tool::FollowupTaskArgs;
    use super::super::message_tool::SendMessageArgs;
    use super::*;
    use crate::tools::handlers::multi_agents_v2::SendMessageHandler;
    use pretty_assertions::assert_eq;

    #[test]
    fn send_message_accepts_claude_to_field() {
        let args: SendMessageArgs =
            serde_json::from_value(serde_json::json!({"to": "team-lead", "message": "hi"}))
                .expect("Claude `to` payload should deserialize");
        assert_eq!(args.target, "team-lead");
        assert_eq!(args.message, "hi");
    }

    #[test]
    fn send_message_still_accepts_native_target_field() {
        let args: SendMessageArgs =
            serde_json::from_value(serde_json::json!({"target": "claude-peer", "message": "yo"}))
                .expect("native `target` payload should deserialize");
        assert_eq!(args.target, "claude-peer");
    }

    #[test]
    fn followup_task_accepts_claude_to_field() {
        let args: FollowupTaskArgs =
            serde_json::from_value(serde_json::json!({"to": "codex-worker", "message": "wake"}))
                .expect("Claude `to` payload should deserialize");
        assert_eq!(args.target, "codex-worker");
    }

    fn message_encrypted(spec: ToolSpec) -> Option<bool> {
        let ToolSpec::Function(tool) = spec else {
            panic!("send_message should be a function tool");
        };
        tool.parameters
            .properties
            .as_ref()
            .and_then(|properties| properties.get("message"))
            .and_then(|schema| schema.encrypted)
    }

    #[test]
    fn plaintext_wrapper_strips_message_encryption_only_when_enabled() {
        assert_eq!(
            message_encrypted(ExternalTeamMessageTool::new(SendMessageHandler, true).spec()),
            None
        );
        assert_eq!(
            message_encrypted(ExternalTeamMessageTool::new(SendMessageHandler, false).spec()),
            Some(true)
        );
        assert_eq!(message_encrypted(SendMessageHandler.spec()), Some(true));
    }
}
