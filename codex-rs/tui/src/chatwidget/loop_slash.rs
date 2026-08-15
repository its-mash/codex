//! Fork-owned: `/loop` slash-command dispatch (durable recurring self-loops backed
//! by the native automation extension). Parsing lives in `crate::loop_command`;
//! execution in `app/loop_actions.rs`.

use super::ChatWidget;
use crate::app_event::AppEvent;
use crate::loop_command::parse_loop_command;

impl ChatWidget {
    pub(super) fn dispatch_loop_command(&mut self, args: &str) {
        match parse_loop_command(args) {
            Ok(action) => {
                let Some(thread_id) = self.thread_id else {
                    self.add_error_message(
                        "Session is still starting; try /loop again in a moment.".to_string(),
                    );
                    return;
                };
                self.app_event_tx
                    .send(AppEvent::LoopCommand { thread_id, action });
            }
            Err(error) => self.add_error_message(error),
        }
    }
}
