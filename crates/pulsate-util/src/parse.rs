//! Duration and byte-size parsing for the Flow config language.
//!
//! Flow uses human units consistently (`30s`, `10MB`) rather than bare numbers
//! (`docs/04-configuration.md`). These parsers are the canonical interpretation;
//! `pulsate-flow` renders the diagnostics on failure.

use std::time::Duration;

/// Error returned when a duration or size string cannot be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    input: String,
    reason: &'static str,
}

impl ParseError {
    fn new(input: &str, reason: &'static str) -> Self {
        Self {
            input: input.to_string(),
            reason,
        }
    }

    /// A short reason describing why parsing failed.
    #[must_use]
    pub fn reason(&self) -> &str {
        self.reason
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid value {:?}: {}", self.input, self.reason)
    }
}

impl std::error::Error for ParseError {}

/// Split a value into its leading numeric part and its trailing unit suffix.
fn split_number_unit(s: &str) -> Option<(u64, &str)> {
    let s = s.trim();
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    if split == 0 {
        return None; // no leading number
    }
    let (num, unit) = s.split_at(split);
    let value: u64 = num.parse().ok()?;
    Some((value, unit.trim()))
}

/// Parse a duration like `500ms`, `30s`, `5m`, `2h`, `1d`.
///
/// # Errors
/// Returns [`ParseError`] for empty input, a missing/unknown unit, or a number
/// that does not fit a `u64`.
pub fn parse_duration(s: &str) -> Result<Duration, ParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseError::new(s, "empty duration"));
    }
    let (value, unit) =
        split_number_unit(s).ok_or_else(|| ParseError::new(s, "expected <number><unit>"))?;
    let dur = match unit {
        "ms" => Duration::from_millis(value),
        "s" => Duration::from_secs(value),
        "m" => Duration::from_secs(value.saturating_mul(60)),
        "h" => Duration::from_secs(value.saturating_mul(3600)),
        "d" => Duration::from_secs(value.saturating_mul(86_400)),
        _ => return Err(ParseError::new(s, "unknown time unit (use ms, s, m, h, d)")),
    };
    Ok(dur)
}

/// Parse a byte size like `512`, `4KB`, `10MB`, `2GB` (decimal units, base 1000).
///
/// A bare number is interpreted as bytes. Units are case-insensitive.
///
/// # Errors
/// Returns [`ParseError`] for empty input, an unknown unit, or overflow.
pub fn parse_size(s: &str) -> Result<u64, ParseError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(ParseError::new(s, "empty size"));
    }
    // A bare integer is bytes.
    if let Ok(bytes) = s.parse::<u64>() {
        return Ok(bytes);
    }
    let (value, unit) =
        split_number_unit(s).ok_or_else(|| ParseError::new(s, "expected <number><unit>"))?;
    let mult: u64 = match unit.to_ascii_uppercase().as_str() {
        "B" => 1,
        "KB" => 1_000,
        "MB" => 1_000_000,
        "GB" => 1_000_000_000,
        "TB" => 1_000_000_000_000,
        _ => {
            return Err(ParseError::new(
                s,
                "unknown size unit (use B, KB, MB, GB, TB)",
            ))
        }
    };
    value
        .checked_mul(mult)
        .ok_or_else(|| ParseError::new(s, "size overflows u64"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_each_unit() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("2h").unwrap(), Duration::from_secs(7200));
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86_400));
    }

    #[test]
    fn duration_errors_are_descriptive() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("30").is_err()); // no unit
        assert!(parse_duration("30x").is_err()); // unknown unit
    }

    #[test]
    fn sizes_parse_bytes_and_units() {
        assert_eq!(parse_size("512").unwrap(), 512);
        assert_eq!(parse_size("4KB").unwrap(), 4_000);
        assert_eq!(parse_size("10MB").unwrap(), 10_000_000);
        assert_eq!(parse_size("2gb").unwrap(), 2_000_000_000); // case-insensitive
    }

    #[test]
    fn size_overflow_is_rejected() {
        assert!(parse_size("99999999999999TB").is_err());
        assert!(parse_size("7PB").is_err()); // unknown unit
    }
}
