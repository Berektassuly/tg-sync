//! Application configuration. API credentials, paths.

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct AppConfig {
    pub api_id: Option<i32>,
    pub api_hash: Option<String>,
    pub data_dir: Option<String>,
    pub session_path: Option<String>,
}

impl AppConfig {
    pub fn load() -> Result<Self, config::ConfigError> {
        dotenv::dotenv().ok();
        let mut c = config::Config::builder();
        c = c.add_source(config::Environment::with_prefix("TG_SYNC"));
        if let Ok(path) = std::env::var("TG_SYNC_CONFIG") {
            c = c.add_source(config::File::with_name(&path));
        }
        c.build()?.try_deserialize()
    }
}
