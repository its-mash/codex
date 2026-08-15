//! Fork-owned: native-teammate (external team) App tests.

use super::*;
use codex_app_server_protocol::ThreadStatusChangedNotification;
use codex_config::external_team::ExternalTeamConfigToml;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn external_team_primary_shutdown_requests_process_exit_only_for_its_active_thread()
-> Result<()> {
    let mut app = make_test_app().await;
    let primary_thread_id = ThreadId::new();
    let side_thread_id = ThreadId::new();
    app.config.external_team = Some(ExternalTeamConfigToml {
        provider: "claude_code".to_string(),
        team_name: "fixture".to_string(),
        agent_name: "codex-worker".to_string(),
        agent_id: Some("codex-worker@fixture".to_string()),
        agent_role: Some("worker".to_string()),
        parent_name: Some("team-lead".to_string()),
        claude_home: app.config.codex_home.clone(),
        poll_interval_ms: Some(100),
    });
    app.active_thread_id = Some(primary_thread_id);
    app.primary_thread_id = Some(primary_thread_id);

    assert_eq!(
        app.external_team_shutdown_exit_thread(&thread_closed_notification(primary_thread_id)),
        Some(primary_thread_id)
    );
    assert_eq!(
        app.external_team_shutdown_exit_thread(&ServerNotification::ThreadStatusChanged(
            ThreadStatusChangedNotification {
                thread_id: primary_thread_id.to_string(),
                status: codex_app_server_protocol::ThreadStatus::NotLoaded,
            },
        )),
        Some(primary_thread_id)
    );
    assert_eq!(
        app.external_team_shutdown_exit_thread(&thread_closed_notification(side_thread_id)),
        None
    );

    app.active_thread_id = Some(side_thread_id);
    assert_eq!(
        app.external_team_shutdown_exit_thread(&thread_closed_notification(side_thread_id)),
        None
    );
    Ok(())
}
