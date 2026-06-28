//! `pulsate-observe` — metrics, request IDs, and structured access logs
//! (`docs/15-observability.md`).
//!
//! [`Telemetry`] holds the core metrics and renders them in Prometheus text
//! format; [`ids`] mints request IDs; [`log`] formats access-log lines.
#![forbid(unsafe_code)]

pub mod ids;
pub mod log;
pub mod metrics;

use metrics::{CounterVec, Histogram};

#[doc(inline)]
pub use ids::{now_ms, request_id};
#[doc(inline)]
pub use log::AccessLog;

/// The core Pulsate metrics, exposed at the metrics endpoint.
#[derive(Debug)]
pub struct Telemetry {
    requests_total: CounterVec,
    errors_total: CounterVec,
    request_duration: Histogram,
}

impl Default for Telemetry {
    fn default() -> Self {
        Self::new()
    }
}

impl Telemetry {
    /// Build the registry with Pulsate's standard metrics.
    #[must_use]
    pub fn new() -> Self {
        Self {
            requests_total: CounterVec::new(
                "pulsate_http_requests_total",
                "Total HTTP requests by method and status",
                &["method", "status"],
                1024,
            ),
            errors_total: CounterVec::new(
                "pulsate_errors_total",
                "Total errors by stable code",
                &["code"],
                512,
            ),
            request_duration: Histogram::new(
                "pulsate_http_request_duration_seconds",
                "HTTP request duration in seconds",
                vec![
                    0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0, 10.0,
                ],
            ),
        }
    }

    /// Record a completed request: count it and observe its duration.
    pub fn record_request(&self, method: &str, status: u16, duration_secs: f64) {
        let status_str = status.to_string();
        self.requests_total.inc(&[method, &status_str]);
        self.request_duration.observe(duration_secs);
    }

    /// Increment the error counter for a stable `PLS-*` code.
    pub fn incr_error(&self, code: &str) {
        self.errors_total.inc(&[code]);
    }

    /// Render all metrics in Prometheus text exposition format.
    #[must_use]
    pub fn render(&self) -> String {
        let mut out = String::new();
        self.requests_total.render_into(&mut out);
        self.errors_total.render_into(&mut out);
        self.request_duration.render_into(&mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_records_and_renders() {
        let t = Telemetry::new();
        t.record_request("GET", 200, 0.012);
        t.record_request("GET", 200, 0.030);
        t.incr_error("PLS-PRX-0003");
        let out = t.render();
        assert!(out.contains("pulsate_http_requests_total{method=\"GET\",status=\"200\"} 2"));
        assert!(out.contains("pulsate_errors_total{code=\"PLS-PRX-0003\"} 1"));
        assert!(out.contains("pulsate_http_request_duration_seconds_count 2"));
    }
}
