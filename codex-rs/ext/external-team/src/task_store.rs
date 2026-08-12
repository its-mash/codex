use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use uuid::Uuid;

const MAX_TASK_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TASKS: usize = 10_000;

#[derive(Clone, Debug)]
pub(crate) struct ClaudeTaskStore {
    root: PathBuf,
    agent_name: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeTask {
    pub(crate) id: String,
    pub(crate) subject: String,
    #[serde(default)]
    pub(crate) description: String,
    #[serde(default)]
    pub(crate) active_form: String,
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) blocks: Vec<String>,
    #[serde(default)]
    pub(crate) blocked_by: Vec<String>,
    #[serde(default)]
    pub(crate) owner: String,
    #[serde(flatten)]
    pub(crate) extra: BTreeMap<String, Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct VersionedTask {
    pub(crate) task: ClaudeTask,
    pub(crate) revision: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskPatch {
    pub(crate) subject: Option<String>,
    pub(crate) description: Option<String>,
    pub(crate) active_form: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) owner: Option<String>,
    pub(crate) blocks: Option<Vec<String>>,
    pub(crate) blocked_by: Option<Vec<String>>,
}

#[derive(Clone, Debug)]
enum TaskMutationPrecondition {
    Any,
    Pending,
    OwnedInProgress { owner: String },
}

impl ClaudeTaskStore {
    pub(crate) fn new(claude_home: &Path, team_name: &str, agent_name: &str) -> Self {
        Self {
            root: claude_home.join("tasks").join(team_name),
            agent_name: agent_name.to_string(),
        }
    }

    pub(crate) async fn list(&self) -> Result<Vec<VersionedTask>, String> {
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || list_sync(&root))
            .await
            .map_err(|error| format!("Claude task reader failed: {error}"))?
    }

    pub(crate) async fn get(&self, id: &str) -> Result<VersionedTask, String> {
        validate_task_id(id)?;
        let path = self.root.join(format!("{id}.json"));
        tokio::task::spawn_blocking(move || read_task_sync(&path))
            .await
            .map_err(|error| format!("Claude task reader failed: {error}"))?
    }

    pub(crate) async fn create(
        &self,
        subject: String,
        description: String,
        active_form: String,
    ) -> Result<VersionedTask, String> {
        validate_nonempty("task subject", &subject)?;
        let root = self.root.clone();
        tokio::task::spawn_blocking(move || {
            with_lock(&root, || {
                std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
                let id = next_task_id(&root)?;
                let task = ClaudeTask {
                    id: id.clone(),
                    subject,
                    description,
                    active_form,
                    status: "pending".to_string(),
                    blocks: Vec::new(),
                    blocked_by: Vec::new(),
                    owner: String::new(),
                    extra: BTreeMap::new(),
                };
                write_task_sync(&root.join(format!("{id}.json")), &task)
            })
        })
        .await
        .map_err(|error| format!("Claude task writer failed: {error}"))?
    }

    pub(crate) async fn claim(
        &self,
        id: &str,
        expected_revision: String,
    ) -> Result<VersionedTask, String> {
        self.patch_with_precondition(
            id,
            TaskPatch {
                status: Some("in_progress".to_string()),
                owner: Some(self.agent_name.clone()),
                ..Default::default()
            },
            expected_revision,
            TaskMutationPrecondition::Pending,
        )
        .await
    }

    pub(crate) async fn complete(
        &self,
        id: &str,
        expected_revision: String,
    ) -> Result<VersionedTask, String> {
        self.patch_with_precondition(
            id,
            TaskPatch {
                status: Some("completed".to_string()),
                ..Default::default()
            },
            expected_revision,
            TaskMutationPrecondition::OwnedInProgress {
                owner: self.agent_name.clone(),
            },
        )
        .await
    }

    pub(crate) async fn patch(
        &self,
        id: &str,
        patch: TaskPatch,
        expected_revision: String,
    ) -> Result<VersionedTask, String> {
        self.patch_with_precondition(
            id,
            patch,
            expected_revision,
            TaskMutationPrecondition::Any,
        )
        .await
    }

    async fn patch_with_precondition(
        &self,
        id: &str,
        patch: TaskPatch,
        expected_revision: String,
        precondition: TaskMutationPrecondition,
    ) -> Result<VersionedTask, String> {
        validate_task_id(id)?;
        if let Some(status) = patch.status.as_deref() {
            validate_status(status)?;
        }
        if let Some(subject) = patch.subject.as_deref() {
            validate_nonempty("task subject", subject)?;
        }
        let root = self.root.clone();
        let id = id.to_string();
        let path = root.join(format!("{id}.json"));
        tokio::task::spawn_blocking(move || {
            with_lock(&root, || {
                let current = read_task_sync(&path)?;
                if current.revision != expected_revision {
                    return Err(format!(
                        "task {id} changed concurrently; expected revision {expected_revision}, current revision {}",
                        current.revision
                    ));
                }
                validate_mutation_precondition(&id, &current.task, precondition)?;
                let mut task = current.task;
                apply_patch(&mut task, patch);
                write_task_sync(&path, &task)
            })
        })
        .await
        .map_err(|error| format!("Claude task writer failed: {error}"))?
    }
}

