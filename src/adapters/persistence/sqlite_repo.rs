//! SQLite-backed repository via libsql. Implements RepoPort with O(1) inserts and efficient range queries.
//!
//! Uses the same libsql backend as grammers-session to avoid duplicate SQLite symbol link errors.
//! Single `messages` table with (chat_id, id) as primary key; batch saves use INSERT OR IGNORE.
//! All chats share one database file: data/messages.db

use crate::domain::{AnalysisResult, DomainError, MediaReference, Message, WeekGroup};
use crate::ports::{AnalysisLogPort, EntityRegistry, RepoPort};
use libsql::{params, Database};
use std::collections::{HashMap, HashSet};
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

/// Targets (whitelist): chat IDs to watch in Watcher mode. One row per chat_id.
const TARGETS_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS targets (
    chat_id INTEGER PRIMARY KEY
)"#;

/// AI Analysis log: tracks which weeks have been analyzed per chat.
/// Stores full AnalysisResult as JSON for retrieval.
const ANALYSIS_LOG_TABLE: &str = r#"
CREATE TABLE IF NOT EXISTS analysis_log (
    chat_id INTEGER NOT NULL,
    week_group TEXT NOT NULL,
    analyzed_at INTEGER NOT NULL,
    summary TEXT NOT NULL,
    result_json TEXT NOT NULL,
    PRIMARY KEY (chat_id, week_group)
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

        conn.execute(TARGETS_TABLE, ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        // AI Analysis: Create analysis_log table for tracking analyzed weeks.
        conn.execute(ANALYSIS_LOG_TABLE, ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        info!(
            path = %db_path.display(),
            "SQLite connected with WAL mode, entity_registry, and analysis_log"
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

    async fn get_target_ids(&self) -> Result<HashSet<i64>, DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let mut rows = conn
            .query("SELECT chat_id FROM targets", ())
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

    async fn update_targets(&self, ids: HashSet<i64>) -> Result<(), DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let tx = conn
            .transaction()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        tx.execute("DELETE FROM targets", ())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        for chat_id in ids {
            tx.execute(
                "INSERT INTO targets (chat_id) VALUES (?1)",
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

// ─────────────────────────────────────────────────────────────────────────────
// AI Analysis: AnalysisLogPort implementation
// ─────────────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
impl AnalysisLogPort for SqliteRepo {
    async fn get_unanalyzed_weeks(&self, chat_id: i64) -> Result<Vec<WeekGroup>, DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        // Find weeks with non-empty messages that haven't been analyzed yet.
        // Uses strftime with 'unixepoch' since date is stored as Unix timestamp.
        let mut rows = conn
            .query(
                r#"
                SELECT DISTINCT strftime('%Y-%W', date, 'unixepoch') as week_group
                FROM messages
                WHERE chat_id = ?1
                  AND text != ''
                  AND text NOT LIKE '%joined the group%'
                  AND text NOT LIKE '%left the group%'
                  AND strftime('%Y-%W', date, 'unixepoch') NOT IN (
                      SELECT week_group FROM analysis_log WHERE chat_id = ?1
                  )
                ORDER BY week_group ASC
                "#,
                params![chat_id],
            )
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        let mut weeks = Vec::new();
        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
        {
            let week_str: String = row.get(0).map_err(|e| DomainError::Repo(e.to_string()))?;
            weeks.push(WeekGroup::new(week_str));
        }

        Ok(weeks)
    }

    async fn get_messages_by_week(
        &self,
        chat_id: i64,
    ) -> Result<Vec<(WeekGroup, Vec<Message>)>, DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        // Fetch all messages with week grouping, filtering out empty/service messages.
        let mut rows = conn
            .query(
                r#"
                SELECT
                    strftime('%Y-%W', date, 'unixepoch') as week_group,
                    chat_id, id, date, text, media_json, from_user_id, reply_to_msg_id
                FROM messages
                WHERE chat_id = ?1
                  AND text != ''
                  AND text NOT LIKE '%joined the group%'
                  AND text NOT LIKE '%left the group%'
                ORDER BY week_group ASC, date ASC
                "#,
                params![chat_id],
            )
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        // Group messages by week using a HashMap, preserving order via insertion.
        let mut week_map: HashMap<String, Vec<Message>> = HashMap::new();
        let mut week_order: Vec<String> = Vec::new();

        while let Some(row) = rows
            .next()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
        {
            let week_str: String = row.get(0).map_err(|e| DomainError::Repo(e.to_string()))?;
            let msg_chat_id: i64 = row.get(1).map_err(|e| DomainError::Repo(e.to_string()))?;
            let id: i32 = row.get(2).map_err(|e| DomainError::Repo(e.to_string()))?;
            let date: i64 = row.get(3).map_err(|e| DomainError::Repo(e.to_string()))?;
            let text: String = row.get::<String>(4).unwrap_or_default();
            let media_json: Option<String> = row.get(5).ok();
            let from_user_id: Option<i64> = row.get(6).ok();
            let reply_to_msg_id: Option<i32> = row.get(7).ok();

            let message = Message {
                id,
                chat_id: msg_chat_id,
                date,
                text,
                media: Self::json_to_media(media_json.as_deref()),
                from_user_id,
                reply_to_msg_id,
            };

            if !week_map.contains_key(&week_str) {
                week_order.push(week_str.clone());
            }
            week_map.entry(week_str).or_default().push(message);
        }

        // Convert to Vec<(WeekGroup, Vec<Message>)> preserving chronological order.
        let result: Vec<(WeekGroup, Vec<Message>)> = week_order
            .into_iter()
            .filter_map(|week| {
                week_map
                    .remove(&week)
                    .map(|messages| (WeekGroup::new(week), messages))
            })
            .collect();

        Ok(result)
    }

    async fn save_analysis(&self, result: &AnalysisResult) -> Result<(), DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        let result_json = serde_json::to_string(result)
            .map_err(|e| DomainError::Repo(format!("Failed to serialize AnalysisResult: {}", e)))?;

        conn.execute(
            r#"
            INSERT INTO analysis_log (chat_id, week_group, analyzed_at, summary, result_json)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT (chat_id, week_group) DO UPDATE SET
                analyzed_at = excluded.analyzed_at,
                summary = excluded.summary,
                result_json = excluded.result_json
            "#,
            params![
                result.chat_id,
                result.week_group.as_str(),
                result.analyzed_at,
                result.summary.as_str(),
                result_json.as_str()
            ],
        )
        .await
        .map_err(|e| DomainError::Repo(e.to_string()))?;

        info!(
            chat_id = result.chat_id,
            week_group = %result.week_group,
            "saved analysis result"
        );

        Ok(())
    }

    async fn get_analysis(
        &self,
        chat_id: i64,
        week_group: &WeekGroup,
    ) -> Result<Option<AnalysisResult>, DomainError> {
        let conn = self
            .db
            .connect()
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        let mut rows = conn
            .query(
                "SELECT result_json FROM analysis_log WHERE chat_id = ?1 AND week_group = ?2",
                params![chat_id, week_group.as_str()],
            )
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;

        if let Some(row) = rows
            .next()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
        {
            let json_str: String = row.get(0).map_err(|e| DomainError::Repo(e.to_string()))?;
            let result: AnalysisResult = serde_json::from_str(&json_str).map_err(|e| {
                DomainError::Repo(format!("Failed to deserialize AnalysisResult: {}", e))
            })?;
            Ok(Some(result))
        } else {
            Ok(None)
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Unit Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use libsql::params;

    /// Helper: Create an in-memory database with schema for testing.
    async fn setup_test_db() -> libsql::Connection {
        let db = libsql::Builder::new_local(":memory:")
            .build()
            .await
            .expect("Failed to create in-memory db");
        let conn = db.connect().expect("Failed to connect");

        // Create required tables
        conn.execute(MESSAGES_TABLE, ()).await.unwrap();
        conn.execute(ANALYSIS_LOG_TABLE, ()).await.unwrap();

        conn
    }

    /// Insert a test message with a specific timestamp.
    async fn insert_message(
        conn: &libsql::Connection,
        chat_id: i64,
        msg_id: i32,
        timestamp: i64,
        text: &str,
    ) {
        conn.execute(
            "INSERT INTO messages (chat_id, id, date, text) VALUES (?1, ?2, ?3, ?4)",
            params![chat_id, msg_id, timestamp, text],
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_week_grouping_basic() {
        let conn = setup_test_db().await;
        let chat_id = 123i64;

        // Week 1 of 2024: Jan 1-7, 2024
        // Week 2 of 2024: Jan 8-14, 2024
        let week1_ts = 1704067200i64; // 2024-01-01 00:00:00 UTC (Monday)
        let week1_ts2 = 1704153600i64; // 2024-01-02 00:00:00 UTC (Tuesday)
        let week2_ts = 1704672000i64; // 2024-01-08 00:00:00 UTC (Monday)

        insert_message(&conn, chat_id, 1, week1_ts, "Hello Week 1").await;
        insert_message(&conn, chat_id, 2, week1_ts2, "Also Week 1").await;
        insert_message(&conn, chat_id, 3, week2_ts, "Hello Week 2").await;

        // Query for week grouping
        let mut rows = conn
            .query(
                r#"
                SELECT strftime('%Y-%W', date, 'unixepoch') as week_group, COUNT(*) as cnt
                FROM messages
                WHERE chat_id = ?1 AND text != ''
                GROUP BY week_group
                ORDER BY week_group ASC
                "#,
                params![chat_id],
            )
            .await
            .unwrap();

        let mut weeks = Vec::new();
        while let Some(row) = rows.next().await.unwrap() {
            let week: String = row.get(0).unwrap();
            let count: i64 = row.get(1).unwrap();
            weeks.push((week, count));
        }

        assert_eq!(weeks.len(), 2, "Should have 2 distinct weeks");
        assert_eq!(weeks[0].1, 2, "Week 1 should have 2 messages");
        assert_eq!(weeks[1].1, 1, "Week 2 should have 1 message");
    }

    #[tokio::test]
    async fn test_analysis_idempotency() {
        let conn = setup_test_db().await;
        let chat_id = 123i64;

        // Insert messages
        let week1_ts = 1704067200i64; // 2024-01-01 (Week 00 or 01 depending on locale)
        insert_message(&conn, chat_id, 1, week1_ts, "Hello").await;

        // Get the week group that was generated
        let mut rows = conn
            .query(
                "SELECT DISTINCT strftime('%Y-%W', date, 'unixepoch') FROM messages WHERE chat_id = ?1",
                params![chat_id],
            )
            .await
            .unwrap();
        let week_group: String = rows.next().await.unwrap().unwrap().get(0).unwrap();

        // Initially, the week should be unanalyzed
        let mut rows = conn
            .query(
                r#"
                SELECT strftime('%Y-%W', date, 'unixepoch') as week_group
                FROM messages
                WHERE chat_id = ?1
                  AND strftime('%Y-%W', date, 'unixepoch') NOT IN (
                      SELECT week_group FROM analysis_log WHERE chat_id = ?1
                  )
                GROUP BY week_group
                "#,
                params![chat_id],
            )
            .await
            .unwrap();
        let unanalyzed: Option<_> = rows.next().await.unwrap();
        assert!(unanalyzed.is_some(), "Week should be unanalyzed initially");

        // Mark the week as analyzed
        conn.execute(
            "INSERT INTO analysis_log (chat_id, week_group, analyzed_at, summary, result_json) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![chat_id, week_group.as_str(), 1704067200i64, "Test summary", "{}"],
        )
        .await
        .unwrap();

        // Now the week should NOT appear in unanalyzed list
        let mut rows = conn
            .query(
                r#"
                SELECT strftime('%Y-%W', date, 'unixepoch') as week_group
                FROM messages
                WHERE chat_id = ?1
                  AND strftime('%Y-%W', date, 'unixepoch') NOT IN (
                      SELECT week_group FROM analysis_log WHERE chat_id = ?1
                  )
                GROUP BY week_group
                "#,
                params![chat_id],
            )
            .await
            .unwrap();
        let unanalyzed_after: Option<_> = rows.next().await.unwrap();
        assert!(
            unanalyzed_after.is_none(),
            "Week should NOT appear after being analyzed"
        );
    }

    #[tokio::test]
    async fn test_service_message_filtering() {
        let conn = setup_test_db().await;
        let chat_id = 123i64;
        let ts = 1704067200i64;

        // Insert regular message
        insert_message(&conn, chat_id, 1, ts, "Hello world").await;
        // Insert service messages that should be filtered
        insert_message(&conn, chat_id, 2, ts, "User joined the group").await;
        insert_message(
            &conn,
            chat_id,
            3,
            ts,
            "Admin left the group via invite link",
        )
        .await;
        // Insert empty message
        insert_message(&conn, chat_id, 4, ts, "").await;

        // Query with filters (same as get_messages_by_week)
        let mut rows = conn
            .query(
                r#"
                SELECT COUNT(*) FROM messages
                WHERE chat_id = ?1
                  AND text != ''
                  AND text NOT LIKE '%joined the group%'
                  AND text NOT LIKE '%left the group%'
                "#,
                params![chat_id],
            )
            .await
            .unwrap();

        let count: i64 = rows.next().await.unwrap().unwrap().get(0).unwrap();
        assert_eq!(
            count, 1,
            "Only the regular message should remain after filtering"
        );
    }
}
