//! OpenAI-compatible adapter for AI analysis.
//!
//! Supports OpenAI API, Azure OpenAI, and local Ollama instances.
//! Implements `AiPort` with robust JSON parsing and markdown stripping.

use crate::domain::{ActionItem, AnalysisResult, DomainError, WeekGroup};
use crate::ports::AiPort;
use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{debug, info, warn};

/// OpenAI-compatible AI adapter.
///
/// Can be configured to work with:
/// - OpenAI API (api.openai.com)
/// - Azure OpenAI
/// - Ollama (localhost)
/// - Any OpenAI-compatible API
pub struct OpenAiAdapter {
    client: reqwest::Client,
    api_url: String,
    api_key: String,
    model: String,
}

impl OpenAiAdapter {
    /// Create a new OpenAI adapter.
    ///
    /// # Arguments
    /// * `api_url` - API endpoint (e.g., "https://api.openai.com/v1/chat/completions")
    /// * `api_key` - API key (can be empty for local Ollama)
    /// * `model` - Model name (e.g., "gpt-4o-mini", "llama3.2")
    pub fn new(api_url: String, api_key: String, model: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_url,
            api_key,
            model,
        }
    }

    /// Build the system prompt with JSON schema instructions.
    fn system_prompt() -> &'static str {
        r#"You are an expert personal assistant analyzing Telegram chat logs for the chat owner.

## Your Task
1. Summarize the key discussions and themes (2-3 concise paragraphs).
2. Extract Action Items (see rules below), with owner and deadline if mentioned.
3. List 3-5 key topics discussed.

## Action Items: What to Extract

### Explicit tasks
- Commitments, promises, or stated to-dos (e.g., "I need to do X", "We should schedule Y", "Let me send you Z").
- Include owner and deadline when present in the thread.

### Unanswered messages (implicit tasks)
- **Identify questions or requests directed at the chat owner** that have no visible reply in the provided chunk.
- Look for: direct questions (@ or by name), "can you...", "could you...", "when will you...", "did you...", requests for input or approval, or follow-ups that were never answered.
- **Format each unanswered item as a single actionable task:** "Reply to [Name] regarding [Topic]".
  - [Name] = the person who asked (or their display name/identifier from the log).
  - [Topic] = a short, clear summary of what they asked (e.g., "meeting time", "approval for X", "status on Y").
- Only include an unanswered item if the chat owner appears to be the addressee and no answer is present in the log.

### Validation (before output)
- Review every action item you generated. Each must be:
  - **Actionable:** Someone could do it without guessing (e.g., "Reply to Alex regarding budget approval" not "Follow up on thing").
  - **Clear:** No vague references; include enough context (name/topic) so the task is unambiguous.
- Remove or rewrite any item that fails this check. Prefer fewer, clear tasks over many vague ones.

## Output Format
You MUST respond with valid JSON only. No markdown, no explanations outside JSON.

```json
{
  "summary": "Concise summary of discussions...",
  "key_topics": ["topic1", "topic2", "topic3"],
  "action_items": [
    {
      "description": "What needs to be done (e.g. 'Reply to [Name] regarding [Topic]' for unanswered items)",
      "owner": "Person responsible (or null)",
      "deadline": "Due date if mentioned (or null)",
      "priority": "high|medium|low (or null)"
    }
  ]
}
```

If there are no action items, return an empty array for action_items.
Keep summaries factual and concise. Focus on actionable information."#
    }

    /// Build the user prompt with CSV data or combined summaries (reduce phase).
    fn user_prompt(context_csv: &str) -> String {
        format!(
            "Analyze the following chat log context for the week. It may be CSV format (Date;User;Message) or combined summaries from multiple chunks.\n\n{}",
            context_csv
        )
    }

    /// Build the summarization prompt for the Map phase.
    fn summarize_prompt(context: &str) -> String {
        format!(
            "Summarize the following chat logs, highlighting key events and topics.\n\n{}",
            context
        )
    }

    /// Sanitize JSON response from LLM.
    ///
    /// LLMs sometimes wrap JSON in markdown code blocks. This strips them.
    fn sanitize_json(raw_text: &str) -> String {
        let trimmed = raw_text.trim();

        // Handle markdown code blocks: ```json ... ``` or ``` ... ```
        if trimmed.starts_with("```") {
            let without_prefix = if trimmed.starts_with("```json") {
                trimmed.strip_prefix("```json").unwrap_or(trimmed)
            } else {
                trimmed.strip_prefix("```").unwrap_or(trimmed)
            };

            // Find closing backticks
            if let Some(end_idx) = without_prefix.rfind("```") {
                return without_prefix[..end_idx].trim().to_string();
            }
            return without_prefix.trim().to_string();
        }

        // Handle cases where JSON might be wrapped in other markdown
        if let Some(start) = trimmed.find('{') {
            if let Some(end) = trimmed.rfind('}') {
                if start < end {
                    return trimmed[start..=end].to_string();
                }
            }
        }

        trimmed.to_string()
    }
}

