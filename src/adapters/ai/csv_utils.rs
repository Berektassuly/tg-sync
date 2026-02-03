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

/// Convert messages to CSV chunks, each under `max_chunk_size` characters.
///
/// Avoids allocating the entire week's messages as a single string (memory bomb).
/// Each chunk includes the header row. Chunks are split when the current string
/// would exceed `max_chunk_size` (conservative for LLM token limits).
///
/// # Arguments
/// * `messages` - Slice of messages to convert
/// * `max_chunk_size` - Maximum characters per chunk (e.g., 50_000 for ~15k tokens)
pub fn messages_to_csv_chunked(
    messages: &[Message],
    max_chunk_size: usize,
) -> Result<Vec<String>, csv::Error> {
    const HEADER: &str = "Date;User;Message\n";

    if messages.is_empty() {
        return Ok(vec![]);
    }

    let mut chunks = Vec::new();
    let mut current = String::with_capacity(max_chunk_size.min(4096));
    current.push_str(HEADER);

    for msg in messages {
        let row = format_message_row(msg)?;
        if current.len() + row.len() > max_chunk_size && current.len() > HEADER.len() {
            chunks.push(std::mem::take(&mut current));
            current = String::with_capacity(max_chunk_size.min(4096));
            current.push_str(HEADER);
        }
        current.push_str(&row);
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    Ok(chunks)
}

fn format_message_row(msg: &Message) -> Result<String, csv::Error> {
    let date_str = DateTime::<Utc>::from_timestamp(msg.date, 0)
        .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_else(|| msg.date.to_string());

    let user_str = msg
        .from_user_id
        .map(|id| id.to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let clean_text = msg.text.replace('\n', " ").replace('\r', "");

    let mut wtr = csv::WriterBuilder::new()
        .delimiter(b';')
        .has_headers(false)
        .from_writer(Vec::new());

    wtr.write_record([&date_str, &user_str, &clean_text])?;
    wtr.flush()?;

    let bytes = wtr.into_inner().map_err(|e| {
        csv::Error::from(std::io::Error::new(
            std::io::ErrorKind::Other,
            e.to_string(),
        ))
    })?;

    let mut row = String::from_utf8(bytes).map_err(|e| {
        csv::Error::from(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            e.to_string(),
        ))
    })?;
    if !row.ends_with('\n') {
        row.push('\n');
    }
    Ok(row)
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
            edit_history: None,
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
            edit_history: None,
        }];

        let csv = messages_to_csv(&messages).unwrap();
        // Should handle semicolons and quotes safely
        assert!(csv.contains("Hello"));
        // Newlines should be replaced with spaces
        assert!(!csv.contains('\n') || csv.lines().count() == 2); // header + 1 data row
    }

    #[test]
    fn test_messages_to_csv_chunked_single() {
        let messages = vec![Message {
            id: 1,
            chat_id: 123,
            date: 1704067200,
            text: "Hello world".to_string(),
            media: None,
            from_user_id: Some(456),
            reply_to_msg_id: None,
            edit_history: None,
        }];

        let chunks = messages_to_csv_chunked(&messages, 50_000).unwrap();
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].contains("Date;User;Message"));
        assert!(chunks[0].contains("Hello world"));
    }

    #[test]
    fn test_messages_to_csv_chunked_splits() {
        let mut messages = Vec::new();
        for i in 0..100 {
            messages.push(Message {
                id: i,
                chat_id: 123,
                date: 1704067200,
                text: "x".repeat(600), // ~620 chars per row with header overhead
                media: None,
                from_user_id: Some(456),
                reply_to_msg_id: None,
                edit_history: None,
            });
        }

        let chunks = messages_to_csv_chunked(&messages, 50_000).unwrap();
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.len() <= 52_000); // Allow small overshoot for last row
            assert!(chunk.starts_with("Date;User;Message"));
        }
    }
}
