//! Fork-owned: `App` behaviour specific to native-teammate mode (this TUI is a
//! member of an externally managed agent team, `config.external_team`).

use super::thread_events::ThreadBufferedEvent;
use super::*;
use codex_app_server_protocol::ThreadStatus;

impl App {
    /// `teammate <agent> @ <team>` when this session is a native Codex teammate, else None.
    ///
    /// In native-teammate mode there is a single thread, so the multi-agent
    /// navigation label stays hidden — but the operator still needs to know which
    /// team member this pane is. `sync_active_agent_label` falls back to this.
    pub(super) fn external_team_agent_label(&self) -> Option<String> {
        let team = self.config.external_team.as_ref()?;
        Some(format!("teammate {} @ {}", team.agent_name, team.team_name))
    }

    /// Returns the primary thread id when an externally managed teammate has been shut down.
    ///
    /// External team shutdown originates in the provider lifecycle rather than from a TUI exit
    /// action. Closing the primary thread must therefore also close the teammate process; leaving
    /// an interactive prompt behind would keep Claude's teammate slot alive after it acknowledged
    /// shutdown.
    pub(super) fn external_team_shutdown_exit_thread(
        &self,
        notification: &ServerNotification,
    ) -> Option<ThreadId> {
        let closed_thread_id = match notification {
            ServerNotification::ThreadClosed(closed) => &closed.thread_id,
            ServerNotification::ThreadStatusChanged(changed)
                if matches!(changed.status, ThreadStatus::NotLoaded) =>
            {
                &changed.thread_id
            }
            _ => return None,
        };
        self.config.external_team.as_ref()?;
        let active_thread_id = self.active_thread_id?;
        let primary_thread_id = self.primary_thread_id?;
        (active_thread_id == primary_thread_id
            && *closed_thread_id == primary_thread_id.to_string())
        .then_some(primary_thread_id)
    }

    /// Whether `event` is the shutdown of this teammate's primary thread. Must be
    /// evaluated before the event is consumed by `handle_thread_event_now`.
    pub(super) fn external_team_shutdown_completed(&self, event: &ThreadBufferedEvent) -> bool {
        matches!(
            event,
            ThreadBufferedEvent::Notification(notification)
                if self.external_team_shutdown_exit_thread(notification.as_ref()).is_some()
        )
    }

    /// Exit the teammate process once its primary thread has shut down.
    pub(super) fn finish_external_team_shutdown(&self, shutdown_completed: bool) {
        if shutdown_completed {
            self.app_event_tx.send(AppEvent::Exit(ExitMode::Immediate));
        }
    }
}
