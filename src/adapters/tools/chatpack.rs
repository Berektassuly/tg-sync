//! Mock Chatpack integration. Implements ProcessorPort.
//!
//! Simulates calling an external crate/binary to process downloaded chat logs.

use crate::domain::DomainError;
use crate::ports::ProcessorPort;
use async_trait::async_trait;
use std::path::Path;
use tracing::info;

/// Mock processor. In production, would invoke external Chatpack tool.
pub struct ChatpackProcessor {
    _bin_path: Option<std::path::PathBuf>,
}

impl ChatpackProcessor {
    pub fn new(bin_path: Option<impl AsRef<Path>>) -> Self {
        Self {
            _bin_path: bin_path.map(|p| p.as_ref().to_path_buf()),
        }
    }
}

#[async_trait]
impl ProcessorPort for ChatpackProcessor {
    async fn process_chat(&self, chat_id: i64, data_path: &Path) -> Result<(), DomainError> {
        info!(chat_id, path = %data_path.display(), "Chatpack process (mock)");
        // Would run: chatpack process --input data_path
        Ok(())
    }
}
