//! Durable native loops, cron schedules, and background-terminal monitors for Codex threads.

mod extension;
mod runtime;
mod state_store;
mod tools;

use std::sync::Arc;
use std::sync::Weak;

use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_extension_api::ExtensionRegistryBuilder;

pub use extension::AutomationExtension;
pub use runtime::AutomationHandle;
pub use runtime::LoopInfo;

pub fn install(
    registry: &mut ExtensionRegistryBuilder<Config>,
    thread_manager: Weak<ThreadManager>,
) {
    let extension = Arc::new(AutomationExtension::new(thread_manager));
    registry.thread_lifecycle_contributor(extension.clone());
    registry.prompt_contributor(extension.clone());
    registry.tool_contributor(extension);
}
