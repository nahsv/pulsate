//! `pulsate-config` — the typed configuration model, validation, and live store.
//!
//! Takes Flow source (via `pulsate-flow`), lowers it into a validated typed
//! [`Config`], compiles a deterministic [`pulsate_core::ConfigSnapshot`], and holds
//! the live config in a lock-free, reloadable [`ConfigStore`]
//! (`docs/02-architecture.md#configuration-loading`).
#![forbid(unsafe_code)]

pub mod compile;
pub mod model;
pub mod router;
pub mod store;

#[doc(inline)]
pub use compile::{compile, Compiled};
#[doc(inline)]
pub use model::{Config, Handler, Host, MatchKind, Named, RouteDef, Site, TlsMode};
#[doc(inline)]
pub use router::{build_router, build_upstreams};
#[doc(inline)]
pub use store::{ConfigStore, Generation};

/// Re-exported so callers render diagnostics without depending on `pulsate-flow`
/// directly.
pub use pulsate_flow::{Diagnostic, Source};
