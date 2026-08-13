use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Weak;
use std::time::Duration;

use chrono::Utc;
use codex_core::ThreadManager;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::Op;
use tokio::sync::Mutex;
use tokio::sync::watch;
use uuid::Uuid;

use crate::state_store::AutomationJob;
use crate::state_store::AutomationKind;
use crate::state_store::AutomationState;
use crate::state_store::AutomationStore;
use crate::state_store::DeliveryAcknowledgement;
use crate::state_store::DeliveryLeaseRequest;
use crate::state_store::JobPatch;
use crate::state_store::MonitorDefinition;
use crate::state_store::ScheduleSpec;

const SCHEDULER_POLL: Duration = Duration::from_secs(1);
const DELIVERY_LEASE_SECONDS: i64 = 30;

#[derive(Clone)]
pub(crate) struct AutomationRuntime {
    store: Arc<AutomationStore>,
    thread_manager: Weak<ThreadManager>,
    thread_id: ThreadId,
    scheduler_owner: String,
    scheduled_in_flight: Arc<Mutex<HashSet<String>>>,
    cancel: watch::Sender<bool>,
    monitor_cancels: Arc<Mutex<HashMap<String, watch::Sender<bool>>>>,
}

/// Host-facing control handle for durable thread loops.
///
/// Hosts use this handle for direct UI and RPC operations; model tool calls use the
/// same underlying runtime, so both paths observe one durable state store.
#[derive(Clone, Debug)]
pub struct AutomationHandle {
    runtime: AutomationRuntime,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoopInfo {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub every_seconds: u64,
    pub enabled: bool,
    pub created_at: i64,
    pub next_due_at: i64,
    pub last_run_at: Option<i64>,
}

impl AutomationHandle {
    pub(crate) fn new(runtime: AutomationRuntime) -> Self {
        Self { runtime }
    }

    pub async fn create_loop(
        &self,
        name: Option<String>,
        prompt: String,
        every_seconds: u64,
    ) -> Result<LoopInfo, String> {
        self.runtime
            .create_loop(name, prompt, every_seconds)
            .await
            .and_then(loop_info)
    }

    pub async fn list_loops(&self) -> Result<Vec<LoopInfo>, String> {
        self.runtime
            .state()
            .await?
            .jobs
            .into_iter()
            .filter(|job| job.kind == AutomationKind::Loop)
            .map(loop_info)
            .collect()
    }

    pub async fn delete_loop(&self, id: String) -> Result<LoopInfo, String> {
        self.runtime
            .delete_job_kind(id, AutomationKind::Loop)
            .await
            .and_then(loop_info)
    }
}

fn loop_info(job: AutomationJob) -> Result<LoopInfo, String> {
    let ScheduleSpec::Interval { every_seconds } = job.schedule else {
        return Err(format!("loop `{}` has a non-interval schedule", job.id));
    };
    Ok(LoopInfo {
        id: job.id,
        name: job.name,
        prompt: job.prompt,
        every_seconds,
        enabled: job.enabled,
        created_at: job.created_at,
        next_due_at: job.next_due_at,
        last_run_at: job.last_run_at,
    })
}

impl std::fmt::Debug for AutomationRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AutomationRuntime")
            .field("thread_id", &self.thread_id)
            .finish_non_exhaustive()
    }
}

impl AutomationRuntime {
    pub(crate) async fn start(
        store: AutomationStore,
        thread_manager: Weak<ThreadManager>,
        thread_id: ThreadId,
    ) -> Result<Self, String> {
        let state = store.read().await?;
        let (cancel, cancel_rx) = watch::channel(false);
        let runtime = Self {
            store: Arc::new(store),
            thread_manager,
            thread_id,
            scheduler_owner: Uuid::now_v7().to_string(),
            scheduled_in_flight: Arc::new(Mutex::new(HashSet::new())),
            cancel,
            monitor_cancels: Arc::new(Mutex::new(HashMap::new())),
        };
        tokio::spawn(run_scheduler(runtime.clone(), cancel_rx));
        for monitor in state.monitors.into_iter().filter(|monitor| monitor.enabled) {
            runtime.spawn_monitor(monitor).await;
        }
        Ok(runtime)
    }

    pub(crate) fn stop(&self) {
        let _ = self.cancel.send(true);
    }

    pub(crate) async fn clear_scheduled_in_flight(&self) {
        self.scheduled_in_flight.lock().await.clear();
    }

    pub(crate) async fn state(&self) -> Result<AutomationState, String> {
        self.store.read().await
    }

    pub(crate) async fn create_loop(
        &self,
        name: Option<String>,
        prompt: String,
        every_seconds: u64,
    ) -> Result<AutomationJob, String> {
        self.store
            .create_job(
                name,
                prompt,
                AutomationKind::Loop,
                ScheduleSpec::Interval { every_seconds },
            )
            .await
    }

