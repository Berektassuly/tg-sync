//! Main sync logic: fetch dialogs -> filter -> incremental download -> save.
//!
//! - Forward history filling (oldest -> newest): starts from last_message_id (or 0)
//!   and paginates until the top of the chat (API returns newest-first; we iterate
//!   by setting max_id = batch_min to fetch older chunks).
//! - **Strict client-side boundary enforcement:** We do not trust the Telegram API to
//!   honour min_id/max_id when offset_id is present. All boundary checks and loop
//!   termination are performed client-side; batches are filtered before processing.
//! - Sends media refs to bounded mpsc channel for async download; send().await provides backpressure when queue is full.
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
    media_tx: mpsc::Sender<MediaReference>,
    /// Delay between message batch requests to avoid FLOOD_WAIT.
    delay: Duration,
}

impl SyncService {
    pub fn new(
        tg: Arc<dyn TgGateway>,
        repo: Arc<dyn RepoPort>,
        state: Arc<dyn StatePort>,
        media_tx: mpsc::Sender<MediaReference>,
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
    /// Forward history filling: paginates from newest down to oldest. Loop termination
    /// is client-side: we break when we see any message with id <= min_id, not when the
    /// API returns empty (API may ignore min_id/max_id). Batches are filtered to the
    /// requested range before processing. If `include_media` is false, message text is
    /// saved but media files are not downloaded.
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
        let mut channel_closed = false;

        loop {
            if channel_closed {
                break;
            }

            let raw = self.tg.get_messages(chat_id, min_id, max_id, limit).await?;

            // Do not use empty list as termination signal: API may ignore min_id/max_id and
            // return out-of-range messages; we enforce boundaries client-side.
            if raw.is_empty() {
                break;
            }

            // Manual termination: when fetching backwards (max_id > 0), the server may return
            // messages with id <= min_id. We must stop as soon as we see one, even if the
            // batch is full, to avoid infinite re-fetching or corrupting state.
            let reached_min = raw.iter().any(|m| m.id <= min_id);
            let raw_min_id = raw.iter().map(|m| m.id).min();

            // Batch filtering: drop any message outside the requested range so we never
            // persist out-of-scope or duplicate data. Handles mixed batches where the
            // boundary was crossed in the middle of a page.
            let mut messages: Vec<_> = raw
                .into_iter()
                .filter(|m| {
                    let above_min = m.id > min_id;
                    let below_max = max_id == 0 || m.id < max_id;
                    above_min && below_max
                })
                .collect();

            if !messages.is_empty() {
                // Process in forward order (oldest -> newest) for consistent history filling
                messages.sort_by_key(|m| m.id);
                let batch_min = messages.iter().map(|m| m.id).min().unwrap_or(0);
                let batch_max = messages
                    .iter()
                    .max_by_key(|m| m.id)
                    .map(|m| m.id)
                    .unwrap_or(0);

                // Queue media refs for download. BACKPRESSURE: send().await yields here when the
                // channel is full; the producer (sync) is thus rate-limited by the consumer (media
                // worker / disk), preventing unbounded buffer growth and OOM.
                if include_media {
                    for msg in &messages {
                        if let Some(ref m) = msg.media {
                            match self.media_tx.send(m.clone()).await {
                                Ok(()) => total_media_queued += 1,
                                Err(_) => {
                                    // Receiver dropped (e.g. media worker exited); exit loop cleanly.
                                    warn!(
                                        chat_id,
                                        msg_id = msg.id,
                                        "media channel closed, stopping media queue for this chat"
                                    );
                                    channel_closed = true;
                                    break;
                                }
                            }
                        }
                    }
                }
                // When include_media is false, messages are saved but media is not queued for download

                // Save batch (repo merges and sorts by id). Only in-range messages reach here.
                self.repo.save_messages(chat_id, &messages).await?;

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

                if reached_min {
                    // Client-side termination: we saw id <= min_id; stop even if we processed valid messages.
                    break;
                }
                max_id = batch_min;
            } else {
                // Filtered to empty: either we crossed the lower bound or server sent only out-of-range ids.
                if reached_min {
                    break;
                }
                // Avoid infinite loop: advance cursor past this page so next request differs.
                max_id = raw_min_id.unwrap_or(max_id);
            }

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
