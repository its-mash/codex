use super::App;
use crate::app_server_session::AppServerSession;
use crate::loop_command::LoopCommandAction;
use codex_app_server_protocol::LoopAutomation;
use codex_protocol::ThreadId;

impl App {
    pub(super) async fn run_loop_command(
        &mut self,
        app_server: &mut AppServerSession,
        thread_id: ThreadId,
        action: LoopCommandAction,
    ) {
        let result = match action {
            LoopCommandAction::List => app_server
                .loop_list(thread_id)
                .await
                .map(|response| format_loop_list(&response.data)),
            LoopCommandAction::Stop { id } => {
                app_server.loop_delete(thread_id, id).await.map(|response| {
                    format!(
                        "Stopped durable loop `{}` ({}).",
                        response.loop_.name, response.loop_.id
                    )
                })
            }
            LoopCommandAction::Create {
                every_seconds,
                prompt,
            } => app_server
                .loop_create(thread_id, prompt, every_seconds)
                .await
                .map(|response| {
                    format!(
                        "Created durable loop `{}` ({}) every {}. Next run: Unix {}.",
                        response.loop_.name,
                        response.loop_.id,
                        format_interval(response.loop_.every_seconds),
                        response.loop_.next_due_at
                    )
                }),
        };
        if self.current_displayed_thread_id() != Some(thread_id) {
            return;
        }
        match result {
            Ok(message) => self.chat_widget.add_info_message(message, None),
            Err(error) => self
                .chat_widget
                .add_error_message(format!("Native loop operation failed: {error}")),
        }
    }
}

fn format_loop_list(loops: &[LoopAutomation]) -> String {
    if loops.is_empty() {
        return "No durable loops are configured for this thread.".to_string();
    }
    let entries = loops
        .iter()
        .map(|loop_| {
            format!(
                "- `{}` ({}) every {}; next Unix {} — {}",
                loop_.name,
                loop_.id,
                format_interval(loop_.every_seconds),
                loop_.next_due_at,
                loop_.prompt
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("Durable loops:\n{entries}")
}

fn format_interval(every_seconds: u64) -> String {
    if every_seconds.is_multiple_of(24 * 60 * 60) {
        format!("{}d", every_seconds / (24 * 60 * 60))
    } else if every_seconds.is_multiple_of(60 * 60) {
        format!("{}h", every_seconds / (60 * 60))
    } else if every_seconds.is_multiple_of(60) {
        format!("{}m", every_seconds / 60)
    } else {
        format!("{every_seconds}s")
    }
}

#[cfg(test)]
#[path = "loop_actions_tests.rs"]
mod tests;