fn validate_mutation_precondition(
    id: &str,
    task: &ClaudeTask,
    precondition: TaskMutationPrecondition,
) -> Result<(), String> {
    match precondition {
        TaskMutationPrecondition::Any => Ok(()),
        TaskMutationPrecondition::Pending if task.status == "pending" => Ok(()),
        TaskMutationPrecondition::Pending => Err(format!(
            "task {id} cannot be claimed from status `{}`",
            task.status
        )),
        TaskMutationPrecondition::OwnedInProgress { owner }
            if task.status == "in_progress" && task.owner == owner =>
        {
            Ok(())
        }
        TaskMutationPrecondition::OwnedInProgress { owner } => Err(format!(
            "task {id} can only be completed while in_progress and owned by `{owner}`"
        )),
    }
}

fn apply_patch(task: &mut ClaudeTask, patch: TaskPatch) {
    if let Some(subject) = patch.subject {
        task.subject = subject;
    }
    if let Some(description) = patch.description {
        task.description = description;
    }
    if let Some(active_form) = patch.active_form {
        task.active_form = active_form;
    }
    if let Some(status) = patch.status {
        task.status = status;
    }
    if let Some(owner) = patch.owner {
        task.owner = owner;
    }
    if let Some(blocks) = patch.blocks {
        task.blocks = blocks;
    }
    if let Some(blocked_by) = patch.blocked_by {
        task.blocked_by = blocked_by;
    }
}

fn list_sync(root: &Path) -> Result<Vec<VersionedTask>, String> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut tasks = entries
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .extension()
                .is_some_and(|extension| extension == "json")
        })
        .take(MAX_TASKS + 1)
        .map(|entry| read_task_sync(&entry.path()))
        .collect::<Result<Vec<_>, _>>()?;
    if tasks.len() > MAX_TASKS {
        return Err(format!("Claude task directory exceeds {MAX_TASKS} tasks"));
    }
    tasks.sort_by(|left, right| task_id_order(&left.task.id, &right.task.id));
    Ok(tasks)
}

fn read_task_sync(path: &Path) -> Result<VersionedTask, String> {
    let metadata = std::fs::metadata(path)
        .map_err(|error| format!("failed to stat {}: {error}", path.display()))?;
    if metadata.len() > MAX_TASK_BYTES {
        return Err(format!("{} exceeds {MAX_TASK_BYTES} bytes", path.display()));
    }
    let bytes = std::fs::read(path)
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    let task = serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))?;
    Ok(VersionedTask {
        task,
        revision: digest(&bytes),
    })
}

fn write_task_sync(path: &Path, task: &ClaudeTask) -> Result<VersionedTask, String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(task).map_err(|error| error.to_string())?;
    let temp_path = parent.join(format!(".codex-task-{}.tmp", Uuid::now_v7()));
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
    result?;
    Ok(VersionedTask {
        task: task.clone(),
        revision: digest(&bytes),
    })
}

fn with_lock<T>(root: &Path, operation: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    std::fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(root.join(".codex-task-write.lock"))
        .map_err(|error| error.to_string())?;
    lock.lock().map_err(|error| error.to_string())?;
    let result = operation();
    let _ = lock.unlock();
    result
}

fn next_task_id(root: &Path) -> Result<String, String> {
    let highest = list_sync(root)?
        .into_iter()
        .filter_map(|versioned| versioned.task.id.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    Ok((highest + 1).to_string())
}

fn task_id_order(left: &str, right: &str) -> std::cmp::Ordering {
    match (left.parse::<u64>(), right.parse::<u64>()) {
        (Ok(left), Ok(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_task_id(id: &str) -> Result<(), String> {
    if id.is_empty() || !id.chars().all(|character| character.is_ascii_digit()) {
        return Err(format!("invalid Claude task id `{id}`"));
    }
    Ok(())
}

fn validate_nonempty(label: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{label} must not be empty"));
    }
    Ok(())
}

fn validate_status(status: &str) -> Result<(), String> {
    match status {
        "pending" | "in_progress" | "completed" => Ok(()),
        _ => Err(format!(
            "invalid Claude task status `{status}`; expected pending, in_progress, or completed"
        )),
    }
}

#[cfg(test)]
#[path = "task_store_tests.rs"]
mod tests;
