//! CSV utilities for AI analysis. Uses the `csv` crate for safe serialization.
//!
//! Converts domain messages to CSV format suitable for LLM context input.

use crate::domain::Message;
use chrono::{DateTime, Utc};

/// Convert messages to a CSV string for LLM context.
///
/// Format: `Date;User;Message` (semicolon-delimited for LLM token efficiency)
///
/// # Arguments
/// * `messages` - Slice of messages to convert (should be pre-filtered)
///
/// # Returns
/// CSV string with header row, or error if serialization fails.
pub fn messages_to_csv(messages: &[Message]) -> Result<String, csv::Error> {
    let mut wtr = csv::WriterBuilder::new()
        .delimiter(b';')
        .has_headers(true)
        .from_writer(Vec::new());

    // Write header
    wtr.write_record(["Date", "User", "Message"])?;

    for msg in messages {
        // Convert Unix timestamp to readable ISO format
        let date_str = DateTime::<Utc>::from_timestamp(msg.date, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_else(|| msg.date.to_string());

        // User ID as string (could be enhanced with user lookup later)
        let user_str = msg
            .from_user_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Clean text: replace newlines with spaces for LLM readability
        // The csv crate handles proper quoting/escaping of special characters
        let clean_text = msg.text.replace('\n', " ").replace('\r', "");

        wtr.write_record([&date_str, &user_str, &clean_text])?;
    }

    wtr.flush()?;
    let bytes = wtr.into_inner().map_err(|e| {
        csv::Error::from(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        ))
    })?;

    String::from_utf8(bytes).map_err(|e| {
        csv::Error::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_messages_to_csv_basic() {
        let messages = vec![Message {
            id: 1,
            chat_id: 123,
            date: 1704067200, // 2024-01-01 00:00:00 UTC
            text: "Hello world".to_string(),
            media: None,
            from_user_id: Some(456),
            reply_to_msg_id: None,
        }];

        let csv = messages_to_csv(&messages).unwrap();
        assert!(csv.contains("Date;User;Message"));
        assert!(csv.contains("2024-01-01"));
        assert!(csv.contains("456"));
        assert!(csv.contains("Hello world"));
    }

    #[test]
    fn test_messages_to_csv_special_chars() {
        let messages = vec![Message {
            id: 1,
            chat_id: 123,
            date: 1704067200,
            text: "Hello; with \"quotes\" and\nnewlines".to_string(),
            media: None,
            from_user_id: Some(456),
            reply_to_msg_id: None,
        }];

        let csv = messages_to_csv(&messages).unwrap();
        // Should handle semicolons and quotes safely
        assert!(csv.contains("Hello"));
        // Newlines should be replaced with spaces
        assert!(!csv.contains('\n') || csv.lines().count() == 2); // header + 1 data row
    }
}
