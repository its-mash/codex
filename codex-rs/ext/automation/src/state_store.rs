use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::str::FromStr;

use chrono::DateTime;
use chrono::Utc;
use codex_utils_string::approx_token_count;
use cron::Schedule;
use serde::Deserialize;
use serde::Serialize;
use uuid::Uuid;

const STATE_VERSION: u32 = 1;
const MAX_STATE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_AUTOMATIONS: usize = 1_000;
pub(crate) const MAX_MONITOR_POLL_SECONDS: u64 = 3_600;
const MAX_AUTOMATION_PROMPT_TOKENS: usize = 4_000;

#[derive(Clone, Debug)]
pub(crate) struct AutomationStore {
    path: PathBuf,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AutomationKind {
    Loop,
    Cron,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ScheduleSpec {
    Interval { every_seconds: u64 },
    Cron { expression: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AutomationJob {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) prompt: String,
    pub(crate) kind: AutomationKind,
    pub(crate) schedule: ScheduleSpec,
    pub(crate) enabled: bool,
    pub(crate) created_at: i64,
    pub(crate) next_due_at: i64,
    pub(crate) last_run_at: Option<i64>,
    #[serde(default)]
    pub(crate) pending_fire_at: Option<i64>,
    #[serde(default)]
    pub(crate) delivery_owner: Option<String>,
    #[serde(default)]
    pub(crate) delivery_lease_expires_at: Option<i64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct MonitorDefinition {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) process_id: i32,
    pub(crate) prompt: String,
    pub(crate) contains: Option<String>,
    pub(crate) poll_seconds: u64,
    pub(crate) once: bool,
    pub(crate) enabled: bool,
    pub(crate) created_at: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct AutomationState {
    #[serde(default = "state_version")]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) jobs: Vec<AutomationJob>,
    #[serde(default)]
    pub(crate) monitors: Vec<MonitorDefinition>,
}

fn state_version() -> u32 {
    STATE_VERSION
}

#[derive(Clone, Debug, Default)]
pub(crate) struct JobPatch {
    pub(crate) prompt: Option<String>,
    pub(crate) enabled: Option<bool>,
    pub(crate) schedule: Option<ScheduleSpec>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeliveryLeaseRequest {
    pub(crate) owner: String,
    pub(crate) claimed_at: DateTime<Utc>,
    pub(crate) duration: chrono::Duration,
    pub(crate) excluded_job_ids: HashSet<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct DeliveryAcknowledgement {
    pub(crate) id: String,
    pub(crate) pending_fire_at: i64,
    pub(crate) owner: String,
    pub(crate) delivered_at: DateTime<Utc>,
}

impl AutomationStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) async fn read(&self) -> Result<AutomationState, String> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || read_state(&path))
            .await
            .map_err(|error| format!("automation state reader failed: {error}"))?
    }

    pub(crate) async fn create_job(
        &self,
        name: Option<String>,
        prompt: String,
        kind: AutomationKind,
        schedule: ScheduleSpec,
    ) -> Result<AutomationJob, String> {
        validate_prompt(&prompt)?;
        let now = Utc::now();
        let next_due_at = next_due(&schedule, now)?;
        let job = AutomationJob {
            id: Uuid::now_v7().to_string(),
            name: name.unwrap_or_else(|| match kind {
                AutomationKind::Loop => "loop".to_string(),
                AutomationKind::Cron => "cron".to_string(),
            }),
            prompt,
            kind,
            schedule,
            enabled: true,
            created_at: now.timestamp_millis(),
            next_due_at,
            last_run_at: None,
            pending_fire_at: None,
            delivery_owner: None,
            delivery_lease_expires_at: None,
        };
        let path = self.path.clone();
        let result = job.clone();
        tokio::task::spawn_blocking(move || {
            mutate_state(&path, |state| {
                ensure_capacity(state)?;
                state.jobs.push(job);
                Ok(())
            })
        })
        .await
        .map_err(|error| format!("automation state writer failed: {error}"))??;
        Ok(result)
    }

    pub(crate) async fn update_job(
        &self,
        id: String,
        patch: JobPatch,
    ) -> Result<AutomationJob, String> {
        if let Some(prompt) = patch.prompt.as_deref() {
            validate_prompt(prompt)?;
        }
        if let Some(schedule) = patch.schedule.as_ref() {
            validate_schedule(schedule)?;
        }
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut updated = None;
            mutate_state(&path, |state| {
                let job = state
                    .jobs
                    .iter_mut()
                    .find(|job| job.id == id)
                    .ok_or_else(|| format!("automation `{id}` not found"))?;
                if let Some(prompt) = patch.prompt {
                    job.prompt = prompt;
                }
                if let Some(enabled) = patch.enabled {
                    job.enabled = enabled;
                    if !enabled {
                        job.pending_fire_at = None;
                        job.delivery_owner = None;
                        job.delivery_lease_expires_at = None;
                    }
                }
                if let Some(schedule) = patch.schedule {
                    job.next_due_at = next_due(&schedule, Utc::now())?;
                    job.schedule = schedule;
                    job.pending_fire_at = None;
                    job.delivery_owner = None;
                    job.delivery_lease_expires_at = None;
                }
                updated = Some(job.clone());
                Ok(())
            })?;
            updated.ok_or_else(|| format!("automation `{id}` not found"))
        })
        .await
        .map_err(|error| format!("automation state writer failed: {error}"))?
    }

