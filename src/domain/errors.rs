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

    /// FloodWait error: caller should reschedule job after `seconds` seconds.
    /// Per Audit §4.1: long waits (≥60s) should not block the worker thread.
    #[error("FloodWait: retry after {seconds} seconds")]
    FloodWait { seconds: u64 },

    #[error("AI analysis failed: {0}")]
    Ai(String),

    #[error("Task tracker error: {0}")]
    TaskTracker(String),
}
