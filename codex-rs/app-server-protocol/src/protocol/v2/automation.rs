use serde::Deserialize;
use serde::Serialize;

use crate::JsonSchema;
use crate::TS;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LoopAutomation {
    pub id: String,
    pub name: String,
    pub prompt: String,
    pub every_seconds: u64,
    pub enabled: bool,
    pub created_at: i64,
    pub next_due_at: i64,
    pub last_run_at: Option<i64>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LoopCreateParams {
    pub thread_id: String,
    #[ts(optional = nullable)]
    pub name: Option<String>,
    pub prompt: String,
    pub every_seconds: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LoopCreateResponse {
    #[serde(rename = "loop")]
    #[ts(rename = "loop")]
    pub loop_: LoopAutomation,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LoopListParams {
    pub thread_id: String,
    /// Opaque pagination cursor returned by a previous call.
    #[ts(optional = nullable)]
    pub cursor: Option<String>,
    /// Optional page size; defaults to a server-defined value.
    #[ts(optional = nullable)]
    pub limit: Option<u32>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LoopListResponse {
    pub data: Vec<LoopAutomation>,
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LoopDeleteParams {
    pub thread_id: String,
    pub id: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, JsonSchema, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export_to = "v2/")]
pub struct LoopDeleteResponse {
    #[serde(rename = "loop")]
    #[ts(rename = "loop")]
    pub loop_: LoopAutomation,
}
