//! A Prometheus-compatible metrics registry.
//!
//! Counters and histograms render to the Prometheus text exposition format.
//! Cardinality is bounded: a [`CounterVec`] refuses new label combinations once
//! it reaches its cap, so adversarial label values cannot grow memory without
//! bound. One mutex per metric; the hot path locks once per observation.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Mutex;

/// A labelled counter family (`name{label=value} value`).
#[derive(Debug)]
pub struct CounterVec {
    name: &'static str,
    help: &'static str,
    label_names: &'static [&'static str],
    cap: usize,
    values: Mutex<HashMap<Vec<String>, u64>>,
}

impl CounterVec {
    /// Create a counter family with a cardinality cap.
    #[must_use]
    pub fn new(
        name: &'static str,
        help: &'static str,
        label_names: &'static [&'static str],
        cap: usize,
    ) -> Self {
        Self {
            name,
            help,
            label_names,
            cap,
            values: Mutex::new(HashMap::new()),
        }
    }

    /// Increment the series for `label_values` by one. New series past the cap
    /// are dropped (counted into nothing) to bound cardinality.
    pub fn inc(&self, label_values: &[&str]) {
        self.add(label_values, 1);
    }

    /// Add `n` to the series for `label_values`.
    pub fn add(&self, label_values: &[&str], n: u64) {
        let key: Vec<String> = label_values.iter().map(ToString::to_string).collect();
        let Ok(mut map) = self.values.lock() else {
            return;
        };
        if let Some(v) = map.get_mut(&key) {
            *v += n;
        } else if map.len() < self.cap {
            map.insert(key, n);
        }
    }

    /// Append this counter family in Prometheus text format to `out`.
    pub fn render_into(&self, out: &mut String) {
        let _ = writeln!(out, "# HELP {} {}", self.name, self.help);
        let _ = writeln!(out, "# TYPE {} counter", self.name);
        let Ok(map) = self.values.lock() else { return };
        // Deterministic output for stable scrapes and tests.
        let mut rows: Vec<_> = map.iter().collect();
        rows.sort_by(|a, b| a.0.cmp(b.0));
        for (labels, value) in rows {
            let _ = writeln!(
                out,
                "{}{} {}",
                self.name,
                render_labels(self.label_names, labels),
                value
            );
        }
    }
}

/// A histogram with explicit buckets (`_bucket`, `_sum`, `_count`).
#[derive(Debug)]
pub struct Histogram {
    name: &'static str,
    help: &'static str,
    buckets: Vec<f64>,
    state: Mutex<HistState>,
}

#[derive(Debug)]
struct HistState {
    bucket_counts: Vec<u64>,
    sum: f64,
    count: u64,
}

impl Histogram {
    /// Create a histogram with ascending bucket upper bounds (seconds).
    #[must_use]
    pub fn new(name: &'static str, help: &'static str, buckets: Vec<f64>) -> Self {
        let n = buckets.len();
        Self {
            name,
            help,
            buckets,
            state: Mutex::new(HistState {
                bucket_counts: vec![0; n],
                sum: 0.0,
                count: 0,
            }),
        }
    }

    /// Record an observation.
    pub fn observe(&self, value: f64) {
        let Ok(mut s) = self.state.lock() else { return };
        s.count += 1;
        s.sum += value;
        for (i, &bound) in self.buckets.iter().enumerate() {
            if value <= bound {
                s.bucket_counts[i] += 1;
            }
        }
    }

    /// Append this histogram in Prometheus text format to `out`.
    pub fn render_into(&self, out: &mut String) {
        let _ = writeln!(out, "# HELP {} {}", self.name, self.help);
        let _ = writeln!(out, "# TYPE {} histogram", self.name);
        let Ok(s) = self.state.lock() else { return };
        // `bucket_counts[i]` already holds the count of values <= buckets[i], so
        // the series is cumulative without re-summing here.
        for (i, &bound) in self.buckets.iter().enumerate() {
            let _ = writeln!(
                out,
                "{}_bucket{{le=\"{bound}\"}} {}",
                self.name, s.bucket_counts[i]
            );
        }
        let _ = writeln!(out, "{}_bucket{{le=\"+Inf\"}} {}", self.name, s.count);
        let _ = writeln!(out, "{}_sum {}", self.name, s.sum);
        let _ = writeln!(out, "{}_count {}", self.name, s.count);
    }
}

fn render_labels(names: &[&str], values: &[String]) -> String {
    if names.is_empty() {
        return String::new();
    }
    let pairs: Vec<String> = names
        .iter()
        .zip(values)
        .map(|(n, v)| format!("{n}=\"{}\"", escape_label(v)))
        .collect();
    format!("{{{}}}", pairs.join(","))
}

fn escape_label(v: &str) -> String {
    v.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counter_renders_sorted_series() {
        let c = CounterVec::new(
            "pulsate_http_requests_total",
            "Total requests",
            &["method", "status"],
            100,
        );
        c.inc(&["GET", "200"]);
        c.inc(&["GET", "200"]);
        c.inc(&["POST", "404"]);
        let mut out = String::new();
        c.render_into(&mut out);
        assert!(out.contains("pulsate_http_requests_total{method=\"GET\",status=\"200\"} 2"));
        assert!(out.contains("pulsate_http_requests_total{method=\"POST\",status=\"404\"} 1"));
        assert!(out.contains("# TYPE pulsate_http_requests_total counter"));
    }

    #[test]
    fn counter_cardinality_is_bounded() {
        let c = CounterVec::new("x", "h", &["k"], 2);
        c.inc(&["a"]);
        c.inc(&["b"]);
        c.inc(&["c"]); // dropped — over cap
        c.inc(&["a"]); // existing series still increments
        let mut out = String::new();
        c.render_into(&mut out);
        assert!(out.contains("x{k=\"a\"} 2"));
        assert!(!out.contains("k=\"c\""));
    }

    #[test]
    fn histogram_buckets_are_cumulative() {
        let h = Histogram::new("dur", "d", vec![0.1, 0.5, 1.0]);
        h.observe(0.05);
        h.observe(0.3);
        h.observe(2.0);
        let mut out = String::new();
        h.render_into(&mut out);
        assert!(out.contains("dur_bucket{le=\"0.1\"} 1"));
        assert!(out.contains("dur_bucket{le=\"0.5\"} 2"));
        assert!(out.contains("dur_bucket{le=\"+Inf\"} 3"));
        assert!(out.contains("dur_count 3"));
    }
}
