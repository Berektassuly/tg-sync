//! Port traits. API boundaries for the hexagon.
//!
//! - Inbound: Called by UI/adapter into the application
//! - Outbound: Called by application into infrastructure

pub mod inbound;
pub mod outbound;

pub use inbound::InputPort;
pub use outbound::{AuthPort, ProcessorPort, RepoPort, StatePort, TgGateway};
