//! Analysis service. Orchestrates AI-powered chat analysis workflow.
//!
//! Coordinates between repository (data), AI adapter (analysis), and filesystem (reports).
//!
//! Implements Map-Reduce pattern for large chats: chunks are summarized separately,
//! then combined for final analysis (avoids OOM and token limit exceeded).

use crate::adapters::ai::messages_to_csv_chunked;
use crate::domain::{AnalysisResult, DomainError, Message, WeekGroup};
use crate::ports::{AiPort, AnalysisLogPort};
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs;
use tracing::{info, warn};

/// Maximum characters per chunk. Conservative for LLM token limits (~15k tokens).
const MAX_CHUNK_SIZE: usize = 50_000;

/// Service for AI-powered chat analysis.
///
/// Orchestrates the flow:
/// 1. Fetch unanalyzed weeks from repository
/// 2. Generate CSV context for each week
/// 3. Send to AI for analysis
/// 4. Save results and generate Markdown reports
pub struct AnalysisService {
    ai: Arc<dyn AiPort>,
    repo: Arc<dyn AnalysisLogPort>,
    reports_dir: PathBuf,
}

impl AnalysisService {
    /// Create a new analysis service.
    ///
    /// # Arguments
    /// * `ai` - AI port implementation (OpenAI, Mock, etc.)
    /// * `repo` - Repository implementing AnalysisLogPort
    /// * `reports_dir` - Directory to save generated reports
    pub fn new(ai: Arc<dyn AiPort>, repo: Arc<dyn AnalysisLogPort>, reports_dir: PathBuf) -> Self {
        Self {
            ai,
            repo,
            reports_dir,
        }
    }

    /// Analyze unprocessed weeks for a chat.
    ///
    /// Returns paths to generated Markdown reports.
    /// Skips already-analyzed weeks (idempotent).
    pub async fn analyze_chat(&self, chat_id: i64) -> Result<Vec<PathBuf>, DomainError> {
        // Ensure reports directory exists
        fs::create_dir_all(&self.reports_dir)
            .await
            .map_err(|e| DomainError::Repo(format!("Failed to create reports dir: {}", e)))?;

        // Get weeks that haven't been analyzed yet
        let unanalyzed_weeks = self.repo.get_unanalyzed_weeks(chat_id).await?;
        if unanalyzed_weeks.is_empty() {
            info!(chat_id, "no unanalyzed weeks found");
            return Ok(Vec::new());
        }

        info!(
            chat_id,
            weeks = unanalyzed_weeks.len(),
            "found unanalyzed weeks"
        );

        // Get all messages grouped by week
        let weeks_data = self.repo.get_messages_by_week(chat_id).await?;

        let mut reports = Vec::new();

        for (week, messages) in weeks_data {
            // Skip if already analyzed
            if !unanalyzed_weeks.contains(&week) {
                continue;
            }

            if messages.is_empty() {
                warn!(chat_id, week = %week, "week has no messages after filtering");
                continue;
            }

            info!(
                chat_id,
                week = %week,
                messages = messages.len(),
                "analyzing week"
            );

            // Generate CSV chunks (avoids memory bomb for large weeks)
            let chunks = self.messages_to_csv_chunked(&messages, MAX_CHUNK_SIZE)?;

            // Map-Reduce: single chunk -> direct analyze; multiple chunks -> summarize then analyze
            let result = self.analyze_week_chunks(chat_id, &week, &chunks).await?;

            // Persist result
            self.repo.save_analysis(&result).await?;

            // Generate and save report
            let report_path = self.generate_report(&result).await?;
            reports.push(report_path);
        }

        info!(
            chat_id,
            reports_generated = reports.len(),
            "analysis complete"
        );

        Ok(reports)
    }

    /// Get list of weeks available for analysis (both analyzed and unanalyzed).
    pub async fn get_available_weeks(&self, chat_id: i64) -> Result<Vec<WeekGroup>, DomainError> {
        let weeks_data = self.repo.get_messages_by_week(chat_id).await?;
        Ok(weeks_data.into_iter().map(|(week, _)| week).collect())
    }

