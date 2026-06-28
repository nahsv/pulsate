//! `xtask` — workspace dev automation (the cargo-xtask pattern).
//!
//! Run as `cargo xtask <command>` (aliased in `.cargo/config.toml`). The layering
//! lint enforces the dependency-DAG rule from `docs/03-repository.md`:
//! **data-plane crates must never depend on control-plane crates.**
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "xtask", about = "Pulsate workspace automation")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Verify the crate dependency layering (data plane ⇏ control plane).
    LintLayering,
}

/// Crates that run on the request hot path. They may depend on shared and on
/// each other, never on a control-plane crate (`docs/02-architecture.md`).
const DATA_PLANE: &[&str] = &[
    "pulsate-net",
    "pulsate-tls",
    "pulsate-http",
    "pulsate-http3",
    "pulsate-router",
    "pulsate-pipeline",
    "pulsate-proxy",
    "pulsate-cache",
    "pulsate-waf",
];

/// Crates that own mutable policy off the hot path.
const CONTROL_PLANE: &[&str] = &[
    "pulsate-control",
    "pulsate-config",
    "pulsate-flow",
    "pulsate-acme",
    "pulsate-cluster",
    "pulsate-secrets",
    "pulsate-dashboard",
];

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::LintLayering => lint_layering(),
    }
}

fn lint_layering() -> ExitCode {
    let root = workspace_root();
    let mut violations = Vec::new();

    for dp in DATA_PLANE {
        let manifest = root.join("crates").join(dp).join("Cargo.toml");
        let text = match std::fs::read_to_string(&manifest) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("xtask: cannot read {}: {e}", manifest.display());
                return ExitCode::FAILURE;
            }
        };
        for cp in CONTROL_PLANE {
            // A dependency edge appears as a line naming the crate, declared as
            // `name = { path = ... }`.
            if text.lines().any(|l| line_declares_dep(l, cp)) {
                violations.push(format!(
                    "{dp} depends on control-plane crate {cp} (forbidden by docs/03-repository.md)"
                ));
            }
        }
    }

    if violations.is_empty() {
        println!(
            "layering OK: {} data-plane crates checked against {} control-plane crates",
            DATA_PLANE.len(),
            CONTROL_PLANE.len()
        );
        ExitCode::SUCCESS
    } else {
        eprintln!("layering violations:");
        for v in &violations {
            eprintln!("  - {v}");
        }
        ExitCode::FAILURE
    }
}

/// Whether a manifest line declares a dependency on `crate_name`.
fn line_declares_dep(line: &str, crate_name: &str) -> bool {
    let line = line.trim();
    if line.starts_with('#') {
        return false;
    }
    // Match `crate-name = ...` (the key), not a substring inside a path/url.
    line.split_once('=')
        .is_some_and(|(key, _)| key.trim() == crate_name)
}

/// Locate the workspace root from this crate's manifest directory.
fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is `<root>/xtask`; the workspace root is its parent.
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .map_or_else(|| manifest_dir.to_path_buf(), Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dep_line_matches_key_not_substring() {
        assert!(line_declares_dep(
            "pulsate-config = { path = \"../pulsate-config\" }",
            "pulsate-config"
        ));
        // The path mentions the name but the key is a different crate: not a dep edge.
        assert!(!line_declares_dep(
            "pulsate-core = { path = \"../pulsate-config\" }",
            "pulsate-config"
        ));
        assert!(!line_declares_dep(
            "# pulsate-config = ...",
            "pulsate-config"
        ));
    }
}
