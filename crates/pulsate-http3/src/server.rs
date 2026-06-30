//! QUIC / HTTP-3 listener.
//!
//! Binds a `quinn` UDP endpoint over a rustls server config (TLS 1.3 only, ALPN
//! `h3`) built from the shared [`pulsate_tls::CertResolver`], accepts QUIC
//! connections, reads HTTP/3 requests with `h3` + `h3-quinn`, and dispatches each
//! through [`crate::dispatch::dispatch`] — the same routing/middleware/cache/proxy
//! core the HTTP/1 and HTTP/2 listeners use. Responses are translated back onto
//! the request stream, so behaviour is identical across protocols.
//!
//! 0-RTT (early data) is intentionally left disabled: the rustls config keeps
//! `max_early_data_size` at `0`, so the listener never accepts 0-RTT payloads.
//! Enabling it safely requires application-layer anti-replay protection that is
//! out of scope here (`docs/09-security.md`).
//!
//! Interop with real third-party HTTP/3 clients (`curl --http3`, browsers) is the
//! remaining production gate and is not exercised by the in-process tests.

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BufMut, Bytes, BytesMut};
use http::Response as HttpResponse;
use pulsate_core::Body;
use pulsate_http::Gateway;
use pulsate_tls::CertResolver;
use quinn::crypto::rustls::QuicServerConfig;
use quinn::{Endpoint, IdleTimeout, ServerConfig, VarInt};
use tokio::sync::watch;

/// Application close code sent when the endpoint is torn down (HTTP/3
/// `H3_NO_ERROR`, RFC 9114 §8.1).
const H3_NO_ERROR: u32 = 0x0100;

/// Hop-by-hop / connection-specific headers that MUST NOT appear on an HTTP/3
/// message (RFC 9114 §4.2). They are stripped before a response is framed.
const CONNECTION_SPECIFIC: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
];

/// Tunable QUIC transport / HTTP-3 listener knobs.
#[derive(Debug, Clone, Copy)]
pub struct TransportConfig {
    /// Idle timeout after which an inactive connection is closed.
    pub max_idle_timeout: Duration,
    /// Maximum concurrent client-initiated bidirectional streams (requests).
    pub max_concurrent_bidi_streams: u32,
    /// Maximum concurrent client-initiated unidirectional streams.
    pub max_concurrent_uni_streams: u32,
    /// Deadline for draining in-flight requests on graceful shutdown before the
    /// endpoint is force-closed.
    pub drain_timeout: Duration,
}

impl Default for TransportConfig {
    fn default() -> Self {
        Self {
            max_idle_timeout: Duration::from_secs(30),
            max_concurrent_bidi_streams: 100,
            max_concurrent_uni_streams: 100,
            drain_timeout: Duration::from_secs(10),
        }
    }
}

/// Error building or binding an HTTP/3 listener.
#[derive(Debug)]
pub enum Http3Error {
    /// The rustls config is not QUIC-compatible, or the TLS 1.3 / cert-resolver
    /// configuration was rejected.
    Crypto(String),
    /// A transport-config value was out of range.
    Transport(String),
    /// Binding the UDP socket failed.
    Io(std::io::Error),
}

impl std::fmt::Display for Http3Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Crypto(e) => write!(f, "http3 quic crypto: {e}"),
            Self::Transport(e) => write!(f, "http3 transport config: {e}"),
            Self::Io(e) => write!(f, "http3 bind: {e}"),
        }
    }
}

impl std::error::Error for Http3Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(e) => Some(e),
            Self::Crypto(_) | Self::Transport(_) => None,
        }
    }
}

