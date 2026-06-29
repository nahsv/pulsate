//! In-process QUIC + HTTP/3 round-trip.
//!
//! Binds a real [`Http3Listener`] on loopback, then drives an in-process
//! `quinn` + `h3` client through a full request/response exchange and asserts the
//! routed response. No external tools are needed.
//!
//! Interop with third-party clients (`curl --http3`, browsers) is the remaining
//! production gate and is intentionally **not** covered here.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use bytes::{Buf, BufMut, BytesMut};
use pulsate_http::Gateway;
use pulsate_http3::server::{Http3Listener, TransportConfig};
use pulsate_proxy::Registry;
use pulsate_router::{Handler, MatchKind, Route, Router, SiteRoutes};
use pulsate_tls::{certified_key_from_pem, CertResolver};

/// A router with a single `respond` route at `/hello`.
fn router() -> Arc<Router> {
    Arc::new(Router::new(vec![SiteRoutes {
        hosts: vec!["localhost".into(), ":default".into()],
        routes: vec![Route {
            kind: MatchKind::Exact,
            middleware: Vec::new(),
            cache: None,
            pattern: "/hello".into(),
            method: None,
            handler: Handler::Respond {
                status: 200,
                body: "hello over h3".into(),
            },
        }],
    }]))
}

/// Self-signed cert/key PEM pair plus the DER cert, for `host`.
fn self_signed(host: &str) -> (String, String, rustls::pki_types::CertificateDer<'static>) {
    let cert = rcgen::generate_simple_self_signed(vec![host.to_string()]).unwrap();
    let der = cert.cert.der().clone();
    (cert.cert.pem(), cert.key_pair.serialize_pem(), der)
}

/// Build a `quinn` client endpoint that trusts `cert_der` and offers ALPN `h3`.
fn client_endpoint(cert_der: rustls::pki_types::CertificateDer<'static>) -> quinn::Endpoint {
    let mut roots = rustls::RootCertStore::empty();
    roots.add(cert_der).unwrap();

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut crypto = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_root_certificates(roots)
        .with_no_client_auth();
    crypto.alpn_protocols = vec![b"h3".to_vec()];

    let client_config = quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(crypto).unwrap(),
    ));
    let mut endpoint = quinn::Endpoint::client(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
    endpoint.set_default_client_config(client_config);
    endpoint
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn h3_round_trip_returns_routed_response() {
    let (cert_pem, key_pem, cert_der) = self_signed("localhost");

    let mut resolver = CertResolver::new();
    resolver.set_default(certified_key_from_pem(cert_pem.as_bytes(), key_pem.as_bytes()).unwrap());

    let gateway = Arc::new(Gateway::new(router(), Arc::new(Registry::new())));
    let listener = Http3Listener::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        resolver,
        gateway,
        TransportConfig::default(),
    )
    .unwrap();
    let server_addr = listener.local_addr().unwrap();

    // Run the listener until signalled.
    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        listener
            .serve(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    // Drive the whole client exchange under a hard timeout so a protocol bug
    // fails the test instead of hanging.
    let (status, body) = tokio::time::timeout(Duration::from_secs(10), async move {
        let endpoint = client_endpoint(cert_der);
        let conn = endpoint
            .connect(server_addr, "localhost")
            .unwrap()
            .await
            .unwrap();

        let (mut driver, mut send_request) = h3::client::new(h3_quinn::Connection::new(conn))
            .await
            .unwrap();
        let drive = tokio::spawn(async move {
            // Run the connection until it closes; errors here are not the
            // assertion under test.
            let _ = std::future::poll_fn(|cx| driver.poll_close(cx)).await;
        });

        let req = http::Request::builder()
            .method(http::Method::GET)
            .uri("https://localhost/hello")
            .body(())
            .unwrap();
        let mut stream = send_request.send_request(req).await.unwrap();
        stream.finish().await.unwrap();

        let resp = stream.recv_response().await.unwrap();
        let status = resp.status();

        let mut body = BytesMut::new();
        while let Some(mut chunk) = stream.recv_data().await.unwrap() {
            body.put(chunk.copy_to_bytes(chunk.remaining()));
        }

        // Let the connection close cleanly, then stop the driver.
        drop(send_request);
        endpoint.wait_idle().await;
        drive.abort();

        (status, body.freeze())
    })
    .await
    .expect("client exchange timed out");

    assert_eq!(status, http::StatusCode::OK);
    assert_eq!(&body[..], b"hello over h3");

    let _ = shutdown_tx.send(());
    let _ = server.await;
}
