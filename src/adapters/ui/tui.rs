//! Implements InputPort. Inquire-based interactive prompts.
//!
//! Cyberpunk/Neon theme: prompt prefix [?], colored ChatType indicators.

use crate::domain::{ChatType, DomainError};
use crate::ports::{InputPort, TgGateway};
use crate::usecases::SyncService;
use async_trait::async_trait;
use inquire::ui::{Color, RenderConfig, StyleSheet, Styled};
use inquire::{set_global_render_config, Confirm, MultiSelect, Text};
use std::sync::Arc;

/// Neon Purple (#bc13fe) for prompt prefix and accents.
const NEON_PURPLE: Color = Color::Rgb {
    r: 0xbc,
    g: 0x13,
    b: 0xfe,
};
/// Cyber Green (#0ff0fc) for prompts and help.
const CYBER_GREEN: Color = Color::Rgb {
    r: 0x0f,
    g: 0xf0,
    b: 0xfc,
};
/// Yellow for Channel indicator.
const CHANNEL_YELLOW: (u8, u8, u8) = (255, 255, 0);
/// Cyan for User (Private) indicator.
const USER_CYAN: (u8, u8, u8) = (0, 255, 255);
/// Green for Group/Supergroup.
const GROUP_GREEN: (u8, u8, u8) = (0, 255, 128);

fn ansi_rgb(r: u8, g: u8, b: u8) -> String {
    format!("\x1b[38;2;{};{};{}m", r, g, b)
}

const RESET: &str = "\x1b[0m";

/// Returns the ChatType indicator with ANSI color: [U] cyan, [G]/[S] green, [C] yellow.
fn chat_type_indicator(kind: ChatType) -> String {
    let (tag, r, g, b) = match kind {
        ChatType::Private => ("[U]", USER_CYAN.0, USER_CYAN.1, USER_CYAN.2),
        ChatType::Group => ("[G]", GROUP_GREEN.0, GROUP_GREEN.1, GROUP_GREEN.2),
        ChatType::Supergroup => ("[S]", GROUP_GREEN.0, GROUP_GREEN.1, GROUP_GREEN.2),
        ChatType::Channel => ("[C]", CHANNEL_YELLOW.0, CHANNEL_YELLOW.1, CHANNEL_YELLOW.2),
    };
    format!("{}{}{}", ansi_rgb(r, g, b), tag, RESET)
}

/// Applies the global Cyberpunk/Neon RenderConfig for inquire prompts.
pub(crate) fn apply_theme() {
    let config = RenderConfig::default_colored()
        .with_prompt_prefix(Styled::new("[?] ").with_fg(NEON_PURPLE))
        .with_answered_prompt_prefix(Styled::new("tg-archiver> ").with_fg(NEON_PURPLE))
        .with_help_message(StyleSheet::default().with_fg(CYBER_GREEN))
        .with_option(StyleSheet::default().with_fg(Color::White))
        .with_highlighted_option_prefix(Styled::new("❯ ").with_fg(NEON_PURPLE))
        .with_selected_checkbox(Styled::new("◉").with_fg(CYBER_GREEN))
        .with_unselected_checkbox(Styled::new("○").with_fg(Color::DarkGrey));
    set_global_render_config(config);
}

/// TUI adapter. Inquire prompts with neon theme.
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

        let include_media = Confirm::new("Download media files?")
            .with_default(true)
            .with_help_message("Photos, videos, documents. Press Enter for Yes.")
            .prompt()
            .map_err(|e| DomainError::Auth(e.to_string()))?;

        self.sync_service
            .sync_chats(&chat_ids, 100, include_media)
            .await
    }

    async fn run_auth(&self) -> Result<(), DomainError> {
        let _phone = Text::new("Phone number:")
            .prompt()
            .map_err(|e| DomainError::Auth(e.to_string()))?;
        Ok(())
    }
}
