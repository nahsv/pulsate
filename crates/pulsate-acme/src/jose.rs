//! JOSE primitives for the ACME account (RFC 7515 JWS, RFC 7518 ES256,
//! RFC 7638 JWK thumbprint), as required by RFC 8555.
//!
//! ACME authenticates every request with a flattened JWS signed by the account
//! key. Pulsate uses an ECDSA P-256 key (`ES256`) — the smallest widely-accepted
//! account key, and the one Let's Encrypt recommends. The first request
//! (`newAccount`) embeds the public key as a `jwk`; every later request
//! identifies the account by its `kid` (the account URL the CA returns).
//!
//! The same account key produces the HTTP-01 *key authorization*
//! (`token "." thumbprint`), so this module is also what the data plane needs to
//! answer challenges.

use base64ct::{Base64UrlUnpadded, Encoding};
use p256::ecdsa::{signature::Signer, Signature, SigningKey};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, LineEnding};
use pulsate_core::{Code, PulsateError, Result};
use serde::Serialize;
use sha2::{Digest, Sha256};

/// Base64url-encode without padding (the encoding JOSE uses everywhere).
fn b64(bytes: impl AsRef<[u8]>) -> String {
    Base64UrlUnpadded::encode_string(bytes.as_ref())
}

fn crypto_err(msg: impl Into<String>) -> PulsateError {
    PulsateError::new(Code::SYS_GENERIC, msg)
}

/// An ACME account key: an ECDSA P-256 signing key.
///
/// Cheap to clone is *not* offered deliberately — the key is secret and should
/// live behind an `Arc` in the client, not be copied around.
#[derive(Clone)]
pub struct AccountKey {
    signing: SigningKey,
}

impl std::fmt::Debug for AccountKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.debug_struct("AccountKey").finish_non_exhaustive()
    }
}

impl AccountKey {
    /// Generate a fresh random account key.
    #[must_use]
    pub fn generate() -> Self {
        Self {
            signing: SigningKey::random(&mut rand_core::OsRng),
        }
    }

    /// Load an account key from a PKCS#8 PEM (as persisted by [`to_pkcs8_pem`]).
    ///
    /// [`to_pkcs8_pem`]: AccountKey::to_pkcs8_pem
    ///
    /// # Errors
    /// Returns an error if the PEM is not a valid P-256 PKCS#8 private key.
    pub fn from_pkcs8_pem(pem: &str) -> Result<Self> {
        let signing = SigningKey::from_pkcs8_pem(pem)
            .map_err(|e| crypto_err(format!("invalid account key PEM: {e}")))?;
        Ok(Self { signing })
    }

    /// Serialize the account key as PKCS#8 PEM, for persistence via
    /// `pulsate-secrets`. The returned string contains private key material.
    ///
    /// # Errors
    /// Returns an error if the key cannot be encoded.
    pub fn to_pkcs8_pem(&self) -> Result<String> {
        self.signing
            .to_pkcs8_pem(LineEnding::LF)
            .map(|s| s.as_str().to_owned())
            .map_err(|e| crypto_err(format!("cannot encode account key: {e}")))
    }

    /// The public JWK for this key, as embedded in a `newAccount` request.
    #[must_use]
    pub fn jwk(&self) -> Jwk {
        let point = self.signing.verifying_key().to_encoded_point(false);
        // `false` => uncompressed, so both coordinates are present.
        let x = point.x().expect("P-256 point has an x coordinate");
        let y = point.y().expect("P-256 point has a y coordinate");
        Jwk {
            crv: "P-256",
            kty: "EC",
            x: b64(x),
            y: b64(y),
        }
    }

