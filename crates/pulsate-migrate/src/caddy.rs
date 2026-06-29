//! The Caddy importer: a line-based Caddyfile reader plus a mapping to Flow.
//!
//! Handles the common single-level Caddyfile shape — a site address block with
//! `reverse_proxy`, `file_server`/`root`, and `redir` directives. Matchers,
//! snippets, and named-matcher blocks need manual review.

use crate::{Builder, Fidelity, Import};

/// Import a Caddyfile into Flow.
pub fn import(text: &str) -> Import {
    let mut b = Builder::default();
    b.line("# Imported from Caddy by `pulsate import caddy`. Review the notes.");

    let mut site: Option<Vec<String>> = None;
    let mut root: Option<String> = None;
    let mut routes: Vec<String> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_suffix('{') {
            site = Some(
                header
                    .split_whitespace()
                    .map(host_only)
                    .filter(|h| !h.is_empty())
                    .collect(),
            );
            root = None;
            routes.clear();
            continue;
        }
        if line == "}" {
            if let Some(hosts) = site.take() {
                emit_site(&mut b, &hosts, root.take(), &routes);
            }
            routes.clear();
            continue;
        }
        // A directive line inside a site block.
        map_directive(line, &mut root, &mut routes, &mut b);
    }

    b.finish()
}

fn map_directive(line: &str, root: &mut Option<String>, routes: &mut Vec<String>, b: &mut Builder) {
    let mut words = line.split_whitespace();
    let Some(directive) = words.next() else {
        return;
    };
    let args: Vec<&str> = words.collect();
    match directive {
        "reverse_proxy" => {
            if let Some(target) = args.first() {
                routes.push(format!("route /* ~> proxy({})", with_scheme(target)));
                b.note("reverse_proxy", Fidelity::Exact, "mapped to proxy()");
            }
        }
        "root" => {
            // `root * /srv/www` — the optional first arg is a path matcher.
            *root = args.last().map(ToString::to_string);
        }
        "file_server" => {
            let dir = root.clone().unwrap_or_else(|| ".".to_string());
            routes.push(format!("route /* ~> files(\"{dir}\")"));
            b.note("file_server", Fidelity::Exact, "mapped to files()");
        }
        "redir" => match args.as_slice() {
            [from, to, code] => {
                routes.push(format!(
                    "route {from} ~> redirect(to=\"{to}\", status={code})"
                ));
                b.note("redir", Fidelity::Exact, "mapped to redirect()");
            }
            [from, to] => {
                routes.push(format!("route {from} ~> redirect(to=\"{to}\", status=302)"));
                b.note("redir", Fidelity::Approximate, "default status 302 assumed");
            }
            _ => b.note("redir", Fidelity::Manual, "unrecognized redir form"),
        },
        "respond" => {
            let code = args
                .iter()
                .find_map(|a| a.parse::<u16>().ok())
                .unwrap_or(200);
            routes.push(format!("route /* ~> respond(status={code})"));
            b.note("respond", Fidelity::Approximate, "mapped to respond()");
        }
        "tls" => b.note(
            "tls",
            Fidelity::Approximate,
            "Caddy TLS maps to `tls auto`; explicit certs need a tls{} block",
        ),
        other => b.note(
            other,
            Fidelity::Manual,
            "Caddy directive needs manual review",
        ),
    }
}

fn emit_site(b: &mut Builder, hosts: &[String], _root: Option<String>, routes: &[String]) {
    let host_line = if hosts.is_empty() {
        ":default".to_string()
    } else {
        hosts.join(" ")
    };
    b.open(&format!("site {host_line} {{"));
    b.line("tls auto");
    if routes.is_empty() {
        b.line("route /* ~> respond(status=404)");
        b.note(
            "site",
            Fidelity::Manual,
            "no recognized handler; added a placeholder route",
        );
    } else {
        for r in routes {
            b.line(r);
        }
    }
    b.close();
}

/// Strip a scheme/port from a Caddy site address, keeping the hostname.
fn host_only(addr: &str) -> String {
    let addr = addr
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    addr.split('/').next().unwrap_or(addr).to_string()
}

fn with_scheme(target: &str) -> String {
    if target.contains("://") {
        target.to_string()
    } else {
        format!("http://{target}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reverse_proxy_maps_to_proxy() {
        let conf = "app.com {\n  reverse_proxy localhost:3000\n}";
        let imported = import(conf);
        assert!(imported
            .flow
            .contains("route /* ~> proxy(http://localhost:3000)"));
    }
}
