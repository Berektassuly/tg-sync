//! Core domain layer. No external I/O dependencies.
//!
//! Entities and business rules live here. Dependencies flow inward.

pub mod entities;
pub mod errors;

pub use entities::{
    ActionItem, AnalysisResult, Chat, ChatType, MediaReference, MediaType, Message, MessageEdit,
    SignInResult, WeekGroup,
};
pub use errors::DomainError;
