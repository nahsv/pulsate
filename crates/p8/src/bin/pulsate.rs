//! The `pulsate` binary — the long-form name for the same gateway as `p8`.
#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    p8::run()
}
