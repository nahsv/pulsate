//! The HAProxy importer: a section/keyword reader plus a mapping to Flow.
//!
//! `haproxy.cfg` is organized into sections that start at column 0 with a
//! keyword (`global`, `defaults`, `frontend`, `backend`, `listen`); the indented
//! lines below belong to that section. We translate `backend`/`listen` server
//! pools into Flow upstreams and `frontend`/`listen` binds + routing rules into
//! Flow sites. ACLs of the `path`/`path_beg` family are resolved so a
//! `use_backend … if <acl>` becomes a path route; richer ACLs are left for
//! manual review.

use crate::{Builder, Fidelity, Import};
use std::collections::HashMap;

/// One config section: its kind, optional name (proxies have one), and the
/// tokenized directive lines beneath it.
struct Section {
    kind: String,
    name: String,
    lines: Vec<Vec<String>>,
}

/// Import a `haproxy.cfg` into Flow.
pub fn import(text: &str) -> Import {
    let sections = parse(text);
    let mut b = Builder::default();
    b.line("# Imported from HAProxy by `p8 import haproxy`. Review the notes.");

    // First pass: every `backend`/`listen` with servers becomes an upstream so
    // sites can reference it by name.
    let mut upstreams: HashMap<String, Vec<String>> = HashMap::new();
    for s in &sections {
        if s.kind == "backend" || s.kind == "listen" {
            let targets = server_targets(s);
            if !targets.is_empty() {
                upstreams.insert(s.name.clone(), targets);
            }
        }
    }
    for (name, targets) in &upstreams {
        b.open(&format!("upstream {name} {{"));
        for t in targets {
            b.line(&format!("target {t}"));
        }
        b.close();
        b.note("backend", Fidelity::Exact, "mapped to a Flow upstream pool");
    }

    // Second pass: frontends and listens become sites.
    for s in &sections {
        if s.kind == "frontend" || s.kind == "listen" {
            map_site(s, &upstreams, &mut b);
        }
    }

    b.finish()
}

/// Split the config into sections keyed by the column-0 keyword lines.
fn parse(text: &str) -> Vec<Section> {
    let mut sections: Vec<Section> = Vec::new();
    for raw in text.lines() {
        let line = strip_comment(raw);
        if line.trim().is_empty() {
            continue;
        }
        let words: Vec<String> = line.split_whitespace().map(ToString::to_string).collect();
        let indented = raw.starts_with([' ', '\t']);
        let is_header = !indented
            && matches!(
                words[0].as_str(),
                "global" | "defaults" | "frontend" | "backend" | "listen"
            );
        if is_header {
            sections.push(Section {
                kind: words[0].clone(),
                name: words.get(1).cloned().unwrap_or_default(),
                lines: Vec::new(),
            });
        } else if let Some(cur) = sections.last_mut() {
            cur.lines.push(words);
        }
    }
    sections
}

fn strip_comment(line: &str) -> &str {
    match line.find('#') {
        Some(i) => &line[..i],
        None => line,
    }
}

/// Collect `server <name> <addr> …` targets from a section, as Flow URLs.
fn server_targets(s: &Section) -> Vec<String> {
    s.lines
        .iter()
        .filter(|l| l.first().map(String::as_str) == Some("server"))
        .filter_map(|l| l.get(2))
        .map(|addr| with_scheme(addr))
        .collect()
}

