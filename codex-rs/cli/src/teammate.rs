//! Fork-owned: `codex teammate` — join an externally managed agent team (Claude
//! Code's teammate spawn contract) as a native Codex teammate. Kept out of
//! `main.rs` so upstream merges only touch a few one-line hooks there.

use clap::Parser;
use codex_arg0::Arg0DispatchPaths;
use codex_tui::Cli as TuiCli;
use codex_utils_cli::CliConfigOverrides;
use std::path::PathBuf;

#[derive(Debug, Parser)]
pub(crate) struct TeammateCommand {
    #[arg(long)]
    team_name: String,

    #[arg(long)]
    agent_name: String,

    #[arg(long)]
    agent_id: Option<String>,

    #[arg(long)]
    agent_type: Option<String>,

    /// Compatibility flag supplied by Claude Code's teammate spawn contract.
    #[arg(long)]
    agent: Option<String>,

    /// Compatibility metadata supplied by Claude Code.
    #[arg(long)]
    agent_color: Option<String>,

    /// Parent Claude session identifier retained for spawn-contract compatibility.
    #[arg(long)]
    parent_session_id: Option<String>,

    #[arg(long)]
    model: Option<String>,

    #[arg(long)]
    effort: Option<String>,

    #[arg(long)]
    claude_home: Option<PathBuf>,

    #[arg(long, default_value_t = 200)]
    poll_interval_ms: u64,

    /// Honor Claude's unattended teammate permission mode.
    #[arg(long, default_value_t = false)]
    dangerously_skip_permissions: bool,

    /// Compatibility metadata used by Claude's optional remote-control UI.
    #[arg(long)]
    remote_control: Option<String>,
}

/// Runs `codex teammate`: configures the interactive TUI as a native teammate and
/// launches it. Mirrors the shape of the other `cli_main` subcommand arms.
pub(crate) async fn run(
    teammate: TeammateCommand,
    mut interactive: TuiCli,
    root_remote: Option<&str>,
    root_remote_auth_token_env: Option<&str>,
    root_config_overrides: CliConfigOverrides,
    arg0_paths: Arg0DispatchPaths,
) -> anyhow::Result<()> {
    crate::reject_remote_mode_for_subcommand(root_remote, root_remote_auth_token_env, "teammate")?;
    configure_external_teammate(&mut interactive, &teammate)?;
    crate::prepend_config_flags(&mut interactive.config_overrides, root_config_overrides);
    let exit_info = crate::run_interactive_tui(
        interactive,
        /*remote*/ None,
        /*remote_auth_token_env*/ None,
        arg0_paths,
    )
    .await?;
    crate::handle_app_exit(exit_info)
}

fn configure_external_teammate(
    interactive: &mut TuiCli,
    teammate: &TeammateCommand,
) -> anyhow::Result<()> {
    let claude_home = teammate.claude_home.clone().unwrap_or_else(|| {
        std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME")
                    .or_else(|| std::env::var_os("USERPROFILE"))
                    .map(PathBuf::from)
                    .map(|home| home.join(".claude"))
            })
            .unwrap_or_else(|| PathBuf::from(".claude"))
    });
    if !claude_home.is_absolute() {
        anyhow::bail!(
            "Claude home must be an absolute path; got {}",
            claude_home.display()
        );
    }

    interactive.prompt = None;
    if let Some(model) = teammate.model.as_ref() {
        interactive.shared.model = Some(model.clone());
    }
    if teammate.dangerously_skip_permissions {
        interactive.shared.dangerously_bypass_approvals_and_sandbox = true;
    }

    let agent_role = teammate.agent_type.as_ref().or(teammate.agent.as_ref());
    let overrides = &mut interactive.config_overrides.raw_overrides;
    overrides.extend([
        toml_string_override("external_team.provider", "claude_code"),
        toml_string_override("external_team.team_name", &teammate.team_name),
        toml_string_override("external_team.agent_name", &teammate.agent_name),
        toml_string_override(
            "external_team.claude_home",
            claude_home
                .to_str()
                .ok_or_else(|| anyhow::anyhow!("Claude home is not valid UTF-8"))?,
        ),
        format!(
            "external_team.poll_interval_ms={}",
            teammate.poll_interval_ms
        ),
        "features.multi_agent_v2.enabled=true".to_string(),
        toml_string_override("features.multi_agent_v2.tool_namespace", "external_team"),
    ]);
    if let Some(agent_id) = teammate.agent_id.as_ref() {
        overrides.push(toml_string_override("external_team.agent_id", agent_id));
    }
    if let Some(agent_role) = agent_role {
        overrides.push(toml_string_override("external_team.agent_role", agent_role));
    }
    if let Some(effort) = teammate.effort.as_ref() {
        overrides.push(toml_string_override("model_reasoning_effort", effort));
    }

    // These values are part of Claude Code's teammate spawn contract but do not alter Codex's
    // provider-neutral runtime. Parsing them keeps the launcher compatible without coupling the
    // native transport to Claude's terminal metadata.
    let _ = (
        &teammate.agent_color,
        &teammate.parent_session_id,
        &teammate.remote_control,
    );
    Ok(())
}