/// OpenAI API request structure.
#[derive(Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_format: Option<ResponseFormat>,
}

#[derive(Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ResponseFormat {
    #[serde(rename = "type")]
    format_type: String,
}

/// OpenAI API response structure.
#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(Deserialize)]
struct Choice {
    message: MessageContent,
}

#[derive(Deserialize)]
struct MessageContent {
    content: String,
}

/// Parsed LLM response (matches our JSON schema).
#[derive(Deserialize)]
struct LlmAnalysis {
    summary: String,
    key_topics: Vec<String>,
    action_items: Vec<LlmActionItem>,
}

#[derive(Deserialize)]
struct LlmActionItem {
    description: String,
    owner: Option<String>,
    deadline: Option<String>,
    priority: Option<String>,
}

#[async_trait::async_trait]
impl AiPort for OpenAiAdapter {
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
            "sending context to AI for analysis"
        );

        // Build request
        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![
                ChatMessage {
                    role: "system".to_string(),
                    content: Self::system_prompt().to_string(),
                },
                ChatMessage {
                    role: "user".to_string(),
                    content: Self::user_prompt(context_csv),
                },
            ],
            temperature: 0.3,
            response_format: Some(ResponseFormat {
                format_type: "json_object".to_string(),
            }),
        };

        // Send request
        let response = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| DomainError::Ai(format!("HTTP request failed: {}", e)))?;

        // Check status
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %text, "AI API returned error");
            return Err(DomainError::Ai(format!(
                "API error {}: {}",
                status,
                text.chars().take(200).collect::<String>()
            )));
        }

        // Parse response
        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| DomainError::Ai(format!("Failed to parse API response: {}", e)))?;

        let raw_content = chat_response
            .choices
            .first()
            .map(|c| c.message.content.clone())
            .ok_or_else(|| DomainError::Ai("No response choices returned".to_string()))?;

        debug!(raw_len = raw_content.len(), "received AI response");

        // Sanitize and parse JSON
        let clean_json = Self::sanitize_json(&raw_content);
        let analysis: LlmAnalysis = serde_json::from_str(&clean_json).map_err(|e| {
            warn!(error = %e, json = %clean_json.chars().take(200).collect::<String>(), "JSON parse failed");
            DomainError::Ai(format!("Failed to parse LLM JSON: {}", e))
        })?;

        // Convert to domain entity
        let analyzed_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;

        let action_items: Vec<ActionItem> = analysis
            .action_items
            .into_iter()
            .map(|item| ActionItem {
                description: item.description,
                owner: item.owner,
                deadline: item.deadline,
                priority: item.priority,
            })
            .collect();

        info!(
            chat_id,
            week = %week_group,
            topics = analysis.key_topics.len(),
            actions = action_items.len(),
            "AI analysis complete"
        );

        Ok(AnalysisResult {
            week_group: week_group.clone(),
            chat_id,
            summary: analysis.summary,
            key_topics: analysis.key_topics,
            action_items,
            analyzed_at,
        })
    }

    async fn summarize(&self, context: &str) -> Result<String, DomainError> {
        info!(
            context_len = context.len(),
            "sending context to AI for summarization"
        );

        let request = ChatRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: Self::summarize_prompt(context),
            }],
            temperature: 0.3,
            response_format: None, // Plain text, no JSON
        };

        let response = self
            .client
            .post(&self.api_url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&request)
            .send()
            .await
            .map_err(|e| DomainError::Ai(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            warn!(status = %status, body = %text, "AI API returned error");
            return Err(DomainError::Ai(format!(
                "API error {}: {}",
                status,
                text.chars().take(200).collect::<String>()
            )));
        }

        let chat_response: ChatResponse = response
            .json()
            .await
            .map_err(|e| DomainError::Ai(format!("Failed to parse API response: {}", e)))?;

        let summary = chat_response
            .choices
            .first()
            .map(|c| c.message.content.trim().to_string())
            .ok_or_else(|| DomainError::Ai("No response choices returned".to_string()))?;

        info!(summary_len = summary.len(), "summarization complete");

        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_json_clean() {
        let input = r#"{"summary": "test"}"#;
        assert_eq!(OpenAiAdapter::sanitize_json(input), input);
    }

    #[test]
    fn test_sanitize_json_markdown() {
        let input = r#"```json
{"summary": "test"}
```"#;
        assert_eq!(
            OpenAiAdapter::sanitize_json(input),
            r#"{"summary": "test"}"#
        );
    }

    #[test]
    fn test_sanitize_json_markdown_no_lang() {
        let input = r#"```
{"summary": "test"}
```"#;
        assert_eq!(
            OpenAiAdapter::sanitize_json(input),
            r#"{"summary": "test"}"#
        );
    }

    #[test]
    fn test_sanitize_json_with_text() {
        let input = r#"Here is the analysis:
{"summary": "test", "key_topics": []}"#;
        assert_eq!(
            OpenAiAdapter::sanitize_json(input),
            r#"{"summary": "test", "key_topics": []}"#
        );
    }
}
