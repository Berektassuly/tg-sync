//! Implements TgGateway using grammers Client.
//!
//! Handles FloodWait by sleeping and retrying. Uses raw invoke for GetHistory
//! with min_id for incremental sync.

use crate::adapters::telegram::mapper;
use crate::domain::{Chat, DomainError, MediaReference, Message};
use crate::ports::TgGateway;
use async_trait::async_trait;
use grammers_client::tl;
use grammers_client::Client;
use grammers_client::InvocationError;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, Notify};
use tracing::{debug, info, warn};

/// Audit §4.1: FloodWait threshold in seconds. Waits below this sleep; waits >= this return error.
const FLOOD_WAIT_THRESHOLD_SECS: u64 = 60;

/// Telegram gateway adapter. Wraps grammers Client (clone shared with auth adapter; no global lock).
pub struct GrammersTgGateway {
    client: Client,
    /// If set, sleep this many ms before each message-history request (rate limiting).
    export_delay_ms: Option<u64>,
    /// Audit §2.1: Cache full Peer objects by chat_id to avoid iter_dialogs on every call.
    /// Stores the Peer (not just InputPeer) so we can call to_ref() for download operations.
    peer_cache: Mutex<HashMap<i64, grammers_client::peer::Peer>>,
    /// Audit: Request coalescing (singleflight). If a key exists, a resolution is in progress;
    /// waiters clone the Notify and wait; the leader removes the entry and notifies on completion.
    inflight_requests: Mutex<HashMap<i64, Arc<Notify>>>,
}

