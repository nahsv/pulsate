//! Minimal JSON writing (no serde dependency) for the admin API responses.

use std::fmt::Write as _;

/// Escape a string for embedding in a JSON document.
#[must_use]
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out
}

/// A `"key":"value"` JSON string field.
#[must_use]
pub fn field(key: &str, value: &str) -> String {
    format!("\"{}\":\"{}\"", esc(key), esc(value))
}

/// A `"key":<number>` JSON field.
#[must_use]
pub fn num_field(key: &str, value: u64) -> String {
    format!("\"{}\":{value}", esc(key))
}

/// Wrap comma-joined fields into a JSON object.
#[must_use]
pub fn object(fields: &[String]) -> String {
    format!("{{{}}}", fields.join(","))
}
