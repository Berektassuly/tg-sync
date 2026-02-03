//! Task tracker outbound port. Create tasks in external systems (e.g. Trello).

use crate::domain::DomainError;

/// Port for creating tasks in an external task tracker.
///
/// Implemented by adapters (e.g. Trello). When not configured, the analysis
/// service skips sending action items but still generates the Markdown report.
#[async_trait::async_trait]
pub trait TaskTrackerPort: Send + Sync {
    /// Create a single task in the tracker.
    ///
    /// # Arguments
    /// * `title` - Short task title (e.g. card name)
    /// * `description` - Optional longer description
    /// * `due` - Optional due date string (format is adapter-specific, e.g. ISO date)
    ///
    /// # Errors
    /// Returns `DomainError` if the API call fails.
    async fn create_task(
        &self,
        title: &str,
        description: &str,
        due: Option<String>,
    ) -> Result<(), DomainError>;
}
