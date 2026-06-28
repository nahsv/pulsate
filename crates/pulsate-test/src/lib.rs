//! `pulsate-test` — shared test fixtures, fake upstreams, and a loopback harness
//! for the integration and conformance suites.
//!
//! Most tests today live next to the code they exercise; this crate is where
//! cross-crate fixtures and fakes will be collected as that suite grows.
#![forbid(unsafe_code)]

#[allow(unused_imports)]
use pulsate_core as _;
