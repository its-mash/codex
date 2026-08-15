use std::sync::Arc;
use std::sync::Mutex;

use anyhow::Result;
use codex_core::TurnInputRequest;
use codex_core::config::Config;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ExternalAgent;
use codex_extension_api::ExternalAgentStatus;
use codex_extension_api::ExternalMessageDelivery;
use codex_extension_api::ExternalTeamFuture;
use codex_extension_api::ExternalTeamHandle;
use codex_extension_api::ExternalTeamProvider;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_features::Feature;
use codex_protocol::protocol::EventMsg;
use codex_protocol::user_input::UserInput;
use core_test_support::responses::ev_completed;
use core_test_support::responses::ev_function_call_with_namespace;
use core_test_support::responses::ev_response_created;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_codex::test_codex;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::json;

#[derive(Clone, Debug, Eq, PartialEq)]
struct SentMessage {
    target: String,
    content: String,
    delivery: ExternalMessageDelivery,
}

#[derive(Debug)]
struct RecordingExternalTeam {
    sent: Mutex<Vec<SentMessage>>,
    agents: Vec<ExternalAgent>,
}

impl RecordingExternalTeam {
    fn new() -> Self {
        Self {
            sent: Mutex::new(Vec::new()),
            agents: vec![
                ExternalAgent {
                    id: "codex-worker@fixture".to_string(),
                    name: "codex-worker".to_string(),
                    role: Some("worker".to_string()),
                    status: ExternalAgentStatus::Active,
                },
                ExternalAgent {
                    id: "claude-lead@fixture".to_string(),
                    name: "claude-lead".to_string(),
                    role: Some("lead".to_string()),
                    status: ExternalAgentStatus::Idle,
                },
            ],
        }
    }
}

impl ExternalTeamProvider for RecordingExternalTeam {
    fn identity(&self) -> ExternalAgent {
        self.agents[0].clone()
    }

    fn parent(&self) -> ExternalAgent {
        self.agents[1].clone()
    }

    fn resolve_agent<'a>(
        &'a self,
        target: &'a str,
    ) -> ExternalTeamFuture<'a, Result<Option<ExternalAgent>, String>> {
        Box::pin(async move {
            let target = target.strip_prefix("external:").unwrap_or(target);
            Ok(self
                .agents
                .iter()
                .find(|agent| agent.name == target || agent.id == target)
                .cloned())
        })
    }

    fn list_agents(&self) -> ExternalTeamFuture<'_, Result<Vec<ExternalAgent>, String>> {
        Box::pin(async { Ok(self.agents.clone()) })
    }

    fn send_message<'a>(
        &'a self,
        target: &'a ExternalAgent,
        content: &'a str,
        delivery: ExternalMessageDelivery,
    ) -> ExternalTeamFuture<'a, Result<(), String>> {
        Box::pin(async move {
            self.sent
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(SentMessage {
                    target: target.name.clone(),
                    content: content.to_string(),
                    delivery,
                });
            Ok(())
        })
    }

    fn interrupt<'a>(
        &'a self,
        target: &'a ExternalAgent,
        _reason: &'a str,
    ) -> ExternalTeamFuture<'a, Result<(), String>> {
        Box::pin(async move { Err(format!("{} cannot be interrupted", target.name)) })
    }
}

struct ExternalTeamFixture {
    provider: Arc<RecordingExternalTeam>,
}

impl ThreadLifecycleContributor<Config> for ExternalTeamFixture {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            let provider: Arc<dyn ExternalTeamProvider> = self.provider.clone();
            input.thread_store.insert(ExternalTeamHandle::new(provider));
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn collaboration_tools_route_external_targets_through_native_provider() -> Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let response = mount_sse_sequence(
        &server,
        vec![
            sse(vec![
                ev_response_created("resp-1"),
                ev_function_call_with_namespace(
                    "send-1",
                    "collaboration",
                    "send_message",
                    &json!({
                        "target": "external:claude-lead",
                        "message": "queued status"
                    })
                    .to_string(),
                ),
                ev_function_call_with_namespace(
                    "wake-1",
                    "collaboration",
                    "followup_task",
                    &json!({
                        "target": "claude-lead",
                        "message": "wake for fixture"
                    })
                    .to_string(),
                ),
                ev_function_call_with_namespace("list-1", "collaboration", "list_agents", "{}"),
                ev_completed("resp-1"),
            ]),
            sse(vec![ev_response_created("resp-2"), ev_completed("resp-2")]),
        ],
    )
    .await;
    let provider = Arc::new(RecordingExternalTeam::new());
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.thread_lifecycle_contributor(Arc::new(ExternalTeamFixture {
        provider: Arc::clone(&provider),
    }));
    let test = test_codex()
        .with_extensions(Arc::new(extensions.build()))
        .with_config(|config| {
            config
                .features
                .enable(Feature::MultiAgentV2)
                .expect("enable multi-agent v2");
        })
        .build(&server)
        .await?;

    test.codex
        .start_or_steer_turn(TurnInputRequest::user_input(vec![UserInput::Text {
            text: "Exercise the external team fixture.".to_string(),
            text_elements: Vec::new(),
        }]))
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let mut sent = provider
        .sent
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    sent.sort_by(|left, right| left.content.cmp(&right.content));
    let requests = response.requests();
    assert_eq!(requests.len(), 2);
    let tool_outputs = ["send-1", "wake-1", "list-1"]
        .into_iter()
        .map(|call_id| {
            (
                call_id,
                requests[1]
                    .function_call_output_text(call_id)
                    .unwrap_or_default(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sent,
        vec![
            SentMessage {
                target: "claude-lead".to_string(),
                content: "queued status".to_string(),
                delivery: ExternalMessageDelivery::Queue,
            },
            SentMessage {
                target: "claude-lead".to_string(),
                content: "wake for fixture".to_string(),
                delivery: ExternalMessageDelivery::Wake,
            },
        ],
        "tool outputs: {tool_outputs:?}"
    );
    let listed: serde_json::Value = serde_json::from_str(
        &requests[1]
            .function_call_output_text("list-1")
            .expect("list_agents output text"),
    )?;
    let lead = listed["agents"]
        .as_array()
        .expect("agents array")
        .iter()
        .find(|agent| agent["agent_name"] == "claude-lead")
        .expect("external lead should be listed");
    assert_eq!(lead["agent_status"], "pending_init");
    Ok(())
}