impl GrammersTgGateway {
    /// Create gateway with a client (use same session via clone in main).
    /// `export_delay_ms`: optional delay in ms before each history batch request (e.g. 500 for throttling).
    pub fn new(client: Client, export_delay_ms: Option<u64>) -> Self {
        Self {
            client,
            export_delay_ms,
            peer_cache: Mutex::new(HashMap::new()),
            inflight_requests: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve chat_id to InputPeer, using cache to avoid repeated iter_dialogs (FLOOD_WAIT risk).
    /// Audit §2.1: Caches the full Peer object so download_media can use to_ref() later.
    /// Audit: Singleflight — only one iter_dialogs in flight per chat_id; others wait via Notify.
    async fn resolve_input_peer(&self, chat_id: i64) -> Result<tl::enums::InputPeer, DomainError> {
        loop {
            // 1. Fast path: check cache (no lock held across await)
            if let Some(peer) = self.get_cached_peer(chat_id).await {
                if let Some(peer_ref) = peer.to_ref().await {
                    return Ok(peer_ref.into());
                }
                // to_ref() failed, fall through to re-fetch
            }

            // 2. Coalescing: either wait for an in-flight resolution or become the leader
            {
                let mut inflight = self.inflight_requests.lock().await;
                if let Some(notify) = inflight.get(&chat_id) {
                    let notify = Arc::clone(notify);
                    drop(inflight);
                    notify.notified().await;
                    continue; // Re-check cache; leader may have populated it
                }
                inflight.insert(chat_id, Arc::new(Notify::new()));
            }

            // 3. We are the leader: one network request for this chat_id
            let result = self.resolve_input_peer_fetch(chat_id).await;

            // 4. Remove from inflight and wake waiters (minimal critical section)
            {
                let mut inflight = self.inflight_requests.lock().await;
                let notify = inflight.remove(&chat_id);
                drop(inflight);
                if let Some(n) = notify {
                    n.notify_waiters();
                }
            }

            return result;
        }
    }

    /// Performs the actual iter_dialogs fetch. Call only from the singleflight leader.
    async fn resolve_input_peer_fetch(
        &self,
        chat_id: i64,
    ) -> Result<tl::enums::InputPeer, DomainError> {
        let peer = {
            let mut dialogs = self.client.iter_dialogs();
            let mut found = None;
            while let Some(dialog) = dialogs
                .next()
                .await
                .map_err(|e| DomainError::TgGateway(e.to_string()))?
            {
                let p = dialog.peer();
                if p.id().bot_api_dialog_id() == chat_id {
                    found = Some(p.clone());
                    break;
                }
            }
            found.ok_or_else(|| {
                DomainError::TgGateway(format!("peer {} not found in dialogs", chat_id))
            })?
        };

        self.peer_cache.lock().await.insert(chat_id, peer.clone());

        let peer_ref = peer
            .to_ref()
            .await
            .ok_or_else(|| DomainError::TgGateway("peer not in session cache".into()))?;
        Ok(peer_ref.into())
    }

    /// Audit §2.1: Get cached Peer for PeerRef conversion. Avoids dialog re-iteration in download_media.
    /// Returns None if not cached; caller should call resolve_input_peer first to populate cache.
    async fn get_cached_peer(&self, chat_id: i64) -> Option<grammers_client::peer::Peer> {
        self.peer_cache.lock().await.get(&chat_id).cloned()
    }
}

#[async_trait]
impl TgGateway for GrammersTgGateway {
    async fn get_dialogs(&self) -> Result<Vec<Chat>, DomainError> {
        let mut dialogs = self.client.iter_dialogs();
        let mut chats = Vec::new();
        while let Some(dialog) = dialogs
            .next()
            .await
            .map_err(|e| DomainError::TgGateway(e.to_string()))?
        {
            let peer = dialog.peer();
            let id = peer.id().bot_api_dialog_id();
            let title = peer
                .name()
                .map(String::from)
                .unwrap_or_else(|| peer.id().to_string());
            let kind = mapper::chat_type_from_peer(peer);
            let approx_message_count = dialog.last_message.as_ref().map(|m| m.id());
            chats.push(mapper::dialog_to_chat(
                id,
                &title,
                peer.username().as_deref(),
                kind,
                approx_message_count,
            ));
        }
        Ok(chats)
    }

    async fn get_messages(
        &self,
        chat_id: i64,
        min_id: i32,
        max_id: i32,
        limit: i32,
    ) -> Result<Vec<Message>, DomainError> {
        use tl::enums::messages::Messages;

        if let Some(ms) = self.export_delay_ms {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }

        let input_peer = self.resolve_input_peer(chat_id).await?;

        // When max_id > 0 we're paginating backward (older messages). Telegram requires
        // offset_id = max_id so the API returns the next page starting from that message.
        // With offset_id = 0 we'd get the newest page again and filtering by max_id yields empty.
        let offset_id = if max_id > 0 { max_id } else { 0 };

        for attempt in 0..3 {
            let req = tl::functions::messages::GetHistory {
                peer: input_peer.clone(),
                offset_id,
                offset_date: 0,
                add_offset: 0,
                limit,
                max_id,
                min_id,
                hash: 0,
            };

            match self.client.invoke(&req).await {
                Ok(raw) => {
                    let (messages, _users, _chats) = match raw {
                        Messages::Messages(m) => (m.messages, m.users, m.chats),
                        Messages::Slice(m) => (m.messages, m.users, m.chats),
                        Messages::ChannelMessages(m) => (m.messages, m.users, m.chats),
                        Messages::NotModified(_) => return Ok(vec![]),
                    };
                    let mut out = Vec::new();
                    for msg in messages {
                        if let Some((m, _)) = mapper::message_to_domain(&msg, chat_id) {
                            out.push(m);
                        }
                    }
                    return Ok(out);
                }
                Err(InvocationError::Rpc(rpc)) if rpc.code == 420 => {
                    let wait_secs = rpc.value.unwrap_or(60) as u64;
                    // Audit §4.1: Long waits (≥60s) should not block the worker thread.
                    // Return error so caller (job scheduler) can reschedule.
                    if wait_secs >= FLOOD_WAIT_THRESHOLD_SECS {
                        info!(
                            attempt,
                            wait_secs,
                            threshold = FLOOD_WAIT_THRESHOLD_SECS,
                            "FloodWait exceeds threshold, returning error for rescheduling"
                        );
                        return Err(DomainError::FloodWait { seconds: wait_secs });
                    }
                    warn!(attempt, wait_secs, "FloodWait (short), sleeping");
                    tokio::time::sleep(Duration::from_secs(wait_secs)).await;
                }
                Err(e) => return Err(DomainError::TgGateway(e.to_string())),
            }
        }
        Err(DomainError::TgGateway("FloodWait max retries".into()))
    }

    async fn download_media(
        &self,
        media_ref: &MediaReference,
        dest_path: &Path,
    ) -> Result<(), DomainError> {
        // Audit §2.1: First ensure peer is cached via resolve_input_peer.
        // This populates the peer_cache if not already present.
        let _ = self
            .resolve_input_peer(media_ref.chat_id)
            .await
            .map_err(|e| DomainError::Media(format!("peer resolution failed: {}", e)))?;

        // Audit §2.1: Use cached Peer to get PeerRef without re-iterating dialogs.
        // This avoids the FloodWait risk from repeated getDialogs calls.
        let peer = self
            .get_cached_peer(media_ref.chat_id)
            .await
            .ok_or_else(|| {
                DomainError::Media(format!(
                    "peer {} not in cache after resolve",
                    media_ref.chat_id
                ))
            })?;

        let peer_ref = peer
            .to_ref()
            .await
            .ok_or_else(|| DomainError::Media("peer not in session cache".into()))?;

        let messages = self
            .client
            .get_messages_by_id(peer_ref, &[media_ref.message_id])
            .await
            .map_err(|e| DomainError::Media(e.to_string()))?;

        let msg = messages
            .into_iter()
            .next()
            .and_then(|o| o)
            .ok_or_else(|| DomainError::Media("message not found".into()))?;

        let media = msg
            .media()
            .ok_or_else(|| DomainError::Media("message has no media".into()))?;

        self.client
            .download_media(&media, dest_path)
            .await
            .map_err(|e| DomainError::Media(e.to_string()))?;

        debug!(
            chat_id = media_ref.chat_id,
            msg_id = media_ref.message_id,
            path = %dest_path.display(),
            "media downloaded"
        );
        Ok(())
    }

    async fn get_me_id(&self) -> Result<i64, DomainError> {
        let me = self
            .client
            .get_me()
            .await
            .map_err(|e| DomainError::TgGateway(e.to_string()))?;
        Ok(me.id().bot_api_dialog_id())
    }

    async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), DomainError> {
        self.resolve_input_peer(chat_id).await?;
        let peer = self
            .get_cached_peer(chat_id)
            .await
            .ok_or_else(|| DomainError::TgGateway("peer not in cache after resolve".into()))?;
        let peer_ref = peer
            .to_ref()
            .await
            .ok_or_else(|| DomainError::TgGateway("peer not in session cache".into()))?;
        self.client
            .send_message(peer_ref, text)
            .await
            .map_err(|e| DomainError::TgGateway(e.to_string()))?;
        Ok(())
    }
}
