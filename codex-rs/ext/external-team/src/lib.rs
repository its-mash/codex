//! Native integration for Codex threads that are members of an externally owned agent team.

mod claude;
mod extension;
mod runtime;
mod task_store;
mod task_tools;

pub use extension::ExternalTeamExtension;

use std::sync::Arc;

use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_extension_api::ExtensionRegistryBuilder;

pub fn install(
    registry: &mut ExtensionRegistryBuilder<Config>,
    thread_manager: std::sync::Weak<ThreadManager>,
) {
    let extension = Arc::new(ExternalTeamExtension::new(thread_manager));
    registry.thread_lifecycle_contributor(extension.clone());
    registry.prompt_contributor(extension.clone());
    registry.turn_item_contributor(extension.clone());
    registry.tool_contributor(extension);
}
