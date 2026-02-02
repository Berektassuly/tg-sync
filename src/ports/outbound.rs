//! Outbound ports. Application calls into infrastructure.
//!
//! Implemented by adapters.

use crate::domain::{Chat, DomainError, MediaReference, Message, SignInResult};
use std::collections::HashSet;

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

    /// Get the current user's ID (for Saved Messages / "me"). Used by Watcher for notifications.
    async fn get_me_id(&self) -> Result<i64, DomainError>;

    /// Send a text message to a chat (e.g. Saved Messages for alerts). `chat_id` is the dialog id (e.g. own user id for Saved Messages).
    async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), DomainError>;
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

    /// Get the set of chat IDs that are blacklisted (excluded from backup).
    async fn get_blacklisted_ids(&self) -> Result<HashSet<i64>, DomainError>;

    /// Sync the blacklist with the given set. Replaces the stored blacklist with `ids`.
    async fn update_blacklist(&self, ids: HashSet<i64>) -> Result<(), DomainError>;

    /// Get the set of chat IDs that are watched (target whitelist for Watcher mode).
    async fn get_target_ids(&self) -> Result<HashSet<i64>, DomainError>;

    /// Sync the target list with the given set. Replaces the stored targets with `ids`.
    async fn update_targets(&self, ids: HashSet<i64>) -> Result<(), DomainError>;
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

/// Audit §6.2: Persistent entity registry for access_hash caching.
/// Stores (peer_id, access_hash) to avoid re-iterating dialogs (FLOOD_WAIT risk).
#[async_trait::async_trait]
pub trait EntityRegistry: Send + Sync {
    /// Get cached access_hash for a peer. Returns None if not cached.
    async fn get_access_hash(&self, peer_id: i64) -> Result<Option<i64>, DomainError>;

    /// Save or update an entity's access_hash in the registry.
    async fn save_entity(
        &self,
        peer_id: i64,
        access_hash: i64,
        peer_type: &str,
        username: Option<&str>,
    ) -> Result<(), DomainError>;
}

// ─────────────────────────────────────────────────────────────────────────────
// AI Analysis Ports
// ─────────────────────────────────────────────────────────────────────────────

use crate::domain::{AnalysisResult, WeekGroup};

/// AI Analysis port. Send context to LLM, receive structured analysis.
///
/// Implementations may use OpenAI, Ollama, Anthropic, or any compatible API.
/// The adapter handles prompt construction and response parsing.
#[async_trait::async_trait]
pub trait AiPort: Send + Sync {
    /// Analyze chat context (CSV format). Returns structured analysis result.
    ///
    /// # Arguments
    /// * `chat_id` - The chat being analyzed (for result metadata)
    /// * `week_group` - The week being analyzed (e.g., "2024-05")
    /// * `context_csv` - CSV-formatted chat log: "Date;User;Message"
    ///
    /// # Errors
    /// Returns `DomainError::Ai` if the LLM API fails or returns invalid JSON.
    async fn analyze(
        &self,
        chat_id: i64,
        week_group: &WeekGroup,
        context_csv: &str,
    ) -> Result<AnalysisResult, DomainError>;
}

/// Analysis log persistence. Track which weeks have been analyzed.
///
/// Implemented by `SqliteRepo` to persist analysis state and results.
#[async_trait::async_trait]
pub trait AnalysisLogPort: Send + Sync {
    /// Get all week groups for a chat that have NOT been analyzed yet.
    ///
    /// Returns weeks in chronological order (oldest first).
    async fn get_unanalyzed_weeks(&self, chat_id: i64) -> Result<Vec<WeekGroup>, DomainError>;

    /// Get messages grouped by week for CSV export.
    ///
    /// Filters out:
    /// - Empty messages
    /// - Service messages (joins/leaves)
    /// - Stickers without captions
    ///
    /// Returns: Vec<(WeekGroup, Vec<Message>)> sorted chronologically.
    async fn get_messages_by_week(
        &self,
        chat_id: i64,
    ) -> Result<Vec<(WeekGroup, Vec<Message>)>, DomainError>;

    /// Save analysis result after LLM processing.
    ///
    /// Uses UPSERT semantics: if the week was already analyzed, the result is replaced.
    async fn save_analysis(&self, result: &AnalysisResult) -> Result<(), DomainError>;

    /// Get previously saved analysis for a chat+week.
    ///
    /// Returns `None` if the week has not been analyzed.
    async fn get_analysis(
        &self,
        chat_id: i64,
        week_group: &WeekGroup,
    ) -> Result<Option<AnalysisResult>, DomainError>;
}
