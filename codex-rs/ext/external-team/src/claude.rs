use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use codex_extension_api::ExternalAgent;
use codex_extension_api::ExternalAgentStatus;
use codex_extension_api::ExternalMessageDelivery;
use codex_extension_api::ExternalTeamFuture;
use codex_extension_api::ExternalTeamProvider;
use codex_utils_string::approx_token_count;
use serde::Deserialize;
use serde::Serialize;
use serde_json::Value;
use sha2::Digest;
use sha2::Sha256;
use tokio::sync::Mutex;
use uuid::Uuid;

const MAX_TEAM_FILE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_INBOX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MESSAGE_BYTES: usize = 128 * 1024;
const MAX_MODEL_MESSAGE_TOKENS: usize = 8_000;
const OUTBOUND_WRITE_ATTEMPTS: usize = 4;

#[derive(Clone, Debug)]
pub(crate) struct ClaudeProviderConfig {
    pub(crate) claude_home: PathBuf,
    pub(crate) team_name: String,
    pub(crate) agent_name: String,
    pub(crate) agent_id: String,
    pub(crate) agent_role: Option<String>,
    pub(crate) parent_name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ClaudeInboundMessage {
    pub(crate) id: String,
    pub(crate) author: String,
    pub(crate) content: String,
    pub(crate) kind: String,
}

#[derive(Clone)]
pub(crate) struct ClaudeCodeProvider {
    config: ClaudeProviderConfig,
    writer_lock: Arc<Mutex<()>>,
}

impl std::fmt::Debug for ClaudeCodeProvider {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ClaudeCodeProvider")
            .field("team_name", &self.config.team_name)
            .field("agent_name", &self.config.agent_name)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeTeamConfig {
    #[serde(default)]
    lead_agent_id: String,
    #[serde(default)]
    members: Vec<ClaudeMember>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeMember {
    #[serde(default)]
    agent_id: Option<String>,
    name: String,
    #[serde(default)]
    agent_type: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    prompt: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ClaudeMessage {
    from: String,
    #[serde(alias = "content")]
    text: String,
    #[serde(default)]
    timestamp: Value,
    #[serde(rename = "msgV", default = "message_version")]
    message_version: u8,
    #[serde(alias = "id", default)]
    msg_id: String,
    #[serde(rename = "type", default = "message_kind")]
    kind: String,
    #[serde(default)]
    read: bool,
}

fn message_version() -> u8 {
    1
}

fn message_kind() -> String {
    "message".to_string()
}

impl ClaudeCodeProvider {
    pub(crate) fn new(mut config: ClaudeProviderConfig) -> Result<Self, String> {
        validate_component("team name", &config.team_name)?;
        validate_component("agent name", &config.agent_name)?;
        if let Some(parent_name) = config.parent_name.as_deref() {
            validate_component("parent name", parent_name)?;
        }
        if config.parent_name.is_none()
            && let Some(parent_name) = configured_lead_name(&config)
        {
            config.parent_name = Some(parent_name);
        }
        Ok(Self {
            config,
            writer_lock: Arc::new(Mutex::new(())),
        })
    }

    pub(crate) fn team_name(&self) -> &str {
        &self.config.team_name
    }

    pub(crate) fn agent_name(&self) -> &str {
        &self.config.agent_name
    }

    fn team_dir(&self) -> PathBuf {
        self.config
            .claude_home
            .join("teams")
            .join(&self.config.team_name)
    }

    fn team_config_path(&self) -> PathBuf {
        self.team_dir().join("config.json")
    }

    fn inbox_path(&self, name: &str) -> PathBuf {
        self.team_dir().join("inboxes").join(format!("{name}.json"))
    }

    async fn team_config(&self) -> Result<ClaudeTeamConfig, String> {
        read_json_limited(self.team_config_path(), MAX_TEAM_FILE_BYTES).await
    }

    pub(crate) async fn is_on_roster(&self) -> Result<bool, String> {
        match self.team_config().await {
            Ok(team) => Ok(team
                .members
                .iter()
                .any(|member| member.name == self.config.agent_name)),
            Err(error) if error.starts_with("missing file:") => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub(crate) async fn pending_messages(&self) -> Result<Vec<ClaudeInboundMessage>, String> {
        let team = self.team_config().await?;
        let mut messages = Vec::new();
        let initial_assignment = team
            .members
            .iter()
            .find(|member| member.name == self.config.agent_name)
            .filter(|member| !member.prompt.trim().is_empty())
            .map(|member| (self.parent_name_from_team(&team), member.prompt.clone()));
        if let Some((author, content)) = &initial_assignment {
            validate_model_message(content)?;
            messages.push(ClaudeInboundMessage {
                id: format!("initial:{}", stable_digest(content)),
                author: author.clone(),
                content: content.clone(),
                kind: "new_task".to_string(),
            });
        }

        let inbox: Vec<Value> = match read_json_limited(
            self.inbox_path(&self.config.agent_name),
            MAX_INBOX_FILE_BYTES,
        )
        .await
        {
            Ok(inbox) => inbox,
            Err(error) if error.starts_with("missing file:") => Vec::new(),
            Err(error) => return Err(error),
        };
        messages.extend(inbox.into_iter().enumerate().filter_map(|(index, value)| {
            let mut message: ClaudeMessage = match serde_json::from_value(value.clone()) {
                Ok(message) => message,
                Err(error) => {
                    tracing::warn!(%error, "skipping malformed Claude teammate message");
                    return None;
                }
            };
            if message.msg_id.is_empty() {
                message.msg_id = format!("legacy:{}", stable_value_digest(&value));
            }
            if message.text.len() > MAX_MESSAGE_BYTES {
                tracing::warn!(
                    message_id = %message.msg_id,
                    bytes = message.text.len(),
                    "dropping oversized Claude teammate message"
                );
                return None;
            }
            if let Err(error) = validate_model_message(&message.text) {
                tracing::warn!(message_id = %message.msg_id, %error, "dropping oversized Claude teammate message");
                return None;
            }
            if index == 0
                && initial_assignment.as_ref().is_some_and(|(author, content)| {
                    message.from == *author && message.text == *content
                })
            {
                return None;
            }
            Some(ClaudeInboundMessage {
                id: message.msg_id,
                author: message.from,
                content: message.text,
                kind: message.kind,
            })
        }));
        Ok(messages)
    }

    fn parent_name_from_team(&self, team: &ClaudeTeamConfig) -> String {
        self.config.parent_name.clone().unwrap_or_else(|| {
            team.lead_agent_id
                .split('@')
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or("team-lead")
                .to_string()
        })
    }

    // The in-process writer lock is intentionally held across the blocking append so
    // this process's inbox writes are ordered; tokio's Mutex is async-aware.
    #[allow(clippy::await_holding_invalid_type)]
    async fn append_message(&self, target: &str, content: &str) -> Result<(), String> {
        if content.trim().is_empty() {
            return Err("external teammate messages must not be empty".to_string());
        }
        if content.len() > MAX_MESSAGE_BYTES {
            return Err(format!(
                "external teammate message exceeds {MAX_MESSAGE_BYTES} bytes"
            ));
        }
        validate_component("target name", target)?;
        let _writer_guard = self.writer_lock.lock().await;
        let inbox_path = self.inbox_path(target);
        let lock_path = self.team_dir().join(".codex-inbox-write.lock");
        let message = ClaudeMessage {
            from: self.config.agent_name.clone(),
            text: content.to_string(),
            timestamp: Value::String(chrono_timestamp()),
            message_version: 1,
            msg_id: Uuid::now_v7().to_string(),
            kind: "message".to_string(),
            read: false,
        };
        tokio::task::spawn_blocking(move || append_message_sync(&inbox_path, &lock_path, message))
            .await
            .map_err(|error| format!("Claude inbox writer task failed: {error}"))?
    }

    pub(crate) async fn send_parent_message(&self, content: &str) -> Result<(), String> {
        let parent = self.parent();
        let parent = self.resolve_agent(&parent.name).await?.ok_or_else(|| {
            format!(
                "external parent `{}` is not on team `{}`",
                parent.name, self.config.team_name
            )
        })?;
        self.send_message(&parent, content, ExternalMessageDelivery::Queue)
            .await
    }

    pub(crate) async fn acknowledge_shutdown(
        &self,
        author: &str,
        request_id: &str,
    ) -> Result<(), String> {
        let author = self
            .resolve_agent(author)
            .await?
            .ok_or_else(|| format!("shutdown requester `{author}` is not on the external team"))?;
        if author.name != self.parent().name {
            return Err(format!(
                "external agent `{}` is not authorized to shut down `{}`",
                author.name, self.config.agent_name
            ));
        }
        let response = serde_json::json!({
            "type": "shutdown_response",
            "request_id": request_id,
            "approve": true,
            "reason": "Codex teammate acknowledged the native shutdown request."
        });
        self.send_message(
            &author,
            &response.to_string(),
            ExternalMessageDelivery::Queue,
        )
        .await
    }

    pub(crate) async fn is_parent(&self, author: &str) -> Result<bool, String> {
        Ok(self
            .resolve_agent(author)
            .await?
            .is_some_and(|agent| agent.name == self.parent().name))
    }
}

fn validate_model_message(content: &str) -> Result<(), String> {
    let tokens = approx_token_count(content);
    if tokens > MAX_MODEL_MESSAGE_TOKENS {
        Err(format!(
            "Claude teammate message is approximately {tokens} tokens; the model-visible limit is {MAX_MODEL_MESSAGE_TOKENS}"
        ))
    } else {
        Ok(())
    }
}

fn configured_lead_name(config: &ClaudeProviderConfig) -> Option<String> {
    let path = config
        .claude_home
        .join("teams")
        .join(&config.team_name)
        .join("config.json");
    let metadata = std::fs::metadata(&path).ok()?;
    if metadata.len() > MAX_TEAM_FILE_BYTES {
        return None;
    }
    let team: ClaudeTeamConfig = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    team.lead_agent_id
        .split('@')
        .next()
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

impl ExternalTeamProvider for ClaudeCodeProvider {
    fn identity(&self) -> ExternalAgent {
        ExternalAgent {
            id: self.config.agent_id.clone(),
            name: self.config.agent_name.clone(),
            role: self.config.agent_role.clone(),
            status: ExternalAgentStatus::Active,
        }
    }

    fn parent(&self) -> ExternalAgent {
        let name = self
            .config
            .parent_name
            .clone()
            .unwrap_or_else(|| "team-lead".to_string());
        ExternalAgent {
            id: format!("{name}@{}", self.config.team_name),
            name,
            role: Some("lead".to_string()),
            status: ExternalAgentStatus::Unknown,
        }
    }

    fn resolve_agent<'a>(
        &'a self,
        target: &'a str,
    ) -> ExternalTeamFuture<'a, Result<Option<ExternalAgent>, String>> {
        Box::pin(async move {
            let target = target.strip_prefix("external:").unwrap_or(target);
            Ok(self
                .list_agents()
                .await?
                .into_iter()
                .find(|agent| agent.name == target || agent.id == target))
        })
    }

    fn list_agents(&self) -> ExternalTeamFuture<'_, Result<Vec<ExternalAgent>, String>> {
        Box::pin(async move {
            Ok(self
                .team_config()
                .await?
                .members
                .into_iter()
                .map(|member| ExternalAgent {
                    id: member
                        .agent_id
                        .unwrap_or_else(|| format!("{}@{}", member.name, self.config.team_name)),
                    name: member.name,
                    role: member.agent_type,
                    status: member
                        .status
                        .as_deref()
                        .map(external_status)
                        .unwrap_or(ExternalAgentStatus::Unknown),
                })
                .collect())
        })
    }

    fn send_message<'a>(
        &'a self,
        target: &'a ExternalAgent,
        content: &'a str,
        _delivery: ExternalMessageDelivery,
    ) -> ExternalTeamFuture<'a, Result<(), String>> {
        Box::pin(async move {
            let resolved = self.resolve_agent(&target.id).await?;
            if resolved
                .as_ref()
                .is_none_or(|agent| agent.name != target.name)
            {
                return Err(format!(
                    "external agent `{}` is no longer on team `{}`",
                    target.name, self.config.team_name
                ));
            }
            self.append_message(&target.name, content).await
        })
    }

    fn interrupt<'a>(
        &'a self,
        target: &'a ExternalAgent,
        _reason: &'a str,
    ) -> ExternalTeamFuture<'a, Result<(), String>> {
        Box::pin(async move {
            Err(format!(
                "Claude Code does not expose a stable external interrupt contract for `{}`",
                target.name
            ))
        })
    }
}