    /// The RFC 7638 JWK thumbprint: `base64url(SHA-256(canonical-jwk-json))`.
    ///
    /// The canonical form has exactly the required members (`crv`, `kty`, `x`,
    /// `y`), lexicographically ordered, with no whitespace.
    #[must_use]
    pub fn thumbprint(&self) -> String {
        let jwk = self.jwk();
        // Built by hand to guarantee member order and the absence of whitespace,
        // independent of any serializer's behavior.
        let canonical = format!(
            r#"{{"crv":"{}","kty":"{}","x":"{}","y":"{}"}}"#,
            jwk.crv, jwk.kty, jwk.x, jwk.y
        );
        b64(Sha256::digest(canonical.as_bytes()))
    }

    /// The HTTP-01 key authorization for `token`: `token "." thumbprint`
    /// (RFC 8555 §8.1). This is the exact body served at the challenge path.
    #[must_use]
    pub fn key_authorization(&self, token: &str) -> String {
        format!("{token}.{}", self.thumbprint())
    }

    /// Build a signed, flattened JWS request body for ACME.
    ///
    /// `payload` is `None` for a POST-as-GET (RFC 8555 §6.3, empty payload) and
    /// `Some(value)` for a normal POST. `key_id` selects `jwk` embedding (for
    /// `newAccount`) or `kid` (every other request).
    ///
    /// # Errors
    /// Returns an error if the payload or header cannot be serialized.
    pub fn sign_request(
        &self,
        url: &str,
        nonce: &str,
        key_id: KeyId<'_>,
        payload: Option<&serde_json::Value>,
    ) -> Result<String> {
        let protected = Protected {
            alg: "ES256",
            nonce,
            url,
            jwk: match key_id {
                KeyId::Jwk => Some(self.jwk()),
                KeyId::Kid(_) => None,
            },
            kid: match key_id {
                KeyId::Kid(kid) => Some(kid),
                KeyId::Jwk => None,
            },
        };

        let protected_b64 = b64(serde_json::to_vec(&protected)
            .map_err(|e| crypto_err(format!("cannot serialize JWS header: {e}")))?);
        let payload_b64 = match payload {
            // POST-as-GET sends a genuinely empty payload, not `"null"`.
            None => String::new(),
            Some(v) => b64(serde_json::to_vec(v)
                .map_err(|e| crypto_err(format!("cannot serialize JWS payload: {e}")))?),
        };

        let signing_input = format!("{protected_b64}.{payload_b64}");
        // `Signer<Signature>` for a P-256 `SigningKey` hashes with SHA-256, i.e.
        // ES256. `Signature::to_bytes` is the fixed 64-byte `R || S` form JWS
        // requires (not DER).
        let signature: Signature = self.signing.sign(signing_input.as_bytes());
        let signature_b64 = b64(signature.to_bytes());

        let jws = Jws {
            protected: protected_b64,
            payload: payload_b64,
            signature: signature_b64,
        };
        serde_json::to_string(&jws).map_err(|e| crypto_err(format!("cannot serialize JWS: {e}")))
    }
}

/// How a JWS identifies the signing account.
#[derive(Debug, Clone, Copy)]
pub enum KeyId<'a> {
    /// Embed the public key (`newAccount` only).
    Jwk,
    /// Reference the account URL the CA assigned.
    Kid(&'a str),
}

/// A public JSON Web Key for an ECDSA P-256 key. Field order matches the
/// RFC 7638 canonical ordering so a direct serialization is already canonical.
#[derive(Debug, Clone, Serialize)]
pub struct Jwk {
    /// The curve: always `P-256` for an ES256 account key.
    pub crv: &'static str,
    /// The key type: always `EC`.
    pub kty: &'static str,
    /// The base64url-encoded affine x coordinate.
    pub x: String,
    /// The base64url-encoded affine y coordinate.
    pub y: String,
}

/// The protected header of an ACME JWS.
#[derive(Debug, Serialize)]
struct Protected<'a> {
    alg: &'static str,
    nonce: &'a str,
    url: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    jwk: Option<Jwk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    kid: Option<&'a str>,
}

/// A flattened JWS (RFC 7515 §7.2.2), the body shape ACME expects.
#[derive(Debug, Serialize)]
struct Jws {
    protected: String,
    payload: String,
    signature: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::{signature::Verifier, VerifyingKey};

