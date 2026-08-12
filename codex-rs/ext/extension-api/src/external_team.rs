use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Boxed future returned by an external team provider.
pub type ExternalTeamFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// One agent whose lifecycle is owned by an external team runtime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExternalAgent {
    pub id: String,
    pub name: String,
    pub role: Option<String>,
    pub status: ExternalAgentStatus,
}

/// Provider-neutral status for an external teammate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalAgentStatus {
    Active,
    Idle,
    Stopped,
    Unknown,
}

/// Whether a message is informational or should wake an idle teammate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExternalMessageDelivery {
    Queue,
    Wake,
}

/// Provider boundary for a team whose roster and transport are not owned by Codex.
///
/// Implementations must keep agent IDs stable for the lifetime of a team, return exact matches
/// from [`ExternalTeamProvider::resolve_agent`], and provide at-least-once delivery or report an
/// error. Codex invokes providers from model-tool execution paths, so returned futures must be
/// sendable and bounded by the provider's own timeout policy.
pub trait ExternalTeamProvider: Send + Sync {
    fn identity(&self) -> ExternalAgent;

    fn parent(&self) -> ExternalAgent;

    fn resolve_agent<'a>(
        &'a self,
        target: &'a str,
    ) -> ExternalTeamFuture<'a, Result<Option<ExternalAgent>, String>>;

    fn list_agents(&self) -> ExternalTeamFuture<'_, Result<Vec<ExternalAgent>, String>>;

    fn send_message<'a>(
        &'a self,
        target: &'a ExternalAgent,
        content: &'a str,
        delivery: ExternalMessageDelivery,
    ) -> ExternalTeamFuture<'a, Result<(), String>>;

    fn interrupt<'a>(
        &'a self,
        target: &'a ExternalAgent,
        reason: &'a str,
    ) -> ExternalTeamFuture<'a, Result<(), String>>;
}

/// Thread-scoped access to an installed external team provider.
#[derive(Clone)]
pub struct ExternalTeamHandle {
    provider: Arc<dyn ExternalTeamProvider>,
}

impl ExternalTeamHandle {
    pub fn new(provider: Arc<dyn ExternalTeamProvider>) -> Self {
        Self { provider }
    }

    pub fn provider(&self) -> &Arc<dyn ExternalTeamProvider> {
        &self.provider
    }
}

impl std::fmt::Debug for ExternalTeamHandle {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ExternalTeamHandle")
            .finish_non_exhaustive()
    }
}
