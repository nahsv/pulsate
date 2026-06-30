//! `pulsate up` — load a config and serve it.
//!
//! Compile the config into a routing table, bind a plain-HTTP listener (and
//! optionally a TLS listener), and serve until a shutdown signal (Ctrl-C /
//! SIGTERM) triggers a graceful drain
//! (`docs/02-architecture.md#graceful-shutdown`). Listen addresses come from
//! flags. The `pulsate { http https }` engine block, multi-listener
//! reconciliation, and ACME wiring are not yet handled here.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use pulsate_config::{build_router, build_upstreams, ConfigStore, Source};
use pulsate_control::AdminApi;
use pulsate_core::Lifecycle;
use pulsate_http::Gateway;
use pulsate_net::ListenerConfig;
use pulsate_waf::AuditLog;
use tokio::sync::watch;

/// Options for `pulsate up`.
#[derive(Debug, Clone)]
pub struct UpOptions {
    /// Path to the config file.
    pub config: PathBuf,
    /// Plain-HTTP listen address.
    pub listen: SocketAddr,
    /// Optional TLS listener.
    pub tls: Option<TlsOptions>,
    /// Optional Prometheus metrics listen address.
    pub metrics: Option<SocketAddr>,
    /// Optional admin-API / dashboard listen address.
    pub admin: Option<SocketAddr>,
    /// Admin bearer token; generated if `None`.
    pub admin_token: Option<String>,
    /// Advertise HTTP/3 (`Alt-Svc`) on this UDP port, if set.
    pub http3_port: Option<u16>,
}

/// TLS listener options for `pulsate up`.
#[derive(Debug, Clone)]
pub struct TlsOptions {
    /// TLS listen address.
    pub listen: SocketAddr,
    /// PEM certificate chain path.
    pub cert: PathBuf,
    /// PEM private key path.
    pub key: PathBuf,
}

