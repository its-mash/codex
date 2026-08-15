//! Fork-owned: configuration for joining an externally managed agent team (e.g. a
//! Claude Code team) as a native Codex teammate. Kept out of `config_toml.rs` so
//! upstream merges only touch the single `external_team` field hook there.

use codex_utils_absolute_path::AbsolutePathBuf;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Configuration for joining a team whose control plane is owned by another agent runtime.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, JsonSchema)]
#[schemars(deny_unknown_fields)]
pub struct ExternalTeamConfigToml {
    pub provider: String,
    pub team_name: String,
    pub agent_name: String,
    pub agent_id: Option<String>,
    pub agent_role: Option<String>,
    pub parent_name: Option<String>,
    pub claude_home: AbsolutePathBuf,
    pub poll_interval_ms: Option<u64>,
}

impl ExternalTeamConfigToml {
    /// Validates the external-team block at config-load time.
    pub fn validate(&self) -> std::io::Result<()> {
        if self.provider != "claude_code" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!(
                    "unsupported external_team.provider `{}`; expected `claude_code`",
                    self.provider
                ),
            ));
        }
        if self.team_name.trim().is_empty() || self.agent_name.trim().is_empty() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "external_team.team_name and external_team.agent_name must not be empty",
            ));
        }
        Ok(())
    }
}
