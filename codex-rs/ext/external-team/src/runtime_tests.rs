use pretty_assertions::assert_eq;
use tempfile::TempDir;

use super::DeliveryJournal;
use super::bounded_summary;
use super::external_agent_path;
use super::is_shutdown_request;
use super::shutdown_request;

#[test]
fn external_agent_paths_are_stable_and_sanitized() {
    assert_eq!(
        external_agent_path("Team Lead").to_string(),
        "/root/external/team_lead"
    );
    assert_eq!(
        external_agent_path("root").to_string(),
        "/root/external/agent_4813494d137e"
    );
}

#[tokio::test]
async fn delivery_journal_persists_message_deduplication() {
    let temp_dir = TempDir::new().expect("temp dir");
    let path = temp_dir.path().join("journal.json");
    let journal = DeliveryJournal::load(path.clone())
        .await
        .expect("load empty journal");
    journal
        .record("message-1".to_string())
        .await
        .expect("record delivery");

    let reloaded = DeliveryJournal::load(path).await.expect("reload journal");
    assert!(reloaded.contains("message-1").await);
    assert!(!reloaded.contains("message-2").await);
}

#[test]
fn parses_native_shutdown_request_aliases() {
    let camel = shutdown_request(
        r#"{"type":"shutdown_request","requestId":"shutdown-1@worker","from":"team-lead"}"#,
    )
    .expect("parse camel-case request")
    .expect("shutdown request");
    let snake = shutdown_request(r#"{"type":"shutdown_request","request_id":"shutdown-2@worker"}"#)
        .expect("parse snake-case request")
        .expect("shutdown request");

    assert_eq!(camel.request_id, "shutdown-1@worker");
    assert_eq!(snake.request_id, "shutdown-2@worker");
    assert!(
        shutdown_request("ordinary teammate message")
            .expect("ordinary message")
            .is_none()
    );
}

#[test]
fn identifies_control_shaped_content_before_authority_is_applied() {
    assert!(is_shutdown_request(
        r#"{"type":"shutdown_request","request_id":"shutdown-1"}"#
    ));
    assert!(is_shutdown_request(r#"{"type":"shutdown_request"}"#));
    assert!(!is_shutdown_request("ordinary teammate message"));
}

#[test]
fn bounds_idle_summary_by_unicode_characters() {
    let text = "🦀".repeat(501);
    let summary = bounded_summary(&text);
    assert_eq!(summary.chars().count(), 501);
    assert!(summary.ends_with('…'));
}