    pub(crate) async fn delete_job(&self, id: String) -> Result<AutomationJob, String> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut removed = None;
            mutate_state(&path, |state| {
                let index = state
                    .jobs
                    .iter()
                    .position(|job| job.id == id)
                    .ok_or_else(|| format!("automation `{id}` not found"))?;
                removed = Some(state.jobs.remove(index));
                Ok(())
            })?;
            removed.ok_or_else(|| format!("automation `{id}` not found"))
        })
        .await
        .map_err(|error| format!("automation state writer failed: {error}"))?
    }

    pub(crate) async fn job(&self, id: &str) -> Result<AutomationJob, String> {
        self.read()
            .await?
            .jobs
            .into_iter()
            .find(|job| job.id == id)
            .ok_or_else(|| format!("automation `{id}` not found"))
    }

    pub(crate) async fn claim_due(
        &self,
        request: DeliveryLeaseRequest,
    ) -> Result<Vec<AutomationJob>, String> {
        let lease_expires_at = request
            .claimed_at
            .checked_add_signed(request.duration)
            .ok_or_else(|| "automation delivery lease is out of range".to_string())?
            .timestamp_millis();
        if lease_expires_at <= request.claimed_at.timestamp_millis() {
            return Err("automation delivery lease duration must be positive".to_string());
        }
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut due = Vec::new();
            mutate_state(&path, |state| {
                for job in &mut state.jobs {
                    if request.excluded_job_ids.contains(&job.id) {
                        continue;
                    }
                    let claim_available = job.delivery_owner.as_deref()
                        == Some(request.owner.as_str())
                        || job.delivery_lease_expires_at.is_none_or(|expires_at| {
                            expires_at <= request.claimed_at.timestamp_millis()
                        });
                    if job.enabled
                        && claim_available
                        && (job.pending_fire_at.is_some()
                            || job.next_due_at <= request.claimed_at.timestamp_millis())
                    {
                        if job.pending_fire_at.is_none() {
                            job.pending_fire_at = Some(job.next_due_at);
                        }
                        job.delivery_owner = Some(request.owner.clone());
                        job.delivery_lease_expires_at = Some(lease_expires_at);
                        due.push(job.clone());
                    }
                }
                Ok(())
            })?;
            Ok(due)
        })
        .await
        .map_err(|error| format!("automation scheduler failed: {error}"))?
    }

    pub(crate) async fn acknowledge_delivery(
        &self,
        acknowledgement: DeliveryAcknowledgement,
    ) -> Result<(), String> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            mutate_state(&path, |state| {
                let job = state
                    .jobs
                    .iter_mut()
                    .find(|job| job.id == acknowledgement.id)
                    .ok_or_else(|| format!("automation `{}` not found", acknowledgement.id))?;
                if job.pending_fire_at != Some(acknowledgement.pending_fire_at)
                    || job.delivery_owner.as_deref() != Some(acknowledgement.owner.as_str())
                {
                    return Err(format!(
                        "automation `{}` delivery claim changed before acknowledgement",
                        acknowledgement.id
                    ));
                }
                job.last_run_at = Some(acknowledgement.delivered_at.timestamp_millis());
                job.next_due_at = next_due(&job.schedule, acknowledgement.delivered_at)?;
                job.pending_fire_at = None;
                job.delivery_owner = None;
                job.delivery_lease_expires_at = None;
                Ok(())
            })
        })
        .await
        .map_err(|error| format!("automation delivery acknowledgement failed: {error}"))?
    }

    pub(crate) async fn add_monitor(
        &self,
        name: Option<String>,
        process_id: i32,
        prompt: String,
        contains: Option<String>,
        poll_seconds: u64,
        once: bool,
    ) -> Result<MonitorDefinition, String> {
        validate_prompt(&prompt)?;
        if !(1..=MAX_MONITOR_POLL_SECONDS).contains(&poll_seconds) {
            return Err(format!(
                "monitor poll_seconds must be between 1 and {MAX_MONITOR_POLL_SECONDS}"
            ));
        }
        let monitor = MonitorDefinition {
            id: Uuid::now_v7().to_string(),
            name: name.unwrap_or_else(|| format!("process-{process_id}")),
            process_id,
            prompt,
            contains,
            poll_seconds,
            once,
            enabled: true,
            created_at: Utc::now().timestamp_millis(),
        };
        let result = monitor.clone();
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            mutate_state(&path, |state| {
                ensure_capacity(state)?;
                state.monitors.push(monitor);
                Ok(())
            })
        })
        .await
        .map_err(|error| format!("automation state writer failed: {error}"))??;
        Ok(result)
    }

    pub(crate) async fn stop_monitor(&self, id: String) -> Result<MonitorDefinition, String> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut stopped = None;
            mutate_state(&path, |state| {
                let monitor = state
                    .monitors
                    .iter_mut()
                    .find(|monitor| monitor.id == id)
                    .ok_or_else(|| format!("monitor `{id}` not found"))?;
                monitor.enabled = false;
                stopped = Some(monitor.clone());
                Ok(())
            })?;
            stopped.ok_or_else(|| format!("monitor `{id}` not found"))
        })
        .await
        .map_err(|error| format!("automation state writer failed: {error}"))?
    }
}

