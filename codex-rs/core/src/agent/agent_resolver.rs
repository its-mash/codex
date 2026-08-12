use crate::function_tool::FunctionCallError;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use codex_extension_api::ExternalAgent;
use codex_extension_api::ExternalTeamHandle;
use codex_protocol::ThreadId;
use codex_protocol::error::CodexErrorDetails;
use std::sync::Arc;

pub(crate) enum ResolvedAgentTarget {
    Local(ThreadId),
    External {
        handle: Arc<ExternalTeamHandle>,
        agent: ExternalAgent,
    },
}

/// Resolves a single tool-facing agent target to a local thread or external teammate.
pub(crate) async fn resolve_agent_target(
    session: &Arc<Session>,
    turn: &Arc<TurnContext>,
    target: &str,
) -> Result<ResolvedAgentTarget, FunctionCallError> {
    register_session_root(session, turn);
    if let Ok(thread_id) = ThreadId::from_string(target) {
        return Ok(ResolvedAgentTarget::Local(thread_id));
    }

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

    let local_result = session
        .services
        .agent_control
        .resolve_agent_reference(session.thread_id, &turn.session_source, target)
        .await;
    if let Ok(thread_id) = local_result {
        return Ok(ResolvedAgentTarget::Local(thread_id));
    }
    let local_error = local_result.expect_err("local result was checked above");
    if target.starts_with('/') {
        return Err(resolve_local_error(local_error));
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
    Err(resolve_local_error(local_error))
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

fn resolve_local_error(err: codex_protocol::error::CodexErr) -> FunctionCallError {
    match err.details() {
        CodexErrorDetails::UnsupportedOperation(message) => {
            FunctionCallError::RespondToModel(message.clone())
        }
        _ => FunctionCallError::RespondToModel(err.to_string()),
    }
}

fn register_session_root(session: &Arc<Session>, turn: &Arc<TurnContext>) {
    session
        .services
        .agent_control
        .register_session_root(session.thread_id, turn.parent_thread_id);
}