    #[test]
    fn pkcs8_roundtrip_preserves_key() {
        let key = AccountKey::generate();
        let pem = key.to_pkcs8_pem().unwrap();
        let loaded = AccountKey::from_pkcs8_pem(&pem).unwrap();
        // Same key => same public JWK => same thumbprint.
        assert_eq!(key.thumbprint(), loaded.thumbprint());
        assert_eq!(key.jwk().x, loaded.jwk().x);
    }

    #[test]
    fn thumbprint_is_deterministic_and_url_safe() {
        let key = AccountKey::generate();
        let t1 = key.thumbprint();
        let t2 = key.thumbprint();
        assert_eq!(t1, t2);
        // base64url, unpadded: no '+', '/', or '='.
        assert!(!t1.contains(['+', '/', '=']));
        // SHA-256 => 32 bytes => 43 base64url chars unpadded.
        assert_eq!(t1.len(), 43);
    }

    #[test]
    fn key_authorization_is_token_dot_thumbprint() {
        let key = AccountKey::generate();
        let ka = key.key_authorization("tok123");
        let (token, thumb) = ka.split_once('.').unwrap();
        assert_eq!(token, "tok123");
        assert_eq!(thumb, key.thumbprint());
    }

    #[test]
    fn jwk_canonical_json_orders_members() {
        // Hand-canonical form used for the thumbprint must be crv,kty,x,y.
        let key = AccountKey::generate();
        let jwk = key.jwk();
        let canonical = format!(
            r#"{{"crv":"{}","kty":"{}","x":"{}","y":"{}"}}"#,
            jwk.crv, jwk.kty, jwk.x, jwk.y
        );
        // Members appear in lexicographic order with no whitespace.
        assert!(canonical.starts_with(r#"{"crv":"P-256","kty":"EC","x":"#));
        assert_eq!(b64(Sha256::digest(canonical.as_bytes())), key.thumbprint());
    }

    #[test]
    fn signed_request_verifies_against_account_public_key() {
        let key = AccountKey::generate();
        let payload = serde_json::json!({"termsOfServiceAgreed": true});
        let body = key
            .sign_request(
                "https://ca/acme/new-acct",
                "nonce-1",
                KeyId::Jwk,
                Some(&payload),
            )
            .unwrap();

        // Reconstruct the signing input and verify the signature with the public
        // key — exactly what the CA does.
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let protected_b64 = v["protected"].as_str().unwrap();
        let payload_b64 = v["payload"].as_str().unwrap();
        let sig_b64 = v["signature"].as_str().unwrap();
        let signing_input = format!("{protected_b64}.{payload_b64}");
        let sig_bytes = Base64UrlUnpadded::decode_vec(sig_b64).unwrap();
        let signature = Signature::from_slice(&sig_bytes).unwrap();
        let vk: VerifyingKey = *key.signing.verifying_key();
        assert!(vk.verify(signing_input.as_bytes(), &signature).is_ok());

        // newAccount embeds the jwk, never a kid.
        let header: serde_json::Value =
            serde_json::from_slice(&Base64UrlUnpadded::decode_vec(protected_b64).unwrap()).unwrap();
        assert_eq!(header["alg"], "ES256");
        assert!(header.get("jwk").is_some());
        assert!(header.get("kid").is_none());
    }

    #[test]
    fn post_as_get_has_empty_payload() {
        let key = AccountKey::generate();
        let body = key
            .sign_request(
                "https://ca/acme/order/1",
                "nonce-2",
                KeyId::Kid("https://ca/acct/1"),
                None,
            )
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        // Empty string payload, and the header carries kid (not jwk).
        assert_eq!(v["payload"], "");
        let header: serde_json::Value = serde_json::from_slice(
            &Base64UrlUnpadded::decode_vec(v["protected"].as_str().unwrap()).unwrap(),
        )
        .unwrap();
        assert_eq!(header["kid"], "https://ca/acct/1");
        assert!(header.get("jwk").is_none());
    }
}
