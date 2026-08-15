use std::sync::Arc;

use codex_extension_api::ExternalMessageDelivery;
use codex_extension_api::ExternalTeamProvider;
use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::ClaudeCodeProvider;
use super::ClaudeInboundMessage;
use super::ClaudeMessage;
use super::ClaudeProviderConfig;

fn provider(temp_dir: &TempDir) -> ClaudeCodeProvider {
    ClaudeCodeProvider::new(ClaudeProviderConfig {
        claude_home: temp_dir.path().to_path_buf(),
        team_name: "native-test".to_string(),
        agent_name: "codex-worker".to_string(),
        agent_id: "codex-worker@native-test".to_string(),
        agent_role: Some("reviewer".to_string()),
        parent_name: None,
    })
    .expect("provider config should be valid")
}

async fn write_team(temp_dir: &TempDir) {
    let team_dir = temp_dir.path().join("teams/native-test");
    tokio::fs::create_dir_all(team_dir.join("inboxes"))
        .await
        .expect("create test team");
    let roster = json!({
        "name": "native-test",
        "leadAgentId": "team-lead@native-test",
        "members": [
            {
                "name": "team-lead",
                "agentType": "lead",
                "status": "working"
            },
            {
                "name": "codex-worker",
                "agentType": "reviewer",
                "prompt": "Review the supplied fixture only."
            }
        ]
    });
    tokio::fs::write(
        team_dir.join("config.json"),
        serde_json::to_vec_pretty(&roster).expect("serialize roster"),
    )
    .await
    .expect("write roster");
}

#[tokio::test]
async fn reads_roster_initial_assignment_and_inbox_contract() {
    let temp_dir = TempDir::new().expect("temp dir");
    write_team(&temp_dir).await;
    let inbox = json!([{
        "from": "team-lead",
        "text": "Send a concise status update.",
        "timestamp": "2026-08-11T12:00:00.000Z",
        "msgV": 1,
        "msg_id": "message-1",
        "type": "message",
        "read": false
    }]);
    tokio::fs::write(
        temp_dir
            .path()
            .join("teams/native-test/inboxes/codex-worker.json"),
        serde_json::to_vec_pretty(&inbox).expect("serialize inbox"),
    )
    .await
    .expect("write inbox");

    let provider = provider(&temp_dir);
    assert!(provider.is_on_roster().await.expect("read roster"));
    assert_eq!(
        provider.list_agents().await.expect("list agents"),
        vec![
            codex_extension_api::ExternalAgent {
                id: "team-lead@native-test".to_string(),
                name: "team-lead".to_string(),
                role: Some("lead".to_string()),
                status: codex_extension_api::ExternalAgentStatus::Active,
            },
            codex_extension_api::ExternalAgent {
                id: "codex-worker@native-test".to_string(),
                name: "codex-worker".to_string(),
                role: Some("reviewer".to_string()),
                status: codex_extension_api::ExternalAgentStatus::Unknown,
            },
        ]
    );
    let messages = provider.pending_messages().await.expect("read messages");
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[1],
        ClaudeInboundMessage {
            id: "message-1".to_string(),
            author: "team-lead".to_string(),
            content: "Send a concise status update.".to_string(),
            kind: "message".to_string(),
        }
    );
    assert_eq!(messages[0].author, "team-lead");
    assert_eq!(messages[0].content, "Review the supplied fixture only.");
    assert_eq!(messages[0].kind, "new_task");
}

#[tokio::test]
async fn skips_only_the_first_inbox_copy_of_the_roster_assignment() {
    let temp_dir = TempDir::new().expect("temp dir");
    write_team(&temp_dir).await;
    let inbox = json!([
        {
            "from": "team-lead",
            "text": "Review the supplied fixture only.",
            "timestamp": "2026-08-11T12:00:00.000Z",
            "msgV": 1,
            "msg_id": "initial-inbox-copy",
            "type": "message",
            "read": false
        },
        {
            "from": "team-lead",
            "text": "Review the supplied fixture only.",
            "timestamp": "2026-08-11T12:01:00.000Z",
            "msgV": 1,
            "msg_id": "intentional-repeat",
            "type": "message",
            "read": false
        },
        {
            "from": "team-lead",
            "text": "Then report completion.",
            "timestamp": "2026-08-11T12:02:00.000Z",
            "msgV": 1,
            "msg_id": "followup",
            "type": "message",
            "read": false
        }
    ]);
    tokio::fs::write(
        temp_dir
            .path()
            .join("teams/native-test/inboxes/codex-worker.json"),
        serde_json::to_vec_pretty(&inbox).expect("serialize inbox"),
    )
    .await
    .expect("write inbox");

    let messages = provider(&temp_dir)
        .pending_messages()
        .await
        .expect("read messages");
    assert_eq!(
        messages,
        vec![
            ClaudeInboundMessage {
                id: format!(
                    "initial:{}",
                    super::stable_digest("Review the supplied fixture only.")
                ),
                author: "team-lead".to_string(),
                content: "Review the supplied fixture only.".to_string(),
                kind: "new_task".to_string(),
            },
            ClaudeInboundMessage {
                id: "intentional-repeat".to_string(),
                author: "team-lead".to_string(),
                content: "Review the supplied fixture only.".to_string(),
                kind: "message".to_string(),
            },
            ClaudeInboundMessage {
                id: "followup".to_string(),
                author: "team-lead".to_string(),
                content: "Then report completion.".to_string(),
                kind: "message".to_string(),
            },
        ]
    );
}