fn append_message_sync(
    inbox_path: &Path,
    lock_path: &Path,
    message: ClaudeMessage,
) -> Result<(), String> {
    let Some(parent) = inbox_path.parent() else {
        return Err("Claude inbox has no parent directory".to_string());
    };
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let lock_file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .map_err(|error| format!("failed to open Claude inbox lock: {error}"))?;
    lock_file
        .lock()
        .map_err(|error| format!("failed to lock Claude inbox: {error}"))?;
    let result = (|| {
        let encoded_message = serde_json::to_value(&message).map_err(|error| error.to_string())?;
        for attempt in 0..OUTBOUND_WRITE_ATTEMPTS {
            let mut messages = read_messages_sync(inbox_path)?;
            if !messages
                .iter()
                .any(|item| message_id(item) == Some(message.msg_id.as_str()))
            {
                messages.push(encoded_message.clone());
            }
            atomic_json_write(inbox_path, &messages)?;
            if read_messages_sync(inbox_path)?
                .iter()
                .any(|item| message_id(item) == Some(message.msg_id.as_str()))
            {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(10 * (attempt as u64 + 1)));
        }
        Err(format!(
            "failed to verify message {} in Claude inbox {}",
            message.msg_id,
            inbox_path.display()
        ))
    })();
    let _ = lock_file.unlock();
    result
}

