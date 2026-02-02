//! SQLite-backed repository via libsql. Implements RepoPort with O(1) inserts and efficient range queries.
//!
//! Uses the same libsql backend as grammers-session to avoid duplicate SQLite symbol link errors.
//! Single `messages` table with (chat_id, id) as primary key; batch saves use INSERT OR IGNORE.
//! All chats share one database file: data/messages.db

use crate::domain::{DomainError, MediaReference, Message};
use crate::ports::{EntityRegistry, RepoPort};
use libsql::{params, Database};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use tracing::info;

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

/// Audit §6.2: Persistent entity registry for access_hash caching.
/// Avoids re-iterating dialogs (getDialogs) which triggers FLOOD_WAIT.
const ENTITY_REGISTRY_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS entity_registry (
    peer_id INTEGER PRIMARY KEY,
    access_hash INTEGER NOT NULL,
    peer_type TEXT NOT NULL,
    username TEXT,
    updated_at INTEGER NOT NULL
)"#;

/// Blacklist: chat IDs to exclude from backup. One row per chat_id.
const BLACKLIST_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS blacklist (
    chat_id INTEGER PRIMARY KEY
)"#;

/// SQLite repository. One database file (messages.db) in the given base directory.
/// Chat IDs are stored as a column; all chats share the same file.
pub struct SqliteRepo {
    db: Database,
    db_path: PathBuf,
}

impl SqliteRepo {
    /// Connect to (or create) the SQLite database and ensure the schema exists.
    /// Call this once at startup; the returned repo is safe to share via Arc.
    ///
    /// Audit §5.3: Sets WAL mode and synchronous=NORMAL for concurrent read/write
    /// and better performance without sacrificing durability.
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

        // Audit §5.3: WAL mode enables concurrent readers + one writer.
        // PRAGMA returns a row (new value); use query and consume rows (execute fails when rows are returned).
        let mut wal_rows = conn
            .query("PRAGMA journal_mode=WAL", ())
            .await
            .map_err(|e| DomainError::Repo(format!("WAL pragma failed: {}", e)))?;
        while wal_rows
            .next()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
            .is_some()
        {}
        // Audit §5.3: synchronous=NORMAL is safe with WAL and faster than FULL.
        let mut sync_rows = conn
            .query("PRAGMA synchronous=NORMAL", ())
            .await
            .map_err(|e| DomainError::Repo(format!("synchronous pragma failed: {}", e)))?;
        while sync_rows
            .next()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
            .is_some()
        {}

        conn.execute(MESSAGES_TABLE, ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        conn.execute(MESSAGES_INDEX, ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        // Audit §6.2: Entity registry for persistent access_hash caching.
        conn.execute(ENTITY_REGISTRY_TABLE, ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        conn.execute(BLACKLIST_TABLE, ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        info!(
            path = %db_path.display(),
            "SQLite connected with WAL mode and entity_registry"
        );

        Ok(Self {
            db,
            db_path: db_path.to_path_buf(),
        })
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
        let abs_path = self
            .db_path
            .canonicalize()
            .unwrap_or_else(|_| self.db_path.clone());
        info!(
            path = %abs_path.display(),
            chat_id,
            count = messages.len(),
            "saved messages to disk"
        );
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

    async fn get_blacklisted_ids(&self) -> Result<HashSet<i64>, DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let mut rows = conn
            .query("SELECT chat_id FROM blacklist", ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let mut ids = HashSet::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
        {
            let chat_id: i64 = row.get(0).map_err(|e| DomainError::Repo(e.to_string()))?;
            ids.insert(chat_id);
        }
        Ok(ids)
    }

    async fn update_blacklist(&self, ids: HashSet<i64>) -> Result<(), DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let tx = conn
            .transaction()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        tx.execute("DELETE FROM blacklist", ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        for chat_id in ids {
            tx.execute(
                "INSERT INTO blacklist (chat_id) VALUES (?1)",
                params![chat_id],
            )
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        }
        tx.commit()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        Ok(())
    }
}

/// Audit §6.2: Persistent entity registry implementation.
/// Enables fast InputPeer resolution without re-iterating dialogs.
#[async_trait::async_trait]
impl EntityRegistry for SqliteRepo {
    async fn get_access_hash(&self, peer_id: i64) -> Result<Option<i64>, DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let mut rows = conn
            .query(
                "SELECT access_hash FROM entity_registry WHERE peer_id = ?1",
                params![peer_id],
            )
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
        {
            let access_hash: i64 = row.get(0).map_err(|e| DomainError::Repo(e.to_string()))?;
            Ok(Some(access_hash))
        } else {
            Ok(None)
        }
    }

    async fn save_entity(
        &self,
        peer_id: i64,
        access_hash: i64,
        peer_type: &str,
        username: Option<&str>,
    ) -> Result<(), DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        conn.execute(
            r#"
            INSERT INTO entity_registry (peer_id, access_hash, peer_type, username, updated_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT (peer_id) DO UPDATE SET
                access_hash = excluded.access_hash,
                peer_type = excluded.peer_type,
                username = excluded.username,
                updated_at = excluded.updated_at
            "#,
            params![peer_id, access_hash, peer_type, username, now],
        )
        .await
        .map_err(|e| DomainError::Repo(e.to_string()))?;

        Ok(())
    }
}
