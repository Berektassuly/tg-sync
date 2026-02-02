//! Implements RepoPort. Saves messages as JSON Lines (JSONL) per chat.
//! One file per chat: data/{chat_id}.jsonl. Append-only writes; line-by-line reads with pagination.
//! Newest-first reads use reverse block scanning from EOF for O(k) performance.

use crate::domain::{DomainError, Message};
use crate::ports::RepoPort;
use std::collections::HashMap;
use std::io::{ErrorKind, SeekFrom};
use std::path::Path;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tracing::info;

/// Block size for reverse reads. Tune for disk/SSD; 4KB is a reasonable default.
const REVERSE_READ_BLOCK: u64 = 4096;

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

    /// Reads up to `max_lines` lines from the end of the file (newest first) by scanning
    /// backwards in fixed-size blocks. O(k) in the number of lines read; does not scan the whole file.
    async fn read_lines_reverse(
        path: &std::path::Path,
        max_lines: usize,
    ) -> Result<Vec<String>, DomainError> {
        let mut f = match fs::File::open(path).await {
            Ok(file) => file,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(vec![]),
            Err(e) => return Err(DomainError::Repo(e.to_string())),
        };
        let len = f
            .metadata()
            .await
            .map_err(|e| DomainError::Repo(e.to_string()))?
            .len();
        if len == 0 {
            return Ok(vec![]);
        }

        let mut lines: Vec<String> = Vec::with_capacity(max_lines.min(1024));
        let mut pending: Vec<u8> = Vec::new();
        let mut pos = len;

        while lines.len() < max_lines && pos > 0 {
            let read_start = pos.saturating_sub(REVERSE_READ_BLOCK);
            let to_read = (pos - read_start) as usize;

            f.seek(SeekFrom::Start(read_start))
                .await
                .map_err(|e| DomainError::Repo(e.to_string()))?;
            let mut block = vec![0u8; to_read];
            f.read_exact(&mut block)
                .await
                .map_err(|e| DomainError::Repo(e.to_string()))?;
            pos = read_start;

            // File order: block (just read, nearer BOF) then pending (nearer EOF)
            let mut buf = block;
            buf.extend(pending.drain(..));

            while lines.len() < max_lines {
                if let Some(last_nl) = buf.iter().rposition(|&b| b == b'\n') {
                    let line_bytes = buf.split_off(last_nl + 1);
                    buf.pop(); // drop the \n
                    let line = String::from_utf8_lossy(&line_bytes).into_owned();
                    lines.push(line);
                } else {
                    break;
                }
            }
            pending = buf;
        }

        if lines.len() < max_lines && !pending.is_empty() {
            let line = String::from_utf8_lossy(&pending).into_owned();
            lines.push(line);
        }

        Ok(lines)
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

    /// Reads messages by scanning backwards from EOF. O(k) in lines read; no full-file scan.
    /// Returns newest first; deduplicates by message id (keeps last occurrence = newest).
    async fn get_messages(
        &self,
        chat_id: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<Message>, DomainError> {
        let path = self.chat_path(chat_id);
        let need = (offset as usize).saturating_add(limit as usize);
        if need == 0 {
            return Ok(vec![]);
        }
        let lines = Self::read_lines_reverse(&path, need).await?;
        if lines.is_empty() {
            return Ok(vec![]);
        }

        let window = lines
            .iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect::<Vec<_>>();

        let mut by_id: HashMap<i32, Message> = HashMap::with_capacity(window.len());
        for line in window {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Ok(m) = serde_json::from_str::<Message>(trimmed) {
                by_id.insert(m.id, m);
            }
        }
        let mut out: Vec<Message> = by_id.into_values().collect();
        out.sort_by(|a, b| b.date.cmp(&a.date));
        Ok(out)
    }
}
