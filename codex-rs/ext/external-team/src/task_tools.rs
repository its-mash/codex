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

use crate::task_store::ClaudeTaskStore;
use crate::task_store::TaskPatch;

const MAX_TASK_TOOL_OUTPUT_TOKENS: usize = 8_000;

#[derive(Clone, Copy, Debug)]
enum TaskToolKind {
    List,
    Get,
    Create,
    Claim,
    Update,
    Complete,
}

#[derive(Clone, Debug)]
pub(crate) struct ClaudeTaskTool {
    store: Arc<ClaudeTaskStore>,
    kind: TaskToolKind,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskIdArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskMutationArgs {
    id: String,
    expected_revision: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateTaskArgs {
    subject: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    active_form: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateTaskArgs {
    id: String,
    expected_revision: String,
    subject: Option<String>,
    description: Option<String>,
    active_form: Option<String>,
    status: Option<String>,
    owner: Option<String>,
    blocks: Option<Vec<String>>,
    blocked_by: Option<Vec<String>>,
}

impl ClaudeTaskTool {
    pub(crate) fn all(store: Arc<ClaudeTaskStore>) -> Vec<Arc<dyn ToolExecutor<ToolCall>>> {
        [
            TaskToolKind::List,
            TaskToolKind::Get,
            TaskToolKind::Create,
            TaskToolKind::Claim,
            TaskToolKind::Update,
            TaskToolKind::Complete,
        ]
        .into_iter()
        .map(|kind| {
            Arc::new(Self {
                store: store.clone(),
                kind,
            }) as Arc<dyn ToolExecutor<ToolCall>>
        })
        .collect()
    }

    async fn handle_call(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let value = match self.kind {
            TaskToolKind::List => {
                parse_args::<EmptyArgs>(&call)?;
                serde_json::to_value(self.store.list().await.map_err(tool_error)?)
            }
            TaskToolKind::Get => {
                let args = parse_args::<TaskIdArgs>(&call)?;
                serde_json::to_value(self.store.get(&args.id).await.map_err(tool_error)?)
            }
            TaskToolKind::Create => {
                let args = parse_args::<CreateTaskArgs>(&call)?;
                serde_json::to_value(
                    self.store
                        .create(args.subject, args.description, args.active_form)
                        .await
                        .map_err(tool_error)?,
                )
            }
            TaskToolKind::Claim => {
                let args = parse_args::<TaskMutationArgs>(&call)?;
                serde_json::to_value(
                    self.store
                        .claim(&args.id, args.expected_revision)
                        .await
                        .map_err(tool_error)?,
                )
            }
            TaskToolKind::Update => {
                let args = parse_args::<UpdateTaskArgs>(&call)?;
                serde_json::to_value(
                    self.store
                        .patch(
                            &args.id,
                            TaskPatch {
                                subject: args.subject,
                                description: args.description,
                                active_form: args.active_form,
                                status: args.status,
                                owner: args.owner,
                                blocks: args.blocks,
                                blocked_by: args.blocked_by,
                            },
                            args.expected_revision,
                        )
                        .await
                        .map_err(tool_error)?,
                )
            }
            TaskToolKind::Complete => {
                let args = parse_args::<TaskMutationArgs>(&call)?;
                serde_json::to_value(
                    self.store
                        .complete(&args.id, args.expected_revision)
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
            .min(MAX_TASK_TOOL_OUTPUT_TOKENS);
        if output_tokens > token_limit {
            return Err(FunctionCallError::RespondToModel(format!(
                "task result is approximately {output_tokens} tokens, exceeding this call's {token_limit}-token limit; use task_get for individual task IDs"
            )));
        }
        Ok(Box::new(JsonToolOutput::new(value)))
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EmptyArgs {}

impl ToolExecutor<ToolCall> for ClaudeTaskTool {
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

impl TaskToolKind {
    fn name(self) -> &'static str {
        match self {
            Self::List => "task_list",
            Self::Get => "task_get",
            Self::Create => "task_create",
            Self::Claim => "task_claim",
            Self::Update => "task_update",
            Self::Complete => "task_complete",
        }
    }

    fn spec(self) -> ToolSpec {
        match self {
            Self::List => function_tool(
                self.name(),
                "List the current Claude-owned shared task board. Each task includes a revision for conflict-safe updates.",
                json!({"type": "object", "properties": {}, "additionalProperties": false}),
            ),
            Self::Get => function_tool(
                self.name(),
                "Read one task from the Claude-owned shared task board.",
                task_id_schema(),
            ),
            Self::Create => function_tool(
                self.name(),
                "Create a pending task on the shared team task board.",
                json!({
                    "type": "object",
                    "properties": {
                        "subject": {"type": "string"},
                        "description": {"type": "string"},
                        "active_form": {"type": "string"}
                    },
                    "required": ["subject"],
                    "additionalProperties": false
                }),
            ),
            Self::Claim => function_tool(
                self.name(),
                "Atomically claim a pending shared task for this Codex teammate and mark it in_progress. expected_revision is required so simultaneous claims have one winner.",
                task_mutation_schema(),
            ),
            Self::Update => function_tool(
                self.name(),
                "Update fields on a shared task. expected_revision is required so concurrent team edits are rejected instead of overwritten.",
                update_schema(),
            ),
            Self::Complete => function_tool(
                self.name(),
                "Mark an in-progress task owned by this Codex teammate completed. expected_revision is required.",
                task_mutation_schema(),
            ),
        }
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

fn task_id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {"id": {"type": "string"}},
        "required": ["id"],
        "additionalProperties": false
    })
}

fn task_mutation_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string"},
            "expected_revision": {"type": "string"}
        },
        "required": ["id", "expected_revision"],
        "additionalProperties": false
    })
}

fn update_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": {"type": "string"},
            "expected_revision": {"type": "string"},
            "subject": {"type": "string"},
            "description": {"type": "string"},
            "active_form": {"type": "string"},
            "status": {"type": "string", "enum": ["pending", "in_progress", "completed"]},
            "owner": {"type": "string"},
            "blocks": {"type": "array", "items": {"type": "string"}},
            "blocked_by": {"type": "array", "items": {"type": "string"}}
        },
        "required": ["id", "expected_revision"],
        "additionalProperties": false
    })
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
