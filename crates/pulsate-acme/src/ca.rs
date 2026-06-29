//! Well-known ACME certificate-authority directory URLs.
//!
//! The protocol client ([`crate::AcmeClient`]) is CA-agnostic — it takes a
//! directory URL. These constants name the common authorities so configuration
//! can default sensibly and offer staging for testing.

/// Let's Encrypt production. The default CA when configuration names none.
pub const LETS_ENCRYPT: &str = "https://acme-v02.api.letsencrypt.org/directory";

/// Let's Encrypt staging — untrusted certs, but far higher rate limits. Use this
/// while testing issuance so you don't burn production rate limits.
pub const LETS_ENCRYPT_STAGING: &str = "https://acme-staging-v02.api.letsencrypt.org/directory";

/// ZeroSSL (also ACME, ES256 accounts).
pub const ZEROSSL: &str = "https://acme.zerossl.com/v2/DV90";

/// Google Trust Services.
pub const GOOGLE: &str = "https://dv.acme-v02.api.pki.goog/directory";

/// The CA used when configuration does not specify one: Let's Encrypt production.
pub const DEFAULT: &str = LETS_ENCRYPT;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_lets_encrypt_production() {
        assert_eq!(DEFAULT, LETS_ENCRYPT);
        assert_ne!(DEFAULT, LETS_ENCRYPT_STAGING);
    }

    #[test]
    fn all_directories_are_https() {
        for url in [LETS_ENCRYPT, LETS_ENCRYPT_STAGING, ZEROSSL, GOOGLE] {
            assert!(url.starts_with("https://"), "{url} must be https");
        }
    }
}
