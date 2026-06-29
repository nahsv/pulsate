//! Transport-agnostic RFC 8555 ACME protocol client.
//!
//! The protocol logic here is generic over an [`AcmeTransport`] so the entire
//! order state machine — directory discovery, nonce handling, account
//! registration, order creation, HTTP-01 authorization, finalization, and
//! certificate download — is exercised offline against a mock CA in tests. A
//! real HTTPS transport is a thin adapter implementing one method (next
//! increment; see `docs/ROADMAP.md`).
//!
//! Everything is signed with the account key via [`crate::AccountKey`] (ES256
//! JWS, RFC 7515). The first request embeds the public `jwk`; the CA returns an
//! account URL that is used as the `kid` for every later request.

use std::future::Future;
use std::sync::Mutex;

use base64ct::Encoding;
use pulsate_core::{Code, PulsateError, Result};
use serde::Deserialize;

use crate::jose::{AccountKey, KeyId};

/// HTTP method for an ACME request. ACME uses GET only for the directory and
/// nonce; everything else is POST (including POST-as-GET reads).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// `GET` — directory only.
    Get,
    /// `HEAD` — `newNonce` only.
    Head,
    /// `POST` — every signed request, including POST-as-GET reads.
    Post,
}

/// The subset of an HTTP response the ACME client needs.
#[derive(Debug, Clone, Default)]
pub struct HttpResponse {
    /// HTTP status code.
    pub status: u16,
    /// `Replay-Nonce` header — fed back into the nonce pool.
    pub replay_nonce: Option<String>,
    /// `Location` header — the account URL on `newAccount`, the order URL on
    /// `newOrder`.
    pub location: Option<String>,
    /// `Retry-After` header in seconds, if present (used when polling).
    pub retry_after: Option<u64>,
    /// Response body bytes.
    pub body: Vec<u8>,
}

impl HttpResponse {
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// An async HTTP transport for ACME. One method; the protocol logic lives in
/// [`AcmeClient`]. Implemented by a real HTTPS client in production and by a
/// mock CA in tests.
pub trait AcmeTransport {
    /// Execute a single request. `body` is the signed JWS for POSTs, `None` for
    /// GET/HEAD.
    fn execute(
        &self,
        method: Method,
        url: &str,
        body: Option<String>,
    ) -> impl Future<Output = Result<HttpResponse>> + Send;
}

/// The ACME directory (RFC 8555 §7.1.1): the CA's endpoint URLs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Directory {
    /// `newNonce` endpoint.
    pub new_nonce: String,
    /// `newAccount` endpoint.
    pub new_account: String,
    /// `newOrder` endpoint.
    pub new_order: String,
    /// `revokeCert` endpoint, if the CA advertises one.
    #[serde(default)]
    pub revoke_cert: Option<String>,
}

