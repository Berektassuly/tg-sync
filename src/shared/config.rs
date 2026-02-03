//! Application configuration. API credentials, paths.

use serde::Deserialize;

/// Default capacity for the media refs channel. Bounded channel provides backpressure:
/// when full, the sync producer blocks on send().await until the media worker consumes.
pub const DEFAULT_MEDIA_QUEUE_SIZE: usize = 1000;

#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    pub api_id: Option<i32>,
    pub api_hash: Option<String>,
    pub data_dir: Option<String>,
    pub session_path: Option<String>,
    /// Optional delay in ms between message-history API requests (rate limiting). Read from EXPORT_DELAY_MS.
    #[serde(default)]
    pub export_delay_ms: Option<u64>,

    /// Delay in ms between sync batch requests (rate limiting to avoid FLOOD_WAIT). Read from SYNC_DELAY_MS.
    #[serde(default)]
    pub sync_delay_ms: Option<u64>,

    /// Max number of media refs buffered between sync loop and media worker (backpressure). Read from MEDIA_QUEUE_SIZE.
    #[serde(default)]
    pub media_queue_size: Option<usize>,

    /// Watcher cycle sleep in seconds (default 600). Read from TG_SYNC_WATCHER_CYCLE_SECS.
    #[serde(default)]
    pub watcher_cycle_secs: Option<u64>,

    // ─────────────────────────────────────────────────────────────────────────
    // AI Analysis Configuration
    // ─────────────────────────────────────────────────────────────────────────
    /// AI API key (e.g., OpenAI). Read from TG_SYNC_AI_API_KEY.
    #[serde(default)]
    pub ai_api_key: Option<String>,

    /// AI API URL. Defaults to OpenAI. Read from TG_SYNC_AI_API_URL.
    #[serde(default)]
    pub ai_api_url: Option<String>,

    /// AI model name. Defaults to "gpt-4o-mini". Read from TG_SYNC_AI_MODEL.
    #[serde(default)]
    pub ai_model: Option<String>,

    // ─────────────────────────────────────────────────────────────────────────
    // Task Tracker (Trello) Configuration
    // ─────────────────────────────────────────────────────────────────────────
    /// Trello API key. Read from TRELLO_KEY.
    #[serde(default)]
    pub trello_key: Option<String>,

    /// Trello API token. Read from TRELLO_TOKEN.
    #[serde(default)]
    pub trello_token: Option<String>,

    /// Trello board ID (optional; for reference). Read from TRELLO_BOARD_ID.
    #[serde(default)]
    pub trello_board_id: Option<String>,

    /// Trello list ID where action-item cards are created. Read from TRELLO_LIST_ID.
    #[serde(default)]
    pub trello_list_id: Option<String>,
}

impl AppConfig {
    pub fn load() -> Result<Self, config::ConfigError> {
        dotenv::dotenv().ok();
        let mut c = config::Config::builder();
        c = c.add_source(config::Environment::with_prefix("TG_SYNC"));
        if let Ok(path) = std::env::var("TG_SYNC_CONFIG") {
            c = c.add_source(config::File::with_name(&path));
        }
        let mut cfg: Self = c.build()?.try_deserialize()?;
        // EXPORT_DELAY_MS is read directly (no TG_SYNC_ prefix) so .env can use EXPORT_DELAY_MS=500
        if let Ok(s) = std::env::var("EXPORT_DELAY_MS") {
            if let Ok(ms) = s.parse::<u64>() {
                cfg.export_delay_ms = Some(ms);
            }
        }
        // SYNC_DELAY_MS: delay between message batch requests in sync loop (avoid FLOOD_WAIT)
        if let Ok(s) = std::env::var("SYNC_DELAY_MS") {
            if let Ok(ms) = s.parse::<u64>() {
                cfg.sync_delay_ms = Some(ms);
            }
        }
        // MEDIA_QUEUE_SIZE: bounded channel buffer for media refs (backpressure; default 1000)
        if let Ok(s) = std::env::var("TG_SYNC_MEDIA_QUEUE_SIZE") {
            if let Ok(n) = s.parse::<usize>() {
                cfg.media_queue_size = Some(n);
            }
        }
        // WATCHER_CYCLE_SECS: sleep between watcher cycles (default 600)
        if let Ok(s) = std::env::var("TG_SYNC_WATCHER_CYCLE_SECS") {
            if let Ok(n) = s.parse::<u64>() {
                cfg.watcher_cycle_secs = Some(n);
            }
        }
        Ok(cfg)
    }

