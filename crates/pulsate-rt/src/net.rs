//! Networking primitives the data plane binds and accepts on.
//!
//! Thin newtype wrappers over the Tokio types so callers in `pulsate-net` never
//! name a backend type directly — keeping the runtime seam intact
//! (`docs/02-architecture.md#async-runtime`). Socket-option tuning
//! (`SO_REUSEPORT`, `TCP_NODELAY`, limits) is not done here; it belongs to
//! `pulsate-net`.

use std::net::SocketAddr;

/// A TCP listener.
#[derive(Debug)]
pub struct TcpListener {
    inner: tokio::net::TcpListener,
}

impl TcpListener {
    /// Bind a TCP listener to `addr`.
    ///
    /// # Errors
    /// Returns the OS error if the address cannot be bound (e.g. in use).
    pub async fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        Ok(Self {
            inner: tokio::net::TcpListener::bind(addr).await?,
        })
    }

    /// Accept the next inbound connection and its peer address.
    ///
    /// # Errors
    /// Returns the OS error if accepting fails.
    pub async fn accept(&self) -> std::io::Result<(TcpStream, SocketAddr)> {
        let (stream, peer) = self.inner.accept().await?;
        Ok((TcpStream { inner: stream }, peer))
    }

    /// The local address the listener is bound to.
    ///
    /// # Errors
    /// Returns the OS error if the local address cannot be read.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

/// An accepted or dialed TCP stream.
#[derive(Debug)]
pub struct TcpStream {
    inner: tokio::net::TcpStream,
}

impl TcpStream {
    /// Dial a TCP connection to `addr`.
    ///
    /// # Errors
    /// Returns the OS error if the connection cannot be established.
    pub async fn connect(addr: SocketAddr) -> std::io::Result<Self> {
        Ok(Self {
            inner: tokio::net::TcpStream::connect(addr).await?,
        })
    }

    /// The peer address of the stream.
    ///
    /// # Errors
    /// Returns the OS error if the peer address cannot be read.
    pub fn peer_addr(&self) -> std::io::Result<SocketAddr> {
        self.inner.peer_addr()
    }

    /// Consume the wrapper, exposing the backend stream to crates that drive a
    /// codec over it (`pulsate-http`). This is the one place the backend type is
    /// deliberately surfaced.
    #[must_use]
    pub fn into_inner(self) -> tokio::net::TcpStream {
        self.inner
    }
}

/// A bound UDP socket (used by the QUIC/HTTP-3 listener and DNS).
#[derive(Debug)]
pub struct UdpSocket {
    inner: tokio::net::UdpSocket,
}

impl UdpSocket {
    /// Bind a UDP socket to `addr`.
    ///
    /// # Errors
    /// Returns the OS error if the address cannot be bound.
    pub async fn bind(addr: SocketAddr) -> std::io::Result<Self> {
        Ok(Self {
            inner: tokio::net::UdpSocket::bind(addr).await?,
        })
    }

    /// The local address the socket is bound to.
    ///
    /// # Errors
    /// Returns the OS error if the local address cannot be read.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.inner.local_addr()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Runtime;

    #[test]
    fn tcp_listener_binds_to_ephemeral_port() {
        let rt = Runtime::new(Some(1)).unwrap();
        rt.block_on(async {
            let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
            let listener = TcpListener::bind(addr).await.unwrap();
            let bound = listener.local_addr().unwrap();
            assert_ne!(bound.port(), 0);
        });
    }

    #[test]
    fn udp_socket_binds_to_ephemeral_port() {
        let rt = Runtime::new(Some(1)).unwrap();
        rt.block_on(async {
            let addr: SocketAddr = "127.0.0.1:0".parse().unwrap();
            let sock = UdpSocket::bind(addr).await.unwrap();
            assert_ne!(sock.local_addr().unwrap().port(), 0);
        });
    }
}