pub(crate) fn validate_schedule(schedule: &ScheduleSpec) -> Result<(), String> {
    match schedule {
        ScheduleSpec::Interval { every_seconds } if *every_seconds == 0 => {
            Err("interval every_seconds must be at least 1".to_string())
        }
        ScheduleSpec::Interval { .. } => Ok(()),
        ScheduleSpec::Cron { expression } => parse_cron(expression).map(|_| ()),
    }
}

fn next_due(schedule: &ScheduleSpec, now: DateTime<Utc>) -> Result<i64, String> {
    validate_schedule(schedule)?;
    match schedule {
        ScheduleSpec::Interval { every_seconds } => {
            let interval_millis = i64::try_from(*every_seconds)
                .ok()
                .and_then(|seconds| seconds.checked_mul(1_000))
                .ok_or_else(|| "interval is too large to represent".to_string())?;
            now.timestamp_millis()
                .checked_add(interval_millis)
                .ok_or_else(|| "next interval occurrence is out of range".to_string())
        }
        ScheduleSpec::Cron { expression } => parse_cron(expression)?
            .after(&now)
            .next()
            .map(|next| next.timestamp_millis())
            .ok_or_else(|| "cron expression has no future occurrence".to_string()),
    }
}

fn parse_cron(expression: &str) -> Result<Schedule, String> {
    let fields = expression.split_whitespace().count();
    let normalized = if fields == 5 {
        format!("0 {expression}")
    } else {
        expression.to_string()
    };
    Schedule::from_str(&normalized).map_err(|error| format!("invalid cron expression: {error}"))
}

