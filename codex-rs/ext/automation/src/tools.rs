use std::sync::Arc;

use codex_extension_api::FunctionCallError;
use codex_extension_api::JsonToolOutput;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema;
use codex_utils_string::approx_token_count;
use serde::Deserialize;
use serde_json::Value;
use serde_json::json;

use crate::runtime::AutomationRuntime;
use crate::state_store::AutomationKind;
use crate::state_store::JobPatch;
use crate::state_store::MAX_MONITOR_POLL_SECONDS;
use crate::state_store::ScheduleSpec;

const MAX_AUTOMATION_TOOL_OUTPUT_TOKENS: usize = 8_000;

#[derive(Clone, Copy, Debug)]
enum AutomationToolKind {
    LoopCreate,
    LoopList,
    LoopStop,
    CronCreate,
    CronList,
    CronUpdate,
    CronDelete,
    CronRun,
    MonitorStart,
    MonitorList,
    MonitorStop,
    // Claude-compatible aliases so role instructions written for Claude Code
    // (CronCreate/CronList/CronDelete) resolve verbatim on a native Codex
    // teammate — same runtime, Claude's field names/schema.
    CronCreateClaude,
    CronListClaude,
    CronDeleteClaude,
    // Claude-compatible Monitor: LAUNCHES a command and streams its output as
    // wake events (Claude semantics), unlike monitor_start which attaches to an
    // existing process.
    MonitorClaude,
}