/// An ACME order (RFC 8555 §7.1.3).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Order {
    /// Order status: `pending`, `ready`, `processing`, `valid`, or `invalid`.
    pub status: String,
    /// Authorization URLs that must be satisfied before finalization.
    #[serde(default)]
    pub authorizations: Vec<String>,
    /// The finalize endpoint to POST the CSR to.
    pub finalize: String,
    /// The certificate download URL, present once the order is `valid`.
    #[serde(default)]
    pub certificate: Option<String>,
    /// Order URL (from the `Location` header, not the body) — filled in by the
    /// client, not deserialized.
    #[serde(skip)]
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Authorization {
    challenges: Vec<Challenge>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Challenge {
    r#type: String,
    url: String,
    token: String,
}

/// An HTTP-01 challenge the data plane must answer: serve `key_authorization`
/// at `/.well-known/acme-challenge/{token}`.
#[derive(Debug, Clone)]
pub struct Http01Challenge {
    /// The challenge token; the response is served at
    /// `/.well-known/acme-challenge/{token}`.
    pub token: String,
    /// The exact body to serve at the challenge path (`token "." thumbprint`).
    pub key_authorization: String,
    /// The challenge URL to POST once the response is in place.
    pub url: String,
}

fn protocol_err(msg: impl Into<String>) -> PulsateError {
    PulsateError::new(Code::ACME_CHALLENGE, msg)
}

/// A registered ACME client bound to one account key and one CA directory.
pub struct AcmeClient<T: AcmeTransport> {
    transport: T,
    account_key: AccountKey,
    directory: Directory,
    /// Account URL (`kid`) once registered.
    account_url: Mutex<Option<String>>,
    /// Pool of unused replay nonces.
    nonces: Mutex<Vec<String>>,
}

impl<T: AcmeTransport> AcmeClient<T> {
    /// Fetch the CA directory and construct a client. Does not register an
    /// account yet — call [`register_account`](Self::register_account).
    ///
    /// # Errors
    /// Returns an error if the directory cannot be fetched or parsed.
    pub async fn discover(
        transport: T,
        account_key: AccountKey,
        directory_url: &str,
    ) -> Result<Self> {
        let resp = transport.execute(Method::Get, directory_url, None).await?;
        if !resp.is_success() {
            return Err(protocol_err(format!(
                "directory fetch failed: HTTP {}",
                resp.status
            )));
        }
        let directory: Directory = serde_json::from_slice(&resp.body)
            .map_err(|e| protocol_err(format!("invalid ACME directory: {e}")))?;
        Ok(Self {
            transport,
            account_key,
            directory,
            account_url: Mutex::new(None),
            nonces: Mutex::new(Vec::new()),
        })
    }

    /// Get a fresh nonce, either from the pool or by asking the CA (RFC 8555 §7.2).
    async fn nonce(&self) -> Result<String> {
        if let Some(n) = self.nonces.lock().expect("nonce pool not poisoned").pop() {
            return Ok(n);
        }
        let resp = self
            .transport
            .execute(Method::Head, &self.directory.new_nonce, None)
            .await?;
        resp.replay_nonce
            .ok_or_else(|| protocol_err("newNonce response had no Replay-Nonce header"))
    }

    /// Stash a nonce returned by any response, so the next request can reuse it.
    fn store_nonce(&self, resp: &HttpResponse) {
        if let Some(n) = &resp.replay_nonce {
            self.nonces
                .lock()
                .expect("nonce pool not poisoned")
                .push(n.clone());
        }
    }

    /// Send a signed request, retrying once on a `badNonce` error (RFC 8555 §6.5).
    async fn signed(&self, url: &str, payload: Option<&serde_json::Value>) -> Result<HttpResponse> {
        for attempt in 0..2 {
            let nonce = self.nonce().await?;
            // Clone the kid out from under the lock so the JWS body can be built
            // without holding it. `None` => embed the jwk (newAccount).
            let kid_owned = self
                .account_url
                .lock()
                .expect("account url not poisoned")
                .clone();
            let key_id = match &kid_owned {
                Some(k) => KeyId::Kid(k),
                None => KeyId::Jwk,
            };
            let body = self
                .account_key
                .sign_request(url, &nonce, key_id, payload)?;
            let resp = self
                .transport
                .execute(Method::Post, url, Some(body))
                .await?;
            self.store_nonce(&resp);

            if resp.status == 400 && body_is_bad_nonce(&resp.body) && attempt == 0 {
                continue; // fetch a fresh nonce and retry once
            }
            return Ok(resp);
        }
        unreachable!("loop returns on the second attempt")
    }

    /// Register (or recover) the account for this key (RFC 8555 §7.3).
    ///
    /// # Errors
    /// Returns an error if the CA rejects the account registration.
    pub async fn register_account(&self, contact: &[String]) -> Result<String> {
        let payload = serde_json::json!({
            "termsOfServiceAgreed": true,
            "contact": contact,
        });
        let resp = self
            .signed(&self.directory.new_account, Some(&payload))
            .await?;
        if !resp.is_success() {
            return Err(account_err(&resp));
        }
        let url = resp
            .location
            .ok_or_else(|| protocol_err("newAccount response had no Location header"))?;
        *self.account_url.lock().expect("account url not poisoned") = Some(url.clone());
        Ok(url)
    }

    /// Create an order for `dns_names` (RFC 8555 §7.4).
    ///
    /// # Errors
    /// Returns an error if the CA rejects the order.
    pub async fn new_order(&self, dns_names: &[String]) -> Result<Order> {
        let identifiers: Vec<_> = dns_names
            .iter()
            .map(|n| serde_json::json!({"type": "dns", "value": n}))
            .collect();
        let payload = serde_json::json!({ "identifiers": identifiers });
        let resp = self
            .signed(&self.directory.new_order, Some(&payload))
            .await?;
        if !resp.is_success() {
            return Err(protocol_err(format!(
                "newOrder failed: HTTP {}",
                resp.status
            )));
        }
        let mut order: Order = serde_json::from_slice(&resp.body)
            .map_err(|e| protocol_err(format!("invalid order: {e}")))?;
        order.url = resp.location.unwrap_or_default();
        Ok(order)
    }

    /// Fetch an authorization and extract its HTTP-01 challenge, computing the
    /// key authorization the data plane must serve.
    ///
    /// # Errors
    /// Returns an error if the authorization has no HTTP-01 challenge.
    pub async fn http01_challenge(&self, authz_url: &str) -> Result<Http01Challenge> {
        let resp = self.signed(authz_url, None).await?; // POST-as-GET
        if !resp.is_success() {
            return Err(protocol_err(format!(
                "authorization fetch failed: HTTP {}",
                resp.status
            )));
        }
        let authz: Authorization = serde_json::from_slice(&resp.body)
            .map_err(|e| protocol_err(format!("invalid authorization: {e}")))?;
        let challenge = authz
            .challenges
            .into_iter()
            .find(|c| c.r#type == "http-01")
            .ok_or_else(|| protocol_err("authorization offered no http-01 challenge"))?;
        Ok(Http01Challenge {
            key_authorization: self.account_key.key_authorization(&challenge.token),
            token: challenge.token,
            url: challenge.url,
        })
    }

    /// Tell the CA the challenge response is in place (RFC 8555 §7.5.1).
    ///
    /// # Errors
    /// Returns an error if the CA rejects the challenge submission.
    pub async fn submit_challenge(&self, challenge_url: &str) -> Result<()> {
        // An empty JSON object payload triggers validation.
        let payload = serde_json::json!({});
        let resp = self.signed(challenge_url, Some(&payload)).await?;
        if !resp.is_success() {
            return Err(protocol_err(format!(
                "challenge submission failed: HTTP {}",
                resp.status
            )));
        }
        Ok(())
    }

    /// Poll the order until it leaves `pending`/`processing`, up to `max_attempts`.
    ///
    /// # Errors
    /// Returns an error if the order becomes `invalid` or never settles.
    pub async fn poll_order(&self, order_url: &str, max_attempts: usize) -> Result<Order> {
        for _ in 0..max_attempts {
            let resp = self.signed(order_url, None).await?; // POST-as-GET
            let mut order: Order = serde_json::from_slice(&resp.body)
                .map_err(|e| protocol_err(format!("invalid order: {e}")))?;
            order_url.clone_into(&mut order.url);
            match order.status.as_str() {
                "ready" | "valid" => return Ok(order),
                "invalid" => return Err(protocol_err("order became invalid")),
                _ => {} // pending / processing — keep polling
            }
        }
        Err(protocol_err(
            "order did not settle within the attempt budget",
        ))
    }

    /// Finalize the order with a CSR covering `dns_names`, returning the order
    /// (now carrying a `certificate` URL once valid) and the freshly generated
    /// private key PEM.
    ///
    /// # Errors
    /// Returns an error if CSR generation or finalization fails.
    pub async fn finalize(&self, order: &Order, dns_names: &[String]) -> Result<(Order, String)> {
        let key_pair = rcgen::KeyPair::generate()
            .map_err(|e| protocol_err(format!("keypair generation failed: {e}")))?;
        let params = rcgen::CertificateParams::new(dns_names.to_vec())
            .map_err(|e| protocol_err(format!("CSR params failed: {e}")))?;
        let csr = params
            .serialize_request(&key_pair)
            .map_err(|e| protocol_err(format!("CSR generation failed: {e}")))?;
        let csr_b64 = base64ct::Base64UrlUnpadded::encode_string(csr.der());

        let payload = serde_json::json!({ "csr": csr_b64 });
        let resp = self.signed(&order.finalize, Some(&payload)).await?;
        if !resp.is_success() {
            return Err(protocol_err(format!(
                "finalize failed: HTTP {}",
                resp.status
            )));
        }
        let mut finalized: Order = serde_json::from_slice(&resp.body)
            .map_err(|e| protocol_err(format!("invalid finalized order: {e}")))?;
        order.url.clone_into(&mut finalized.url);
        Ok((finalized, key_pair.serialize_pem()))
    }

    /// Download the issued certificate chain as PEM (RFC 8555 §7.4.2).
    ///
    /// # Errors
    /// Returns an error if the certificate cannot be fetched.
    pub async fn download_certificate(&self, certificate_url: &str) -> Result<String> {
        let resp = self.signed(certificate_url, None).await?; // POST-as-GET
        if !resp.is_success() {
            return Err(protocol_err(format!(
                "certificate download failed: HTTP {}",
                resp.status
            )));
        }
        String::from_utf8(resp.body)
            .map_err(|e| protocol_err(format!("certificate was not valid UTF-8 PEM: {e}")))
    }

    /// The CA directory this client discovered.
    #[must_use]
    pub fn directory(&self) -> &Directory {
        &self.directory
    }
}

fn body_is_bad_nonce(body: &[u8]) -> bool {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned))
        .is_some_and(|t| t.ends_with(":badNonce"))
}

