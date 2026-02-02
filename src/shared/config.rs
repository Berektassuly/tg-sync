//! Application configuration. API credentials, paths.

use serde::Deserialize;

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
        Ok(cfg)
    }

    /// Returns sync delay in milliseconds. Defaults to 500 if unset or invalid.
    pub fn sync_delay_ms_or_default(&self) -> u64 {
        self.sync_delay_ms.unwrap_or(500)
    }

    /// Returns media queue buffer size. Defaults to 1000 if unset or invalid.
    pub fn media_queue_size_or_default(&self) -> usize {
        self.media_queue_size.unwrap_or(1000)
    }
}