impl From<std::io::Error> for Http3Error {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

/// A bound QUIC/HTTP-3 listener, ready to serve.
pub struct Http3Listener {
    endpoint: Endpoint,
    gateway: Arc<Gateway>,
    transport: TransportConfig,
}

impl Http3Listener {
    /// Bind a UDP endpoint on `addr` using `resolver` for SNI certificate
    /// selection, dispatching requests through `gateway`.
    ///
    /// The rustls config is TLS 1.3 only with ALPN `h3` and 0-RTT disabled, and
    /// is reused as the QUIC crypto.
    ///
    /// # Errors
    /// Returns [`Http3Error`] if the TLS config is not QUIC-compatible, carries an
    /// out-of-range transport value, or the UDP socket cannot be bound.
    pub fn bind(
        addr: SocketAddr,
        resolver: CertResolver,
        gateway: Arc<Gateway>,
        transport: TransportConfig,
    ) -> Result<Self, Http3Error> {
        let rustls_config = server_crypto(resolver)?;
        let quic_crypto = QuicServerConfig::try_from(rustls_config)
            .map_err(|e| Http3Error::Crypto(e.to_string()))?;

        let mut server_config = ServerConfig::with_crypto(Arc::new(quic_crypto));
        server_config.transport_config(Arc::new(build_transport(&transport)?));

        let endpoint = Endpoint::server(server_config, addr)?;
        Ok(Self {
            endpoint,
            gateway,
            transport,
        })
    }

    /// The local UDP address the endpoint is bound to (useful when binding to
    /// port `0`).
    ///
    /// # Errors
    /// Returns the underlying socket error if the address cannot be read.
    pub fn local_addr(&self) -> Result<SocketAddr, std::io::Error> {
        self.endpoint.local_addr()
    }

    /// Serve connections until `shutdown` resolves, then drain gracefully.
    ///
    /// On shutdown each live connection is sent an HTTP/3 `GOAWAY`; in-flight
    /// requests are allowed to finish until [`TransportConfig::drain_timeout`]
    /// elapses, after which the endpoint is force-closed.
    pub async fn serve<F>(self, shutdown: F)
    where
        F: Future<Output = ()> + Send,
    {
        let (drain_tx, drain_rx) = watch::channel(false);
        let Self {
            endpoint,
            gateway,
            transport,
        } = self;

        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                biased;
                () = &mut shutdown => break,
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else { break };
                    let gateway = Arc::clone(&gateway);
                    let drain = drain_rx.clone();
                    tokio::spawn(drive_connection(incoming, gateway, drain));
                }
            }
        }

        // Signal every connection task to send GOAWAY and drain in-flight work,
        // then wait for the endpoint to go idle within the drain deadline.
        let _ = drain_tx.send(true);
        if tokio::time::timeout(transport.drain_timeout, endpoint.wait_idle())
            .await
            .is_err()
        {
            endpoint.close(VarInt::from_u32(H3_NO_ERROR), b"shutdown");
            endpoint.wait_idle().await;
        }
    }
}

/// Build the rustls server config used as QUIC crypto: TLS 1.3 only, ALPN `h3`,
/// 0-RTT disabled, certificate selected by SNI via `resolver`.
fn server_crypto(resolver: CertResolver) -> Result<Arc<rustls::ServerConfig>, Http3Error> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| Http3Error::Crypto(format!("tls 1.3 setup: {e}")))?
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(resolver));
    config.alpn_protocols = vec![b"h3".to_vec()];
    // 0-RTT disabled: keep early data at 0 so the listener never accepts
    // replayable 0-RTT payloads (already the default; set to document intent).
    config.max_early_data_size = 0;
    Ok(Arc::new(config))
}

/// Translate [`TransportConfig`] into a `quinn` transport config.
fn build_transport(cfg: &TransportConfig) -> Result<quinn::TransportConfig, Http3Error> {
    let idle = IdleTimeout::try_from(cfg.max_idle_timeout)
        .map_err(|e| Http3Error::Transport(format!("max_idle_timeout: {e}")))?;
    let mut transport = quinn::TransportConfig::default();
    transport.max_idle_timeout(Some(idle));
    transport.max_concurrent_bidi_streams(VarInt::from_u32(cfg.max_concurrent_bidi_streams));
    transport.max_concurrent_uni_streams(VarInt::from_u32(cfg.max_concurrent_uni_streams));
    Ok(transport)
}

