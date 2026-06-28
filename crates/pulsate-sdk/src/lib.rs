//! `pulsate-sdk` — the guest-side plugin contract.
//!
//! A Pulsate plugin is a WebAssembly module that targets the host ABI defined here
//! (`docs/12-plugins.md`). The crate is `no_std`-friendly and dependency-free so
//! it can compile to `wasm32-unknown-unknown`. It defines the ABI version every
//! plugin advertises and documents the required exports and the host imports a
//! plugin may call (subject to capability grants on the host side).
//!
//! # The contract (ABI v1)
//!
//! A plugin must export:
//! - `pulsate_abi_version() -> i32` returning [`ABI_VERSION`].
//! - `eval(i32) -> i32`, the entry point the host calls per request.
//!
//! A plugin *may* import (only if granted the matching capability):
//! - `p8::log(i32)` — record a value to the host log (`log` capability).
//!
//! The [`declare_plugin!`] macro emits the ABI export so authors only write
//! `eval`.
#![forbid(unsafe_code)]
#![no_std]

/// The host ABI version this SDK targets. Must equal the host's `ABI_VERSION`.
pub const ABI_VERSION: i32 = 1;

/// Emit the required `pulsate_abi_version` export so a plugin author only has to
/// provide an `eval` function.
///
/// ```ignore
/// pulsate_sdk::declare_plugin!();
///
/// #[no_mangle]
/// pub extern "C" fn eval(input: i32) -> i32 { input * 2 }
/// ```
#[macro_export]
macro_rules! declare_plugin {
    () => {
        #[no_mangle]
        pub extern "C" fn pulsate_abi_version() -> i32 {
            $crate::ABI_VERSION
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abi_version_is_stable() {
        assert_eq!(ABI_VERSION, 1);
    }
}