/// Run the gateway. Returns a process exit code.
#[allow(clippy::too_many_lines)] // top-level wiring of listeners + admin
pub async fn up(opts: UpOptions) -> u8 {
    let name = opts.config.display().to_string();
    let text = match std::fs::read_to_string(&opts.config) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("pulsate: cannot read {name}: {e}");
            return crate::exit::RUNTIME;
        }
    };

    let store = match ConfigStore::load(&name, &text) {
        Ok(s) => Arc::new(s),
        Err(diags) => {
            let source = Source::new(&name, &text);
            for d in &diags {
                eprint!("{}", d.render(&source));
            }
            eprintln!("error: {name} is invalid — {} problem(s)", diags.len());
            return crate::exit::CONFIG_INVALID;
        }
    };

    let gateway = {
        let config = &store.current().config;
        let alt_svc = opts
            .http3_port
            .and_then(|p| pulsate_http3::Http3Config::enabled(p).alt_svc());
        Arc::new(
            Gateway::new(
                Arc::new(build_router(config)),
                Arc::new(build_upstreams(config)),
            )
            .with_alt_svc(alt_svc),
        )
    };
    let audit = Arc::new(AuditLog::new());
    audit.append("gateway started");
    let listener_cfg = ListenerConfig::default();
    let (lifecycle_tx, lifecycle_rx) = watch::channel(Lifecycle::Running);

    // Translate Ctrl-C / SIGTERM into a drain signal.
    spawn_signal_listener(lifecycle_tx);

    let mut tasks = Vec::new();

    // Prometheus metrics listener (shares the serving telemetry).
    if let Some(addr) = opts.metrics {
        match pulsate_net::bind(addr) {
            Ok(listener) => {
                println!("pulsate: metrics on http://{addr}/metrics");
                let telemetry = Arc::clone(&gateway.telemetry);
                let rx = lifecycle_rx.clone();
                tasks.push(tokio::spawn(serve_metrics(listener, telemetry, rx)));
            }
            Err(e) => eprintln!("pulsate: cannot bind metrics {addr}: {e}"),
        }
    }

    // Admin API + embedded dashboard (loopback by default).
    if let Some(addr) = opts.admin {
        let generated = opts.admin_token.is_none();
        let token = opts.admin_token.clone().unwrap_or_else(generate_token);
        match pulsate_net::bind(addr) {
            Ok(listener) => {
                println!("pulsate: admin + dashboard on http://{addr}/");
                // Never print the bearer token to stdout — it would land verbatim
                // in journald/Docker/k8s logs (M11). For a generated token, persist
                // it to a 0600 file and log the path; otherwise log only a short
                // fingerprint so the operator can correlate without leaking it.
                if generated {
                    match write_token_file(&token) {
                        Ok(path) => println!(
                            "pulsate: generated admin token written to {} (fingerprint {})",
                            path.display(),
                            token_fingerprint(&token)
                        ),
                        Err(e) => eprintln!(
                            "pulsate: could not persist generated admin token ({e}); \
                             fingerprint {}",
                            token_fingerprint(&token)
                        ),
                    }
                } else {
                    println!(
                        "pulsate: admin token fingerprint {}",
                        token_fingerprint(&token)
                    );
                }
                let api = Arc::new(AdminApi::new(
                    Arc::clone(&store),
                    Arc::clone(&gateway),
                    Arc::clone(&audit),
                    token,
                ));
                let rx = lifecycle_rx.clone();
                tasks.push(tokio::spawn(pulsate_control::serve_admin(
                    listener, api, rx,
                )));
            }
            Err(e) => eprintln!("pulsate: cannot bind admin {addr}: {e}"),
        }
    }

    // Plain-HTTP listener.
    match pulsate_net::bind(opts.listen) {
        Ok(listener) => {
            println!(
                "pulsate: listening on http://{} ({} sites, {} upstreams)",
                opts.listen,
                gateway.router.site_count(),
                gateway.upstreams.len()
            );
            let gateway = Arc::clone(&gateway);
            let rx = lifecycle_rx.clone();
            tasks.push(tokio::spawn(async move {
                let _ = pulsate_net::serve(listener, rx, listener_cfg, move |stream, peer| {
                    let gateway = Arc::clone(&gateway);
                    async move {
                        let _ = pulsate_http::serve_connection(stream, peer, gateway).await;
                    }
                })
                .await;
            }));
        }
        Err(e) => {
            eprintln!("pulsate: cannot bind {}: {e}", opts.listen);
            return crate::exit::RUNTIME;
        }
    }

    // Optional TLS listener.
    if let Some(tls) = &opts.tls {
        match build_tls_listener(
            tls,
            Arc::clone(&gateway),
            lifecycle_rx.clone(),
            listener_cfg,
        ) {
            Ok(task) => {
                println!("pulsate: listening on https://{}", tls.listen);
                tasks.push(task);
            }
            Err(code) => return code,
        }
    }

    for task in tasks {
        let _ = task.await;
    }
    println!("pulsate: shutdown complete");
    crate::exit::OK
}