fn map_site(s: &Section, upstreams: &HashMap<String, Vec<String>>, b: &mut Builder) {
    // Gather hostnames from `acl … hdr(host)` rules and bind TLS state.
    let mut hosts: Vec<String> = Vec::new();
    let mut tls = false;
    let mut acls: HashMap<String, String> = HashMap::new();
    let mut routes: Vec<(String, String)> = Vec::new(); // (pattern, backend)
    let mut default_backend: Option<String> = None;

    for l in &s.lines {
        match l.first().map(String::as_str) {
            Some("bind") => {
                if l.iter().any(|w| w == "ssl") {
                    tls = true;
                }
            }
            Some("acl") => {
                // `acl <name> path_beg /api` | `acl <name> path /exact`
                if let (Some(name), Some(kind), Some(val)) = (l.get(1), l.get(2), l.get(3)) {
                    if kind.starts_with("path") {
                        let pat = if kind == "path" {
                            format!("= {val}")
                        } else {
                            format!("{}/*", val.trim_end_matches('/'))
                        };
                        acls.insert(name.clone(), pat);
                    } else if kind.starts_with("hdr") || kind == "ssl_fc_sni" {
                        hosts.push(val.clone());
                    }
                }
            }
            Some("use_backend") => {
                // `use_backend api if is_api`
                let be = l.get(1).cloned().unwrap_or_default();
                let acl = l.iter().position(|w| w == "if").and_then(|i| l.get(i + 1));
                match acl.and_then(|a| acls.get(a)) {
                    Some(pat) => routes.push((pat.clone(), be)),
                    None => b.note(
                        "use_backend",
                        Fidelity::Manual,
                        "conditional needs a non-path ACL translated by hand",
                    ),
                }
            }
            Some("default_backend") => default_backend = l.get(1).cloned(),
            Some("redirect") => map_redirect(l, b, &mut routes),
            _ => {}
        }
    }

    if hosts.is_empty() {
        hosts.push(":default".to_string());
    }
    b.open(&format!("site {} {{", hosts.join(" ")));
    if tls {
        b.line("tls auto");
        b.note(
            "bind ssl",
            Fidelity::Approximate,
            "TLS mapped to `tls auto` (ACME); explicit crt needs a tls{} block",
        );
    }

    // A `listen` section fuses frontend + backend: if it has its own servers and
    // no explicit backend routing, proxy to the implicit upstream of its name.
    let self_pool = upstreams.contains_key(&s.name);
    if default_backend.is_none() && routes.is_empty() && self_pool {
        default_backend = Some(s.name.clone());
    }

    for (pat, be) in &routes {
        b.line(&format!("route {pat} ~> {}", backend_handler(be)));
        b.note(
            "use_backend",
            Fidelity::Exact,
            "mapped to a path route + proxy()",
        );
    }
    if let Some(be) = &default_backend {
        b.line(&format!("route /* ~> {}", backend_handler(be)));
        b.note(
            "default_backend",
            Fidelity::Exact,
            "mapped to the catch-all route",
        );
    } else if routes.is_empty() {
        b.line("route /* ~> respond(status=404)");
        b.note(
            "frontend",
            Fidelity::Manual,
            "no backend resolved; added a placeholder route",
        );
    }
    b.close();
}

/// `redirect location <url> code 301` / `redirect prefix <url>` → a route.
fn map_redirect(l: &[String], b: &mut Builder, routes: &mut Vec<(String, String)>) {
    let (Some("location" | "prefix"), Some(u)) = (l.get(1).map(String::as_str), l.get(2)) else {
        b.note("redirect", Fidelity::Manual, "unrecognized redirect form");
        return;
    };
    let url = u.clone();
    let code = l
        .iter()
        .position(|w| w == "code")
        .and_then(|i| l.get(i + 1))
        .map_or_else(|| "302".to_string(), Clone::clone);
    b.note("redirect", Fidelity::Approximate, "mapped to redirect()");
    // Encode the redirect in the backend slot; `backend_handler` decodes it.
    routes.push(("/*".to_string(), format!("__redirect:{url}:{code}")));
}

/// Resolve a backend reference to a Flow handler: a backend name becomes
/// `proxy(@name)`; a `__redirect:`-encoded slot becomes `redirect()`.
fn backend_handler(be: &str) -> String {
    if let Some(rest) = be.strip_prefix("__redirect:") {
        if let Some((url, code)) = rest.rsplit_once(':') {
            return format!("redirect(to=\"{url}\", status={code})");
        }
    }
    format!("proxy(@{be})")
}

fn with_scheme(addr: &str) -> String {
    if addr.contains("://") {
        addr.to_string()
    } else {
        format!("http://{addr}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_servers_become_an_upstream() {
        let cfg = "
backend web
    server w1 10.0.0.1:8080 check
    server w2 10.0.0.2:8080 check
frontend http
    bind *:80
    default_backend web
";
        let imported = import(cfg);
        assert!(imported.flow.contains("upstream web {"));
        assert!(imported.flow.contains("target http://10.0.0.1:8080"));
        assert!(imported.flow.contains("route /* ~> proxy(@web)"));
    }

    #[test]
    fn acl_path_becomes_a_route() {
        let cfg = "
frontend fe
    bind *:443 ssl crt /etc/cert.pem
    acl is_api path_beg /api
    use_backend api if is_api
    default_backend web
backend api
    server a 10.0.0.9:9000
backend web
    server w 10.0.0.1:8080
";
        let imported = import(cfg);
        assert!(imported.flow.contains("tls auto"));
        assert!(imported.flow.contains("route /api/* ~> proxy(@api)"));
        assert!(imported.flow.contains("route /* ~> proxy(@web)"));
    }
}
