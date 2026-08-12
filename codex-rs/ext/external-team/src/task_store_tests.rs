use pretty_assertions::assert_eq;
use serde_json::json;
use tempfile::TempDir;

use super::ClaudeTaskStore;
use super::TaskPatch;

fn store(temp_dir: &TempDir) -> ClaudeTaskStore {
    ClaudeTaskStore::new(temp_dir.path(), "native-test", "codex-worker")
}

#[tokio::test]
async fn reads_and_claims_claude_task_contract_without_losing_unknown_fields() {
    let temp_dir = TempDir::new().expect("temp dir");
    let root = temp_dir.path().join("tasks/native-test");
    tokio::fs::create_dir_all(&root)
        .await
        .expect("create tasks");
    let task = json!({
        "id": "1",
        "subject": "Review fixture",
        "description": "Do not access a live target.",
        "activeForm": "Reviewing fixture",
        "status": "pending",
        "blocks": [],
        "blockedBy": [],
        "providerMetadata": {"keep": true}
    });
    tokio::fs::write(
        root.join("1.json"),
        serde_json::to_vec_pretty(&task).expect("serialize task"),
    )
    .await
    .expect("write task");

    let store = store(&temp_dir);
    let current = store.get("1").await.expect("get task");
    let claimed = store
        .claim("1", current.revision)
        .await
        .expect("claim task");
    assert_eq!(claimed.task.status, "in_progress");
    assert_eq!(claimed.task.owner, "codex-worker");
    assert_eq!(
        claimed.task.extra.get("providerMetadata"),
        Some(&json!({"keep": true}))
    );
}

#[tokio::test]
async fn rejects_stale_task_updates() {
    let temp_dir = TempDir::new().expect("temp dir");
    let store = store(&temp_dir);
    let created = store
        .create("Review fixture".to_string(), String::new(), String::new())
        .await
        .expect("create task");
    store
        .patch(
            &created.task.id,
            TaskPatch {
                description: Some("First update".to_string()),
                ..Default::default()
            },
            created.revision.clone(),
        )
        .await
        .expect("first update");

    let error = store
        .complete(&created.task.id, created.revision)
        .await
        .expect_err("stale revision must fail");
    assert!(error.contains("changed concurrently"));
}

#[tokio::test]
async fn simultaneous_claims_have_exactly_one_winner() {
    let temp_dir = TempDir::new().expect("temp dir");
    let store = store(&temp_dir);
    let created = store
        .create("Claim once".to_string(), String::new(), String::new())
        .await
        .expect("create task");
    let revision = created.revision;

    let (first, second) = tokio::join!(
        store.claim(&created.task.id, revision.clone()),
        store.claim(&created.task.id, revision),
    );
    assert_eq!(usize::from(first.is_ok()) + usize::from(second.is_ok()), 1);

    let claimed = store.get(&created.task.id).await.expect("read winner");
    assert_eq!(claimed.task.status, "in_progress");
    assert_eq!(claimed.task.owner, "codex-worker");
    assert!(
        store
            .claim(&created.task.id, claimed.revision)
            .await
            .expect_err("claimed task cannot be claimed again")
            .contains("cannot be claimed")
    );
}

#[tokio::test]
async fn creates_monotonic_numeric_task_ids() {
    let temp_dir = TempDir::new().expect("temp dir");
    let store = store(&temp_dir);
    let first = store
        .create("First".to_string(), String::new(), String::new())
        .await
        .expect("first task");
    let second = store
        .create("Second".to_string(), String::new(), String::new())
        .await
        .expect("second task");
    assert_eq!(
        (first.task.id, second.task.id),
        ("1".to_string(), "2".to_string())
    );
}