fn build_tls_listener(
    tls: &TlsOptions,
    gateway: Arc<Gateway>,
    rx: watch::Receiver<Lifecycle>,
    cfg: ListenerConfig,
) -> Result<tokio::task::JoinHandle<()>, u8> {
    let cert_pem = std::fs::read(&tls.cert).map_err(|e| {
        eprintln!("pulsate: cannot read cert {}: {e}", tls.cert.display());
        crate::exit::RUNTIME
    })?;
    let key_pem = std::fs::read(&tls.key).map_err(|e| {
        eprintln!("pulsate: cannot read key {}: {e}", tls.key.display());
        crate::exit::RUNTIME
    })?;
    let ck = pulsate_tls::certified_key_from_pem(&cert_pem, &key_pem).map_err(|e| {
        eprintln!("pulsate: {e}");
        crate::exit::RUNTIME
    })?;
    let mut resolver = pulsate_tls::CertResolver::new();
    resolver.set_default(ck);
    let config = pulsate_tls::server_config(resolver).map_err(|e| {
        eprintln!("pulsate: {e}");
        crate::exit::RUNTIME
    })?;
    let acceptor = pulsate_tls::acceptor(config);

    let listener = pulsate_net::bind(tls.listen).map_err(|e| {
        eprintln!("pulsate: cannot bind {}: {e}", tls.listen);
        crate::exit::RUNTIME
    })?;

    Ok(tokio::spawn(async move {
        let _ = pulsate_net::serve(listener, rx, cfg, move |stream, peer| {
            let gateway = Arc::clone(&gateway);
            let acceptor = acceptor.clone();
            async move {
                // A failed handshake simply drops the connection.
                if let Ok(tls_stream) = acceptor.accept(stream).await {
                    let _ = pulsate_http::serve_connection(tls_stream, peer, gateway).await;
                }
            }
        })
        .await;
    }))
}

/// Serve the Prometheus exposition over a minimal raw HTTP/1.1 responder. The
/// metrics endpoint is intentionally tiny and dependency-light; it answers any
/// path with the current exposition and closes the connection.
async fn serve_metrics(
    listener: tokio::net::TcpListener,
    telemetry: Arc<pulsate_http::Telemetry>,
    mut lifecycle: watch::Receiver<Lifecycle>,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        tokio::select! {
            changed = lifecycle.changed() => {
                if changed.is_err() || *lifecycle.borrow() != Lifecycle::Running {
                    break;
                }
            }
            accepted = listener.accept() => {
                let Ok((mut stream, _)) = accepted else { continue };
                let body = telemetry.render();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = stream.read(&mut buf).await; // drain request head
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: text/plain; version=0.0.4; charset=utf-8\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = stream.write_all(resp.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        }
    }
}

/// Generate a 256-bit admin token from the operating-system CSPRNG, hex-encoded.
/// Unguessable even on a loopback default; operators may still set `--admin-token`
/// explicitly (`docs/09-security.md`).
fn generate_token() -> String {
    use ring::rand::{SecureRandom, SystemRandom};
    let rng = SystemRandom::new();
    let mut bytes = [0u8; 32];
    rng.fill(&mut bytes)
        .expect("operating-system CSPRNG must be available at startup");
    hex(&bytes)
}

/// A short, non-reversible fingerprint of a token: the first 8 hex chars of its
/// SHA-256 digest. Safe to log; the token itself never appears (M11).
fn token_fingerprint(token: &str) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, token.as_bytes());
    hex(&digest.as_ref()[..4])
}

/// Persist a generated token to `./pulsate-admin-token` with `0600` permissions
/// (best effort on non-Unix), returning the path written.
fn write_token_file(token: &str) -> std::io::Result<PathBuf> {
    use std::io::Write as _;
    let path = std::env::current_dir()?.join("pulsate-admin-token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        // Re-assert mode in case the file pre-existed with looser permissions.
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(token.as_bytes())?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(&path, token.as_bytes())?;
    }
    Ok(path)
}

/// Lower-case hex-encode bytes.
fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(out, "{b:02x}");
    }
    out
}

fn spawn_signal_listener(tx: watch::Sender<Lifecycle>) {
    tokio::spawn(async move {
        // Ctrl-C is portable; SIGTERM is the container/systemd stop signal.
        #[cfg(unix)]
        {
            use tokio::signal::unix::{signal, SignalKind};
            let Ok(mut term) = signal(SignalKind::terminate()) else {
                // No SIGTERM handler available: fall back to Ctrl-C only.
                let _ = tokio::signal::ctrl_c().await;
                let _ = tx.send(Lifecycle::Draining);
                return;
            };
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = term.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        eprintln!("pulsate: draining (grace {:?})", Duration::from_secs(30));
        let _ = tx.send(Lifecycle::Draining);
    });
}
