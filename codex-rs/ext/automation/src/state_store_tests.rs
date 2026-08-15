use chrono::Duration;
use chrono::Utc;
use pretty_assertions::assert_eq;
use std::collections::HashSet;
use tempfile::TempDir;

use super::AutomationKind;
use super::AutomationStore;
use super::DeliveryAcknowledgement;
use super::DeliveryLeaseRequest;
use super::JobPatch;
use super::ScheduleSpec;

fn store(temp_dir: &TempDir) -> AutomationStore {
    AutomationStore::new(temp_dir.path().join("automation.json"))
}

fn lease(owner: &str, claimed_at: chrono::DateTime<Utc>) -> DeliveryLeaseRequest {
    DeliveryLeaseRequest {
        owner: owner.to_string(),
        claimed_at,
        duration: Duration::seconds(30),
        excluded_job_ids: HashSet::new(),
    }
}

fn lease_excluding(
    owner: &str,
    claimed_at: chrono::DateTime<Utc>,
    excluded_job_id: &str,
) -> DeliveryLeaseRequest {
    DeliveryLeaseRequest {
        excluded_job_ids: HashSet::from([excluded_job_id.to_string()]),
        ..lease(owner, claimed_at)
    }
}

#[tokio::test]
async fn interval_jobs_retry_until_acknowledged_then_advance_durably() {
    let temp_dir = TempDir::new().expect("temp dir");
    let store = store(&temp_dir);
    let job = store
        .create_job(
            Some("reconcile".to_string()),
            "Check the mock fixture.".to_string(),
            AutomationKind::Loop,
            ScheduleSpec::Interval { every_seconds: 60 },
        )
        .await
        .expect("create loop");
    let due_at =
        chrono::DateTime::from_timestamp_millis(job.next_due_at).expect("valid due timestamp");

    assert_eq!(
        store
            .claim_due(lease("scheduler-a", due_at - Duration::milliseconds(1)))
            .await
            .expect("claim before due"),
        Vec::new()
    );
    let claimed = store
        .claim_due(lease("scheduler-a", due_at))
        .await
        .expect("claim at due");
    assert_eq!(
        claimed.iter().map(|job| job.id.clone()).collect::<Vec<_>>(),
        vec![job.id.clone()]
    );
    assert_eq!(
        store
            .claim_due(lease("scheduler-a", due_at))
            .await
            .expect("retry unacknowledged claim"),
        claimed
    );
    let pending_fire_at = claimed[0]
        .pending_fire_at
        .expect("claim should have a durable fire id");
    store
        .acknowledge_delivery(DeliveryAcknowledgement {
            id: job.id.clone(),
            pending_fire_at,
            owner: "scheduler-a".to_string(),
            delivered_at: due_at,
        })
        .await
        .expect("acknowledge delivery");
    assert_eq!(
        store
            .claim_due(lease("scheduler-a", due_at))
            .await
            .expect("claim after ack"),
        Vec::new()
    );
    let reloaded = store.job(&job.id).await.expect("reload job");
    assert!(reloaded.next_due_at > job.next_due_at);
    assert_eq!(reloaded.last_run_at, Some(due_at.timestamp_millis()));
    assert_eq!(reloaded.pending_fire_at, None);
    assert_eq!(reloaded.delivery_owner, None);
    assert_eq!(reloaded.delivery_lease_expires_at, None);
}