#[tokio::test]
async fn writes_native_claude_inbox_envelope() {
    let temp_dir = TempDir::new().expect("temp dir");
    write_team(&temp_dir).await;
    let provider = Arc::new(provider(&temp_dir));
    let target = provider
        .resolve_agent("team-lead")
        .await
        .expect("resolve parent")
        .expect("parent should exist");

    provider
        .send_message(&target, "Native reply", ExternalMessageDelivery::Wake)
        .await
        .expect("write message");

    let bytes = tokio::fs::read(
        temp_dir
            .path()
            .join("teams/native-test/inboxes/team-lead.json"),
    )
    .await
    .expect("read parent inbox");
    let messages: Vec<ClaudeMessage> = serde_json::from_slice(&bytes).expect("parse parent inbox");
    assert_eq!(messages.len(), 1);
    let message = &messages[0];
    assert_eq!(message.from, "codex-worker");
    assert_eq!(message.text, "Native reply");
    assert_eq!(message.message_version, 1);
    assert_eq!(message.kind, "message");
    assert!(!message.read);
    assert!(!message.msg_id.is_empty());
    assert!(
        message
            .timestamp
            .as_str()
            .is_some_and(|timestamp| timestamp.ends_with('Z'))
    );
}

#[tokio::test]
async fn accepts_legacy_inbox_fields_without_losing_native_messages() {
    let temp_dir = TempDir::new().expect("temp dir");
    write_team(&temp_dir).await;
    let inbox = json!([
        {
            "from": "team-lead",
            "content": "Legacy envelope",
            "timestamp": 1786450000000_i64,
            "id": "legacy-1"
        },
        {
            "from": "team-lead",
            "text": "Native envelope",
            "timestamp": "2026-08-11T12:00:00.000Z",
            "msgV": 1,
            "msg_id": "native-1",
            "type": "message",
            "read": false
        }
    ]);
    tokio::fs::write(
        temp_dir
            .path()
            .join("teams/native-test/inboxes/codex-worker.json"),
        serde_json::to_vec_pretty(&inbox).expect("serialize inbox"),
    )
    .await
    .expect("write inbox");

    let messages = provider(&temp_dir)
        .pending_messages()
        .await
        .expect("read mixed inbox");
    assert_eq!(
        messages
            .into_iter()
            .skip(1)
            .map(|message| (message.id, message.content, message.kind))
            .collect::<Vec<_>>(),
        vec![
            (
                "legacy-1".to_string(),
                "Legacy envelope".to_string(),
                "message".to_string()
            ),
            (
                "native-1".to_string(),
                "Native envelope".to_string(),
                "message".to_string()
            )
        ]
    );
}

async fn write_three_member_team(temp_dir: &TempDir) {
    let team_dir = temp_dir.path().join("teams/native-test");
    tokio::fs::create_dir_all(team_dir.join("inboxes"))
        .await
        .expect("create test team");
    let roster = json!({
        "name": "native-test",
        "leadAgentId": "team-lead@native-test",
        "members": [
            {
                "name": "team-lead",
                "agentType": "lead",
                "status": "working"
            },
            {
                "name": "codex-worker",
                "agentType": "reviewer",
                "prompt": "Review the supplied fixture only."
            },
            {
                "name": "claude-worker",
                "agentType": "worker",
                "status": "idle"
            }
        ]
    });
    tokio::fs::write(
        team_dir.join("config.json"),
        serde_json::to_vec_pretty(&roster).expect("serialize roster"),
    )
    .await
    .expect("write roster");
}

