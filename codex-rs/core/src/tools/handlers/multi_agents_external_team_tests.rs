//! Fork-owned: MultiAgentV2 external-team tests. Child of `multi_agents_tests` so
//! it can reuse that module's fixtures without touching upstream test bodies.

use super::*;
use crate::tools::handlers::multi_agents_v2::external_team::ExternalTeamMessageTool;

/// On an external-team thread `send_message` runs in plaintext mode: the
/// `InterAgentCommunication` carries rendered plaintext content, not an
/// encrypted payload, so the runtime can forward it off-process.
#[tokio::test]
async fn external_team_send_message_delivers_plaintext_content() {
    let (mut session, mut turn) = make_session_and_context().await;
    let manager = thread_manager();
    let root = manager
        .start_thread(StartThreadOptions::new((*turn.config).clone()))
        .await
        .expect("root thread should start");
    session.services.agent_control = manager.agent_control();
    session.thread_id = root.thread_id;
    let mut config = (*turn.config).clone();
    config
        .features
        .enable(Feature::MultiAgentV2)
        .expect("test config should allow feature update");
    set_turn_config(&mut turn, config);

    let session = Arc::new(session);
    let turn = Arc::new(turn);
    SpawnAgentHandlerV2::default()
        .handle(invocation(
            session.clone(),
            turn.clone(),
            "spawn_agent",
            function_payload(json!({
                "message": "encrypted-spawn-message",
                "task_name": "test_process"
            })),
        ))
        .await
        .expect("spawn_agent should succeed");
    let child_thread_id = session
        .services
        .agent_control
        .resolve_agent_reference(session.thread_id, &turn.session_source, "test_process")
        .await
        .expect("relative path should resolve");

    ExternalTeamMessageTool::new(SendMessageHandlerV2, /*plaintext*/ true)
        .handle(invocation(
            session,
            turn,
            "send_message",
            function_payload(json!({
                "target": "test_process",
                "message": "plaintext-send-message"
            })),
        ))
        .await
        .expect("send_message should accept plaintext external-team content");

    assert!(manager.captured_ops().iter().any(|(id, op)| {
        *id == child_thread_id
            && matches!(
                op,
                Op::InterAgentCommunication { communication }
                    if communication.author == AgentPath::root()
                        && communication.recipient.as_str() == "/root/test_process"
                        && communication.other_recipients.is_empty()
                        && communication.content == "Message Type: MESSAGE\nTask name: /root/test_process\nSender: /root\nPayload:\nplaintext-send-message"
                        && communication.encrypted_content.is_none()
                        && !communication.trigger_turn
            )
    }));
}
