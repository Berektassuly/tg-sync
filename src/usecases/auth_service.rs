//! Handle Login / 2FA flow. Delegates to TgGateway/auth adapter.
//!
//! Skeleton: actual flow depends on grammers Client API.

use crate::domain::DomainError;

pub struct AuthService;

impl AuthService {
    pub fn new() -> Self {
        Self
    }

    /// Check if we are already authenticated.
    pub fn is_authenticated(&self) -> bool {
        // Delegated to adapter; skeleton returns false
        false
    }

    /// Run full auth flow (phone -> code -> 2FA if needed).
    pub async fn run_auth_flow(&self) -> Result<(), DomainError> {
        // Implemented by adapter (telegram client) or UI prompts
        Err(DomainError::Auth("not implemented".into()))
    }
}
