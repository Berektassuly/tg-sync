//! Main sync logic: fetch dialogs -> filter -> incremental download -> save.
//!
//! - Verifies `last_message_id` from StatePort
//! - Uses `min_id` to fetch ONLY new messages
//! - Sends media refs to mpsc channel for async download (non-blocking)
//! - Updates state only after successful save

use crate::domain::{DomainError, MediaReference};
use crate::ports::{RepoPort, StatePort, TgGateway};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Sync service. Coordinates incremental text sync and media pipeline.
pub struct SyncService {
    tg: Arc<dyn TgGateway>,
    repo: Arc<dyn RepoPort>,
    state: Arc<dyn StatePort>,
    media_tx: mpsc::UnboundedSender<MediaReference>,
}

impl SyncService {
    pub fn new(
        tg: Arc<dyn TgGateway>,
        repo: Arc<dyn RepoPort>,
        state: Arc<dyn StatePort>,
        media_tx: mpsc::UnboundedSender<MediaReference>,
    ) -> Self {
        Self {
            tg,
            repo,
            state,
            media_tx,
        }
    }

    /// Sync a single chat. Fetches all new messages (id > last_message_id) via pagination.
    pub async fn sync_chat(&self, chat_id: i64, limit: i32) -> Result<SyncStats, DomainError> {
        let last_id = self.state.get_last_message_id(chat_id).await?;
        let min_id = last_id;
        let mut max_id = 0i32; // 0 = no upper bound; we paginate by setting max_id to min(batch)

        let mut total_synced = 0usize;
        let mut total_media_queued = 0usize;
        let mut latest_max_id = last_id;

        loop {
            let mut messages = self
                .tg
                .get_messages(chat_id, min_id, max_id, limit)
                .await?;

            // Defensive: only keep messages within our range (API may return boundary)
            messages.retain(|m| m.id > last_id && (max_id == 0 || m.id < max_id));

            if messages.is_empty() {
                break;
            }

            // Extract media refs and queue for download (non-blocking)
            for msg in &messages {
                if let Some(ref m) = msg.media {
                    if self.media_tx.send(m.clone()).is_ok() {
                        total_media_queued += 1;
                    } else {
                        warn!(
                            chat_id,
                            msg_id = msg.id,
                            "media channel closed, dropping ref"
                        );
                    }
                }
            }

            // Save batch (repo merges and sorts by id)
            self.repo.save_messages(chat_id, &messages).await?;

            let batch_max = messages.iter().map(|m| m.id).max().unwrap_or(0);
            let batch_min = messages.iter().map(|m| m.id).min().unwrap_or(0);
            total_synced += messages.len();
            latest_max_id = latest_max_id.max(batch_max);

            // Cursor for next iteration: fetch older messages (id < batch_min)
            max_id = batch_min;

            if messages.len() < limit as usize {
                break;
            }

            // Small delay between batches to avoid aggressive rate limits
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }

        if total_synced > 0 {
            self.state.set_last_message_id(chat_id, latest_max_id).await?;
            info!(
                chat_id,
                count = total_synced,
                media_queued = total_media_queued,
                last_id = latest_max_id,
                "synced messages"
            );
        }

        Ok(SyncStats {
            messages_synced: total_synced,
            media_queued: total_media_queued,
        })
    }

    /// Sync multiple chats. Runs sequentially to respect rate limits.
    pub async fn sync_chats(
        &self,
        chat_ids: &[i64],
        limit_per_chat: i32,
    ) -> Result<(), DomainError> {
        for &chat_id in chat_ids {
            self.sync_chat(chat_id, limit_per_chat).await?;
        }
        Ok(())
    }
}

/// Result of a single chat sync.
#[derive(Debug, Default)]
pub struct SyncStats {
    pub messages_synced: usize,
    pub media_queued: usize,
}
