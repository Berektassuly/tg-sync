//! Inbound port. UI (adapter) calls into the application.

use crate::domain::DomainError;

/// Input port: UI/CLI invokes application use cases.
#[async_trait::async_trait]
pub trait InputPort: Send + Sync {
    /// Run the main menu and dispatch to the selected mode (Full Backup, Watcher, AI Analysis).
    async fn run(&self) -> Result<(), DomainError>;

    /// Run interactive sync flow (select chats, sync, process). Used internally by Full Backup.
    async fn run_sync(&self) -> Result<(), DomainError>;

    /// Handle login / 2FA flow. Returns when authenticated.
    async fn run_auth(&self) -> Result<(), DomainError>;
}
