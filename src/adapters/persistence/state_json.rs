//! Implements StatePort using a JSON file.
//!
//! Tracks last_message_id per chat for incremental sync.

use crate::domain::DomainError;
use crate::ports::StatePort;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// State: chat_id -> last_message_id
#[derive(Debug, Default, Serialize, Deserialize)]
struct StateData {
    last_message_ids: HashMap<i64, i32>,
}

/// JSON file-based state storage.
pub struct StateJson {
    path: std::path::PathBuf,
    cache: tokio::sync::RwLock<StateData>,
}

impl StateJson {
    pub fn new(path: impl AsRef<Path>) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            cache: tokio::sync::RwLock::new(StateData::default()),
        }
    }

    /// Load state from disk. Call after construction or when path changes.
    pub async fn load(&self) -> Result<(), DomainError> {
        let data = match fs::read_to_string(&self.path).await {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => StateData::default(),
        };
        *self.cache.write().await = data;
        Ok(())
    }

    async fn save(&self) -> Result<(), DomainError> {
        let data = self.cache.read().await;
        let json =
            serde_json::to_string_pretty(&*data).map_err(|e| DomainError::State(e.to_string()))?;
        let mut f = fs::File::create(&self.path)
            .await
            .map_err(|e| DomainError::State(e.to_string()))?;
        f.write_all(json.as_bytes())
            .await
            .map_err(|e| DomainError::State(e.to_string()))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl StatePort for StateJson {
    async fn get_last_message_id(&self, chat_id: i64) -> Result<i32, DomainError> {
        let cache = self.cache.read().await;
        Ok(cache.last_message_ids.get(&chat_id).copied().unwrap_or(0))
    }

    async fn set_last_message_id(&self, chat_id: i64, message_id: i32) -> Result<(), DomainError> {
        {
            let mut cache = self.cache.write().await;
            cache.last_message_ids.insert(chat_id, message_id);
        }
        self.save().await
    }
}
