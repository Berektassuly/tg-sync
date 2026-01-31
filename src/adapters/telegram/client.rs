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
use tokio::sync::Mutex;
use tracing::{debug, warn};

/// Telegram gateway adapter. Wraps grammers Client (shared with auth adapter via Arc).
pub struct GrammersTgGateway {
    client: Arc<Mutex<Client>>,
    /// If set, sleep this many ms before each message-history request (rate limiting).
    export_delay_ms: Option<u64>,
    /// Cache InputPeer by chat_id so we don't call iter_dialogs on every get_messages/download_media (avoids FLOOD_WAIT).
    peer_cache: Mutex<HashMap<i64, tl::enums::InputPeer>>,
}

impl GrammersTgGateway {
    /// Create gateway with shared client (Arc<Mutex<Client>>) so auth and gateway can share the same session.
    /// `export_delay_ms`: optional delay in ms before each history batch request (e.g. 500 for throttling).
    pub fn new(client: Arc<Mutex<Client>>, export_delay_ms: Option<u64>) -> Self {
        Self {
            client,
            export_delay_ms,
            peer_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Resolve chat_id to InputPeer, using cache to avoid repeated iter_dialogs (getDialogs) and FLOOD_WAIT.
    async fn resolve_input_peer(&self, chat_id: i64) -> Result<tl::enums::InputPeer, DomainError> {
        {
            let cache = self.peer_cache.lock().await;
            if let Some(peer) = cache.get(&chat_id) {
                return Ok(peer.clone());
            }
        }
        let peer = {
            let guard = self.client.lock().await;
            let mut dialogs = guard.iter_dialogs();
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
        let peer_ref = peer
            .to_ref()
            .await
            .ok_or_else(|| DomainError::TgGateway("peer not in session cache".into()))?;
        let input_peer: tl::enums::InputPeer = peer_ref.into();
        self.peer_cache
            .lock()
            .await
            .insert(chat_id, input_peer.clone());
        Ok(input_peer)
    }
}

#[async_trait]
impl TgGateway for GrammersTgGateway {
    async fn get_dialogs(&self) -> Result<Vec<Chat>, DomainError> {
        let guard = self.client.lock().await;
        let mut dialogs = guard.iter_dialogs();
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
            chats.push(Chat {
                id,
                title,
                username: peer.username().map(String::from),
                kind,
            });
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
            let guard = self.client.lock().await;
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

            match guard.invoke(&req).await {
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
                    warn!(attempt, wait_secs, "FloodWait, sleeping");
                    drop(guard);
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
        let peer = {
            let guard = self.client.lock().await;
            let mut dialogs = guard.iter_dialogs();
            let mut found = None;
            while let Some(dialog) = dialogs
                .next()
                .await
                .map_err(|e| DomainError::TgGateway(e.to_string()))?
            {
                let p = dialog.peer();
                if p.id().bot_api_dialog_id() == media_ref.chat_id {
                    found = Some(p.clone());
                    break;
                }
            }
            found.ok_or_else(|| {
                DomainError::Media(format!("peer {} not found", media_ref.chat_id))
            })?
        };

        let peer_ref = peer
            .to_ref()
            .await
            .ok_or_else(|| DomainError::Media("peer not in session cache".into()))?;

        let messages = self
            .client
            .lock()
            .await
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
            .lock()
            .await
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
}
