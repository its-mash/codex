//! Fork-owned: operator-visible notices for inbound inter-agent traffic.
//!
//! The upstream TUI ignores `RawResponseItemCompleted`; a native teammate needs
//! to see messages that arrive from external teammates (`/root/external/*`) and
//! native automation firings (`/root/automation`) instead of only the model's
//! reaction to them.

use super::ChatWidget;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::plaintext_agent_message_content;

impl ChatWidget {
    /// Render a visible notice for an inbound inter-agent message — a teammate
    /// message or a native automation (loop/cron/monitor) firing.
    pub(super) fn render_inbound_inter_agent(&mut self, item: &ResponseItem) {
        let ResponseItem::AgentMessage {
            author, content, ..
        } = item
        else {
            return;
        };
        let text = plaintext_agent_message_content(content).unwrap_or_default();
        if let Some(line) = inbound_inter_agent_notice(author, &text) {
            self.add_info_message(line, None);
        }
    }
}

/// Build a one-line operator notice for an inbound inter-agent message, or None
/// when `author` is neither an external teammate (`/root/external/*`) nor native
/// automation (`/root/automation`). Loop/cron/monitor firings and new-task
/// assignments are labelled by their delivery prefix; everything else from a
/// teammate reads as a plain message.
fn inbound_inter_agent_notice(author: &str, text: &str) -> Option<String> {
    let is_external = author.starts_with("/root/external");
    let is_automation = author.contains("automation");
    if !is_external && !is_automation {
        return None;
    }
    let head: String = text
        .trim()
        .lines()
        .next()
        .unwrap_or_default()
        .chars()
        .take(160)
        .collect();
    let sender = author.rsplit('/').next().unwrap_or(author);
    Some(if head.starts_with("[CRON") {
        format!("⏰ cron fired — {head}")
    } else if head.starts_with("[LOOP") {
        format!("🔁 loop fired — {head}")
    } else if head.starts_with("[MONITOR") {
        format!("👁 monitor — {head}")
    } else if head.starts_with("[NEW_TASK]") {
        format!("📥 new task from {sender}")
    } else if is_automation {
        format!("⏰ automation — {head}")
    } else {
        format!("📨 message from {sender}: {head}")
    })
}

#[cfg(test)]
mod tests {
    use super::inbound_inter_agent_notice;

    #[test]
    fn teammate_message_shows_sender_and_head() {
        let notice = inbound_inter_agent_notice("/root/external/team_lead", "PING_NATIVE").unwrap();
        assert!(notice.contains("message from team_lead"), "{notice}");
        assert!(notice.contains("PING_NATIVE"), "{notice}");
    }

    #[test]
    fn automation_firings_are_labelled() {
        assert!(
            inbound_inter_agent_notice("/root/automation", "[CRON:MANUAL x (id)]\nre-mine")
                .unwrap()
                .starts_with("⏰ cron fired")
        );
        assert!(
            inbound_inter_agent_notice("/root/automation", "[LOOP:x id]\ntick")
                .unwrap()
                .starts_with("🔁 loop fired")
        );
        assert!(
            inbound_inter_agent_notice("/root/automation", "[MONITOR:x id]\nHIT")
                .unwrap()
                .starts_with("👁 monitor")
        );
    }

    #[test]
    fn new_task_shows_sender() {
        let notice =
            inbound_inter_agent_notice("/root/external/team_lead", "[NEW_TASK]\ndo the thing")
                .unwrap();
        assert!(notice.contains("new task from team_lead"), "{notice}");
    }

    #[test]
    fn own_and_local_output_is_not_surfaced() {
        // The thread's own model output and local sub-agents are not inbound team
        // traffic, so they produce no notice (avoids double-rendering the model).
        assert!(inbound_inter_agent_notice("/root", "hello from me").is_none());
        assert!(inbound_inter_agent_notice("/root/child_agent", "child output").is_none());
    }
}
