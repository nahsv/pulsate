//! A minimal CIDR range type for IP allow/deny rules.

use std::net::IpAddr;
use std::str::FromStr;

/// An IPv4 or IPv6 CIDR range (`10.0.0.0/8`, `2001:db8::/32`, or a bare address
/// treated as a `/32` or `/128`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    base: IpAddr,
    prefix: u8,
}

impl Cidr {
    /// Whether `ip` falls within this range. Mismatched families never match.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.base, ip) {
            (IpAddr::V4(base), IpAddr::V4(ip)) => {
                let mask = v4_mask(self.prefix);
                (u32::from_be_bytes(base.octets()) & mask)
                    == (u32::from_be_bytes(ip.octets()) & mask)
            }
            (IpAddr::V6(base), IpAddr::V6(ip)) => {
                let mask = v6_mask(self.prefix);
                (u128::from_be_bytes(base.octets()) & mask)
                    == (u128::from_be_bytes(ip.octets()) & mask)
            }
            _ => false,
        }
    }
}

fn v4_mask(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else if prefix >= 32 {
        u32::MAX
    } else {
        u32::MAX << (32 - prefix)
    }
}

fn v6_mask(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else if prefix >= 128 {
        u128::MAX
    } else {
        u128::MAX << (128 - prefix)
    }
}

impl FromStr for Cidr {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let base: IpAddr = addr
            .parse()
            .map_err(|_| format!("invalid IP address in CIDR `{s}`"))?;
        let max = if base.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p
                .parse::<u8>()
                .map_err(|_| format!("invalid prefix in CIDR `{s}`"))?,
            None => max,
        };
        if prefix > max {
            return Err(format!("prefix /{prefix} too large in CIDR `{s}`"));
        }
        Ok(Cidr { base, prefix })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v4_range_contains() {
        let c: Cidr = "10.0.0.0/8".parse().unwrap();
        assert!(c.contains("10.255.1.2".parse().unwrap()));
        assert!(!c.contains("11.0.0.1".parse().unwrap()));
    }

    #[test]
    fn host_address_is_slash_32() {
        let c: Cidr = "192.0.2.5".parse().unwrap();
        assert!(c.contains("192.0.2.5".parse().unwrap()));
        assert!(!c.contains("192.0.2.6".parse().unwrap()));
    }

    #[test]
    fn v6_range_contains_and_family_isolation() {
        let c: Cidr = "2001:db8::/32".parse().unwrap();
        assert!(c.contains("2001:db8::1".parse().unwrap()));
        assert!(!c.contains("2001:db9::1".parse().unwrap()));
        // v4 never matches a v6 range.
        assert!(!c.contains("10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn rejects_bad_input() {
        assert!("notanip/8".parse::<Cidr>().is_err());
        assert!("10.0.0.0/40".parse::<Cidr>().is_err());
    }
}
