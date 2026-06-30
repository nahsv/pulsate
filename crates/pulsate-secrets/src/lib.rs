//! `pulsate-secrets` — secret resolution backends.
//!
//! A `secret://name` reference in the config is resolved at load time (and on
//! rotation) by a configured backend, so secrets never appear literally in the
//! file (`docs/09-security.md`). Environment and file backends are implemented;
//! Vault and cloud-KMS backends are not.
//!
//! Backends expose a synchronous [`resolve`](SecretsBackend::resolve) and also
//! satisfy the async `pulsate_core::SecretsBackend` trait, since env/file lookups
//! do not block.
#![forbid(unsafe_code)]

use std::path::PathBuf;

use pulsate_core::{BoxFuture, Code, PulsateError, Result};
use zeroize::Zeroizing;

/// A backend that resolves a secret reference to its value.
pub trait SecretsBackend: Send + Sync {
    /// A stable backend name.
    fn name(&self) -> &str;

    /// Resolve `reference` to its secret value.
    ///
    /// # Errors
    /// Returns an error if the secret is missing or unreadable.
    fn resolve(&self, reference: &str) -> Result<String>;
}

/// Reads secrets from environment variables (`secret://DB_PASSWORD` → `$DB_PASSWORD`).
#[derive(Debug, Default, Clone, Copy)]
pub struct EnvBackend;

impl SecretsBackend for EnvBackend {
    #[allow(clippy::unnecessary_literal_bound)] // matches the trait signature
    fn name(&self) -> &str {
        "env"
    }

    fn resolve(&self, reference: &str) -> Result<String> {
        std::env::var(reference).map_err(|_| {
            PulsateError::new(
                Code::SYS_GENERIC,
                format!("secret `{reference}` not found in environment"),
            )
        })
    }
}

/// Reads secrets from files in a directory (`secret://api_key` → `<dir>/api_key`).
#[derive(Debug, Clone)]
pub struct FileBackend {
    dir: PathBuf,
}

impl FileBackend {
    /// Create a file backend rooted at `dir`.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }
}

impl SecretsBackend for FileBackend {
    #[allow(clippy::unnecessary_literal_bound)] // matches the trait signature
    fn name(&self) -> &str {
        "file"
    }

    fn resolve(&self, reference: &str) -> Result<String> {
        // Reject path traversal in the reference name.
        if reference.contains('/') || reference.contains("..") {
            return Err(PulsateError::new(
                Code::SYS_GENERIC,
                format!("invalid secret name `{reference}`"),
            ));
        }
        let path = self.dir.join(reference);
        // Hold the full file contents in a buffer that is zeroized on drop so the
        // plaintext (often larger than the trimmed secret) does not linger in
        // freed heap memory (LOW).
        let raw = Zeroizing::new(std::fs::read_to_string(&path).map_err(|e| {
            PulsateError::new(
                Code::SYS_GENERIC,
                format!("secret `{reference}` unreadable: {e}"),
            )
        })?);
        Ok(raw.trim_end_matches(['\n', '\r']).to_string())
    }
}

/// Implement the async `pulsate_core::SecretsBackend` for a sync backend. The
/// lookup does not block, so the returned future is immediately ready. (A
/// blanket impl is impossible here under the orphan rule, so each backend opts
/// in explicitly via this macro.)
macro_rules! impl_core_backend {
    ($ty:ty) => {
        impl pulsate_core::SecretsBackend for $ty {
            fn backend(&self) -> &str {
                SecretsBackend::name(self)
            }

            fn resolve<'a>(&'a self, reference: &'a str) -> BoxFuture<'a, Result<String>> {
                let result = SecretsBackend::resolve(self, reference);
                Box::pin(async move { result })
            }
        }
    };
}

impl_core_backend!(EnvBackend);
impl_core_backend!(FileBackend);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_backend_reads_var() {
        std::env::set_var("PULSE_TEST_SECRET", "swordfish");
        let backend = EnvBackend;
        assert_eq!(backend.resolve("PULSE_TEST_SECRET").unwrap(), "swordfish");
        assert!(backend.resolve("PULSE_TEST_MISSING_XYZ").is_err());
    }

    #[test]
    fn file_backend_reads_and_trims() {
        let dir = std::env::temp_dir().join(format!("pulsate-secrets-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("api_key"), "topsecret\n").unwrap();

        let backend = FileBackend::new(&dir);
        assert_eq!(backend.resolve("api_key").unwrap(), "topsecret");
        assert!(backend.resolve("missing").is_err());
        // Traversal is rejected.
        assert!(backend.resolve("../etc/passwd").is_err());

        std::fs::remove_dir_all(&dir).ok();
    }
}
