//! Implements RepoPort. Saves messages as JSON per chat (append/merge).

use crate::domain::{DomainError, Message};
use crate::ports::RepoPort;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// File-system repository. One JSON file per chat.
pub struct FsRepo {
    base_dir: std::path::PathBuf,
}

impl FsRepo {
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        Self {
            base_dir: base_dir.as_ref().to_path_buf(),
        }
    }

    fn chat_path(&self, chat_id: i64) -> std::path::PathBuf {
        self.base_dir.join(format!("{}.json", chat_id))
    }
}

#[async_trait::async_trait]
impl RepoPort for FsRepo {
    async fn save_messages(&self, chat_id: i64, messages: &[Message]) -> Result<(), DomainError> {
        if messages.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let path = self.chat_path(chat_id);
        let mut existing: Vec<Message> = match fs::read_to_string(&path).await {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => vec![],
        };
        let existing_ids: std::collections::HashSet<i32> = existing.iter().map(|m| m.id).collect();
        for m in messages {
            if !existing_ids.contains(&m.id) {
                existing.push(m.clone());
            }
        }
        existing.sort_by_key(|m| m.id);
        let json = serde_json::to_string_pretty(&existing)
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let mut f = fs::File::create(&path)
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        f.write_all(json.as_bytes())
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        Ok(())
    }
}
