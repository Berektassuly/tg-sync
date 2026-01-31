//! Core domain layer. No external I/O dependencies.
//!
//! Entities and business rules live here. Dependencies flow inward.

pub mod entities;
pub mod errors;

pub use entities::{Chat, ChatType, MediaReference, MediaType, Message};
pub use errors::DomainError;