fn ensure_capacity(state: &AutomationState) -> Result<(), String> {
    if state.jobs.len() + state.monitors.len() >= MAX_AUTOMATIONS {
        Err(format!(
            "automation state is limited to {MAX_AUTOMATIONS} entries"
        ))
    } else {
        Ok(())
    }
}

fn validate_prompt(prompt: &str) -> Result<(), String> {
    if prompt.trim().is_empty() {
        Err("automation prompt must not be empty".to_string())
    } else if approx_token_count(prompt) > MAX_AUTOMATION_PROMPT_TOKENS {
        Err(format!(
            "automation prompt exceeds the {MAX_AUTOMATION_PROMPT_TOKENS}-token model-context limit"
        ))
    } else {
        Ok(())
    }
}

fn read_state(path: &Path) -> Result<AutomationState, String> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.len() > MAX_STATE_BYTES => {
            return Err(format!(
                "{} exceeds {MAX_STATE_BYTES} bytes",
                path.display()
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(AutomationState {
                version: STATE_VERSION,
                ..Default::default()
            });
        }
        Err(error) => return Err(error.to_string()),
    }
    let bytes = std::fs::read(path).map_err(|error| error.to_string())?;
    let state: AutomationState =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    if state.version != STATE_VERSION {
        return Err(format!(
            "unsupported automation state version {}",
            state.version
        ));
    }
    if state.jobs.len() + state.monitors.len() > MAX_AUTOMATIONS {
        return Err(format!(
            "automation state exceeds the {MAX_AUTOMATIONS}-entry limit"
        ));
    }
    for job in &state.jobs {
        validate_prompt(&job.prompt)
            .map_err(|error| format!("automation `{}` is invalid: {error}", job.id))?;
        validate_schedule(&job.schedule)
            .map_err(|error| format!("automation `{}` is invalid: {error}", job.id))?;
    }
    for monitor in &state.monitors {
        validate_prompt(&monitor.prompt)
            .map_err(|error| format!("monitor `{}` is invalid: {error}", monitor.id))?;
        if !(1..=MAX_MONITOR_POLL_SECONDS).contains(&monitor.poll_seconds) {
            return Err(format!(
                "monitor `{}` poll_seconds must be between 1 and {MAX_MONITOR_POLL_SECONDS}",
                monitor.id
            ));
        }
    }
    Ok(state)
}

fn mutate_state<T>(
    path: &Path,
    mutation: impl FnOnce(&mut AutomationState) -> Result<T, String>,
) -> Result<T, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(parent.join(".automation.lock"))
        .map_err(|error| error.to_string())?;
    lock.lock().map_err(|error| error.to_string())?;
    let mut state = read_state(path)?;
    let result = mutation(&mut state);
    if result.is_ok() {
        write_state(path, &state)?;
    }
    let _ = lock.unlock();
    result
}

fn write_state(path: &Path, state: &AutomationState) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    let temp_path = parent.join(format!(".automation-{}.tmp", Uuid::now_v7()));
    let bytes = serde_json::to_vec_pretty(state).map_err(|error| error.to_string())?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temp_path, path).map_err(|error| error.to_string())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp_path);
    }
    result
}

#[cfg(test)]
#[path = "state_store_tests.rs"]
mod tests;
