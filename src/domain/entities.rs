//! Domain entities. Pure data structures for the core business.
//!
//! No Telegram/IO types here — these are mapped from adapters.

use serde::{Deserialize, Serialize};

/// Represents a Telegram chat (user, group, or channel).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chat {
    pub id: i64,
    pub title: String,
    pub username: Option<String>,
    #[serde(rename = "type")]
    pub kind: ChatType,
    /// Approximate message count heuristic from dialog top/last message ID (no full history fetch).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approx_message_count: Option<i32>,
}

/// Classification of a Telegram chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatType {
    /// Private DM with a user.
    #[serde(alias = "user")]
    Private,
    /// Small group chat.
    Group,
    /// Supergroup (megagroup).
    Supergroup,
    /// Broadcast channel.
    Channel,
}

/// One prior version of a message (used for edit history).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEdit {
    pub date: i64,
    pub text: String,
}

/// A single message from a chat.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub id: i32,
    pub chat_id: i64,
    pub date: i64,
    pub text: String,
    pub media: Option<MediaReference>,
    pub from_user_id: Option<i64>,
    pub reply_to_msg_id: Option<i32>,
    /// Previous versions when the message was edited. Oldest first.
    #[serde(default)]
    pub edit_history: Option<Vec<MessageEdit>>,
}

/// Reference to downloadable media. Sent to media pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaReference {
    pub message_id: i32,
    pub chat_id: i64,
    pub media_type: MediaType,
    /// Opaque handle for the adapter to resolve (e.g. file reference, input location).
    pub opaque_ref: String,
}

/// Result of a sign-in attempt. Either success or 2FA password required.
#[derive(Debug, Clone)]
pub enum SignInResult {
    Success,
    PasswordRequired { hint: Option<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    Photo,
    Video,
    Document,
    Audio,
    Voice,
    Sticker,
    Animation,
    Other,
}

// ─────────────────────────────────────────────────────────────────────────────
// AI Analysis Entities
// ─────────────────────────────────────────────────────────────────────────────

/// Weekly grouping key for analysis (e.g., "2024-05").
/// Format: "YYYY-WW" where WW is ISO week number.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct WeekGroup(pub String);

impl WeekGroup {
    /// Create from SQLite strftime output: "YYYY-WW"
    pub fn new(year_week: impl Into<String>) -> Self {
        Self(year_week.into())
    }

    /// Get the inner string value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for WeekGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Single action item extracted from chat analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionItem {
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<String>,
}

/// Result of LLM analysis for a week's chat data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisResult {
    pub week_group: WeekGroup,
    pub chat_id: i64,
    pub summary: String,
    pub key_topics: Vec<String>,
    pub action_items: Vec<ActionItem>,
    /// Unix timestamp when analysis was performed.
    pub analyzed_at: i64,
}