#[derive(Clone, Debug)]
pub(crate) struct AutomationTool {
    runtime: AutomationRuntime,
    kind: AutomationToolKind,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoopCreateArgs {
    prompt: String,
    name: Option<String>,
    every_seconds: Option<u64>,
    every_minutes: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CronCreateArgs {
    expression: String,
    prompt: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CronUpdateArgs {
    id: String,
    prompt: Option<String>,
    expression: Option<String>,
    enabled: Option<bool>,
}

/// Claude Code's `CronCreate` shape: field `cron` (not `expression`), plus
/// `recurring`/`durable`. Codex crons are durable + recurring, so `durable` is
/// accepted-and-ignored and `recurring: false` is treated as recurring with a
/// note (Codex has no one-shot cron primitive).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeCronCreateArgs {
    cron: String,
    prompt: String,
    #[serde(default)]
    recurring: Option<bool>,
    #[serde(default)]
    durable: Option<bool>,
}

/// Claude Code's `Monitor` shape: `command` is launched and its output streamed.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaudeMonitorArgs {
    command: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    contains: Option<String>,
    #[serde(default)]
    persistent: Option<bool>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MonitorStartArgs {
    process_id: i32,
    prompt: String,
    name: Option<String>,
    contains: Option<String>,
    poll_seconds: Option<u64>,
    once: Option<bool>,
}

impl AutomationTool {
    pub(crate) fn all(runtime: AutomationRuntime) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        [
            AutomationToolKind::LoopCreate,
            AutomationToolKind::LoopList,
            AutomationToolKind::LoopStop,
            AutomationToolKind::CronCreate,
            AutomationToolKind::CronList,
            AutomationToolKind::CronUpdate,
            AutomationToolKind::CronDelete,
            AutomationToolKind::CronRun,
            AutomationToolKind::MonitorStart,
            AutomationToolKind::MonitorList,
            AutomationToolKind::MonitorStop,
            AutomationToolKind::CronCreateClaude,
            AutomationToolKind::CronListClaude,
            AutomationToolKind::CronDeleteClaude,
            AutomationToolKind::MonitorClaude,
        ]
        .into_iter()
        .map(|kind| {
            Arc::new(Self {
                runtime: runtime.clone(),
                kind,
            }) as Arc<dyn ToolExecutor<ToolCall>>
        })
        .collect()
    }

    async fn handle_call(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let value = match self.kind {
            AutomationToolKind::LoopCreate => {
                let args = parse_args::<LoopCreateArgs>(&call)?;
                let every_seconds = interval_seconds(args.every_seconds, args.every_minutes)?;
                serde_json::to_value(
                    self.runtime
                        .create_loop(args.name, args.prompt, every_seconds)
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::LoopList => {
                parse_args::<EmptyArgs>(&call)?;
                let jobs = self
                    .runtime
                    .state()
                    .await
                    .map_err(tool_error)?
                    .jobs
                    .into_iter()
                    .filter(|job| job.kind == AutomationKind::Loop)
                    .collect::<Vec<_>>();
                serde_json::to_value(jobs)
            }
            AutomationToolKind::LoopStop => {
                let args = parse_args::<IdArgs>(&call)?;
                serde_json::to_value(
                    self.runtime
                        .delete_job_kind(args.id, AutomationKind::Loop)
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::CronCreate => {
                let args = parse_args::<CronCreateArgs>(&call)?;
                serde_json::to_value(
                    self.runtime
                        .create_cron(args.name, args.prompt, args.expression)
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::CronList => {
                parse_args::<EmptyArgs>(&call)?;
                let jobs = self
                    .runtime
                    .state()
                    .await
                    .map_err(tool_error)?
                    .jobs
                    .into_iter()
                    .filter(|job| job.kind == AutomationKind::Cron)
                    .collect::<Vec<_>>();
                serde_json::to_value(jobs)
            }
            AutomationToolKind::CronUpdate => {
                let args = parse_args::<CronUpdateArgs>(&call)?;
                serde_json::to_value(
                    self.runtime
                        .update_job(
                            args.id,
                            JobPatch {
                                prompt: args.prompt,
                                enabled: args.enabled,
                                schedule: args
                                    .expression
                                    .map(|expression| ScheduleSpec::Cron { expression }),
                            },
                        )
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::CronDelete => {
                let args = parse_args::<IdArgs>(&call)?;
                serde_json::to_value(
                    self.runtime
                        .delete_job_kind(args.id, AutomationKind::Cron)
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::CronRun => {
                let args = parse_args::<IdArgs>(&call)?;
                serde_json::to_value(self.runtime.run_job(&args.id).await.map_err(tool_error)?)
            }
            AutomationToolKind::MonitorStart => {
                let args = parse_args::<MonitorStartArgs>(&call)?;
                serde_json::to_value(
                    self.runtime
                        .create_monitor(
                            args.name,
                            args.process_id,
                            args.prompt,
                            args.contains,
                            args.poll_seconds.unwrap_or(5),
                            args.once.unwrap_or(true),
                        )
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::MonitorList => {
                parse_args::<EmptyArgs>(&call)?;
                serde_json::to_value(self.runtime.state().await.map_err(tool_error)?.monitors)
            }
            AutomationToolKind::MonitorStop => {
                let args = parse_args::<IdArgs>(&call)?;
                serde_json::to_value(
                    self.runtime
                        .stop_monitor(args.id)
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::CronCreateClaude => {
                let args = parse_args::<ClaudeCronCreateArgs>(&call)?;
                // `durable` is always true here; `recurring: false` has no Codex
                // one-shot primitive, so it is created recurring (caller can
                // cron_delete after the first fire).
                let _ = args.durable;
                let _ = args.recurring;
                serde_json::to_value(
                    self.runtime
                        .create_cron(None, args.prompt, args.cron)
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::CronListClaude => {
                parse_args::<EmptyArgs>(&call)?;
                let jobs = self
                    .runtime
                    .state()
                    .await
                    .map_err(tool_error)?
                    .jobs
                    .into_iter()
                    .filter(|job| job.kind == AutomationKind::Cron)
                    .collect::<Vec<_>>();
                serde_json::to_value(jobs)
            }
            AutomationToolKind::CronDeleteClaude => {
                let args = parse_args::<IdArgs>(&call)?;
                serde_json::to_value(
                    self.runtime
                        .delete_job_kind(args.id, AutomationKind::Cron)
                        .await
                        .map_err(tool_error)?,
                )
            }
            AutomationToolKind::MonitorClaude => {
                let args = parse_args::<ClaudeMonitorArgs>(&call)?;
                // persistent/timeout are honored best-effort: the launched
                // process is watched until it exits, monitor_stop, or shutdown.
                let _ = args.persistent;
                let _ = args.timeout_ms;
                serde_json::to_value(
                    self.runtime
                        .launch_monitor(args.description, args.command, args.contains, false)
                        .await
                        .map_err(tool_error)?,
                )
            }
        }
        .map_err(|error| FunctionCallError::Fatal(error.to_string()))?;
        let output_tokens = approx_token_count(&value.to_string());
        let token_limit = call
            .truncation_policy
            .token_budget()
            .min(MAX_AUTOMATION_TOOL_OUTPUT_TOKENS);
        if output_tokens > token_limit {
            return Err(FunctionCallError::RespondToModel(format!(
                "automation result is approximately {output_tokens} tokens, exceeding this call's {token_limit}-token limit"
            )));
        }
        Ok(Box::new(JsonToolOutput::new(value)))
    }
}

impl ToolExecutor<ToolCall> for AutomationTool {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(self.kind.name())
    }

    fn spec(&self) -> ToolSpec {
        self.kind.spec()
    }

    fn handle(&self, call: ToolCall) -> codex_extension_api::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

impl AutomationToolKind {
    fn name(self) -> &'static str {
        match self {
            Self::LoopCreate => "loop_create",
            Self::LoopList => "loop_list",
            Self::LoopStop => "loop_stop",
            Self::CronCreate => "cron_create",
            Self::CronList => "cron_list",
            Self::CronUpdate => "cron_update",
            Self::CronDelete => "cron_delete",
            Self::CronRun => "cron_run",
            Self::MonitorStart => "monitor_start",
            Self::MonitorList => "monitor_list",
            Self::MonitorStop => "monitor_stop",
            Self::CronCreateClaude => "CronCreate",
            Self::CronListClaude => "CronList",
            Self::CronDeleteClaude => "CronDelete",
            Self::MonitorClaude => "Monitor",
        }
    }

    fn spec(self) -> ToolSpec {
        let (description, schema) = match self {
            Self::LoopCreate => (
                "Create a durable recurring self-loop. Exactly one of every_seconds or every_minutes is required. Each fire enters Codex through native inter-agent delivery and wakes an idle thread.",
                json!({
                    "type": "object",
                    "properties": {
                        "prompt": {"type": "string"},
                        "name": {"type": "string"},
                        "every_seconds": {"type": "integer", "minimum": 1},
                        "every_minutes": {"type": "integer", "minimum": 1}
                    },
                    "required": ["prompt"],
                    "additionalProperties": false
                }),
            ),
            Self::LoopList => (
                "List recurring self-loops for this thread identity.",
                empty_schema(),
            ),
            Self::LoopStop => ("Stop and delete a recurring self-loop.", id_schema()),
            Self::CronCreate => (
                "Create a durable UTC cron. Five-field expressions are accepted and run at second zero; six- and seven-field expressions are also accepted.",
                json!({
                    "type": "object",
                    "properties": {
                        "expression": {"type": "string"},
                        "prompt": {"type": "string"},
                        "name": {"type": "string"}
                    },
                    "required": ["expression", "prompt"],
                    "additionalProperties": false
                }),
            ),
            Self::CronList => (
                "List durable UTC cron jobs for this thread identity.",
                empty_schema(),
            ),
            Self::CronUpdate => (
                "Update a cron prompt, expression, or enabled state.",
                json!({
                    "type": "object",
                    "properties": {
                        "id": {"type": "string"},
                        "prompt": {"type": "string"},
                        "expression": {"type": "string"},
                        "enabled": {"type": "boolean"}
                    },
                    "required": ["id"],
                    "additionalProperties": false
                }),
            ),
            Self::CronDelete => ("Delete a durable cron job.", id_schema()),
            Self::CronRun => (
                "Run a cron or loop immediately through native inter-agent delivery.",
                id_schema(),
            ),
            Self::MonitorStart => (
                "Attach a native monitor to an existing unified-exec process ID. This never launches a command. It wakes Codex when output matches, the process exits, or attachment fails.",
                json!({
                    "type": "object",
                    "properties": {
                        "process_id": {"type": "integer"},
                        "prompt": {"type": "string"},
                        "name": {"type": "string"},
                        "contains": {"type": "string"},
                        "poll_seconds": {
                            "type": "integer",
                            "minimum": 1,
                            "maximum": MAX_MONITOR_POLL_SECONDS
                        },
                        "once": {"type": "boolean"}
                    },
                    "required": ["process_id", "prompt"],
                    "additionalProperties": false
                }),
            ),
            Self::MonitorList => ("List native background-terminal monitors.", empty_schema()),
            Self::MonitorStop => ("Stop a native background-terminal monitor.", id_schema()),
            Self::CronCreateClaude => (
                "Schedule a prompt on a recurring UTC cron. Standard 5-field cron expression. Each fire enters through native inter-agent delivery and wakes an idle thread. (Claude-compatible alias of cron_create.)",
                json!({
                    "type": "object",
                    "properties": {
                        "cron": {"type": "string"},
                        "prompt": {"type": "string"},
                        "recurring": {"type": "boolean"},
                        "durable": {"type": "boolean"}
                    },
                    "required": ["cron", "prompt"],
                    "additionalProperties": false
                }),
            ),
            Self::CronListClaude => (
                "List scheduled cron jobs for this thread identity. (Claude-compatible alias of cron_list.)",
                empty_schema(),
            ),
            Self::CronDeleteClaude => (
                "Cancel a cron job by id. (Claude-compatible alias of cron_delete.)",
                id_schema(),
            ),
            Self::MonitorClaude => (
                "Launch a command and stream its output as wake events: each stdout line (optionally filtered by `contains`) wakes this thread. Use for standing listeners (e.g. `campaign.py listen`, `peer.py listen`). Unlike monitor_start, this LAUNCHES the command. Set persistent=true for session-length watches. Stop via monitor_stop with the returned id.",
                json!({
                    "type": "object",
                    "properties": {
                        "command": {"type": "string"},
                        "description": {"type": "string"},
                        "contains": {"type": "string"},
                        "persistent": {"type": "boolean"},
                        "timeout_ms": {"type": "integer"}
                    },
                    "required": ["command"],
                    "additionalProperties": false
                }),
            ),
        };
        function_tool(self.name(), description, schema)
    }
}

fn function_tool(name: &str, description: &str, schema: Value) -> ToolSpec {
    ToolSpec::Function(ResponsesApiTool {
        name: name.to_string(),
        description: description.to_string(),
        strict: false,
        defer_loading: None,
        parameters: parse_tool_input_schema(&schema)
            .unwrap_or_else(|error| panic!("static {name} schema must parse: {error}")),
        output_schema: None,
    })
}

fn empty_schema() -> Value {
    json!({"type": "object", "properties": {}, "additionalProperties": false})
}

fn id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"id": {"type": "string"}},
        "required": ["id"],
        "additionalProperties": false
    })
}

fn interval_seconds(
    every_seconds: Option<u64>,
    every_minutes: Option<u64>,
) -> Result<u64, FunctionCallError> {
    match (every_seconds, every_minutes) {
        (Some(seconds), None) if seconds > 0 => Ok(seconds),
        (None, Some(minutes)) if minutes > 0 => minutes.checked_mul(60).ok_or_else(|| {
            FunctionCallError::RespondToModel("loop interval is too large".to_string())
        }),
        (Some(_), Some(_)) => Err(FunctionCallError::RespondToModel(
            "pass exactly one of every_seconds or every_minutes".to_string(),
        )),
        _ => Err(FunctionCallError::RespondToModel(
            "a positive every_seconds or every_minutes value is required".to_string(),
        )),
    }
}

fn parse_args<T>(call: &ToolCall) -> Result<T, FunctionCallError>
where
    T: for<'de> Deserialize<'de>,
{
    serde_json::from_str(call.function_arguments()?).map_err(|error| {
        FunctionCallError::RespondToModel(format!("invalid {} arguments: {error}", call.tool_name))
    })
}

fn tool_error(error: String) -> FunctionCallError {
    FunctionCallError::RespondToModel(error)
}

#[cfg(test)]
mod claude_alias_tests {
    use super::AutomationToolKind;
    use super::ClaudeCronCreateArgs;
    use super::ClaudeMonitorArgs;

    #[test]
    fn claude_named_tools_match_claude_code_names() {
        assert_eq!(AutomationToolKind::CronCreateClaude.name(), "CronCreate");
        assert_eq!(AutomationToolKind::CronListClaude.name(), "CronList");
        assert_eq!(AutomationToolKind::CronDeleteClaude.name(), "CronDelete");
        assert_eq!(AutomationToolKind::MonitorClaude.name(), "Monitor");
    }

    #[test]
    fn claude_named_tool_specs_carry_the_claude_name() {
        for (kind, expected) in [
            (AutomationToolKind::CronCreateClaude, "CronCreate"),
            (AutomationToolKind::CronListClaude, "CronList"),
            (AutomationToolKind::CronDeleteClaude, "CronDelete"),
            (AutomationToolKind::MonitorClaude, "Monitor"),
        ] {
            match kind.spec() {
                codex_extension_api::ToolSpec::Function(tool) => {
                    assert_eq!(tool.name, expected);
                }
                other => panic!("expected a function tool for {expected}, got {other:?}"),
            }
        }
    }

    #[test]
    fn cron_create_accepts_claude_field_names() {
        // Claude uses `cron` (not Codex's `expression`), plus recurring/durable.
        let args: ClaudeCronCreateArgs = serde_json::from_value(serde_json::json!({
            "cron": "23 * * * *",
            "prompt": "re-mine intel",
            "recurring": true,
            "durable": true
        }))
        .expect("Claude CronCreate payload should deserialize");
        assert_eq!(args.cron, "23 * * * *");
        assert_eq!(args.prompt, "re-mine intel");
        assert_eq!(args.recurring, Some(true));
    }

    #[test]
    fn monitor_accepts_claude_launch_payload() {
        // Claude's Monitor LAUNCHES a command (campaign/peer listeners).
        let args: ClaudeMonitorArgs = serde_json::from_value(serde_json::json!({
            "command": "cd $BBTEAM_PROGRAM_DIR && python3 .claude/lib/campaign.py listen 2>&1",
            "description": "campaign changes",
            "persistent": true
        }))
        .expect("Claude Monitor payload should deserialize");
        assert!(args.command.contains("campaign.py listen"));
        assert_eq!(args.persistent, Some(true));
    }
}
