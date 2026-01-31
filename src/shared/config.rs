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
        Ok(cfg)
    }
}