fn read_messages_sync(path: &Path) -> Result<Vec<Value>, String> {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() as u64 <= MAX_INBOX_FILE_BYTES => {
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())
        }
        Ok(bytes) => Err(format!(
            "Claude inbox {} exceeds {MAX_INBOX_FILE_BYTES} bytes ({} bytes)",
            path.display(),
            bytes.len()
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.to_string()),
    }
}

fn message_id(value: &Value) -> Option<&str> {
    value
        .get("msg_id")
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
}

fn atomic_json_write<T>(path: &Path, value: &T) -> Result<(), String>
where
    T: Serialize,
{
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", path.display()))?;
    let temp_path = parent.join(format!(".codex-{}.tmp", Uuid::now_v7()));
    let result = (|| {
        let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)
            .map_err(|error| error.to_string())?;
        file.write_all(&bytes).map_err(|error| error.to_string())?;
        file.sync_all().map_err(|error| error.to_string())?;
        std::fs::rename(&temp_path, path).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temp_path);
    }
    result
}

async fn read_json_limited<T>(path: PathBuf, max_bytes: u64) -> Result<T, String>
where
    T: for<'de> Deserialize<'de>,
{
    let metadata = tokio::fs::metadata(&path)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => format!("missing file: {}", path.display()),
            _ => format!("failed to stat {}: {error}", path.display()),
        })?;
    if metadata.len() > max_bytes {
        return Err(format!(
            "{} exceeds the {max_bytes}-byte provider limit",
            path.display()
        ));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
    serde_json::from_slice(&bytes)
        .map_err(|error| format!("failed to parse {}: {error}", path.display()))
}

fn stable_digest(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn stable_value_digest(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).unwrap_or_default();
    format!("{:x}", Sha256::digest(bytes))
}

fn chrono_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn external_status(status: &str) -> ExternalAgentStatus {
    match status.to_ascii_lowercase().as_str() {
        "active" | "working" => ExternalAgentStatus::Active,
        "idle" => ExternalAgentStatus::Idle,
        "stopped" | "shutdown" => ExternalAgentStatus::Stopped,
        _ => ExternalAgentStatus::Unknown,
    }
}

fn validate_component(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value == "." || value == ".." || value.contains(['/', '\\', '\0']) {
        return Err(format!("invalid Claude {label} `{value}`"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "claude_tests.rs"]
mod tests;
