//! Outbound ports. Application calls into infrastructure.
//!
//! Implemented by adapters.

use crate::domain::{Chat, DomainError, MediaReference, Message, SignInResult};

/// Telegram API gateway. Fetch dialogs, messages, media.
#[async_trait::async_trait]
pub trait TgGateway: Send + Sync {
    /// Fetch all dialogs (chats) the user participates in.
    async fn get_dialogs(&self) -> Result<Vec<Chat>, DomainError>;

    /// Fetch messages from a chat. Uses `min_id` and `max_id` for incremental sync:
    /// only messages with min_id < id < max_id are returned (when max_id > 0).
    ///
    /// - `min_id`: 0 = fetch from beginning; N = fetch only messages with id > N
    /// - `max_id`: 0 = no upper bound; N = fetch only messages with id < N (for pagination)
    /// - `limit`: max messages per request
    async fn get_messages(
        &self,
        chat_id: i64,
        min_id: i32,
        max_id: i32,
        limit: i32,
    ) -> Result<Vec<Message>, DomainError>;

    /// Download media file to the given path. Uses `opaque_ref` from MediaReference.
    async fn download_media(
        &self,
        media_ref: &MediaReference,
        dest_path: &std::path::Path,
    ) -> Result<(), DomainError>;
}

/// Repository port. Persist and load chat messages.
#[async_trait::async_trait]
pub trait RepoPort: Send + Sync {
    /// Save messages (append/merge). Implementations use INSERT OR IGNORE / dedupe by message id.
    async fn save_messages(&self, chat_id: i64, messages: &[Message]) -> Result<(), DomainError>;

    /// Load messages for a chat, newest first. Use limit/offset for pagination.
    async fn get_messages(
        &self,
        chat_id: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Message>, DomainError>;
}

/// State port. Track last synced message ID per chat for incremental sync.
#[async_trait::async_trait]
pub trait StatePort: Send + Sync {
    /// Get last known message ID for a chat. Returns 0 if none.
    async fn get_last_message_id(&self, chat_id: i64) -> Result<i32, DomainError>;

    /// Update last message ID after successful save.
    async fn set_last_message_id(&self, chat_id: i64, message_id: i32) -> Result<(), DomainError>;
}

/// Authentication port. Check auth state and perform login/2FA via Telegram.
#[async_trait::async_trait]
pub trait AuthPort: Send + Sync {
    /// Returns true if the session is already authorized.
    async fn is_authenticated(&self) -> Result<bool, DomainError>;

    /// Request a login code for the given phone. Must be followed by sign_in with the code.
    async fn request_login_code(&self, phone: &str, api_hash: &str) -> Result<(), DomainError>;

    /// Submit the code received via Telegram/SMS. Returns Success or PasswordRequired (2FA).
    async fn sign_in(&self, code: &str) -> Result<SignInResult, DomainError>;

    /// Complete 2FA after sign_in returned PasswordRequired. Call once per flow.
    async fn check_password(&self, password: &[u8]) -> Result<(), DomainError>;
}

/// Processor port. Invoke external tool (e.g. Chatpack) on archived data.
#[async_trait::async_trait]
pub trait ProcessorPort: Send + Sync {
    /// Process the given chat's data. Called after sync/download.
    async fn process_chat(
        &self,
        chat_id: i64,
        data_path: &std::path::Path,
    ) -> Result<(), DomainError>;
}
