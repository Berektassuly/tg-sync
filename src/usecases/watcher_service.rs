//! Watcher (Daemon) use case: sync target chats periodically and notify via Saved Messages when keywords are found.
//!
//! Orchestrates SyncService, RepoPort, and TgGateway. Does not block the main thread; uses tokio::time::sleep.

use crate::domain::DomainError;
use crate::ports::{RepoPort, TgGateway};
use crate::usecases::sync_service::SyncService;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Hardcoded keywords (case-insensitive match). Notify when any new message contains one of these.
const KEYWORDS: &[&str] = &["Urgent", "Bug", "Error", "Production"];

/// Watcher service. Runs a loop: sync target chats -> check new messages for keywords -> notify to Saved Messages -> sleep.
pub struct WatcherService {
    tg: Arc<dyn TgGateway>,
    repo: Arc<dyn RepoPort>,
    sync_service: Arc<SyncService>,
    /// Sleep duration between cycles.
    cycle_sleep: Duration,
}

impl WatcherService {
    pub fn new(
        tg: Arc<dyn TgGateway>,
        repo: Arc<dyn RepoPort>,
        sync_service: Arc<SyncService>,
        cycle_sleep: Duration,
    ) -> Self {
        Self {
            tg,
            repo,
            sync_service,
            cycle_sleep,
        }
    }

    /// Run the watcher loop. Iterates target chats, syncs, checks for keywords, notifies, then sleeps.
    /// Call this from the Watcher menu branch; it runs until the user stops the process.
    pub async fn run_loop(&self) -> Result<(), DomainError> {
        let me_id = self.tg.get_me_id().await?;
        info!(
            me_id,
            "Watcher started; notifications will go to Saved Messages"
        );

        loop {
            let target_ids = self.repo.get_target_ids().await?;
            if target_ids.is_empty() {
                info!("No target chats; sleeping until next cycle");
                tokio::time::sleep(self.cycle_sleep).await;
                continue;
            }

            let chat_titles = self.chat_id_to_title_map(&target_ids).await?;

            for &chat_id in &target_ids {
                if let Err(e) = self
                    .sync_and_notify_keywords(
                        chat_id,
                        me_id,
                        chat_titles.get(&chat_id).map(|s| s.as_str()),
                    )
                    .await
                {
                    warn!(chat_id, error = %e, "Watcher sync/notify failed for chat");
                }
            }

            info!(
                cycle_secs = self.cycle_sleep.as_secs(),
                "Cycle complete; sleeping"
            );
            tokio::time::sleep(self.cycle_sleep).await;
        }
    }

    /// Build a map chat_id -> title for the given ids (from get_dialogs).
    async fn chat_id_to_title_map(
        &self,
        target_ids: &std::collections::HashSet<i64>,
    ) -> Result<HashMap<i64, String>, DomainError> {
        let dialogs = self.tg.get_dialogs().await?;
        let mut map = HashMap::new();
        for chat in dialogs {
            if target_ids.contains(&chat.id) {
                map.insert(chat.id, chat.title);
            }
        }
        Ok(map)
    }

    /// Sync one chat (text-only), then load newly synced messages, check keywords, and send alerts to Saved Messages.
    async fn sync_and_notify_keywords(
        &self,
        chat_id: i64,
        saved_messages_id: i64,
        chat_title: Option<&str>,
    ) -> Result<(), DomainError> {
        let stats = self.sync_service.sync_chat(chat_id, 100, false).await?;

        if stats.messages_synced == 0 {
            return Ok(());
        }

        let new_messages = self
            .repo
            .get_messages(chat_id, stats.messages_synced as u32, 0)
            .await?;

        let fallback = chat_id.to_string();
        let title = chat_title.unwrap_or(&fallback);

        for msg in &new_messages {
            if let Some(keyword) = find_keyword(&msg.text) {
                let alert = format!(
                    "[ALERT] Keyword '{}' found in chat '{}': {}",
                    keyword,
                    title,
                    truncate_message(&msg.text)
                );
                if let Err(e) = self.tg.send_message(saved_messages_id, &alert).await {
                    warn!(chat_id, error = %e, "Failed to send alert to Saved Messages");
                } else {
                    info!(chat_id, keyword, "Alert sent to Saved Messages");
                }
            }
        }

        Ok(())
    }
}

/// Returns the first matching keyword (case-insensitive) in `text`, or None.
fn find_keyword(text: &str) -> Option<&'static str> {
    let lower = text.to_lowercase();
    KEYWORDS
        .iter()
        .find(|k| lower.contains(&k.to_lowercase()))
        .copied()
}

/// Truncate message text for the alert to avoid overly long notifications.
fn truncate_message(text: &str) -> String {
    const MAX: usize = 200;
    let t = text.trim();
    if t.len() <= MAX {
        t.to_string()
    } else {
        format!("{}...", &t[..MAX])
    }
}
