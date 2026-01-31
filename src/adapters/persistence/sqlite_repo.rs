//! SQLite-backed repository via libsql. Implements RepoPort with O(1) inserts and efficient range queries.
//!
//! Uses the same libsql backend as grammers-session to avoid duplicate SQLite symbol link errors.
//! Single `messages` table with (chat_id, id) as primary key; batch saves use INSERT OR IGNORE.

use crate::domain::{DomainError, MediaReference, Message};
use crate::ports::RepoPort;
use libsql::{params, Database};
use std::path::Path;

const MESSAGES_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS messages (
    chat_id INTEGER NOT NULL,
    id INTEGER NOT NULL,
    date INTEGER NOT NULL,
    text TEXT NOT NULL DEFAULT '',
    media_json TEXT,
    from_user_id INTEGER,
    reply_to_msg_id INTEGER,
    PRIMARY KEY (chat_id, id)
)"#;
const MESSAGES_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_messages_chat_date ON messages (chat_id, date DESC)";

/// SQLite repository. One database file (messages.db) in the given base directory.
pub struct SqliteRepo {
    db: Database,
}

impl SqliteRepo {
    /// Connect to (or create) the SQLite database and ensure the schema exists.
    /// Call this once at startup; the returned repo is safe to share via Arc.
    pub async fn connect(base_dir: impl AsRef<Path>) -> Result<Self, DomainError> {
        let base = base_dir.as_ref();
        std::fs::create_dir_all(base).map_err(|e| DomainError::Repo(e.to_string()))?;
        let db_path = base.join("messages.db");
        let path_str = db_path.to_string_lossy();
        let db = libsql::Builder::new_local(path_str.as_ref())
            .build()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let conn = db.connect().map_err(|e| DomainError::Repo(e.to_string()))?;
        conn.execute(MESSAGES_TABLE, ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        conn.execute(MESSAGES_INDEX, ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        Ok(Self { db })
    }

    fn media_to_json(media: &Option<MediaReference>) -> Option<String> {
        media.as_ref().and_then(|m| serde_json::to_string(m).ok())
    }

    fn json_to_media(s: Option<&str>) -> Option<MediaReference> {
        s.and_then(|s| serde_json::from_str(s).ok())
    }
}

#[async_trait::async_trait]
impl RepoPort for SqliteRepo {
    async fn save_messages(&self, chat_id: i64, messages: &[Message]) -> Result<(), DomainError> {
        if messages.is_empty() {
            return Ok(());
        }
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let tx = conn
            .transaction()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        for m in messages {
            let media_json = Self::media_to_json(&m.media);
            tx.execute(
                r#"
                INSERT INTO messages (chat_id, id, date, text, media_json, from_user_id, reply_to_msg_id)
                VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                ON CONFLICT (chat_id, id) DO NOTHING
                "#,
                params![chat_id, m.id, m.date, m.text.as_str(), media_json, m.from_user_id, m.reply_to_msg_id],
            )
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        }
        tx.commit()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        Ok(())
    }

    async fn get_messages(
        &self,
        chat_id: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Message>, DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let mut rows = conn
            .query(
                r#"
                SELECT chat_id, id, date, text, media_json, from_user_id, reply_to_msg_id
                FROM messages
                WHERE chat_id = ?1
                ORDER BY date DESC
                LIMIT ?2 OFFSET ?3
                "#,
                params![chat_id, limit as i64, offset as i64],
            )
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let mut messages = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
        {
            let id: i32 = row.get(1).map_err(|e| DomainError::Repo(e.to_string()))?;
            let chat_id: i64 = row.get(0).map_err(|e| DomainError::Repo(e.to_string()))?;
            let date: i64 = row.get(2).map_err(|e| DomainError::Repo(e.to_string()))?;
            let text: String = row.get::<String>(3).unwrap_or_default();
            let media_json: Option<String> = row.get(4).ok();
            let from_user_id: Option<i64> = row.get(5).ok();
            let reply_to_msg_id: Option<i32> = row.get(6).ok();
            messages.push(Message {
                id,
                chat_id,
                date,
                text,
                media: Self::json_to_media(media_json.as_deref()),
                from_user_id,
                reply_to_msg_id,
            });
        }
        Ok(messages)
    }
}
