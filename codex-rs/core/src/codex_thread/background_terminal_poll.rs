//! Fork-owned: non-writing polling of a unified-exec background terminal, used by
//! the native automation `Monitor` tool to stream matching stdout lines.

use super::CodexThread;
use crate::unified_exec::WriteStdinRequest;
use codex_protocol::protocol::TruncationPolicy;
use std::time::Duration;

/// One non-writing poll of a unified-exec background terminal.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackgroundTerminalPoll {
    pub output: String,
    pub process_id: Option<i32>,
    pub exit_code: Option<i32>,
}

impl CodexThread {
    /// Polls an existing unified-exec process without creating a command or writing input.
    pub async fn poll_background_terminal(
        &self,
        process_id: i32,
        wait: Duration,
    ) -> Result<BackgroundTerminalPoll, String> {
        let output = self
            .session
            .services
            .unified_exec_manager
            .write_stdin(WriteStdinRequest {
                process_id,
                input: "",
                yield_time_ms: wait.as_millis().try_into().unwrap_or(u64::MAX),
                max_output_tokens: Some(4_096),
                truncation_policy: TruncationPolicy::Bytes(64 * 1024),
                interaction_event: None,
            })
            .await
            .map_err(|error| error.to_string())?;
        Ok(BackgroundTerminalPoll {
            output: String::from_utf8_lossy(&output.raw_output).into_owned(),
            process_id: output.process_id,
            exit_code: output.exit_code,
        })
    }
}
