use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Weak;
use std::time::Duration;

use codex_core::ThreadManager;
use codex_protocol::AgentPath;
use codex_protocol::ThreadId;
use codex_protocol::protocol::InterAgentCommunication;
use codex_protocol::protocol::Op;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;
use tokio::sync::watch;
use uuid::Uuid;

use crate::claude::ClaudeCodeProvider;

const JOURNAL_ID_LIMIT: usize = 10_000;

#[derive(Clone)]
pub(crate) struct ExternalTeamRuntime {
    provider: Arc<ClaudeCodeProvider>,
    journal: Arc<DeliveryJournal>,
    final_message: Arc<Mutex<Option<FinalMessage>>>,
    cancel: watch::Sender<bool>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct FinalMessage {
    id: String,
    text: String,
}

impl std::fmt::Debug for ExternalTeamRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalTeamRuntime")
            .field("team_name", &self.provider.team_name())
            .field("agent_name", &self.provider.agent_name())
            .finish_non_exhaustive()
    }
}

impl ExternalTeamRuntime {
    pub(crate) async fn start(
        provider: Arc<ClaudeCodeProvider>,
        journal_path: PathBuf,
        thread_manager: Weak<ThreadManager>,
        thread_id: ThreadId,
        poll_interval: Duration,
    ) -> Result<Self, String> {
        let journal = Arc::new(DeliveryJournal::load(journal_path).await?);
        let (cancel, cancel_rx) = watch::channel(false);
        let runtime = Self {
            provider,
            journal,
            final_message: Arc::new(Mutex::new(None)),
            cancel,
        };
        tokio::spawn(run_inbox_worker(
            runtime.clone(),
            thread_manager,
            thread_id,
            poll_interval,
            cancel_rx,
        ));
        Ok(runtime)
    }

    pub(crate) fn stop(&self) {
        let _ = self.cancel.send(true);
    }

    pub(crate) async fn capture_final(&self, id: String, text: String) {
        *self.final_message.lock().await = Some(FinalMessage { id, text });
    }

    pub(crate) async fn deliver_final_and_idle(&self) -> Result<(), String> {
        let Some(final_message) = self.final_message.lock().await.clone() else {
            return Ok(());
        };
        let journal_id = format!("final:{}", final_message.id);
        if !self.journal.contains(&journal_id).await {
            self.provider
                .send_parent_message(&final_message.text)
                .await?;
            self.journal.record(journal_id).await?;
        }
        let idle_journal_id = format!("idle:{}", final_message.id);
        if !self.journal.contains(&idle_journal_id).await {
            let summary = bounded_summary(&final_message.text);
            let idle = serde_json::json!({
                "type": "idle_notification",
                "from": self.provider.agent_name(),
                "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                "idleReason": "available",
                "summary": summary,
            });
            self.provider.send_parent_message(&idle.to_string()).await?;
            self.journal.record(idle_journal_id).await?;
        }
        *self.final_message.lock().await = None;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ShutdownRequest {
    #[serde(rename = "requestId", alias = "request_id")]
    request_id: String,
}

async fn run_inbox_worker(
    runtime: ExternalTeamRuntime,
    thread_manager: Weak<ThreadManager>,
    thread_id: ThreadId,
    poll_interval: Duration,
    mut cancel: watch::Receiver<bool>,
) {
    let mut first_roster_seen = false;
    loop {
        if *cancel.borrow() {
            return;
        }
        let Some(thread_manager) = thread_manager.upgrade() else {
            return;
        };
        let Ok(thread) = thread_manager.get_thread(thread_id).await else {
            tokio::select! {
                _ = tokio::time::sleep(poll_interval) => {}
                changed = cancel.changed() => {
                    if changed.is_err() || *cancel.borrow() {
                        return;
                    }
                }
            }
            continue;
        };

        match runtime.provider.is_on_roster().await {
            Ok(true) => first_roster_seen = true,
            Ok(false) if first_roster_seen => {
                if let Err(error) = thread.submit(Op::Shutdown).await {
                    tracing::warn!(%thread_id, %error, "failed to stop removed external teammate");
                }
                return;
            }
            Ok(false) => {}
            Err(error) => {
                tracing::debug!(%thread_id, %error, "external team roster is not ready");
            }
        }

        match runtime.provider.pending_messages().await {
            Ok(messages) => {
                for message in messages {
                    if runtime.journal.contains(&message.id).await {
                        continue;
                    }
                    if is_shutdown_request(&message.content) {
                        match runtime.provider.is_parent(&message.author).await {
                            Ok(false) => {}
                            Err(error) => {
                                tracing::warn!(%thread_id, %error, "failed to authorize external shutdown request");
                                break;
                            }
                            Ok(true) => {
                                let request = match shutdown_request(&message.content) {
                                    Ok(Some(request)) => request,
                                    Ok(None) => continue,
                                    Err(error) => {
                                        tracing::warn!(%thread_id, %error, "invalid external shutdown request");
                                        break;
                                    }
                                };
                                if let Err(error) = runtime
                                    .provider
                                    .acknowledge_shutdown(&message.author, &request.request_id)
                                    .await
                                {
                                    tracing::warn!(%thread_id, %error, "failed to acknowledge external shutdown request");
                                    break;
                                }
                                if let Err(error) = runtime.journal.record(message.id).await {
                                    tracing::warn!(%thread_id, %error, "failed to journal external shutdown request");
                                    break;
                                }
                                if let Err(error) = thread.submit(Op::Shutdown).await {
                                    tracing::warn!(%thread_id, %error, "failed to stop external teammate");
                                }
                                return;
                            }
                        }
                    }
                    let author = external_agent_path(&message.author);
                    let content = if message.kind == "new_task" {
                        format!("[NEW_TASK]\n{}", message.content)
                    } else {
                        message.content
                    };
                    let mut communication = InterAgentCommunication::new(
                        author,
                        AgentPath::root(),
                        Vec::new(),
                        content,
                        /*trigger_turn*/ true,
                    );
                    communication.internal_chat_message_metadata_passthrough = None;
                    match thread
                        .submit(Op::InterAgentCommunication { communication })
                        .await
                    {
                        Ok(_) => {
                            if let Err(error) = runtime.journal.record(message.id).await {
                                tracing::warn!(%thread_id, %error, "failed to journal external message");
                                break;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%thread_id, %error, "failed to submit external message");
                            break;
                        }
                    }
                }
            }
            Err(error) => {
                tracing::debug!(%thread_id, %error, "external inbox reconciliation failed");
            }
        }

        tokio::select! {
            _ = tokio::time::sleep(poll_interval) => {}
            changed = cancel.changed() => {
                if changed.is_err() || *cancel.borrow() {
                    return;
                }
            }
        }
    }
}

fn is_shutdown_request(content: &str) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return false;
    };
    value.get("type").and_then(Value::as_str) == Some("shutdown_request")
}

