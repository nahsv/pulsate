//! `pulsate-util` — small shared helpers with no internal dependencies.
//!
//! Buffer pooling and the human-unit parsers (`30s`, `10MB`) used across the
//! workspace. A leaf crate (`docs/03-repository.md`): it depends on nothing
//! internal so anything may use it.
#![forbid(unsafe_code)]

pub mod parse;
pub mod pool;

#[doc(inline)]
pub use parse::{parse_duration, parse_size, ParseError};
#[doc(inline)]
pub use pool::BufferPool;
