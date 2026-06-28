//! `pulsate-http` — HTTP serving and terminal-handler execution.
//!
//! Wraps hyper's auto server (HTTP/1.1 or HTTP/2) to serve an accepted
//! connection, routing each request through a [`pulsate_router::Router`], running
//! the middleware pipeline, and executing the matched handler.
#![forbid(unsafe_code)]

pub mod gateway;
pub mod handlers;
pub mod serve;

#[doc(inline)]
pub use gateway::Gateway;
#[doc(inline)]
pub use serve::serve_connection;

/// Re-exported so the binary can hold the telemetry registry without a direct
/// dependency on `pulsate-observe`.
pub use pulsate_observe::Telemetry;