    pub(crate) async fn create_cron(
        &self,
        name: Option<String>,
        prompt: String,
        expression: String,
    ) -> Result<AutomationJob, String> {
        self.store
            .create_job(
                name,
                prompt,
                AutomationKind::Cron,
                ScheduleSpec::Cron { expression },
            )
            .await
    }

    pub(crate) async fn update_job(
        &self,
        id: String,
        patch: JobPatch,
    ) -> Result<AutomationJob, String> {
        self.store.update_job(id, patch).await
    }

    pub(crate) async fn delete_job(&self, id: String) -> Result<AutomationJob, String> {
        self.store.delete_job(id).await
    }

    pub(crate) async fn delete_job_kind(
        &self,
        id: String,
        expected_kind: AutomationKind,
    ) -> Result<AutomationJob, String> {
        let job = self.store.job(&id).await?;
        if job.kind != expected_kind {
            let expected = match expected_kind {
                AutomationKind::Loop => "loop",
                AutomationKind::Cron => "cron",
            };
            return Err(format!("automation `{id}` is not a {expected}"));
        }
        self.delete_job(id).await
    }

    pub(crate) async fn run_job(&self, id: &str) -> Result<AutomationJob, String> {
        let job = self.store.job(id).await?;
        self.deliver_job(&job, "MANUAL").await?;
        Ok(job)
    }

    pub(crate) async fn create_monitor(
        &self,
        name: Option<String>,
        process_id: i32,
        prompt: String,
        contains: Option<String>,
        poll_seconds: u64,
        once: bool,
    ) -> Result<MonitorDefinition, String> {
        let monitor = self
            .store
            .add_monitor(name, process_id, prompt, contains, poll_seconds, once)
            .await?;
        self.spawn_monitor(monitor.clone()).await;
        Ok(monitor)
    }

    pub(crate) async fn stop_monitor(&self, id: String) -> Result<MonitorDefinition, String> {
        if let Some(cancel) = self.monitor_cancels.lock().await.remove(&id) {
            let _ = cancel.send(true);
        }
        self.store.stop_monitor(id).await
    }

    /// Claude-`Monitor`-style launch-and-watch: spawn `command`, stream its
    /// stdout, and wake the thread on each (optionally `contains`-filtered) line.
    /// Session-only (not persisted), matching Claude's Monitor semantics. Stop
    /// with `monitor_stop` using the returned id, or it ends when the process
    /// exits / the thread shuts down.
    pub(crate) async fn launch_monitor(
        &self,
        name: Option<String>,
        command: String,
        contains: Option<String>,
        once: bool,
    ) -> Result<serde_json::Value, String> {
        use tokio::io::AsyncBufReadExt;
        let mut child = tokio::process::Command::new("bash")
            .arg("-lc")
            .arg(&command)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| format!("failed to launch monitor command: {error}"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "monitor command produced no stdout handle".to_string())?;
        let id = format!("moncmd-{}", Uuid::now_v7());
        let display = name.clone().unwrap_or_else(|| "monitor".to_string());
        let (cancel, mut cancel_rx) = watch::channel(false);
        self.monitor_cancels
            .lock()
            .await
            .insert(id.clone(), cancel);
        let runtime = self.clone();
        let monitor_id = id.clone();
        let contains_filter = contains.clone();
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(stdout).lines();
            loop {
                tokio::select! {
                    next = lines.next_line() => match next {
                        Ok(Some(line)) => {
                            let matched = contains_filter
                                .as_deref()
                                .is_none_or(|needle| line.contains(needle));
                            if matched {
                                let _ = runtime
                                    .deliver(format!("[MONITOR:{display} {monitor_id}]\n{line}"))
                                    .await;
                                if once {
                                    break;
                                }
                            }
                        }
                        Ok(None) | Err(_) => break,
                    },
                    changed = cancel_rx.changed() => {
                        if changed.is_err() || *cancel_rx.borrow() {
                            break;
                        }
                    }
                }
            }
            let _ = child.kill().await;
            runtime.monitor_cancels.lock().await.remove(&monitor_id);
        });
        Ok(serde_json::json!({
            "id": id,
            "kind": "command_monitor",
            "command": command,
            "matching": contains,
            "once": once,
        }))
    }

    async fn spawn_monitor(&self, monitor: MonitorDefinition) {
        let (cancel, cancel_rx) = watch::channel(false);
        self.monitor_cancels
            .lock()
            .await
            .insert(monitor.id.clone(), cancel);
        tokio::spawn(run_monitor(self.clone(), monitor, cancel_rx));
    }

    async fn deliver_job(&self, job: &AutomationJob, source: &str) -> Result<(), String> {
        let kind = match job.kind {
            AutomationKind::Loop => "LOOP",
            AutomationKind::Cron => "CRON",
        };
        self.deliver(format!(
            "[{kind}:{source} {} ({})]\n{}",
            job.name, job.id, job.prompt
        ))
        .await
    }

    async fn deliver(&self, content: String) -> Result<(), String> {
        let manager = self
            .thread_manager
            .upgrade()
            .ok_or_else(|| "thread manager is no longer available".to_string())?;
        let thread = manager
            .get_thread(self.thread_id)
            .await
            .map_err(|error| error.to_string())?;
        let communication = InterAgentCommunication::new(
            AgentPath::try_from("/root/automation".to_string())
                .expect("static automation path must be valid"),
            AgentPath::root(),
            Vec::new(),
            content,
            /*trigger_turn*/ true,
        );
        thread
            .submit(Op::InterAgentCommunication { communication })
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

async fn run_scheduler(runtime: AutomationRuntime, mut cancel: watch::Receiver<bool>) {
    loop {
        if *cancel.borrow() {
            return;
        }
        let claimed_at = Utc::now();
        let excluded_job_ids = runtime.scheduled_in_flight.lock().await.clone();
        match runtime
            .store
            .claim_due(DeliveryLeaseRequest {
                owner: runtime.scheduler_owner.clone(),
                claimed_at,
                duration: chrono::Duration::seconds(DELIVERY_LEASE_SECONDS),
                excluded_job_ids,
            })
            .await
        {
            Ok(jobs) => {
                for job in jobs {
                    match runtime.deliver_job(&job, "SCHEDULED").await {
                        Ok(()) => {
                            runtime
                                .scheduled_in_flight
                                .lock()
                                .await
                                .insert(job.id.clone());
                            let Some(pending_fire_at) = job.pending_fire_at else {
                                tracing::warn!(job_id = %job.id, "claimed automation had no pending fire id");
                                continue;
                            };
                            if let Err(error) = runtime
                                .store
                                .acknowledge_delivery(DeliveryAcknowledgement {
                                    id: job.id.clone(),
                                    pending_fire_at,
                                    owner: runtime.scheduler_owner.clone(),
                                    delivered_at: Utc::now(),
                                })
                                .await
                            {
                                tracing::warn!(job_id = %job.id, %error, "failed to acknowledge automation delivery");
                            }
                        }
                        Err(error) => {
                            tracing::warn!(job_id = %job.id, %error, "failed to deliver automation");
                        }
                    }
                }
            }
            Err(error) => tracing::warn!(%error, "failed to reconcile automation schedule"),
        }
        tokio::select! {
            _ = tokio::time::sleep(SCHEDULER_POLL) => {}
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return;
                }
            }
        }
    }
}

