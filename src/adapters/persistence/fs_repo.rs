//! Implements RepoPort. Saves messages as JSON Lines (JSONL) per chat.
//! One file per chat: data/{chat_id}.jsonl. Append-only writes; line-by-line reads with pagination.

use crate::domain::{DomainError, Message};
use crate::ports::RepoPort;
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::info;

/// File-system repository. One JSONL file per chat (one JSON object per line).
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
        self.base_dir.join(format!("{}.jsonl", chat_id))
    }
}

#[async_trait::async_trait]
impl RepoPort for FsRepo {
    /// Appends messages as one JSON object per line. Does not read the existing file.
    async fn save_messages(&self, chat_id: i64, messages: &[Message]) -> Result<(), DomainError> {
        if messages.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(&self.base_dir)
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let path = self.chat_path(chat_id);
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        for m in messages {
            let line = serde_json::to_string(m).map_err(|e| DomainError::Repo(e.to_string()))?;
            f.write_all(line.as_bytes())
                .await
                .map_err(|e| DomainError::Repo(e.to_string()))?;
            f.write_all(b"\n")
                .await
                .map_err(|e| DomainError::Repo(e.to_string()))?;
        }
        f.flush()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let abs_path = path.canonicalize().unwrap_or_else(|_| path.clone());
        info!(
            path = %abs_path.display(),
            chat_id,
            count = messages.len(),
            "saved messages to disk (JSONL)"
        );
        Ok(())
    }

    /// Reads messages line-by-line with pagination. Returns newest first; deduplicates by message id (keeps last occurrence).
    async fn get_messages(
        &self,
        chat_id: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Message>, DomainError> {
        let path = self.chat_path(chat_id);
        let f = match fs::File::open(&path).await {
            Ok(file) => file,
            Err(_) => return Ok(vec![]),
        };
        let mut reader = BufReader::new(f).lines();

        // First pass: count lines without loading into memory
        let mut total: usize = 0;
        while reader
            .next_line()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
            .is_some()
        {
            total += 1;
        }

        if total == 0 {
            return Ok(vec![]);
        }

        // Newest-first window: file order is oldest first, so indices [total-1-offset-limit+1 .. total-offset] in 0-based (inclusive start, exclusive end)
        let end = total.saturating_sub(offset as usize);
        let start = total.saturating_sub((offset as usize).saturating_add(limit as usize));
        if start >= end {
            return Ok(vec![]);
        }
        let take_count = end - start;

        // Second pass: skip to start, take take_count lines, parse and dedupe
        let f = fs::File::open(&path)
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?;
        let mut reader = BufReader::new(f).lines();
        let mut skip = start;
        while skip > 0 {
            match reader
                .next_line()
                .await
                .map_err(|e| DomainError::Repo(e.to_string()))?
            {
                Some(_) => skip -= 1,
                None => return Ok(vec![]),
            }
        }
        let mut by_id: HashMap<i32, Message> = HashMap::with_capacity(take_count);
        for _ in 0..take_count {
            let line = match reader
                .next_line()
                .await
                .map_err(|e| DomainError::Repo(e.to_string()))?
            {
                Some(l) => l,
                None => break,
            };
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Message>(trimmed) {
                Ok(m) => {
                    by_id.insert(m.id, m);
                }
                Err(_) => continue,
            }
        }
        let mut out: Vec<Message> = by_id.into_values().collect();
        out.sort_by(|a, b| b.date.cmp(&a.date));
        Ok(out)
    }
}
