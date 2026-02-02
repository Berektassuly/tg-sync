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
