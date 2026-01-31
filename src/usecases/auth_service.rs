//! Handle login / 2FA flow. Delegates to AuthPort; collects user input (phone, code, 2FA) here.
//!
//! Keeps authentication workflow in the use-case layer; main.rs only bootstraps and calls run_auth_flow.

use crate::domain::{DomainError, SignInResult};
use crate::ports::AuthPort;
use std::sync::Arc;
use tracing::{info, warn};

pub struct AuthService {
    auth_port: Arc<dyn AuthPort>,
    api_hash: String,
}

impl AuthService {
    pub fn new(auth_port: Arc<dyn AuthPort>, api_hash: String) -> Self {
        Self {
            auth_port,
            api_hash,
        }
    }

    /// Check if we are already authenticated (delegates to auth port).
    pub async fn is_authenticated(&self) -> Result<bool, DomainError> {
        self.auth_port.is_authenticated().await
    }

    /// Run full auth flow: check auth → if not, prompt phone → request code → prompt code →
    /// sign in → if 2FA required, prompt password and check_password.
    pub async fn run_auth_flow(&self) -> Result<(), DomainError> {
        if self.auth_port.is_authenticated().await? {
            info!("Already authorized");
            return Ok(());
        }

        warn!("Not authorized. Running login flow (phone + code from Telegram app/SMS).");

        let phone = inquire::Text::new("Phone number (e.g. +1234567890):")
            .prompt()
            .map_err(|e| DomainError::Auth(format!("input: {}", e)))?;

        self.auth_port
            .request_login_code(phone.trim(), &self.api_hash)
            .await?;

        let code = inquire::Text::new("Login code from Telegram:")
            .prompt()
            .map_err(|e| DomainError::Auth(format!("input: {}", e)))?;

        match self.auth_port.sign_in(code.trim()).await? {
            SignInResult::Success => {
                info!("Signed in successfully");
                Ok(())
            }
            SignInResult::PasswordRequired { hint } => {
                let hint_str = hint.as_deref().unwrap_or("(no hint)");
                let prompt = format!("2FA password (hint: {}):", hint_str);
                let password = inquire::Password::new(&prompt)
                    .prompt()
                    .map_err(|e| DomainError::Auth(format!("input: {}", e)))?;
                self.auth_port.check_password(password.as_bytes()).await?;
                info!("Signed in (2FA completed)");
                Ok(())
            }
        }
    }
}
