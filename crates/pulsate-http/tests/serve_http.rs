//! End-to-end HTTP/1.1 serving test: bind a real listener, serve one connection
//! through the router, and drive a raw HTTP/1.1 exchange as a client.

use std::sync::Arc;

use pulsate_http::Gateway;
use pulsate_proxy::Registry;
use pulsate_router::{Handler, MatchKind, Route, Router, SiteRoutes};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

fn router() -> Arc<Router> {
    Arc::new(Router::new(vec![SiteRoutes {
        hosts: vec!["localhost".into(), ":default".into()],
        routes: vec![
            Route {
                kind: MatchKind::Exact,
                middleware: Vec::new(),
                cache: None,
                pattern: "/hello".into(),
                method: None,
                handler: Handler::Respond {
                    status: 200,
                    body: "hello, pulsate".into(),
                },
            },
            Route {
                kind: MatchKind::Exact,
                middleware: Vec::new(),
                cache: None,
                pattern: "/old".into(),
                method: None,
                handler: Handler::Redirect {
                    to: "/new".into(),
                    status: 308,
                },
            },
        ],
    }]))
}

async fn raw_request(addr: std::net::SocketAddr, request: &str) -> String {
    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream.write_all(request.as_bytes()).await.unwrap();
    stream.flush().await.unwrap();
    let mut buf = Vec::new();
    // The handler returns Connection: close-ish behavior via hyper; read to end.
    stream.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf).to_string()
}

#[tokio::test]
async fn serves_respond_and_redirect_and_404() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let gateway = Arc::new(Gateway::new(router(), Arc::new(Registry::new())));

    // Accept and serve connections in the background.
    tokio::spawn(async move {
        loop {
            let Ok((stream, peer)) = listener.accept().await else {
                break;
            };
            let gateway = Arc::clone(&gateway);
            tokio::spawn(async move {
                let _ = pulsate_http::serve_connection(stream, peer, gateway).await;
            });
        }
    });

    // 200 with body.
    let resp = raw_request(
        addr,
        "GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200"), "got: {resp}");
    assert!(resp.contains("hello, pulsate"), "got: {resp}");

    // 308 redirect with Location.
    let resp = raw_request(
        addr,
        "GET /old HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 308"), "got: {resp}");
    assert!(
        resp.to_lowercase().contains("location: /new"),
        "got: {resp}"
    );

    // Unmatched path → 404.
    let resp = raw_request(
        addr,
        "GET /missing HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");
}
