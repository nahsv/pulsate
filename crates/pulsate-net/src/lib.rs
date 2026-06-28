//! `pulsate-net` — listeners, socket options, connection limits, graceful drain.
//!
//! The transport floor of the data plane: bind a listening socket (with
//! `SO_REUSEPORT` so accepts spread across workers), accept connections under a
//! concurrency cap (backpressure rather than unbounded growth), and hand each
//! accepted stream to a caller-supplied connection handler. On a drain signal it
//! stops accepting and waits for in-flight connections up to a grace deadline
//! (`docs/02-architecture.md#graceful-shutdown`).
//!
//! It is transport-only and protocol-agnostic: TLS termination and HTTP serving
//! are decisions the connection handler makes, keeping this crate independent of
//! the HTTP stack.
#![forbid(unsafe_code)]

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use pulsate_core::Lifecycle;
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{watch, Semaphore};

/// Tuning for a listener.
#[derive(Debug, Clone, Copy)]
pub struct ListenerConfig {
    /// Maximum concurrent connections before new accepts apply backpressure.
    pub max_connections: usize,
    /// How long to wait for in-flight connections to finish on drain.
    pub drain_grace: Duration,
}

impl Default for ListenerConfig {
    fn default() -> Self {
        Self {
            max_connections: 10_000,
            drain_grace: Duration::from_secs(30),
        }
    }
}

/// Bind a TCP listener with `SO_REUSEADDR` and (on Unix) `SO_REUSEPORT`, so
/// multiple workers can share the port and the kernel load-balances accepts.
///
/// # Errors
/// Returns the OS error if the socket cannot be created, bound, or listened.
pub fn bind(addr: SocketAddr) -> std::io::Result<TcpListener> {
    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    TcpListener::from_std(socket.into())
}

/// Run the accept loop on `listener`, handing each connection to `on_conn`.
///
/// Accepts until `lifecycle` leaves [`Lifecycle::Running`], then drains: it stops
/// accepting and waits for outstanding connections to finish, up to
/// `config.drain_grace`. Connections beyond `config.max_connections` wait for a
/// slot (backpressure) rather than being accepted and overwhelming the process.
///
/// # Errors
/// Returns an OS error only on a fatal accept failure.
pub async fn serve<F, Fut>(
    listener: TcpListener,
    mut lifecycle: watch::Receiver<Lifecycle>,
    config: ListenerConfig,
    on_conn: F,
) -> std::io::Result<()>
where
    F: Fn(TcpStream, SocketAddr) -> Fut + Clone + Send + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let limiter = Arc::new(Semaphore::new(config.max_connections));

    loop {
        // Acquire a slot first so we never accept beyond the cap.
        let permit = Arc::clone(&limiter)
            .acquire_owned()
            .await
            .expect("semaphore is never closed");

        tokio::select! {
            biased;
            changed = lifecycle.changed() => {
                // Sender dropped or moved out of Running → begin draining.
                if changed.is_err() || *lifecycle.borrow() != Lifecycle::Running {
                    drop(permit);
                    break;
                }
            }
            accepted = listener.accept() => {
                let (stream, peer) = accepted?;
                let _ = stream.set_nodelay(true);
                let handler = on_conn.clone();
                tokio::spawn(async move {
                    handler(stream, peer).await;
                    drop(permit); // release the slot when the connection ends
                });
            }
        }
    }

    drain(&limiter, config).await;
    Ok(())
}

/// Wait for outstanding connections (held permits) to drain, up to the grace
/// deadline, then return regardless so shutdown always terminates.
async fn drain(limiter: &Arc<Semaphore>, config: ListenerConfig) {
    let want = u32::try_from(config.max_connections).unwrap_or(u32::MAX);
    let all_done = async {
        // Acquiring every permit means no connection still holds one.
        let _ = limiter.acquire_many(want).await;
    };
    let _ = tokio::time::timeout(config.drain_grace, all_done).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[tokio::test]
    async fn serves_then_drains_on_signal() {
        let listener = bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = watch::channel(Lifecycle::Running);

        // Echo handler: read a byte, write it back.
        let server = tokio::spawn(serve(
            listener,
            rx,
            ListenerConfig {
                max_connections: 4,
                drain_grace: Duration::from_secs(1),
            },
            |mut stream, _peer| async move {
                let mut buf = [0u8; 1];
                if stream.read_exact(&mut buf).await.is_ok() {
                    let _ = stream.write_all(&buf).await;
                    let _ = stream.flush().await;
                }
            },
        ));

        // A client round-trips a byte.
        let mut client = TcpStream::connect(addr).await.unwrap();
        client.write_all(b"x").await.unwrap();
        let mut out = [0u8; 1];
        client.read_exact(&mut out).await.unwrap();
        assert_eq!(&out, b"x");

        // Signal drain; the server loop returns.
        tx.send(Lifecycle::Draining).unwrap();
        let result = tokio::time::timeout(Duration::from_secs(2), server).await;
        assert!(result.is_ok(), "server did not drain in time");
    }
}
