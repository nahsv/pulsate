//! `pulsate-migrate` — import nginx / Caddy configs into Flow.
//!
//! The `p8 import` engine (`docs/30-migration-and-import.md`): parse a foreign
//! config, translate the constructs it understands into Flow, and report the
//! *fidelity* of every mapping so an operator knows exactly what was translated
//! exactly, approximated, or left for manual review. An honest, reviewable
//! starting point — not a silent, lossy rewrite.
#![forbid(unsafe_code)]

mod caddy;
mod nginx;

/// How faithfully a source directive was translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fidelity {
    /// Translated exactly.
    Exact,
    /// Translated with a reasonable approximation (review recommended).
    Approximate,
    /// Recognized but needs manual translation.
    Manual,
    /// Not translated.
    Dropped,
}

impl Fidelity {
    /// A short tag for reports.
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Fidelity::Exact => "exact",
            Fidelity::Approximate => "approx",
            Fidelity::Manual => "manual",
            Fidelity::Dropped => "dropped",
        }
    }
}

/// One fidelity note about a translated (or untranslated) directive.
#[derive(Debug, Clone)]
pub struct Note {
    /// The source directive the note is about.
    pub directive: String,
    /// How faithfully it was translated.
    pub fidelity: Fidelity,
    /// A human explanation.
    pub message: String,
}

/// The result of an import: the generated Flow plus the fidelity report.
#[derive(Debug, Clone)]
pub struct Import {
    /// The generated `pulsate.flow` source.
    pub flow: String,
    /// Per-directive fidelity notes.
    pub notes: Vec<Note>,
}

impl Import {
    /// Count notes at the given fidelity.
    #[must_use]
    pub fn count(&self, fidelity: Fidelity) -> usize {
        self.notes.iter().filter(|n| n.fidelity == fidelity).count()
    }

    /// Render the fidelity report as text.
    #[must_use]
    pub fn report(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        for n in &self.notes {
            let _ = writeln!(
                out,
                "  [{}] {}: {}",
                n.fidelity.tag(),
                n.directive,
                n.message
            );
        }
        let _ = writeln!(
            out,
            "summary: {} exact, {} approximate, {} manual, {} dropped",
            self.count(Fidelity::Exact),
            self.count(Fidelity::Approximate),
            self.count(Fidelity::Manual),
            self.count(Fidelity::Dropped),
        );
        out
    }
}

/// The source config format to import from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// An `nginx.conf`.
    Nginx,
    /// A `Caddyfile`.
    Caddy,
}

impl Source {
    /// Parse a format name (`nginx`, `caddy`).
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "nginx" => Some(Source::Nginx),
            "caddy" => Some(Source::Caddy),
            _ => None,
        }
    }
}

/// Import a foreign config into Flow.
#[must_use]
pub fn import(source: Source, text: &str) -> Import {
    match source {
        Source::Nginx => nginx::import(text),
        Source::Caddy => caddy::import(text),
    }
}

/// A small Flow source accumulator shared by the importers.
#[derive(Default)]
pub(crate) struct Builder {
    flow: String,
    notes: Vec<Note>,
    indent: usize,
}

impl Builder {
    pub(crate) fn line(&mut self, s: &str) {
        for _ in 0..self.indent {
            self.flow.push_str("  ");
        }
        self.flow.push_str(s);
        self.flow.push('\n');
    }

    pub(crate) fn open(&mut self, s: &str) {
        self.line(s);
        self.indent += 1;
    }

    pub(crate) fn close(&mut self) {
        self.indent = self.indent.saturating_sub(1);
        self.line("}");
    }

    pub(crate) fn note(&mut self, directive: &str, fidelity: Fidelity, message: &str) {
        self.notes.push(Note {
            directive: directive.to_string(),
            fidelity,
            message: message.to_string(),
        });
    }

    pub(crate) fn finish(self) -> Import {
        Import {
            flow: self.flow,
            notes: self.notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_parses_known_formats() {
        assert_eq!(Source::parse("nginx"), Some(Source::Nginx));
        assert_eq!(Source::parse("CADDY"), Some(Source::Caddy));
        assert_eq!(Source::parse("apache"), None);
    }

    // The generated Flow must at least be syntactically valid Flow.
    fn assert_parses(flow: &str) {
        pulsate_flow::parse("imported.flow", flow)
            .unwrap_or_else(|e| panic!("generated Flow did not parse: {e:?}\n---\n{flow}"));
    }

    #[test]
    fn nginx_round_trips_to_valid_flow() {
        let conf = r"
            upstream backend { server 10.0.0.1:8080; server 10.0.0.2:8080; }
            server {
                listen 443 ssl;
                server_name example.com www.example.com;
                location / { proxy_pass http://backend; }
                location /static { root /var/www; }
                location = /healthz { return 204; }
                location /old { return 301 https://example.com/new; }
            }
        ";
        let imported = import(Source::Nginx, conf);
        assert!(imported.flow.contains("site example.com www.example.com"));
        assert!(imported.flow.contains("proxy(@backend)"));
        assert!(imported.flow.contains("upstream backend"));
        assert!(imported.count(Fidelity::Exact) > 0);
        assert_parses(&imported.flow);
    }

    #[test]
    fn caddy_round_trips_to_valid_flow() {
        let conf = r"
            example.com {
                reverse_proxy localhost:3000
            }
            files.example.com {
                root * /srv/www
                file_server
            }
        ";
        let imported = import(Source::Caddy, conf);
        assert!(imported.flow.contains("site example.com"));
        assert!(imported.flow.contains("proxy("));
        assert_parses(&imported.flow);
    }
}