fn account_err(resp: &HttpResponse) -> PulsateError {
    // Surface rate limiting distinctly so callers can back off (RFC 8555 §6.6).
    let problem = serde_json::from_slice::<serde_json::Value>(&resp.body).ok();
    let kind = problem
        .as_ref()
        .and_then(|v| v.get("type"))
        .and_then(|t| t.as_str())
        .unwrap_or("");
    if kind.ends_with(":rateLimited") || resp.status == 429 {
        return PulsateError::new(
            Code::ACME_RATE_LIMITED,
            "CA rate-limited the account request",
        );
    }
    protocol_err(format!("account registration failed: HTTP {}", resp.status))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    /// A scripted mock CA: maps a URL to a canned response, and records the
    /// signed bodies it received so tests can assert on them.
    #[derive(Default)]
    struct MockCa {
        routes: HashMap<String, HttpResponse>,
        seen: StdMutex<Vec<(Method, String)>>,
    }

    impl MockCa {
        fn route(mut self, url: &str, resp: HttpResponse) -> Self {
            self.routes.insert(url.to_owned(), resp);
            self
        }
    }

    impl AcmeTransport for MockCa {
        async fn execute(
            &self,
            method: Method,
            url: &str,
            _body: Option<String>,
        ) -> Result<HttpResponse> {
            self.seen.lock().unwrap().push((method, url.to_owned()));
            if method == Method::Head {
                // Any HEAD (newNonce) just yields a nonce.
                return Ok(HttpResponse {
                    status: 200,
                    replay_nonce: Some("nonce-head".into()),
                    ..Default::default()
                });
            }
            self.routes
                .get(url)
                .cloned()
                .ok_or_else(|| protocol_err(format!("mock CA has no route for {url}")))
        }
    }

    fn json_resp(status: u16, body: &serde_json::Value) -> HttpResponse {
        HttpResponse {
            status,
            replay_nonce: Some("nonce-next".into()),
            body: serde_json::to_vec(body).unwrap(),
            ..Default::default()
        }
    }

    fn directory_resp() -> HttpResponse {
        json_resp(
            200,
            &serde_json::json!({
                "newNonce": "https://ca/acme/new-nonce",
                "newAccount": "https://ca/acme/new-acct",
                "newOrder": "https://ca/acme/new-order",
            }),
        )
    }

    #[tokio::test]
    async fn discover_parses_directory() {
        let ca = MockCa::default().route("https://ca/acme/directory", directory_resp());
        let client = AcmeClient::discover(ca, AccountKey::generate(), "https://ca/acme/directory")
            .await
            .unwrap();
        assert_eq!(client.directory().new_order, "https://ca/acme/new-order");
    }

    #[tokio::test]
    async fn register_account_records_kid_from_location() {
        let mut acct = json_resp(201, &serde_json::json!({"status": "valid"}));
        acct.location = Some("https://ca/acme/acct/42".into());
        let ca = MockCa::default()
            .route("https://ca/acme/directory", directory_resp())
            .route("https://ca/acme/new-acct", acct);

        let client = AcmeClient::discover(ca, AccountKey::generate(), "https://ca/acme/directory")
            .await
            .unwrap();
        let kid = client
            .register_account(&["mailto:ops@example.com".into()])
            .await
            .unwrap();
        assert_eq!(kid, "https://ca/acme/acct/42");
        // The kid is now remembered for later requests.
        assert_eq!(
            client.account_url.lock().unwrap().as_deref(),
            Some("https://ca/acme/acct/42")
        );
    }

    #[tokio::test]
    async fn rate_limited_account_maps_to_acme_rate_limited_code() {
        let ca = MockCa::default()
            .route("https://ca/acme/directory", directory_resp())
            .route(
                "https://ca/acme/new-acct",
                json_resp(
                    429,
                    &serde_json::json!({"type": "urn:ietf:params:acme:error:rateLimited"}),
                ),
            );
        let client = AcmeClient::discover(ca, AccountKey::generate(), "https://ca/acme/directory")
            .await
            .unwrap();
        let err = client.register_account(&[]).await.unwrap_err();
        assert_eq!(err.code(), Code::ACME_RATE_LIMITED);
    }

    #[tokio::test]
    async fn http01_challenge_computes_key_authorization() {
        let key = AccountKey::generate();
        let expected_thumb = key.thumbprint();
        let ca = MockCa::default()
            .route("https://ca/acme/directory", directory_resp())
            .route(
                "https://ca/acme/authz/1",
                json_resp(
                    200,
                    &serde_json::json!({
                        "status": "pending",
                        "challenges": [
                            {"type": "dns-01", "url": "https://ca/c/dns", "token": "dnstok"},
                            {"type": "http-01", "url": "https://ca/c/http", "token": "httptok"}
                        ]
                    }),
                ),
            );
        let client = AcmeClient::discover(ca, key, "https://ca/acme/directory")
            .await
            .unwrap();
        let ch = client
            .http01_challenge("https://ca/acme/authz/1")
            .await
            .unwrap();
        assert_eq!(ch.token, "httptok");
        assert_eq!(ch.url, "https://ca/c/http");
        assert_eq!(ch.key_authorization, format!("httptok.{expected_thumb}"));
    }

    #[tokio::test]
    async fn poll_order_returns_on_valid_and_errors_on_invalid() {
        let ca = MockCa::default()
            .route("https://ca/acme/directory", directory_resp())
            .route(
                "https://ca/acme/order/ok",
                json_resp(
                    200,
                    &serde_json::json!({"status": "valid", "finalize": "https://ca/f"}),
                ),
            )
            .route(
                "https://ca/acme/order/bad",
                json_resp(
                    200,
                    &serde_json::json!({"status": "invalid", "finalize": "https://ca/f"}),
                ),
            );
        let client = AcmeClient::discover(ca, AccountKey::generate(), "https://ca/acme/directory")
            .await
            .unwrap();
        assert_eq!(
            client
                .poll_order("https://ca/acme/order/ok", 3)
                .await
                .unwrap()
                .status,
            "valid"
        );
        assert!(client
            .poll_order("https://ca/acme/order/bad", 3)
            .await
            .is_err());
    }

    #[tokio::test]
    async fn finalize_generates_csr_and_returns_private_key() {
        let ca = MockCa::default()
            .route("https://ca/acme/directory", directory_resp())
            .route(
                "https://ca/acme/finalize",
                json_resp(
                    200,
                    &serde_json::json!({
                        "status": "valid",
                        "finalize": "https://ca/acme/finalize",
                        "certificate": "https://ca/acme/cert/1"
                    }),
                ),
            );
        let client = AcmeClient::discover(ca, AccountKey::generate(), "https://ca/acme/directory")
            .await
            .unwrap();
        let order = Order {
            status: "ready".into(),
            authorizations: vec![],
            finalize: "https://ca/acme/finalize".into(),
            certificate: None,
            url: "https://ca/acme/order/1".into(),
        };
        let (finalized, key_pem) = client
            .finalize(&order, &["example.com".into()])
            .await
            .unwrap();
        assert_eq!(
            finalized.certificate.as_deref(),
            Some("https://ca/acme/cert/1")
        );
        assert!(key_pem.contains("PRIVATE KEY"));
    }

    #[tokio::test]
    async fn download_certificate_returns_pem() {
        let mut cert = HttpResponse {
            status: 200,
            replay_nonce: Some("n".into()),
            ..Default::default()
        };
        cert.body = b"-----BEGIN CERTIFICATE-----\nMII...\n-----END CERTIFICATE-----\n".to_vec();
        let ca = MockCa::default()
            .route("https://ca/acme/directory", directory_resp())
            .route("https://ca/acme/cert/1", cert);
        let client = AcmeClient::discover(ca, AccountKey::generate(), "https://ca/acme/directory")
            .await
            .unwrap();
        let pem = client
            .download_certificate("https://ca/acme/cert/1")
            .await
            .unwrap();
        assert!(pem.contains("BEGIN CERTIFICATE"));
    }
}
