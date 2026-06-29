//! `pulsate-cli` — implementations of the `p8` subcommands.
//!
//! Command logic lives here rather than in the binary, which keeps the binary
//! thin and the commands unit-testable (`docs/03-repository.md`).
#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::path::Path;

use pulsate_config::{compile, ConfigStore, Source};

pub mod up;
#[doc(inline)]
pub use up::{up, TlsOptions, UpOptions};

/// Stable process exit codes (`docs/25-error-and-status-catalog.md`).
pub mod exit {
    /// Success.
    pub const OK: u8 = 0;
    /// Generic runtime error (e.g. file not readable).
    pub const RUNTIME: u8 = 1;
    /// Config validation failed (`PLS-CFG-*`).
    pub const CONFIG_INVALID: u8 = 2;
}

/// The outcome of running a command: an exit code plus rendered output.
///
/// Returning the text (rather than printing inline) lets tests assert on it; the
/// binary prints `stdout`/`stderr` and exits with `code`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The process exit code.
    pub code: u8,
    /// Text destined for standard output.
    pub stdout: String,
    /// Text destined for standard error.
    pub stderr: String,
}

impl Outcome {
    fn ok(stdout: impl Into<String>) -> Self {
        Self {
            code: exit::OK,
            stdout: stdout.into(),
            stderr: String::new(),
        }
    }

    fn fail(code: u8, stderr: impl Into<String>) -> Self {
        Self {
            code,
            stdout: String::new(),
            stderr: stderr.into(),
        }
    }
}

/// `p8 validate <path>` — parse and validate a config without starting.
///
/// Renders every diagnostic against the source with its span, and returns exit
/// code 2 if any error is found (0 if the config is valid, warnings aside).
#[must_use]
pub fn validate(path: &Path) -> Outcome {
    let name = path.display().to_string();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            return Outcome::fail(exit::RUNTIME, format!("pulsate: cannot read {name}: {e}\n"))
        }
    };
    validate_text(&name, &text)
}

/// Validate already-loaded source text (the testable core of [`validate`]).
#[must_use]
pub fn validate_text(name: &str, text: &str) -> Outcome {
    let source = Source::new(name, text);
    match compile(name, text, 0) {
        Ok(compiled) => {
            let mut out = String::new();
            for w in &compiled.warnings {
                out.push_str(&w.render(&source));
                out.push('\n');
            }
            let sites = compiled.config.sites.len();
            let ups = compiled.config.upstreams.len();
            let _ = writeln!(
                out,
                "ok: {name} is valid — {sites} site(s), {ups} upstream(s), snapshot {}",
                compiled.snapshot.id()
            );
            Outcome::ok(out)
        }
        Err(diags) => {
            let mut out = String::new();
            for d in &diags {
                out.push_str(&d.render(&source));
                out.push('\n');
            }
            let _ = writeln!(
                out,
                "error: {name} is invalid — {} problem(s) found",
                diags.len()
            );
            Outcome::fail(exit::CONFIG_INVALID, out)
        }
    }
}

/// `p8 import <nginx|caddy|haproxy|apache> <path>` — translate a foreign config to Flow.
#[must_use]
pub fn import_config(format: &str, path: &Path) -> Outcome {
    let Some(source) = pulsate_migrate::Source::parse(format) else {
        return Outcome::fail(
            exit::RUNTIME,
            format!(
                "pulsate: unknown import format {format:?} (use {})\n",
                pulsate_migrate::Source::NAMES
            ),
        );
    };
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            return Outcome::fail(
                exit::RUNTIME,
                format!("pulsate: cannot read {}: {e}\n", path.display()),
            )
        }
    };
    let imported = pulsate_migrate::import(source, &text);
    // Validate that the generated Flow at least compiles, surfacing any gaps.
    let valid = compile(&path.display().to_string(), &imported.flow, 0).is_ok();
    let mut out = imported.flow.clone();
    out.push_str("\n# --- fidelity report ---\n");
    for line in imported.report().lines() {
        out.push_str("# ");
        out.push_str(line);
        out.push('\n');
    }
    let _ = writeln!(
        out,
        "# generated Flow compiles: {}",
        if valid { "yes" } else { "no (review needed)" }
    );
    Outcome::ok(out)
}

/// `p8 plugin run <path> <input>` — load a WASM plugin and call `eval`.
#[must_use]
pub fn plugin_run(path: &Path, input: i32) -> Outcome {
    let name = path.display().to_string();
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) => {
            return Outcome::fail(exit::RUNTIME, format!("pulsate: cannot read {name}: {e}\n"))
        }
    };
    let host = match pulsate_plugin::PluginHost::new() {
        Ok(h) => h,
        Err(e) => return Outcome::fail(exit::RUNTIME, format!("pulsate: {e}\n")),
    };
    let plugin = match host.load(&name, &bytes) {
        Ok(p) => p,
        Err(e) => return Outcome::fail(exit::RUNTIME, format!("pulsate: {e}\n")),
    };
    let caps = pulsate_plugin::Capabilities { log: true };
    match host.run(&plugin, caps, 10_000_000, input) {
        Ok(r) => Outcome::ok(format!(
            "plugin {name}: eval({input}) = {} (fuel used {}, {} log line(s))\n",
            r.output,
            r.fuel_used,
            r.logs.len()
        )),
        Err(e) => Outcome::fail(exit::RUNTIME, format!("pulsate: {e}\n")),
    }
}

/// `p8 config dump <path>` — validate, then print the typed config model.
#[must_use]
pub fn config_dump(path: &Path) -> Outcome {
    let name = path.display().to_string();
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) => {
            return Outcome::fail(exit::RUNTIME, format!("pulsate: cannot read {name}: {e}\n"))
        }
    };
    match ConfigStore::load(&name, &text) {
        Ok(store) => Outcome::ok(format!("{:#?}\n", store.current().config)),
        Err(diags) => {
            let source = Source::new(&name, &text);
            let rendered: String = diags.iter().map(|d| d.render(&source)).collect();
            Outcome::fail(exit::CONFIG_INVALID, rendered)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_text_accepts_a_good_config() {
        let out = validate_text(
            "t.flow",
            "upstream api { target http://127.0.0.1:8080 }\nsite a.com { route /* ~> proxy(@api) }",
        );
        assert_eq!(out.code, exit::OK);
        assert!(out.stdout.contains("is valid"));
    }

    #[test]
    fn validate_text_reports_errors_with_exit_2() {
        let out = validate_text("t.flow", "site a.com { route /* ~> proxy(@nope) }");
        assert_eq!(out.code, exit::CONFIG_INVALID);
        assert!(out.stderr.contains("PLS-CFG-0007"));
        assert!(out.stderr.contains("1 problem(s) found"));
    }
}
