//! Outbound ports. Application calls into infrastructure.
//!
//! Implemented by adapters.

use crate::domain::{Chat, DomainError, MediaReference, Message};

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

/// Repository port. Persist chat logs (JSON).
#[async_trait::async_trait]
pub trait RepoPort: Send + Sync {
    /// Append messages to the chat's log file.
    async fn save_messages(&self, chat_id: i64, messages: &[Message]) -> Result<(), DomainError>;
}

/// State port. Track last synced message ID per chat for incremental sync.
#[async_trait::async_trait]
pub trait StatePort: Send + Sync {
    /// Get last known message ID for a chat. Returns 0 if none.
    async fn get_last_message_id(&self, chat_id: i64) -> Result<i32, DomainError>;

    /// Update last message ID after successful save.
    async fn set_last_message_id(&self, chat_id: i64, message_id: i32) -> Result<(), DomainError>;
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
