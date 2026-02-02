//! Implements AuthPort using grammers Client.
//!
//! Holds a client (clone shared with TgGateway in main). No global lock.
//! Stores login token and password token between calls for the auth flow.

use crate::domain::{DomainError, SignInResult};
use crate::ports::AuthPort;
use async_trait::async_trait;
use grammers_client::client::{LoginToken, PasswordToken};
use grammers_client::Client;
use tokio::sync::Mutex;

/// Auth adapter. Wraps grammers Client for login/2FA. Same session as TgGateway via clone in main.
pub struct GrammersAuthAdapter {
    client: Client,
    /// Token from request_login_code; consumed by sign_in.
    login_token: Mutex<Option<LoginToken>>,
    /// Token from sign_in(PasswordRequired); consumed by check_password.
    password_token: Mutex<Option<PasswordToken>>,
}

impl GrammersAuthAdapter {
    /// Create adapter with a client (use same session via clone in main).
    pub fn new(client: Client) -> Self {
        Self {
            client,
            login_token: Mutex::new(None),
            password_token: Mutex::new(None),
        }
    }
}

#[async_trait]
impl AuthPort for GrammersAuthAdapter {
    async fn is_authenticated(&self) -> Result<bool, DomainError> {
        self.client
            .is_authorized()
            .await
            .map_err(|e| DomainError::Auth(e.to_string()))
    }

    async fn request_login_code(&self, phone: &str, api_hash: &str) -> Result<(), DomainError> {
        let token = self
            .client
            .request_login_code(phone, api_hash)
            .await
            .map_err(|e| DomainError::Auth(format!("request_login_code: {}", e)))?;
        *self.login_token.lock().await = Some(token);
        *self.password_token.lock().await = None;
        Ok(())
    }

    async fn sign_in(&self, code: &str) -> Result<SignInResult, DomainError> {
        let token = self.login_token.lock().await.take().ok_or_else(|| {
            DomainError::Auth("request_login_code must be called before sign_in".into())
        })?;
        match self.client.sign_in(&token, code).await {
            Ok(_user) => Ok(SignInResult::Success),
            Err(grammers_client::SignInError::PasswordRequired(pt)) => {
                let hint = pt.hint().map(String::from);
                *self.password_token.lock().await = Some(pt);
                Ok(SignInResult::PasswordRequired { hint })
            }
            Err(grammers_client::SignInError::InvalidCode) => Err(DomainError::Auth(
                "Invalid login code. Run again and enter the correct code.".into(),
            )),
            Err(grammers_client::SignInError::SignUpRequired) => Err(DomainError::Auth(
                "Sign-up required. Create an account with the official Telegram app first.".into(),
            )),
            Err(e) => Err(DomainError::Auth(format!("sign in: {}", e))),
        }
    }

    async fn check_password(&self, password: &[u8]) -> Result<(), DomainError> {
        let pt = self.password_token.lock().await.take().ok_or_else(|| {
            DomainError::Auth("sign_in must return PasswordRequired before check_password".into())
        })?;
        self.client
            .check_password(pt, password)
            .await
            .map_err(|e| DomainError::Auth(format!("check_password: {}", e)))?;
        Ok(())
    }
}
