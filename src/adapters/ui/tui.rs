//! Implements InputPort. Inquire-based interactive prompts.
//!
//! Skeleton: prompts for chat selection and triggers sync.

use crate::domain::{ChatType, DomainError};
use crate::ports::{InputPort, TgGateway};
use crate::usecases::SyncService;
use async_trait::async_trait;
use inquire::{MultiSelect, Text};
use std::sync::Arc;

fn chat_type_indicator(kind: ChatType) -> &'static str {
    match kind {
        ChatType::Private => "[U]",
        ChatType::Group => "[G]",
        ChatType::Supergroup => "[S]",
        ChatType::Channel => "[C]",
    }
}

/// TUI adapter. Inquire prompts.
pub struct TuiInputPort {
    tg: Arc<dyn TgGateway>,
    sync_service: Arc<SyncService>,
}

impl TuiInputPort {
    pub fn new(tg: Arc<dyn TgGateway>, sync_service: Arc<SyncService>) -> Self {
        Self { tg, sync_service }
    }
}

#[async_trait]
impl InputPort for TuiInputPort {
    async fn run_sync(&self) -> Result<(), DomainError> {
        let chats = self.tg.get_dialogs().await?;
        let options: Vec<String> = chats
            .iter()
            .map(|c| format!("{} {} ({})", chat_type_indicator(c.kind), c.title, c.id))
            .collect();
        let selected = MultiSelect::new("Select chats to sync", options)
            .prompt()
            .map_err(|e| DomainError::Auth(e.to_string()))?;
        // Map selected display strings back to chat IDs (match full option string)
        let chat_ids: Vec<i64> = chats
            .iter()
            .filter(|c| {
                selected.contains(&format!(
                    "{} {} ({})",
                    chat_type_indicator(c.kind),
                    c.title,
                    c.id
                ))
            })
            .map(|c| c.id)
            .collect();
        self.sync_service.sync_chats(&chat_ids, 100).await
    }

    async fn run_auth(&self) -> Result<(), DomainError> {
        let _phone = Text::new("Phone number:")
            .prompt()
            .map_err(|e| DomainError::Auth(e.to_string()))?;
        Ok(())
    }
}