    /// Generate CSV chunks, each under MAX_CHUNK_SIZE characters.
    fn messages_to_csv_chunked(
        &self,
        messages: &[Message],
        max_size: usize,
    ) -> Result<Vec<String>, DomainError> {
        messages_to_csv_chunked(messages, max_size)
            .map_err(|e| DomainError::Ai(format!("Failed to generate CSV chunks: {}", e)))
    }

    /// Analyze week data: single chunk -> direct analyze; multiple chunks -> Map-Reduce.
    async fn analyze_week_chunks(
        &self,
        chat_id: i64,
        week: &WeekGroup,
        chunks: &[String],
    ) -> Result<AnalysisResult, DomainError> {
        if chunks.is_empty() {
            return Err(DomainError::Ai("No chunks to analyze".to_string()));
        }

        if chunks.len() == 1 {
            // Case A (Small): Single chunk, call analyze directly
            self.ai.analyze(chat_id, week, &chunks[0]).await
        } else {
            // Case B (Large): Map each chunk to summary, Reduce to final analysis
            let mut summaries = Vec::with_capacity(chunks.len());
            for (i, chunk) in chunks.iter().enumerate() {
                info!(chat_id, week = %week, chunk = i + 1, total = chunks.len(), "map: summarizing chunk");
                let summary = self.ai.summarize(chunk).await?;
                summaries.push(summary);
            }

            let meta_context = summaries.join("\n\n");
            info!(chat_id, week = %week, summaries_len = meta_context.len(), "reduce: analyzing combined summaries");
            self.ai.analyze(chat_id, week, &meta_context).await
        }
    }

    /// Generate a Markdown report from analysis result.
    async fn generate_report(&self, result: &AnalysisResult) -> Result<PathBuf, DomainError> {
        let filename = format!("analysis_{}_{}.md", result.chat_id, result.week_group);
        let path = self.reports_dir.join(&filename);

        let timestamp = DateTime::<Utc>::from_timestamp(result.analyzed_at, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        let mut md = String::new();

        // Header
        md.push_str(&format!("# Weekly Digest: {}\n\n", result.week_group));
        md.push_str(&format!(
            "**Chat ID:** {} | **Analyzed:** {}\n\n",
            result.chat_id, timestamp
        ));
        md.push_str("---\n\n");

        // Summary
        md.push_str("## 📝 Summary\n\n");
        md.push_str(&result.summary);
        md.push_str("\n\n");

        // Key Topics
        if !result.key_topics.is_empty() {
            md.push_str("## 🔑 Key Topics\n\n");
            for topic in &result.key_topics {
                md.push_str(&format!("- {}\n", topic));
            }
            md.push_str("\n");
        }

        // Action Items
        if !result.action_items.is_empty() {
            md.push_str("## 🚀 Action Items\n\n");
            for item in &result.action_items {
                md.push_str(&format!("- [ ] **{}**", item.description));

                let mut meta = Vec::new();
                if let Some(owner) = &item.owner {
                    meta.push(format!("Owner: {}", owner));
                }
                if let Some(deadline) = &item.deadline {
                    meta.push(format!("Due: {}", deadline));
                }
                if let Some(priority) = &item.priority {
                    meta.push(format!("Priority: {}", priority));
                }

                if !meta.is_empty() {
                    md.push_str(&format!(" ({})", meta.join(", ")));
                }
                md.push('\n');
            }
            md.push('\n');
        }

        // Footer
        md.push_str("---\n");
        md.push_str("*Generated by tg-sync AI Analysis*\n");

        // Write to file
        fs::write(&path, md)
            .await
            .map_err(|e| DomainError::Repo(format!("Failed to write report: {}", e)))?;

        info!(path = %path.display(), "report generated");

        Ok(path)
    }
}