fn toml_string_override(key: &str, value: &str) -> String {
    format!("{key}={}", toml::Value::String(value.to_string()))
}

#[cfg(test)]
mod tests {
    use super::configure_external_teammate;
    use crate::MultitoolCli;
    use crate::Subcommand;
    use clap::Parser;
    use pretty_assertions::assert_eq;

    #[test]
    fn teammate_spawn_contract_configures_native_external_runtime() {
        let cli = MultitoolCli::try_parse_from([
            "codex",
            "teammate",
            "--team-name",
            "mock-team",
            "--agent-name",
            "codex-worker",
            "--agent-id",
            "codex-worker@mock-team",
            "--parent-session-id",
            "parent-session",
            "--agent-type",
            "reviewer",
            "--agent-color",
            "blue",
            "--model",
            "gpt-5.6-sol",
            "--effort",
            "high",
            "--claude-home",
            "/tmp/mock-claude",
            "--remote-control",
            "mock-team-codex-worker",
            "--dangerously-skip-permissions",
        ])
        .expect("Claude teammate argv should parse");
        let Some(Subcommand::Teammate(teammate)) = cli.subcommand else {
            panic!("expected teammate subcommand");
        };
        let mut interactive = cli.interactive;
        configure_external_teammate(&mut interactive, &teammate)
            .expect("configure native teammate");
        let overrides = interactive
            .config_overrides
            .parse_overrides()
            .expect("parse generated overrides")
            .into_iter()
            .collect::<std::collections::BTreeMap<_, _>>();

        assert_eq!(interactive.shared.model.as_deref(), Some("gpt-5.6-sol"));
        assert!(interactive.shared.dangerously_bypass_approvals_and_sandbox);
        assert_eq!(
            overrides.get("external_team.provider"),
            Some(&toml::Value::String("claude_code".to_string()))
        );
        assert_eq!(
            overrides.get("external_team.team_name"),
            Some(&toml::Value::String("mock-team".to_string()))
        );
        assert_eq!(
            overrides.get("external_team.agent_name"),
            Some(&toml::Value::String("codex-worker".to_string()))
        );
        assert_eq!(
            overrides.get("external_team.agent_role"),
            Some(&toml::Value::String("reviewer".to_string()))
        );
        assert_eq!(
            overrides.get("model_reasoning_effort"),
            Some(&toml::Value::String("high".to_string()))
        );
        assert_eq!(
            overrides.get("features.multi_agent_v2.enabled"),
            Some(&toml::Value::Boolean(true))
        );
        assert_eq!(
            overrides.get("features.multi_agent_v2.tool_namespace"),
            Some(&toml::Value::String("external_team".to_string()))
        );
    }
}