/// Complete the QUIC handshake and drive one connection's HTTP/3 requests until
/// the peer closes it or a graceful drain finishes.
async fn drive_connection(
    incoming: quinn::Incoming,
    gateway: Arc<Gateway>,
    mut drain: watch::Receiver<bool>,
) {
    let Ok(conn) = incoming.await else { return };
    let peer = conn.remote_address();

    let mut h3_conn: h3::server::Connection<h3_quinn::Connection, Bytes> =
        match h3::server::Connection::new(h3_quinn::Connection::new(conn)).await {
            Ok(conn) => conn,
            Err(_) => return,
        };

    // If shutdown was already requested before this connection landed, send
    // GOAWAY immediately so it only drains rather than accepting new requests.
    let mut draining = *drain.borrow_and_update();
    if draining && h3_conn.shutdown(0).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            biased;
            accepted = h3_conn.accept() => {
                match accepted {
                    Ok(Some(resolver)) => {
                        let gateway = Arc::clone(&gateway);
                        tokio::spawn(async move {
                            // Stream-level errors (peer reset, abort) are routine
                            // and non-fatal to the listener; drop them.
                            let _ = handle_request(resolver, gateway, peer).await;
                        });
                    }
                    Ok(None) | Err(_) => break,
                }
            }
            changed = drain.changed(), if !draining => {
                draining = true;
                if changed.is_err() {
                    break;
                }
                // Stop accepting new requests; in-flight ones keep draining and
                // `accept()` resolves to `Ok(None)` once they complete.
                if h3_conn.shutdown(0).await.is_err() {
                    break;
                }
            }
        }
    }
}

/// Read one HTTP/3 request, dispatch it, and write the response back.
async fn handle_request(
    resolver: h3::server::RequestResolver<h3_quinn::Connection, Bytes>,
    gateway: Arc<Gateway>,
    peer: SocketAddr,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let (request, mut stream) = resolver.resolve_request().await?;
    let (parts, ()) = request.into_parts();

    // Buffer the full request body so the shared dispatch core (which consumes one
    // `Bytes`) sees the same input as the HTTP/1 and HTTP/2 path. Track the running
    // length and abort with 413 once it exceeds the configured cap (H3).
    let max_body = gateway.max_request_body_bytes;
    let mut body = BytesMut::new();
    while let Some(mut chunk) = stream.recv_data().await? {
        let data = chunk.copy_to_bytes(chunk.remaining());
        if body.len().saturating_add(data.len()) > max_body {
            let (head, payload) = translate(&payload_too_large());
            stream.send_response(head).await?;
            if !payload.is_empty() {
                stream.send_data(payload).await?;
            }
            stream.finish().await?;
            return Ok(());
        }
        body.put(data);
    }

    let response = crate::dispatch::dispatch(parts, body.freeze(), peer, &gateway).await;
    let (http_response, body_bytes) = translate(&response);

    stream.send_response(http_response).await?;
    if !body_bytes.is_empty() {
        stream.send_data(body_bytes).await?;
    }
    stream.finish().await?;
    Ok(())
}

/// A `413 Payload Too Large` response for a request body past the configured cap.
fn payload_too_large() -> pulsate_core::Response {
    let mut r = pulsate_core::Response::new(http::StatusCode::PAYLOAD_TOO_LARGE);
    r.headers_mut().insert(
        http::header::CONTENT_TYPE,
        http::HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    r.with_body("request body too large")
}

/// Convert an internal [`pulsate_core::Response`] into an HTTP/3 response head plus
/// its buffered body, stripping connection-specific headers.
fn translate(response: &pulsate_core::Response) -> (HttpResponse<()>, Bytes) {
    let body_bytes = match response.body() {
        Body::Bytes(b) => b.clone(),
        _ => Bytes::new(),
    };

    let mut builder = HttpResponse::builder().status(response.status());
    if let Some(headers) = builder.headers_mut() {
        for (name, value) in response.headers() {
            if CONNECTION_SPECIFIC.contains(&name.as_str()) {
                continue;
            }
            headers.insert(name, value.clone());
        }
        if !headers.contains_key(http::header::CONTENT_LENGTH) {
            if let Ok(value) = http::HeaderValue::from_str(&body_bytes.len().to_string()) {
                headers.insert(http::header::CONTENT_LENGTH, value);
            }
        }
    }

    // `builder` only errors on an invalid status/header set above; fall back to a
    // bare 500 head so a malformed response still yields a valid HTTP/3 frame.
    let head = builder.body(()).unwrap_or_else(|_| {
        let mut fallback = HttpResponse::new(());
        *fallback.status_mut() = http::StatusCode::INTERNAL_SERVER_ERROR;
        fallback
    });
    (head, body_bytes)
}
