//! The canonical request lifecycle stages.
//!
//! These ten names are *normative*: every other subsystem (middleware, proxy,
//! observability, errors) refers to a request's position by these stages. See
//! `docs/02-architecture.md#request-lifecycle`.

use std::fmt;

/// A stage in the request lifecycle, in execution order.
///
/// A request advances `Accept → … → Finalize`; any stage that yields an error
/// diverts to [`Stage::Recover`], which synthesizes a response and resumes at
/// [`Stage::Egress`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum Stage {
    /// [1] Connection established.
    Accept,
    /// [2] TLS/ALPN or QUIC negotiated.
    Handshake,
    /// [3] Request head parsed into a normalized `Request`.
    Decode,
    /// [4] Router resolves site + route from the snapshot.
    Match,
    /// [5] Request-phase middleware run in declared order.
    Ingress,
    /// [6] Terminal handler chosen (proxy/files/redirect).
    Dispatch,
    /// [7] (proxy) pool pick, connect, send, receive.
    Upstream,
    /// [8] Response-phase middleware run in reverse order.
    Egress,
    /// [9] Response body streamed to the client.
    Stream,
    /// [10] Access log, metrics, trace span closed. Runs exactly once.
    Finalize,
    /// Error path: map an error to a response, then resume at `Egress`.
    Recover,
}

impl Stage {
    /// The normative one-based index for the linear stages (`Accept`..=`Finalize`).
    /// [`Stage::Recover`] is off the linear path and returns `None`.
    #[must_use]
    pub const fn index(self) -> Option<u8> {
        Some(match self {
            Stage::Accept => 1,
            Stage::Handshake => 2,
            Stage::Decode => 3,
            Stage::Match => 4,
            Stage::Ingress => 5,
            Stage::Dispatch => 6,
            Stage::Upstream => 7,
            Stage::Egress => 8,
            Stage::Stream => 9,
            Stage::Finalize => 10,
            Stage::Recover => return None,
        })
    }

    /// The lowercase stage name used in logs, traces, and metrics labels.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Stage::Accept => "accept",
            Stage::Handshake => "handshake",
            Stage::Decode => "decode",
            Stage::Match => "match",
            Stage::Ingress => "ingress",
            Stage::Dispatch => "dispatch",
            Stage::Upstream => "upstream",
            Stage::Egress => "egress",
            Stage::Stream => "stream",
            Stage::Finalize => "finalize",
            Stage::Recover => "recover",
        }
    }
}

impl fmt::Display for Stage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Process/worker lifecycle signal, broadcast to every task via a watch channel.
/// Drives graceful drain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Lifecycle {
    /// Serving normally.
    Running,
    /// Stop accepting new work; finish in-flight requests within the grace window.
    Draining,
    /// Grace deadline passed; force-close and exit.
    Stopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_stages_are_ordered_and_indexed() {
        let linear = [
            Stage::Accept,
            Stage::Handshake,
            Stage::Decode,
            Stage::Match,
            Stage::Ingress,
            Stage::Dispatch,
            Stage::Upstream,
            Stage::Egress,
            Stage::Stream,
            Stage::Finalize,
        ];
        for (i, s) in linear.iter().enumerate() {
            assert_eq!(s.index(), Some(u8::try_from(i + 1).unwrap()));
        }
    }

    #[test]
    fn recover_is_off_the_linear_path() {
        assert_eq!(Stage::Recover.index(), None);
        assert_eq!(Stage::Recover.as_str(), "recover");
    }
}
