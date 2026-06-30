//! Branded terminal UI for the `pulsate` CLI.
//!
//! A tiny, dependency-light presentation layer: a teal ASCII wordmark banner,
//! milestone `✓` lines, and a spinner for the (fast) config-compile and
//! listener-bind phases. Everything here is *decoration* — it is written to
//! **stderr** so the machine-readable surfaces stay pristine: scriptable
//! command output and the JSON access-log stream both own stdout
//! (`pulsate-http/src/serve.rs`, `pulsate-http3/src/dispatch.rs`).
//!
//! Decoration (ANSI color + spinner animation) is suppressed when `NO_COLOR`
//! is set or when stderr is not a TTY (pipes / CI), per `std::io::IsTerminal`.
//! In that mode the helpers degrade to plain ASCII lines with no escape codes.

use std::io::{IsTerminal, Write};
use std::time::Duration;

use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;

/// Pulsate brand accent (teal) — RGB(61, 163, 140), matching the webapp.
const ACCENT: (u8, u8, u8) = (61, 163, 140);
/// Dimmer teal for secondary text — RGB(39, 111, 95).
const ACCENT_DIM: (u8, u8, u8) = (39, 111, 95);

/// Compact ASCII wordmark, embedded as a const (no figlet shell-out).
const WORDMARK: &str = r"             _           _
 _ __  _   _| |___  __ _| |_ ___
| '_ \| | | | / __|/ _` | __/ _ \
| |_) | |_| | \__ \ (_| | ||  __/
| .__/ \__,_|_|___/\__,_|\__\___|
|_|";

/// The product tagline shown under the wordmark.
const TAGLINE: &str = "one binary · one config · one request path";

/// Decide whether to emit ANSI color / animation.
///
/// Centralised and pure so the gating decision is unit-testable: decoration is
/// on only when the caller has *not* opted out via `NO_COLOR` **and** the
/// target stream is an interactive terminal.
#[must_use]
fn decorate(no_color: bool, stderr_is_tty: bool) -> bool {
    !no_color && stderr_is_tty
}

/// Live decoration decision for the current process (reads `NO_COLOR` + the
/// stderr TTY state).
#[must_use]
fn enabled() -> bool {
    decorate(
        std::env::var_os("NO_COLOR").is_some(),
        std::io::stderr().is_terminal(),
    )
}

/// Print the branded wordmark banner once, to stderr.
///
/// Teal when decorated; plain text otherwise. The tagline and version sit
/// underneath in dim teal.
pub fn banner() {
    let mut err = std::io::stderr();
    let version = env!("CARGO_PKG_VERSION");
    if enabled() {
        let _ = writeln!(err, "{}", WORDMARK.truecolor(ACCENT.0, ACCENT.1, ACCENT.2));
        let _ = writeln!(
            err,
            "{}",
            format!("{TAGLINE} · v{version}").truecolor(ACCENT_DIM.0, ACCENT_DIM.1, ACCENT_DIM.2)
        );
    } else {
        let _ = writeln!(err, "{WORDMARK}");
        let _ = writeln!(err, "{TAGLINE} · v{version}");
    }
    let _ = writeln!(err);
}

/// Emit a finished milestone line: a teal `✓` followed by `msg`, to stderr.
pub fn step(msg: &str) {
    let mut err = std::io::stderr();
    if enabled() {
        let _ = writeln!(err, "{} {msg}", "✓".truecolor(ACCENT.0, ACCENT.1, ACCENT.2));
    } else {
        let _ = writeln!(err, "✓ {msg}");
    }
}

/// Emit a branded error line: a red `✗` followed by `msg`, to stderr.
pub fn error(msg: &str) {
    let mut err = std::io::stderr();
    if enabled() {
        let _ = writeln!(err, "{} {msg}", "✗".truecolor(220, 80, 80));
    } else {
        let _ = writeln!(err, "✗ {msg}");
    }
}

/// A running (or no-op) progress spinner for a short phase.
///
/// Finish it with [`finish_ok`] or [`finish_err`]. When decoration is off the
/// spinner is inert — nothing is drawn until the finishing line is printed —
/// so piped/CI output and the access-log stream stay clean.
#[must_use]
pub struct Spinner {
    bar: Option<ProgressBar>,
}

/// Start a teal spinner labelled `msg` (animated on a TTY, inert otherwise).
pub fn spinner(msg: &str) -> Spinner {
    if enabled() {
        // Braille frames pre-colored teal; indicatif (via `console`) strips the
        // ANSI codes when measuring width, so colored frames render correctly.
        let frames: Vec<String> = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]
            .iter()
            .map(|f| f.truecolor(ACCENT.0, ACCENT.1, ACCENT.2).to_string())
            .collect();
        let frame_refs: Vec<&str> = frames.iter().map(String::as_str).collect();
        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::with_template("{spinner} {msg}")
                .unwrap_or_else(|_| ProgressStyle::default_spinner())
                .tick_strings(&frame_refs),
        );
        bar.set_message(msg.to_string());
        bar.enable_steady_tick(Duration::from_millis(80));
        Spinner { bar: Some(bar) }
    } else {
        Spinner { bar: None }
    }
}

/// Finish a spinner successfully, replacing it with a teal `✓ msg` line.
pub fn finish_ok(sp: Spinner, msg: &str) {
    match sp.bar {
        Some(bar) => {
            bar.set_style(
                ProgressStyle::with_template("{msg}")
                    .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
            let check = "✓".truecolor(ACCENT.0, ACCENT.1, ACCENT.2).to_string();
            bar.finish_with_message(format!("{check} {msg}"));
        }
        None => step(msg),
    }
}

/// Finish a spinner with a failure, replacing it with a red `✗ msg` line.
pub fn finish_err(sp: Spinner, msg: &str) {
    match sp.bar {
        Some(bar) => {
            bar.set_style(
                ProgressStyle::with_template("{msg}")
                    .unwrap_or_else(|_| ProgressStyle::default_spinner()),
            );
            let cross = "✗".truecolor(220, 80, 80).to_string();
            bar.abandon_with_message(format!("{cross} {msg}"));
        }
        None => error(msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorate_only_on_tty_without_no_color() {
        // The happy path: an interactive terminal and the user has not opted out.
        assert!(decorate(false, true));
    }

    #[test]
    fn decorate_off_when_no_color_set() {
        // NO_COLOR wins even on a TTY.
        assert!(!decorate(true, true));
    }

    #[test]
    fn decorate_off_when_not_a_tty() {
        // Piped / redirected / CI: no color even if NO_COLOR is unset.
        assert!(!decorate(false, false));
    }

    #[test]
    fn decorate_off_when_not_tty_and_no_color() {
        assert!(!decorate(true, false));
    }

    #[test]
    fn wordmark_is_the_exact_branded_art() {
        // Guard against accidental edits to the embedded wordmark.
        assert!(WORDMARK.starts_with("             _           _"));
        assert_eq!(WORDMARK.lines().count(), 6);
    }
}
