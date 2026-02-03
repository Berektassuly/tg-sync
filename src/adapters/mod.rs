//! Infrastructure adapters. Implement outbound ports.
//!
//! Telegram, filesystem, external tools. Map errors to DomainError.

pub mod ai;
pub mod integrations;
pub mod persistence;
pub mod telegram;
pub mod tools;
pub mod ui;