    /// Returns watcher cycle sleep in seconds. Defaults to 600 if unset or invalid.
    pub fn watcher_cycle_secs_or_default(&self) -> u64 {
        self.watcher_cycle_secs.unwrap_or(600)
    }

    /// Returns sync delay in milliseconds. Defaults to 500 if unset or invalid.
    pub fn sync_delay_ms_or_default(&self) -> u64 {
        self.sync_delay_ms.unwrap_or(500)
    }

    /// Returns media queue buffer size. Defaults to DEFAULT_MEDIA_QUEUE_SIZE if unset or invalid.
    pub fn media_queue_size_or_default(&self) -> usize {
        self.media_queue_size.unwrap_or(DEFAULT_MEDIA_QUEUE_SIZE)
    }

    // ─────────────────────────────────────────────────────────────────────────
    // AI Configuration Helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Returns the AI API key if configured. Reads from config or TG_SYNC_AI_API_KEY env.
    pub fn ai_api_key(&self) -> Option<String> {
        self.ai_api_key
            .clone()
            .or_else(|| std::env::var("TG_SYNC_AI_API_KEY").ok())
    }

    /// Returns the AI API URL. Defaults to OpenAI chat completions endpoint.
    pub fn ai_api_url_or_default(&self) -> String {
        self.ai_api_url
            .clone()
            .or_else(|| std::env::var("TG_SYNC_AI_API_URL").ok())
            .unwrap_or_else(|| "https://api.openai.com/v1/chat/completions".to_string())
    }

    /// Returns the AI model name. Defaults to "gpt-4o-mini".
    pub fn ai_model_or_default(&self) -> String {
        self.ai_model
            .clone()
            .or_else(|| std::env::var("TG_SYNC_AI_MODEL").ok())
            .unwrap_or_else(|| "gpt-4o-mini".to_string())
    }

    /// Returns true if AI is configured (API key present).
    pub fn is_ai_configured(&self) -> bool {
        self.ai_api_key().is_some()
    }

    // ─────────────────────────────────────────────────────────────────────────
    // Trello Configuration Helpers
    // ─────────────────────────────────────────────────────────────────────────

    /// Returns Trello API key from config or TRELLO_KEY env.
    pub fn trello_key(&self) -> Option<String> {
        self.trello_key
            .clone()
            .or_else(|| std::env::var("TRELLO_KEY").ok())
    }

    /// Returns Trello API token from config or TRELLO_TOKEN env.
    pub fn trello_token(&self) -> Option<String> {
        self.trello_token
            .clone()
            .or_else(|| std::env::var("TRELLO_TOKEN").ok())
    }

    /// Returns Trello board ID from config or TRELLO_BOARD_ID env (optional).
    pub fn trello_board_id(&self) -> Option<String> {
        self.trello_board_id
            .clone()
            .or_else(|| std::env::var("TRELLO_BOARD_ID").ok())
    }

    /// Returns Trello list ID from config or TRELLO_LIST_ID env.
    pub fn trello_list_id(&self) -> Option<String> {
        self.trello_list_id
            .clone()
            .or_else(|| std::env::var("TRELLO_LIST_ID").ok())
    }

    /// Returns true if Trello task tracker is fully configured.
    pub fn is_trello_configured(&self) -> bool {
        self.trello_key().is_some()
            && self.trello_token().is_some()
            && self.trello_list_id().is_some()
    }
}