#[tokio::test]
async fn routes_peer_member_messages_in_both_directions() {
    let temp_dir = TempDir::new().expect("temp dir");
    write_three_member_team(&temp_dir).await;
    let provider = Arc::new(provider(&temp_dir));

    // Outbound: codex-worker -> claude-worker peer, not the lead.
    let peer = provider
        .resolve_agent("claude-worker")
        .await
        .expect("resolve peer")
        .expect("peer should be on the roster");
    assert_eq!(peer.id, "claude-worker@native-test");
    provider
        .send_message(&peer, "CODEX_PEER_PING", ExternalMessageDelivery::Queue)
        .await
        .expect("write peer message");
    let bytes = tokio::fs::read(
        temp_dir
            .path()
            .join("teams/native-test/inboxes/claude-worker.json"),
    )
    .await
    .expect("read peer inbox");
    let messages: Vec<ClaudeMessage> = serde_json::from_slice(&bytes).expect("parse peer inbox");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].from, "codex-worker");
    assert_eq!(messages[0].text, "CODEX_PEER_PING");
    let lead_inbox = temp_dir
        .path()
        .join("teams/native-test/inboxes/team-lead.json");
    assert!(
        !lead_inbox.exists(),
        "peer message must not be routed to the lead"
    );

    // Inbound: a claude-worker peer message is ordinary teammate communication.
    let inbox = json!([{
        "from": "claude-worker",
        "text": "PEER_PING_CLAUDE",
        "timestamp": "2026-08-12T09:00:00.000Z",
        "msgV": 1,
        "msg_id": "peer-1",
        "type": "message",
        "read": false
    }]);
    tokio::fs::write(
        temp_dir
            .path()
            .join("teams/native-test/inboxes/codex-worker.json"),
        serde_json::to_vec_pretty(&inbox).expect("serialize inbox"),
    )
    .await
    .expect("write inbox");
    let messages = provider.pending_messages().await.expect("read messages");
    assert_eq!(
        messages[1],
        ClaudeInboundMessage {
            id: "peer-1".to_string(),
            author: "claude-worker".to_string(),
            content: "PEER_PING_CLAUDE".to_string(),
            kind: "message".to_string(),
        }
    );

    // Peers never gain lifecycle authority.
    assert!(
        !provider
            .is_parent("claude-worker")
            .await
            .expect("resolve peer")
    );
    assert_eq!(
        provider
            .acknowledge_shutdown("claude-worker", "peer-shutdown-1")
            .await
            .expect_err("peer shutdown must be rejected"),
        "external agent `claude-worker` is not authorized to shut down `codex-worker`"
    );
}

#[tokio::test]
async fn writes_native_shutdown_acknowledgement() {
    let temp_dir = TempDir::new().expect("temp dir");
    write_team(&temp_dir).await;
    provider(&temp_dir)
        .acknowledge_shutdown("team-lead", "shutdown-1@codex-worker")
        .await
        .expect("acknowledge shutdown");

    let bytes = tokio::fs::read(
        temp_dir
            .path()
            .join("teams/native-test/inboxes/team-lead.json"),
    )
    .await
    .expect("read parent inbox");
    let messages: Vec<ClaudeMessage> = serde_json::from_slice(&bytes).expect("parse parent inbox");
    let payload: serde_json::Value =
        serde_json::from_str(&messages[0].text).expect("parse control payload");
    assert_eq!(
        payload,
        json!({
            "type": "shutdown_response",
            "request_id": "shutdown-1@codex-worker",
            "approve": true,
            "reason": "Codex teammate acknowledged the native shutdown request."
        })
    );
}

#[tokio::test]
async fn only_configured_parent_can_acknowledge_shutdown() {
    let temp_dir = TempDir::new().expect("temp dir");
    write_team(&temp_dir).await;
    let provider = provider(&temp_dir);

    assert!(provider.is_parent("team-lead").await.expect("resolve lead"));
    assert!(
        !provider
            .is_parent("codex-worker")
            .await
            .expect("resolve member")
    );
    assert_eq!(
        provider
            .acknowledge_shutdown("codex-worker", "unauthorized-shutdown")
            .await
            .expect_err("non-parent shutdown must be rejected"),
        "external agent `codex-worker` is not authorized to shut down `codex-worker`"
    );
}

#[tokio::test]
async fn missing_team_config_is_an_absent_roster_not_a_parse_failure() {
    let temp_dir = TempDir::new().expect("temp dir");
    assert!(
        !provider(&temp_dir)
            .is_on_roster()
            .await
            .expect("missing roster")
    );
}

#[tokio::test]
async fn drops_inbox_messages_that_exceed_the_model_context_bound() {
    let temp_dir = TempDir::new().expect("temp dir");
    write_team(&temp_dir).await;
    let inbox = json!([{
        "from": "team-lead",
        "text": "x".repeat((super::MAX_MODEL_MESSAGE_TOKENS + 1) * 4),
        "msg_id": "oversized",
        "type": "message"
    }]);
    tokio::fs::write(
        temp_dir
            .path()
            .join("teams/native-test/inboxes/codex-worker.json"),
        serde_json::to_vec(&inbox).expect("serialize inbox"),
    )
    .await
    .expect("write inbox");

    let messages = provider(&temp_dir)
        .pending_messages()
        .await
        .expect("read bounded inbox");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].kind, "new_task");
}

#[test]
fn rejects_path_components_in_team_identity() {
    let temp_dir = TempDir::new().expect("temp dir");
    let error = ClaudeCodeProvider::new(ClaudeProviderConfig {
        claude_home: temp_dir.path().to_path_buf(),
        team_name: "../escape".to_string(),
        agent_name: "codex-worker".to_string(),
        agent_id: "codex-worker@native-test".to_string(),
        agent_role: None,
        parent_name: None,
    })
    .expect_err("path traversal must be rejected");
    assert_eq!(error, "invalid Claude team name `../escape`");
}
