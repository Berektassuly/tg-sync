//! Main sync logic: fetch dialogs -> filter -> incremental download -> save.
//!
//! - Forward history filling (oldest -> newest): starts from last_message_id (or 0)
//!   and paginates until the top of the chat (API returns newest-first; we iterate
//!   by setting max_id = batch_min to fetch older chunks).
//! - Sends media refs to mpsc channel for async download (non-blocking)
//! - Updates state only after successful save
//! - Configurable delay between batches (SYNC_DELAY_MS) to avoid FLOOD_WAIT

use crate::domain::{DomainError, MediaReference};
use crate::ports::{RepoPort, StatePort, TgGateway};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{info, warn};

/// Sync service. Coordinates incremental text sync and media pipeline.
pub struct SyncService {
    tg: Arc<dyn TgGateway>,
    repo: Arc<dyn RepoPort>,
    state: Arc<dyn StatePort>,
    media_tx: mpsc::UnboundedSender<MediaReference>,
    /// Delay between message batch requests to avoid FLOOD_WAIT.
    delay: Duration,
}

impl SyncService {
    pub fn new(
        tg: Arc<dyn TgGateway>,
        repo: Arc<dyn RepoPort>,
        state: Arc<dyn StatePort>,
        media_tx: mpsc::UnboundedSender<MediaReference>,
        delay: Duration,
    ) -> Self {
        Self {
            tg,
            repo,
            state,
            media_tx,
            delay,
        }
    }

    /// Sync a single chat. Fetches all new messages (id > last_message_id) via pagination.
    /// Forward history filling: paginates from newest down to oldest until the API
    /// returns an empty list; processes each batch in ascending id order (oldest -> newest).
    /// If `include_media` is false, message text is saved but media files are not downloaded.
    pub async fn sync_chat(
        &self,
        chat_id: i64,
        limit: i32,
        include_media: bool,
    ) -> Result<SyncStats, DomainError> {
        let last_known_id = self.state.get_last_message_id(chat_id).await?;
        let min_id = last_known_id;
        let mut max_id = 0i32; // 0 = no upper bound; we set max_id = batch_min to fetch older chunks

        let mut total_synced = 0usize;
        let mut total_media_queued = 0usize;
        let mut current_head_id = last_known_id;

        loop {
            let mut messages = self.tg.get_messages(chat_id, min_id, max_id, limit).await?;

            // Defensive: only keep messages within our range (API may return boundary)
            messages.retain(|m| m.id > last_known_id && (max_id == 0 || m.id < max_id));

            if messages.is_empty() {
                break;
            }

            // Process in forward order (oldest -> newest) for consistent history filling
            messages.sort_by_key(|m| m.id);

            // Extract media refs and queue for download (non-blocking) when enabled
            if include_media {
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
            }
            // When include_media is false, messages are saved but media is not queued for download

            // Save batch (repo merges and sorts by id)
            self.repo.save_messages(chat_id, &messages).await?;

            let batch_max = messages.iter().map(|m| m.id).max().unwrap_or(0);
            let batch_min = messages.iter().map(|m| m.id).min().unwrap_or(0);

            // Persist checkpoint immediately so interrupted syncs can resume from this batch
            self.state.set_last_message_id(chat_id, batch_max).await?;

            total_synced += messages.len();
            current_head_id = current_head_id.max(batch_max);

            info!(
                chat_id,
                batch_size = messages.len(),
                batch_id_range = %format!("{}..{}", batch_min, batch_max),
                checkpoint = batch_max,
                "batch saved, checkpoint advanced"
            );

            // Cursor for next iteration: fetch older messages (id < batch_min)
            max_id = batch_min;

            // Rate limit: delay before next batch to avoid FLOOD_WAIT
            tokio::time::sleep(self.delay).await;
        }

        if total_synced > 0 {
            info!(
                chat_id,
                count = total_synced,
                media_queued = total_media_queued,
                last_id = current_head_id,
                "sync completed"
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
        include_media: bool,
    ) -> Result<(), DomainError> {
        if !include_media {
            info!("Skipping media download due to user preference (text-only mode)");
        }
        for &chat_id in chat_ids {
            self.sync_chat(chat_id, limit_per_chat, include_media)
                .await?;
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
