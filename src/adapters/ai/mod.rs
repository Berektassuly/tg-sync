//! AI adapter module. Implements AiPort for LLM integration.
//!
//! Provides OpenAI-compatible adapter and mock adapter for testing.

pub mod csv_utils;
pub mod mock_adapter;
pub mod openai_adapter;

pub use csv_utils::{messages_to_csv, messages_to_csv_chunked};
pub use mock_adapter::MockAiAdapter;
pub use openai_adapter::OpenAiAdapter;