#[tokio::test]
async fn delivery_lease_prevents_overlapping_schedulers_and_allows_takeover() {
    let temp_dir = TempDir::new().expect("temp dir");
    let store = store(&temp_dir);
    let job = store
        .create_job(
            Some("leased".to_string()),
            "Check once per claim.".to_string(),
            AutomationKind::Loop,
            ScheduleSpec::Interval { every_seconds: 60 },
        )
        .await
        .expect("create loop");
    let due_at =
        chrono::DateTime::from_timestamp_millis(job.next_due_at).expect("valid due timestamp");

    let first = store
        .claim_due(lease("scheduler-a", due_at))
        .await
        .expect("first scheduler claims");
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].delivery_owner.as_deref(), Some("scheduler-a"));
    assert_eq!(
        store
            .claim_due(lease("scheduler-b", due_at))
            .await
            .expect("overlapping scheduler checks"),
        Vec::new()
    );

    let takeover_at = due_at + Duration::seconds(31);
    let takeover = store
        .claim_due(lease("scheduler-b", takeover_at))
        .await
        .expect("expired lease is reclaimed");
    assert_eq!(takeover.len(), 1);
    assert_eq!(takeover[0].delivery_owner.as_deref(), Some("scheduler-b"));
    let pending_fire_at = takeover[0]
        .pending_fire_at
        .expect("takeover keeps the durable fire id");
    assert!(
        store
            .acknowledge_delivery(DeliveryAcknowledgement {
                id: job.id.clone(),
                pending_fire_at,
                owner: "scheduler-a".to_string(),
                delivered_at: takeover_at,
            })
            .await
            .is_err()
    );
    store
        .acknowledge_delivery(DeliveryAcknowledgement {
            id: job.id,
            pending_fire_at,
            owner: "scheduler-b".to_string(),
            delivered_at: takeover_at,
        })
        .await
        .expect("current lease owner acknowledges");
}

#[tokio::test]
async fn in_flight_job_is_coalesced_until_the_runtime_clears_it() {
    let temp_dir = TempDir::new().expect("temp dir");
    let store = store(&temp_dir);
    let job = store
        .create_job(
            Some("coalesced".to_string()),
            "Queue at most once per turn.".to_string(),
            AutomationKind::Loop,
            ScheduleSpec::Interval { every_seconds: 1 },
        )
        .await
        .expect("create loop");
    let due_at =
        chrono::DateTime::from_timestamp_millis(job.next_due_at).expect("valid due timestamp");

    assert_eq!(
        store
            .claim_due(lease_excluding("scheduler-a", due_at, &job.id))
            .await
            .expect("excluded reconciliation"),
        Vec::new()
    );
    assert_eq!(
        store
            .claim_due(lease("scheduler-a", due_at))
            .await
            .expect("claim after idle gate clears")
            .len(),
        1
    );
}

#[tokio::test]
async fn accepts_five_field_utc_cron_and_persists_updates() {
    let temp_dir = TempDir::new().expect("temp dir");
    let store = store(&temp_dir);
    let job = store
        .create_job(
            None,
            "Daily mock check.".to_string(),
            AutomationKind::Cron,
            ScheduleSpec::Cron {
                expression: "0 9 * * *".to_string(),
            },
        )
        .await
        .expect("create five-field cron");
    let updated = store
        .update_job(
            job.id.clone(),
            JobPatch {
                prompt: Some("Updated mock check.".to_string()),
                enabled: Some(false),
                schedule: None,
            },
        )
        .await
        .expect("update cron");
    assert_eq!(updated.prompt, "Updated mock check.");
    assert!(!updated.enabled);
    assert!(updated.next_due_at > Utc::now().timestamp_millis());
    assert_eq!(
        store.read().await.expect("reload state").jobs,
        vec![updated]
    );
}

#[tokio::test]
async fn monitor_definitions_are_persisted_and_stoppable() {
    let temp_dir = TempDir::new().expect("temp dir");
    let store = store(&temp_dir);
    let monitor = store
        .add_monitor(
            Some("listener".to_string()),
            1234,
            "Wake on mock peer event.".to_string(),
            Some("MOCK_EVENT".to_string()),
            5,
            true,
        )
        .await
        .expect("create monitor");
    let stopped = store
        .stop_monitor(monitor.id.clone())
        .await
        .expect("stop monitor");
    assert!(!stopped.enabled);
    assert_eq!(
        store.read().await.expect("reload state").monitors,
        vec![stopped]
    );
}

#[tokio::test]
async fn rejects_prompts_that_exceed_the_model_context_bound() {
    let temp_dir = TempDir::new().expect("temp dir");
    let error = store(&temp_dir)
        .create_job(
            None,
            "x".repeat((super::MAX_AUTOMATION_PROMPT_TOKENS + 1) * 4),
            AutomationKind::Loop,
            ScheduleSpec::Interval { every_seconds: 1 },
        )
        .await
        .expect_err("oversized prompt must be rejected");
    assert_eq!(
        error,
        "automation prompt exceeds the 4000-token model-context limit"
    );
}
