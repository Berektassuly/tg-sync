//! Mock AI adapter for testing without API calls.
//!
//! Returns hardcoded responses for development and testing purposes.

use crate::domain::{ActionItem, AnalysisResult, DomainError, WeekGroup};
use crate::ports::AiPort;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::info;

/// Mock AI adapter for testing.
///
/// Returns predetermined responses without making API calls.
/// Simulates network latency with configurable delay.
pub struct MockAiAdapter {
    /// Simulated network delay in milliseconds.
    delay_ms: u64,
}

impl MockAiAdapter {
    /// Create a new mock adapter with default delay (100ms).
    pub fn new() -> Self {
        Self { delay_ms: 100 }
    }

    /// Create a mock adapter with custom delay.
    pub fn with_delay(delay_ms: u64) -> Self {
        Self { delay_ms }
    }
}

impl Default for MockAiAdapter {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AiPort for MockAiAdapter {
    async fn analyze(
        &self,
        chat_id: i64,
        week_group: &WeekGroup,
        context_csv: &str,
    ) -> Result<AnalysisResult, DomainError> {
        info!(
            chat_id,
            week = %week_group,
            csv_len = context_csv.len(),
            "[MOCK] Simulating AI analysis"
        );

        // Simulate network delay
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;

        let analyzed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        // Count lines in CSV (excluding header) to make summary somewhat realistic
        let message_count = context_csv.lines().count().saturating_sub(1);

        Ok(AnalysisResult {
            week_group: week_group.clone(),
            chat_id,
            summary: format!(
                "[MOCK] This is a simulated analysis of {} messages for week {}. \
                 In a real scenario, this would contain a comprehensive summary \
                 of the discussions, key decisions made, and overall context. \
                 The mock adapter is useful for testing the analysis pipeline \
                 without incurring API costs.",
                message_count, week_group
            ),
            key_topics: vec![
                "Mock Topic 1: General Discussion".to_string(),
                "Mock Topic 2: Project Updates".to_string(),
                "Mock Topic 3: Action Planning".to_string(),
            ],
            action_items: vec![
                ActionItem {
                    description: "[MOCK] Review the analysis pipeline implementation".to_string(),
                    owner: Some("Developer".to_string()),
                    deadline: Some("End of week".to_string()),
                    priority: Some("medium".to_string()),
                },
                ActionItem {
                    description: "[MOCK] Configure real AI API key for production".to_string(),
                    owner: None,
                    deadline: None,
                    priority: Some("high".to_string()),
                },
            ],
            analyzed_at,
        })
    }

    async fn summarize(&self, context: &str) -> Result<String, DomainError> {
        info!(
            context_len = context.len(),
            "[MOCK] Simulating AI summarization"
        );

        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;

        let line_count = context.lines().count().saturating_sub(1).max(0);
        Ok(format!(
            "[MOCK] Intermediate summary of {} lines of chat logs. \
             Key events and topics from this chunk. In production, the LLM would \
             extract salient points for the reduce phase.",
            line_count
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_adapter() {
        let adapter = MockAiAdapter::with_delay(10);
        let week = WeekGroup::new("2024-01");
        let csv = "Date;User;Message\n2024-01-01;123;Hello";

        let result = adapter.analyze(123, &week, csv).await.unwrap();

        assert_eq!(result.chat_id, 123);
        assert_eq!(result.week_group, week);
        assert!(!result.summary.is_empty());
        assert_eq!(result.key_topics.len(), 3);
        assert_eq!(result.action_items.len(), 2);
    }
}
