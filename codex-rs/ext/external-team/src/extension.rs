use std::sync::Arc;
use std::sync::Weak;
use std::time::Duration;

use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_extension_api::ContextContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExternalTeamHandle;
use codex_extension_api::PromptFragment;
use codex_extension_api::ThreadIdleCause;
use codex_extension_api::ThreadIdleInput;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ThreadStopInput;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolContributor;
use codex_extension_api::ToolExecutor;
use codex_extension_api::TurnItemContributor;
use codex_protocol::ThreadId;
use codex_protocol::items::AgentMessageContent;
use codex_protocol::items::TurnItem;
use codex_protocol::models::MessagePhase;

use crate::claude::ClaudeCodeProvider;
use crate::claude::ClaudeProviderConfig;
use crate::runtime::ExternalTeamRuntime;
use crate::task_store::ClaudeTaskStore;
use crate::task_tools::ClaudeTaskTool;

const DEFAULT_POLL_INTERVAL_MS: u64 = 200;
const MIN_POLL_INTERVAL_MS: u64 = 50;
const MAX_POLL_INTERVAL_MS: u64 = 5_000;

pub struct ExternalTeamExtension {
    thread_manager: Weak<ThreadManager>,
}

impl ExternalTeamExtension {
    pub fn new(thread_manager: Weak<ThreadManager>) -> Self {
        Self { thread_manager }
    }
}

impl ThreadLifecycleContributor<Config> for ExternalTeamExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let Some(config) = input.config.external_team.as_ref() else {
                return;
            };
            let Ok(thread_id) = ThreadId::from_string(input.thread_store.level_id()) else {
                tracing::warn!(
                    level_id = input.thread_store.level_id(),
                    "external-team extension received an invalid thread id"
                );
                return;
            };
            let agent_id = config
                .agent_id
                .clone()
                .unwrap_or_else(|| format!("{}@{}", config.agent_name, config.team_name));
            let task_store = ClaudeTaskStore::new(
                config.claude_home.as_path(),
                &config.team_name,
                &config.agent_name,
            );
            let provider = match ClaudeCodeProvider::new(ClaudeProviderConfig {
                claude_home: config.claude_home.to_path_buf(),
                team_name: config.team_name.clone(),
                agent_name: config.agent_name.clone(),
                agent_id,
                agent_role: config.agent_role.clone(),
                parent_name: config.parent_name.clone(),
            }) {
                Ok(provider) => Arc::new(provider),
                Err(error) => {
                    tracing::warn!(%thread_id, %error, "failed to configure external team provider");
                    return;
                }
            };
            let provider_handle: Arc<dyn codex_extension_api::ExternalTeamProvider> =
                provider.clone();
            input
                .thread_store
                .insert(ExternalTeamHandle::new(provider_handle));
            input.thread_store.insert(task_store);
            let poll_interval = Duration::from_millis(
                config
                    .poll_interval_ms
                    .unwrap_or(DEFAULT_POLL_INTERVAL_MS)
                    .clamp(MIN_POLL_INTERVAL_MS, MAX_POLL_INTERVAL_MS),
            );
            let journal_path = input
                .config
                .codex_home
                .join("external-team")
                .join(&config.team_name)
                .join(format!("{}.journal.json", config.agent_name));
            match ExternalTeamRuntime::start(
                provider,
                journal_path.to_path_buf(),
                self.thread_manager.clone(),
                thread_id,
                poll_interval,
            )
            .await
            {
                Ok(runtime) => {
                    input.thread_store.insert(runtime);
                }
                Err(error) => {
                    tracing::warn!(%thread_id, %error, "failed to start external team runtime");
                }
            }
        })
    }

    fn on_thread_idle<'a>(&'a self, input: ThreadIdleInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if input.cause != ThreadIdleCause::Completed {
                return;
            }
            let Some(runtime) = input.thread_store.get::<ExternalTeamRuntime>() else {
                return;
            };
            if let Err(error) = runtime.deliver_final_and_idle().await {
                tracing::warn!(%error, "failed to deliver final answer and idle state to external parent");
            }
        })
    }

    fn on_thread_stop<'a>(&'a self, input: ThreadStopInput<'a>) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            if let Some(runtime) = input.thread_store.get::<ExternalTeamRuntime>() {
                runtime.stop();
            }
        })
    }
}

impl ContextContributor for ExternalTeamExtension {
    fn contribute_thread_context<'a>(
        &'a self,
        _session_store: &'a ExtensionData,
        thread_store: &'a ExtensionData,
    ) -> ExtensionFuture<'a, Vec<PromptFragment>> {
        Box::pin(async move {
            let Some(handle) = thread_store.get::<ExternalTeamHandle>() else {
                return Vec::new();
            };
            let identity = handle.provider().identity();
            let parent = handle.provider().parent();
            vec![PromptFragment::developer_capability(format!(
                "You are Codex teammate `{}` in an externally managed agent team. The external parent is `{}`. Use the existing `send_message` and `followup_task` collaboration tools with external teammate names; use `task_list`, `task_get`, `task_create`, `task_claim`, `task_update`, and `task_complete` for the shared team task board. Do not use terminal input, tmux, or provider-specific bus tools for team communication. Your private Codex subagents remain under `/root/...`. Your final answer is automatically delivered to the external parent.",
                identity.name, parent.name
            ))]
        })
    }
}

impl ToolContributor for ExternalTeamExtension {
    fn tools(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        thread_store
            .get::<ClaudeTaskStore>()
            .map(ClaudeTaskTool::all)
            .unwrap_or_default()
    }
}

impl TurnItemContributor for ExternalTeamExtension {
    fn contribute<'a>(
        &'a self,
        thread_store: &'a ExtensionData,
        _turn_store: &'a ExtensionData,
        item: &'a mut TurnItem,
    ) -> ExtensionFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let Some(runtime) = thread_store.get::<ExternalTeamRuntime>() else {
                return Ok(());
            };
            let TurnItem::AgentMessage(message) = item else {
                return Ok(());
            };
            if matches!(message.phase, Some(MessagePhase::Commentary)) {
                return Ok(());
            }
            let text = message
                .content
                .iter()
                .map(|content| match content {
                    AgentMessageContent::Text { text } => text.as_str(),
                })
                .collect::<Vec<_>>()
                .join("");
            if !text.trim().is_empty() {
                runtime.capture_final(message.id.clone(), text).await;
            }
            Ok(())
        })
    }
}
