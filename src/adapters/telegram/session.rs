//! Session management. Load/save grammers session.
//!
//! Uses grammers-session's SqliteSession for persistent file-based storage so
//! authorization is preserved across application restarts.

use grammers_session::storages::SqliteSession;
use std::path::Path;

/// Opens a persistent session storage at the given path.
///
/// Uses SqliteSession (SQLite file) as the backing store. The file is created
/// if it does not exist. Parent directories are created as needed.
///
/// # Errors
///
/// Returns an error if the parent directory cannot be created, or if the
/// SQLite database cannot be opened (e.g. permissions, disk full).
pub async fn open_file_session(path: impl AsRef<Path>) -> anyhow::Result<SqliteSession> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|e| anyhow::anyhow!("create session directory: {}", e))?;
    }
    SqliteSession::open(path)
        .await
        .map_err(|e| anyhow::anyhow!("open session file: {}", e))
}
