//! Implements InputPort. Inquire-based interactive prompts.
//!
//! Cyberpunk/Neon theme: prompt prefix [?], colored ChatType indicators.

use crate::domain::{Chat, ChatType, DomainError};
use crate::ports::{InputPort, RepoPort, TgGateway};
use crate::usecases::{SyncService, WatcherService};
use async_trait::async_trait;
use inquire::ui::{Color, RenderConfig, StyleSheet, Styled};
use inquire::{set_global_render_config, Confirm, CustomType, MultiSelect, Select, Text};
use std::collections::HashSet;
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
    repo: Arc<dyn RepoPort>,
    sync_service: Arc<SyncService>,
    watcher_service: Arc<WatcherService>,
}

impl TuiInputPort {
    pub fn new(
        tg: Arc<dyn TgGateway>,
        repo: Arc<dyn RepoPort>,
        sync_service: Arc<SyncService>,
        watcher_service: Arc<WatcherService>,
    ) -> Self {
        Self {
            tg,
            repo,
            sync_service,
            watcher_service,
        }
    }
}

#[async_trait]
impl InputPort for TuiInputPort {
    async fn run(&self) -> Result<(), DomainError> {
        let options = vec![
            "Full Backup".to_string(),
            "Manage Blacklist (exclude chats from backup)".to_string(),
            "Watcher / Daemon".to_string(),
            "AI Analysis".to_string(),
        ];
        let choice = Select::new("Select mode", options.clone())
            .prompt()
            .map_err(|e| DomainError::Auth(e.to_string()))?;

        match choice.as_str() {
            "Full Backup" => self.run_sync().await,
            "Manage Blacklist (exclude chats from backup)" => self.run_manage_blacklist().await,
            "Watcher / Daemon" => self.run_watcher().await,
            "AI Analysis" => {
                println!("Coming soon");
                Ok(())
            }
            _ => Ok(()),
        }
    }

    async fn run_sync(&self) -> Result<(), DomainError> {
        // Full Backup flow: dialogs -> filter by stored blacklist -> sync (no blacklist UI here).
        let chats = self.tg.get_dialogs().await?;
        if chats.is_empty() {
            println!("No dialogs found.");
            return Ok(());
        }

        let blacklisted_ids = self.repo.get_blacklisted_ids().await?;
        let allowed: Vec<Chat> = chats
            .iter()
            .filter(|c| !blacklisted_ids.contains(&c.id))
            .cloned()
            .collect();
        let allowed_ids: Vec<i64> = allowed.iter().map(|c| c.id).collect();

        if allowed_ids.is_empty() {
            println!(
                "No chats to backup (all excluded by blacklist). Use \"Manage Blacklist\" to change."
            );
            return Ok(());
        }

        let include_media = Confirm::new("Download media files?")
            .with_default(true)
            .with_help_message("Photos, videos, documents. Press Enter for Yes.")
            .prompt()
            .map_err(|e| DomainError::Auth(e.to_string()))?;

        self.sync_service
            .sync_chats(&allowed_ids, 100, include_media)
            .await
    }

    async fn run_auth(&self) -> Result<(), DomainError> {
        let _phone = Text::new("Phone number:")
            .prompt()
            .map_err(|e| DomainError::Auth(e.to_string()))?;
        Ok(())
    }
}

impl TuiInputPort {
    /// Manage Blacklist flow: dialogs -> threshold (optional) -> MultiSelect -> save blacklist.
    async fn run_manage_blacklist(&self) -> Result<(), DomainError> {
        let chats = self.tg.get_dialogs().await?;
        if chats.is_empty() {
            println!("No dialogs found.");
            return Ok(());
        }

        let blacklisted_ids = self.repo.get_blacklisted_ids().await?;

        let threshold: i32 = CustomType::<i32>::new(
            "Do you want to auto-exclude large chats? Enter threshold (or 0 to skip):",
        )
        .with_help_message(
            "Chats with approx. message count above this will be pre-selected for exclusion. 0 = skip.",
        )
        .with_default(0)
        .with_parser(&|s: &str| s.trim().parse::<i32>().map_err(|_| ()))
        .prompt()
        .map_err(|e| DomainError::Auth(e.to_string()))?;

        let large_chat_ids: HashSet<i64> = if threshold > 0 {
            chats
                .iter()
                .filter(|c| c.approx_message_count.map_or(false, |n| n > threshold))
                .map(|c| c.id)
                .collect()
        } else {
            HashSet::new()
        };

        let initial_blacklist: HashSet<i64> =
            blacklisted_ids.union(&large_chat_ids).copied().collect();

        let options: Vec<String> = chats
            .iter()
            .map(|c| format!("{} {} ({})", chat_type_indicator(c.kind), c.title, c.id))
            .collect();
        let default: Vec<usize> = chats
            .iter()
            .enumerate()
            .filter(|(_, c)| initial_blacklist.contains(&c.id))
            .map(|(i, _)| i)
            .collect();

        let selected = MultiSelect::new(
            "Select chats to EXCLUDE from backup (Blacklist)",
            options.clone(),
        )
        .with_default(&default)
        .with_help_message(
            "Checked = excluded from backup. Union of saved blacklist + auto large chats.",
        )
        .prompt()
        .map_err(|e| DomainError::Auth(e.to_string()))?;

        let new_blacklist: HashSet<i64> = chats
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

        self.repo.update_blacklist(new_blacklist.clone()).await?;
        println!(
            "Blacklist updated ({} chats excluded from backup).",
            new_blacklist.len()
        );
        Ok(())
    }

    /// Watcher flow: dialogs -> target list (whitelist) MultiSelect -> update_targets -> run watcher loop.
    async fn run_watcher(&self) -> Result<(), DomainError> {
        let chats = self.tg.get_dialogs().await?;
        if chats.is_empty() {
            println!("No dialogs found.");
            return Ok(());
        }

        let target_ids = self.repo.get_target_ids().await?;
        let options: Vec<String> = chats
            .iter()
            .map(|c| format!("{} {} ({})", chat_type_indicator(c.kind), c.title, c.id))
            .collect();
        let default: Vec<usize> = chats
            .iter()
            .enumerate()
            .filter(|(_, c)| target_ids.contains(&c.id))
            .map(|(i, _)| i)
            .collect();

        let selected = MultiSelect::new("Select chats to WATCH (Target List)", options.clone())
            .with_default(&default)
            .prompt()
            .map_err(|e| DomainError::Auth(e.to_string()))?;

        let new_targets: HashSet<i64> = chats
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

        self.repo.update_targets(new_targets.clone()).await?;

        println!("Watcher started. Notifications will go to Saved Messages. Press Ctrl+C to stop.");
        self.watcher_service.run_loop().await
    }
}
