//! Inbound port. UI (adapter) calls into the application.

use crate::domain::DomainError;

/// Input port: UI/CLI invokes application use cases.
#[async_trait::async_trait]
pub trait InputPort: Send + Sync {
    /// Run interactive sync flow (select chats, sync, process).
    async fn run_sync(&self) -> Result<(), DomainError>;

    /// Handle login / 2FA flow. Returns when authenticated.
    async fn run_auth(&self) -> Result<(), DomainError>;
}
