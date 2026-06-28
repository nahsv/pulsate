//! `pulsate-rt` — the runtime abstraction the data plane is written against.
//!
//! The data plane depends on this thin seam rather than on Tokio directly, so a
//! thread-per-core `io_uring` backend (monoio/glommio) can be slotted in without
//! rewriting `pulsate-http`, `pulsate-proxy`, etc.
//! (`docs/02-architecture.md#async-runtime`). The control plane uses Tokio
//! directly — it is not latency-critical.
//!
//! Exposes only the primitives the data plane uses — `spawn`, `spawn_blocking`,
//! timers, a yield point, and TCP/UDP networking — over a Tokio backend. The
//! surface is small so the seam stays cheap to re-implement. Only the Tokio
//! backend exists.
#![forbid(unsafe_code)]

use std::future::Future;
use std::time::Duration;

pub mod net;

#[doc(inline)]
pub use net::{TcpListener, TcpStream, UdpSocket};

/// A handle to a spawned task. Awaiting it yields the task's output.
///
/// Wraps the backend join handle so callers never name a Tokio type directly.
#[derive(Debug)]
pub struct JoinHandle<T> {
    inner: tokio::task::JoinHandle<T>,
}

impl<T> JoinHandle<T> {
    /// Abort the task. It will stop at its next `.await` point.
    pub fn abort(&self) {
        self.inner.abort();
    }
}

impl<T> Future for JoinHandle<T> {
    type Output = Result<T, JoinError>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        std::pin::Pin::new(&mut self.inner)
            .poll(cx)
            .map_err(|e| JoinError { inner: e })
    }
}

/// Error returned when a joined task panicked or was aborted.
#[derive(Debug)]
pub struct JoinError {
    inner: tokio::task::JoinError,
}

impl JoinError {
    /// Whether the task was aborted (rather than panicking).
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

impl std::fmt::Display for JoinError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "joined task failed: {}", self.inner)
    }
}

impl std::error::Error for JoinError {}

/// Spawn an async task onto the runtime.
pub fn spawn<F>(future: F) -> JoinHandle<F::Output>
where
    F: Future + Send + 'static,
    F::Output: Send + 'static,
{
    JoinHandle {
        inner: tokio::spawn(future),
    }
}

/// Run a blocking closure on the runtime's blocking pool, off the async workers.
/// This is the only sanctioned way to do potentially-blocking work.
pub fn spawn_blocking<F, R>(f: F) -> JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    JoinHandle {
        inner: tokio::task::spawn_blocking(f),
    }
}

/// Complete after `dur` has elapsed.
pub async fn sleep(dur: Duration) {
    tokio::time::sleep(dur).await;
}

/// Run `future`, returning `None` if it does not complete within `dur`.
pub async fn timeout<F: Future>(dur: Duration, future: F) -> Option<F::Output> {
    tokio::time::timeout(dur, future).await.ok()
}

/// Yield to the scheduler, a cooperative pause point for long loops.
pub async fn yield_now() {
    tokio::task::yield_now().await;
}

/// A multi-threaded runtime, sized to available cores by default.
///
/// The binary builds one of these and drives the supervisor on it.
#[derive(Debug)]
pub struct Runtime {
    inner: tokio::runtime::Runtime,
}

impl Runtime {
    /// Build a multi-threaded runtime. `worker_threads` of `None` lets the
    /// backend size to available parallelism.
    ///
    /// # Errors
    /// Returns the backend error if the runtime cannot be constructed.
    pub fn new(worker_threads: Option<usize>) -> std::io::Result<Self> {
        let mut builder = tokio::runtime::Builder::new_multi_thread();
        builder.enable_all();
        if let Some(n) = worker_threads {
            builder.worker_threads(n.max(1));
        }
        Ok(Self {
            inner: builder.build()?,
        })
    }

    /// Run a future to completion on this runtime, blocking the current thread.
    pub fn block_on<F: Future>(&self, future: F) -> F::Output {
        self.inner.block_on(future)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_on_runs_spawned_work() {
        let rt = Runtime::new(Some(2)).unwrap();
        let out = rt.block_on(async {
            let h = spawn(async { 21 + 21 });
            h.await.unwrap()
        });
        assert_eq!(out, 42);
    }

    #[test]
    fn spawn_blocking_returns_value() {
        let rt = Runtime::new(None).unwrap();
        let out = rt.block_on(async { spawn_blocking(|| 7 * 6).await.unwrap() });
        assert_eq!(out, 42);
    }

    #[test]
    fn timeout_elapses_to_none() {
        let rt = Runtime::new(Some(1)).unwrap();
        let out: Option<()> = rt.block_on(async {
            timeout(Duration::from_millis(5), async {
                sleep(Duration::from_secs(60)).await;
            })
            .await
        });
        assert!(out.is_none());
    }
}
