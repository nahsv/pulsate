//! Request identifiers.
//!
//! A request ID is a 26-character Crockford base32 string in the shape of a ULID
//! (`docs/15-observability.md`): a 48-bit millisecond timestamp prefix makes IDs
//! roughly time-sortable, and a per-process monotonic counter guarantees
//! uniqueness within a millisecond without needing a random-number dependency.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static COUNTER: AtomicU64 = AtomicU64::new(0);

const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// Current Unix time in milliseconds (saturating at 0 before the epoch).
#[must_use]
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// Generate a new, roughly time-sortable request ID.
#[must_use]
pub fn request_id() -> String {
    let ms = u128::from(now_ms()) & ((1 << 48) - 1);
    let ctr = u128::from(COUNTER.fetch_add(1, Ordering::Relaxed));
    // Spread the counter across the 80 low bits so consecutive IDs differ widely.
    let low = ctr.wrapping_mul(0x9E37_79B9_7F4A_7C15) & ((1 << 80) - 1);
    encode((ms << 80) | low)
}

fn encode(mut v: u128) -> String {
    let mut out = [0u8; 26];
    for slot in out.iter_mut().rev() {
        *slot = CROCKFORD[(v & 0x1f) as usize];
        v >>= 5;
    }
    // All bytes are ASCII from CROCKFORD, so this never fails.
    String::from_utf8(out.to_vec()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_26_chars_and_unique() {
        let a = request_id();
        let b = request_id();
        assert_eq!(a.len(), 26);
        assert_eq!(b.len(), 26);
        assert_ne!(a, b);
    }

    #[test]
    fn ids_use_only_crockford_alphabet() {
        let id = request_id();
        assert!(id.bytes().all(|c| CROCKFORD.contains(&c)));
    }
}