async fn run_monitor(
    runtime: AutomationRuntime,
    monitor: MonitorDefinition,
    mut cancel: watch::Receiver<bool>,
) {
    let mut runtime_cancel = runtime.cancel.subscribe();
    loop {
        if *cancel.borrow() || *runtime_cancel.borrow() {
            break;
        }
        let manager = match runtime.thread_manager.upgrade() {
            Some(manager) => manager,
            None => break,
        };
        let thread = match manager.get_thread(runtime.thread_id).await {
            Ok(thread) => thread,
            Err(_) => break,
        };
        let wait = Duration::from_secs(monitor.poll_seconds.max(1));
        let poll_result = tokio::select! {
            result = thread.poll_background_terminal(monitor.process_id, wait) => result,
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
                continue;
            }
            changed = runtime_cancel.changed() => {
                if changed.is_err() || *runtime_cancel.borrow() {
                    break;
                }
                continue;
            }
        };
        match poll_result {
            Ok(poll) => {
                let matched = !poll.output.is_empty()
                    && monitor
                        .contains
                        .as_deref()
                        .is_none_or(|needle| poll.output.contains(needle));
                if matched {
                    let content = format!(
                        "[MONITOR {} ({})] {}\nProcess {} output:\n{}",
                        monitor.name, monitor.id, monitor.prompt, monitor.process_id, poll.output
                    );
                    if let Err(error) = runtime.deliver(content).await {
                        tracing::warn!(monitor_id = %monitor.id, %error, "failed to deliver monitor event");
                    }
                    if monitor.once {
                        let _ = runtime.store.stop_monitor(monitor.id.clone()).await;
                        break;
                    }
                }
                if poll.process_id.is_none() {
                    let content = format!(
                        "[MONITOR {} ({})] {}\nProcess {} exited with code {:?}.",
                        monitor.name,
                        monitor.id,
                        monitor.prompt,
                        monitor.process_id,
                        poll.exit_code
                    );
                    let _ = runtime.deliver(content).await;
                    let _ = runtime.store.stop_monitor(monitor.id.clone()).await;
                    break;
                }
            }
            Err(error) => {
                let content = format!(
                    "[MONITOR {} ({})] Process {} is no longer attachable: {error}",
                    monitor.name, monitor.id, monitor.process_id
                );
                let _ = runtime.deliver(content).await;
                let _ = runtime.store.stop_monitor(monitor.id.clone()).await;
                break;
            }
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_millis(10)) => {}
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    break;
                }
            }
            changed = runtime_cancel.changed() => {
                if changed.is_err() || *runtime_cancel.borrow() {
                    break;
                }
            }
        }
    }
    runtime.monitor_cancels.lock().await.remove(&monitor.id);
}
