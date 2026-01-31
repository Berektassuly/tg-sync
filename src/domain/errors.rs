//! Domain errors. Used by ports and use cases.
//!
//! Adapters map infrastructure errors into these.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum DomainError {
    #[error("Telegram gateway error: {0}")]
    TgGateway(String),

    #[error("Repository error: {0}")]
    Repo(String),

    #[error("State error: {0}")]
    State(String),

    #[error("Processor error: {0}")]
    Processor(String),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Media download failed: {0}")]
    Media(String),
}
