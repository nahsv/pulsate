//! Structured access logging.
//!
//! Each completed request emits one JSON line at the [Finalize] stage
//! (`docs/15-observability.md`), correlated to metrics and traces by the request
//! ID. JSON is rendered by hand (no serde dependency) with correct string
//! escaping so log lines are always valid.

use std::fmt::Write as _;

/// One access-log record.
#[derive(Debug, Clone)]
pub struct AccessLog<'a> {
    /// Unix timestamp (ms) when the request completed.
    pub ts_ms: u64,
    /// HTTP method.
    pub method: &'a str,
    /// Request host.
    pub host: &'a str,
    /// Request path.
    pub path: &'a str,
    /// Response status code.
    pub status: u16,
    /// Total request duration in milliseconds.
    pub dur_ms: f64,
    /// Correlation request ID.
    pub request_id: &'a str,
    /// Response body size in bytes.
    pub bytes: u64,
}

impl AccessLog<'_> {
    /// Render the record as a single JSON object (no trailing newline).
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut s = String::with_capacity(160);
        s.push('{');
        let _ = write!(s, "\"ts\":{}", self.ts_ms);
        push_str_field(&mut s, "method", self.method);
        push_str_field(&mut s, "host", self.host);
        push_str_field(&mut s, "path", self.path);
        let _ = write!(s, ",\"status\":{}", self.status);
        let _ = write!(s, ",\"dur_ms\":{:.3}", self.dur_ms);
        push_str_field(&mut s, "req_id", self.request_id);
        let _ = write!(s, ",\"bytes\":{}", self.bytes);
        s.push('}');
        s
    }
}

fn push_str_field(out: &mut String, key: &str, value: &str) {
    let _ = write!(out, ",\"{key}\":\"{}\"", escape(value));
}

fn escape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_valid_json_with_escaping() {
        let log = AccessLog {
            ts_ms: 1_700_000_000_000,
            method: "GET",
            host: "app.example.com",
            path: "/a\"b",
            status: 200,
            dur_ms: 1.5,
            request_id: "01JTEST",
            bytes: 42,
        };
        let json = log.to_json();
        assert!(json.contains("\"status\":200"));
        assert!(json.contains("\"path\":\"/a\\\"b\""));
        assert!(json.contains("\"dur_ms\":1.500"));
        assert!(json.starts_with('{') && json.ends_with('}'));
    }
}
