//! Trello adapter. Implements TaskTrackerPort by creating cards via Trello REST API.

use crate::domain::DomainError;
use crate::ports::TaskTrackerPort;
use reqwest::Client;
use std::sync::Arc;

const TRELLO_CARDS_URL: &str = "https://api.trello.com/1/cards";

/// Trello API adapter for creating cards (tasks).
///
/// Requires API key and token from https://trello.com/app-key.
/// Cards are created in the list specified by `list_id`. `board_id` is stored for reference.
pub struct TrelloAdapter {
    client: Arc<Client>,
    api_key: String,
    token: String,
    #[allow(dead_code)] // reserved for future use (e.g. card labels by board)
    board_id: String,
    list_id: String,
}

impl TrelloAdapter {
    /// Create a new Trello adapter.
    ///
    /// # Arguments
    /// * `api_key` - Trello API key (from app key page)
    /// * `token` - Trello API token (from OAuth or token generation)
    /// * `board_id` - ID of the board (for reference; card creation uses `list_id`)
    /// * `list_id` - ID of the list where cards will be created
    pub fn new(api_key: String, token: String, board_id: String, list_id: String) -> Self {
        Self {
            client: Arc::new(Client::new()),
            api_key,
            token,
            board_id,
            list_id,
        }
    }
}

#[async_trait::async_trait]
impl TaskTrackerPort for TrelloAdapter {
    async fn create_task(
        &self,
        title: &str,
        description: &str,
        due: Option<String>,
    ) -> Result<(), DomainError> {
        let url = format!(
            "{}?key={}&token={}",
            TRELLO_CARDS_URL, self.api_key, self.token
        );

        let mut body = serde_json::json!({
            "idList": self.list_id,
            "name": title,
            "desc": description,
        });

        if let Some(d) = due {
            body["due"] = serde_json::Value::String(d);
        }

        let res = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| DomainError::TaskTracker(format!("Request failed: {}", e)))?;

        if !res.status().is_success() {
            let status = res.status();
            let text = res.text().await.unwrap_or_else(|_| "unknown".to_string());
            return Err(DomainError::TaskTracker(format!(
                "Trello API error {}: {}",
                status, text
            )));
        }

        Ok(())
    }
}
