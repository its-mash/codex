use std::sync::Arc;
use std::sync::Weak;

use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::PromptFragment;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadIdleInput;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ThreadStopInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_protocol::ThreadId;

use crate::runtime::AutomationHandle;
use crate::runtime::AutomationRuntime;
use crate::state_store::AutomationStore;
use crate::tools::AutomationTool;

pub struct AutomationExtension {
    thread_manager: Weak<ThreadManager>,
}

impl AutomationExtension {
    pub fn new(thread_manager: Weak<ThreadManager>) -> Self {
        Self { thread_manager }
    }
}

impl ThreadLifecycleContributor<Config> for AutomationExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let Ok(thread_id) = ThreadId::from_string(input.thread_store.level_id()) else {
                tracing::warn!(
                    level_id = input.thread_store.level_id(),
                    "automation extension received an invalid thread id"
                );
                return;
            };
            let scope = automation_scope(input.config, thread_id);
            let state_path = input
                .config
                .codex_home
                .join("automations")
                .join(format!("{scope}.json"));
            match AutomationRuntime::start(
                AutomationStore::new(state_path.to_path_buf()),
                self.thread_manager.clone(),
                thread_id,
            )
            .await
            {
                Ok(runtime) => {
                    input
                        .thread_store
                        .insert(AutomationHandle::new(runtime.clone()));
                    input.thread_store.insert(runtime);
                }
                Err(error) => {
                    tracing::warn!(%thread_id, %error, "failed to start automation runtime");
                }
            }
        })
    }

    fn on_thread_stop<'a>(&'a self, input: ThreadStopInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(runtime) = input.thread_store.get::<AutomationRuntime>() {
                runtime.stop();
            }
        })
    }

    fn on_thread_idle<'a>(&'a self, input: ThreadIdleInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(runtime) = input.thread_store.get::<AutomationRuntime>() {
                runtime.clear_scheduled_in_flight().await;
            }
        })
    }
}

impl ContextContributor for AutomationExtension {
    fn contribute_thread_context<'a>(
        &'a self,
        _session_store: &'a ExtensionData,
        thread_store: &'a ExtensionData,
    ) -> ExtensionFuture<'a, Vec<PromptFragment>> {
        Box::pin(async move {
            if thread_store.get::<AutomationRuntime>().is_none() {
                return Vec::new();
            }
            vec![PromptFragment::developer_capability(
                "Native durable automation is available. Use loop_create/list/stop for recurring self-work, cron_create/list/update/delete/run for UTC schedules, and monitor_start/list/stop to attach to an existing background terminal. Scheduled and monitor events enter through native inter-agent delivery and wake idle threads; never use tmux keystrokes or ad-hoc sleep loops as a scheduler."
                    .to_string(),
            )]
        })
    }
}

impl ToolContributor for AutomationExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        thread_store
            .get::<AutomationRuntime>()
            .map(|runtime| AutomationTool::all(runtime.as_ref().clone()))
            .unwrap_or_default()
    }
}

fn automation_scope(config: &Config, thread_id: ThreadId) -> String {
    match config.external_team.as_ref() {
        Some(team) => format!(
            "external--{}--{}",
            sanitized_component(&team.team_name),
            sanitized_component(&team.agent_name)
        ),
        None => format!("thread--{thread_id}"),
    }
}

fn sanitized_component(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "extension_tests.rs"]
mod tests;
