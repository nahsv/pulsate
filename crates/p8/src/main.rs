//! The `p8` binary — a thin wrapper around [`p8::run`].
#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    p8::run()
}
