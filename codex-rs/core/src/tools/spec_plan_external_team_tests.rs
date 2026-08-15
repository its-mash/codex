//! Fork-owned: tool-plan tests for external-team (native teammate) mode. Child of
//! `spec_plan_tests` so it can reuse `probe`/`set_feature`/`update_config`.

use super::*;
use codex_config::external_team::ExternalTeamConfigToml;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn external_team_message_schemas_are_plaintext() {
    let plan = probe(|turn| {
        set_feature(turn, Feature::MultiAgentV2, /*enabled*/ true);
        update_config(turn, |config| {
            config.external_team = Some(ExternalTeamConfigToml {
                provider: "claude_code".to_string(),
                team_name: "fixture".to_string(),
                agent_name: "codex-worker".to_string(),
                agent_id: Some("codex-worker@fixture".to_string()),
                agent_role: Some("worker".to_string()),
                parent_name: Some("claude-lead".to_string()),
                claude_home: config.codex_home.clone(),
                poll_interval_ms: Some(100),
            });
        });
    })
    .await;
    let ToolSpec::Namespace(namespace) = plan.visible_spec(MULTI_AGENT_V2_NAMESPACE) else {
        panic!("expected {MULTI_AGENT_V2_NAMESPACE} namespace");
    };
    for tool_name in ["send_message", "followup_task"] {
        let Some(ResponsesApiNamespaceTool::Function(tool)) = namespace.tools.iter().find(|tool| {
            matches!(
                tool,
                ResponsesApiNamespaceTool::Function(tool) if tool.name == tool_name
            )
        }) else {
            panic!("expected {tool_name} in {MULTI_AGENT_V2_NAMESPACE} namespace");
        };
        let properties = tool
            .parameters
            .properties
            .as_ref()
            .expect("tool should use object params");
        assert_eq!(
            properties
                .get("message")
                .and_then(|schema| schema.encrypted),
            None
        );
    }
    let Some(ResponsesApiNamespaceTool::Function(spawn_tool)) =
        namespace.tools.iter().find(|tool| {
            matches!(
                tool,
                ResponsesApiNamespaceTool::Function(tool) if tool.name == "spawn_agent"
            )
        })
    else {
        panic!("expected spawn_agent in {MULTI_AGENT_V2_NAMESPACE} namespace");
    };
    assert_eq!(
        spawn_tool
            .parameters
            .properties
            .as_ref()
            .and_then(|properties| properties.get("message"))
            .and_then(|schema| schema.encrypted),
        Some(true)
    );
}