fn shutdown_request(content: &str) -> Result<Option<ShutdownRequest>, String> {
    let Ok(value) = serde_json::from_str::<Value>(content) else {
        return Ok(None);
    };
    if value.get("type").and_then(Value::as_str) != Some("shutdown_request") {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|error| format!("malformed shutdown_request payload: {error}"))
}

fn bounded_summary(text: &str) -> String {
    const SUMMARY_CHARS: usize = 500;
    let mut characters = text.chars();
    let summary = characters.by_ref().take(SUMMARY_CHARS).collect::<String>();
    if characters.next().is_some() {
        format!("{summary}…")
    } else {
        summary
    }
}

pub(crate) fn external_agent_path(name: &str) -> AgentPath {
    let mut sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect::<String>();
    while sanitized.contains("__") {
        sanitized = sanitized.replace("__", "_");
    }
    sanitized = sanitized.trim_matches('_').to_string();
    if sanitized.is_empty() || sanitized == "root" {
        sanitized = format!("agent_{}", &stable_name_digest(name)[..12]);
    }
    AgentPath::try_from(format!("/root/external/{sanitized}"))
        .expect("sanitized external agent path must be valid")
}

fn stable_name_digest(name: &str) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(name.as_bytes()))
}

#[derive(Debug)]
struct DeliveryJournal {
    path: PathBuf,
    ids: Mutex<BTreeSet<String>>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct DeliveryJournalFile {
    ids: Vec<String>,
}

impl DeliveryJournal {
    async fn load(path: PathBuf) -> Result<Self, String> {
        let ids = match tokio::fs::read(&path).await {
            Ok(bytes) => serde_json::from_slice::<DeliveryJournalFile>(&bytes)
                .map_err(|error| format!("failed to parse {}: {error}", path.display()))?
                .ids
                .into_iter()
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
        };
        Ok(Self {
            path,
            ids: Mutex::new(ids),
        })
    }

    async fn contains(&self, id: &str) -> bool {
        self.ids.lock().await.contains(id)
    }

    async fn record(&self, id: String) -> Result<(), String> {
        let mut ids = self.ids.lock().await;
        ids.insert(id);
        while ids.len() > JOURNAL_ID_LIMIT {
            ids.pop_first();
        }
        let file = DeliveryJournalFile {
            ids: ids.iter().cloned().collect(),
        };
        atomic_json_write(&self.path, &file).await
    }
}

async fn atomic_json_write<T>(path: &Path, value: &T) -> Result<(), String>
where
    T: Serialize,
{
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|error| error.to_string())?;
    let temp_path = parent.join(format!(".codex-{}.tmp", Uuid::now_v7()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    if let Err(error) = tokio::fs::write(&temp_path, bytes).await {
        return Err(error.to_string());
    }
    if let Err(error) = tokio::fs::rename(&temp_path, path).await {
        let _ = tokio::fs::remove_file(temp_path).await;
        return Err(error.to_string());
    }
    Ok(())
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
