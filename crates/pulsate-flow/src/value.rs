//! Flow's typed value model and the typing of bare atoms.
//!
//! The lexer emits bare runs of text as atoms; this module gives them types per
//! `docs/04-configuration.md#value-types` (duration, size, rate, bool, int,
//! float, ref, secret, env, string). Typing is context-free and deterministic:
//! the parser calls [`type_atom`] and the resulting [`Value`] carries the same
//! span as the atom for later diagnostics.

use std::time::Duration;

use pulsate_util::{parse_duration, parse_size};

use crate::span::Span;

/// The window of a rate literal (`100/min`, `10/s`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateWindow {
    /// per second
    Second,
    /// per minute
    Minute,
    /// per hour
    Hour,
}

/// A typed Flow value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// A string or bare identifier/host/path.
    Str(String),
    /// An integer.
    Int(i64),
    /// A floating-point number (weights, percentages).
    Float(f64),
    /// A boolean (`true`/`false`/`on`/`off`).
    Bool(bool),
    /// A duration (`30s`).
    Duration(Duration),
    /// A byte size in bytes (`10MB` → 10_000_000).
    Size(u64),
    /// A rate literal (`100/min`).
    Rate {
        /// Count per window.
        count: u64,
        /// The window unit.
        per: RateWindow,
    },
    /// An array of values.
    Array(Vec<Spanned<Value>>),
    /// A `@name` reference to a named block.
    Ref(String),
    /// A `secret://name` reference, resolved by a secrets backend at load.
    Secret(String),
    /// A `${VAR}` / `${VAR:-default}` environment reference.
    Env {
        /// The variable name.
        var: String,
        /// Optional default if unset.
        default: Option<String>,
    },
}

/// A value (or any node) paired with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Spanned<T> {
    /// The node.
    pub node: T,
    /// Its source span.
    pub span: Span,
}

impl<T> Spanned<T> {
    /// Pair a node with a span.
    pub const fn new(node: T, span: Span) -> Self {
        Self { node, span }
    }
}

impl Value {
    /// A short type name for diagnostics (`duration`, `int`, `ref`, …).
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Str(_) => "string",
            Value::Int(_) => "int",
            Value::Float(_) => "float",
            Value::Bool(_) => "bool",
            Value::Duration(_) => "duration",
            Value::Size(_) => "size",
            Value::Rate { .. } => "rate",
            Value::Array(_) => "array",
            Value::Ref(_) => "ref",
            Value::Secret(_) => "secret",
            Value::Env { .. } => "env",
        }
    }
}

/// Type a bare atom into a [`Value`], deterministically.
///
/// Order matters: references and secrets are recognized by prefix, then the
/// numeric-with-unit forms (rate/duration/size), then plain numbers and bools,
/// falling back to a string for identifiers/hosts/paths/regexes.
#[must_use]
pub fn type_atom(raw: &str) -> Value {
    if let Some(name) = raw.strip_prefix('@') {
        return Value::Ref(name.to_string());
    }
    if let Some(name) = raw.strip_prefix("secret://") {
        return Value::Secret(name.to_string());
    }
    match raw {
        "true" | "on" => return Value::Bool(true),
        "false" | "off" => return Value::Bool(false),
        _ => {}
    }
    if let Some(rate) = try_rate(raw) {
        return rate;
    }
    // Plain integer (before duration/size so "100" is an int, not an error).
    if let Ok(i) = raw.parse::<i64>() {
        return Value::Int(i);
    }
    if let Some(f) = try_float(raw) {
        return Value::Float(f);
    }
    if has_numeric_prefix(raw) {
        if let Ok(d) = parse_duration(raw) {
            return Value::Duration(d);
        }
        if let Ok(s) = parse_size(raw) {
            return Value::Size(s);
        }
    }
    Value::Str(raw.to_string())
}

/// Build an [`Value::Env`] from the inner text of a `${...}` token.
#[must_use]
pub fn type_env(inner: &str) -> Value {
    if let Some((var, default)) = inner.split_once(":-") {
        Value::Env {
            var: var.trim().to_string(),
            default: Some(default.to_string()),
        }
    } else {
        Value::Env {
            var: inner.trim().to_string(),
            default: None,
        }
    }
}

fn has_numeric_prefix(s: &str) -> bool {
    s.as_bytes().first().is_some_and(u8::is_ascii_digit)
}

fn try_float(s: &str) -> Option<f64> {
    // Only treat as float if it contains a dot and parses; avoids hostnames.
    if s.contains('.') && s.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        s.parse::<f64>().ok()
    } else {
        None
    }
}

fn try_rate(s: &str) -> Option<Value> {
    let (num, unit) = s.split_once('/')?;
    let count: u64 = num.parse().ok()?;
    let per = match unit {
        "s" | "sec" => RateWindow::Second,
        "min" | "m" => RateWindow::Minute,
        "h" | "hr" => RateWindow::Hour,
        _ => return None,
    };
    Some(Value::Rate { count, per })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn references_and_secrets() {
        assert_eq!(type_atom("@api"), Value::Ref("api".into()));
        assert_eq!(type_atom("secret://db_pw"), Value::Secret("db_pw".into()));
    }

    #[test]
    fn booleans_and_aliases() {
        assert_eq!(type_atom("true"), Value::Bool(true));
        assert_eq!(type_atom("off"), Value::Bool(false));
    }

    #[test]
    fn numbers_units_and_rates() {
        assert_eq!(type_atom("8080"), Value::Int(8080));
        assert_eq!(type_atom("0.5"), Value::Float(0.5));
        assert_eq!(type_atom("30s"), Value::Duration(Duration::from_secs(30)));
        assert_eq!(type_atom("10MB"), Value::Size(10_000_000));
        assert_eq!(
            type_atom("100/min"),
            Value::Rate {
                count: 100,
                per: RateWindow::Minute
            }
        );
    }

    #[test]
    fn hosts_and_paths_stay_strings() {
        assert_eq!(
            type_atom("app.example.com"),
            Value::Str("app.example.com".into())
        );
        assert_eq!(type_atom("/api/*"), Value::Str("/api/*".into()));
        assert_eq!(type_atom("least_conn"), Value::Str("least_conn".into()));
    }

    #[test]
    fn env_with_and_without_default() {
        assert_eq!(
            type_env("PORT:-8080"),
            Value::Env {
                var: "PORT".into(),
                default: Some("8080".into())
            }
        );
        assert_eq!(
            type_env("ORIGIN_URL"),
            Value::Env {
                var: "ORIGIN_URL".into(),
                default: None
            }
        );
    }
}
