//! The Apache httpd importer: a line reader for `<VirtualHost>` blocks plus a
//! mapping to Flow.
//!
//! Handles the common virtual-host shape — `ServerName`/`ServerAlias`, TLS via
//! `SSLEngine on` or a `:443` address, reverse proxying via `ProxyPass`, static
//! roots via `DocumentRoot`, and `Redirect`. Rewrite rules, `<Directory>` /
//! `<Location>` containers, and regex variants (`ProxyPassMatch`,
//! `RedirectMatch`) are flagged for manual review.

use crate::{Builder, Fidelity, Import};

/// One virtual host being assembled as it is parsed.
#[derive(Default)]
struct Vhost {
    hosts: Vec<String>,
    tls: bool,
    routes: Vec<String>,
    doc_root: Option<String>,
    has_root_route: bool,
}

/// Import an Apache `httpd.conf` / vhost file into Flow.
pub fn import(text: &str) -> Import {
    let mut b = Builder::default();
    b.line("# Imported from Apache by `p8 import apache`. Review the notes.");

    let mut vhost: Option<Vhost> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("<virtualhost") {
            let mut v = Vhost::default();
            // `<VirtualHost *:443>` — a :443 address implies TLS.
            if line.contains(":443") {
                v.tls = true;
            }
            vhost = Some(v);
        } else if lower.starts_with("</virtualhost") {
            if let Some(v) = vhost.take() {
                emit_vhost(&mut b, v);
            }
        } else if let Some(v) = vhost.as_mut() {
            map_directive(line, v, &mut b);
        } else {
            // Server-level directives (Listen, ServerRoot, LoadModule) are config
            // that Flow handles differently; note the notable ones, drop noise.
            note_global(line, &mut b);
        }
    }
    // A file may end without a closing tag in fragments; flush anyway.
    if let Some(v) = vhost.take() {
        emit_vhost(&mut b, v);
    }

    b.finish()
}

fn map_directive(line: &str, v: &mut Vhost, b: &mut Builder) {
    let mut words = line.split_whitespace();
    let Some(directive) = words.next() else {
        return;
    };
    let args: Vec<&str> = words.collect();
    match directive.to_ascii_lowercase().as_str() {
        "servername" => {
            if let Some(h) = args.first() {
                v.hosts.insert(0, host_only(h));
            }
        }
        "serveralias" => v.hosts.extend(args.iter().map(|a| host_only(a))),
        "sslengine" => {
            if args.first().is_some_and(|a| a.eq_ignore_ascii_case("on")) {
                v.tls = true;
            }
        }
        "proxypass" => map_proxy_pass(&args, v, b),
        "documentroot" => v.doc_root = args.first().map(|s| s.trim_matches('"').to_string()),
        "redirect" => map_redirect(&args, v, b),
        "proxypassmatch" | "redirectmatch" | "rewriterule" | "rewritecond" => b.note(
            directive,
            Fidelity::Manual,
            "regex/rewrite rule needs a manual Flow route",
        ),
        "<directory" | "<location" | "<files" | "<ifmodule" => b.note(
            directive,
            Fidelity::Manual,
            "container block needs manual review",
        ),
        // `ProxyPassReverse` and everything else: no Flow equivalent, drop quietly.
        _ => {}
    }
}

/// `ProxyPass /path http://target/ [opts]` (skip `ProxyPass /path !` exclusions).
fn map_proxy_pass(args: &[&str], v: &mut Vhost, b: &mut Builder) {
    let [path, target, ..] = args else {
        b.note("ProxyPass", Fidelity::Manual, "unrecognized ProxyPass form");
        return;
    };
    let (path, target) = (*path, *target);
    if target == "!" {
        b.note(
            "ProxyPass",
            Fidelity::Manual,
            "exclusion (`!`) needs a manual route",
        );
        return;
    }
    let handler = format!("proxy({})", target.trim_end_matches('/'));
    if path == "/" {
        v.routes.push(format!("route /* ~> {handler}"));
        v.has_root_route = true;
    } else {
        v.routes.push(format!(
            "route {}/* ~> {handler}",
            path.trim_end_matches('/')
        ));
    }
    b.note("ProxyPass", Fidelity::Exact, "mapped to proxy()");
}

/// `Redirect [code] /from <url>` | `Redirect /from <url>` (default 302).
fn map_redirect(args: &[&str], v: &mut Vhost, b: &mut Builder) {
    let (code, from, to) = match args {
        [code, from, to] if code.chars().all(|c| c.is_ascii_digit()) => (*code, *from, *to),
        [code, from, to] if matches!(*code, "permanent" | "temp" | "seeother") => {
            let c = if *code == "permanent" { "301" } else { "302" };
            (c, *from, *to)
        }
        [from, to] => ("302", *from, *to),
        _ => {
            b.note("Redirect", Fidelity::Manual, "unrecognized Redirect form");
            return;
        }
    };
    let pattern = if from == "/" {
        "/*".to_string()
    } else {
        from.to_string()
    };
    v.routes.push(format!(
        "route {pattern} ~> redirect(to=\"{to}\", status={code})"
    ));
    if pattern == "/*" {
        v.has_root_route = true;
    }
    b.note("Redirect", Fidelity::Exact, "mapped to redirect()");
}

fn emit_vhost(b: &mut Builder, mut v: Vhost) {
    // DocumentRoot only becomes a catch-all `files()` if nothing else claims `/`.
    if !v.has_root_route {
        if let Some(root) = v.doc_root.take() {
            v.routes.push(format!("route /* ~> files(\"{root}\")"));
            v.has_root_route = true;
            b.note("DocumentRoot", Fidelity::Exact, "mapped to files()");
        }
    }

    let host_line = if v.hosts.is_empty() {
        ":default".to_string()
    } else {
        v.hosts.join(" ")
    };
    b.open(&format!("site {host_line} {{"));
    if v.tls {
        b.line("tls auto");
        b.note(
            "SSLEngine",
            Fidelity::Approximate,
            "TLS mapped to `tls auto` (ACME); explicit certs need a tls{} block",
        );
    }
    if v.routes.is_empty() {
        b.line("route /* ~> respond(status=404)");
        b.note(
            "VirtualHost",
            Fidelity::Manual,
            "no recognized handler; added a placeholder route",
        );
    } else {
        // Longest path first so specific proxies win over the catch-all.
        v.routes.sort_by_key(|r| std::cmp::Reverse(route_len(r)));
        for r in &v.routes {
            b.line(r);
        }
    }
    b.close();
}

fn note_global(line: &str, b: &mut Builder) {
    let directive = line.split_whitespace().next().unwrap_or_default();
    if directive.eq_ignore_ascii_case("listen") {
        b.note(
            "Listen",
            Fidelity::Dropped,
            "bind addresses are set via Flow listeners / CLI, not the site",
        );
    }
}

/// The match-prefix length of an emitted `route <pat> ~> …` line, for ordering.
fn route_len(route: &str) -> usize {
    route
        .strip_prefix("route ")
        .and_then(|r| r.split_whitespace().next())
        .map_or(0, str::len)
}

/// Strip a scheme/port from an Apache server name, keeping the hostname.
fn host_only(addr: &str) -> String {
    let addr = addr
        .trim_matches('"')
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    addr.split(['/', ':']).next().unwrap_or(addr).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vhost_with_proxy_and_tls() {
        let conf = r"
<VirtualHost *:443>
    ServerName example.com
    ServerAlias www.example.com
    SSLEngine on
    ProxyPass /api http://localhost:8080/
    ProxyPass / http://localhost:3000/
</VirtualHost>
";
        let imported = import(conf);
        assert!(imported.flow.contains("site example.com www.example.com"));
        assert!(imported.flow.contains("tls auto"));
        assert!(imported
            .flow
            .contains("route /api/* ~> proxy(http://localhost:8080)"));
        assert!(imported
            .flow
            .contains("route /* ~> proxy(http://localhost:3000)"));
        // The specific /api route must precede the catch-all.
        let api = imported.flow.find("/api/*").unwrap();
        let root = imported.flow.find("route /* ").unwrap();
        assert!(api < root);
    }

    #[test]
    fn document_root_and_redirect() {
        let conf = r"
<VirtualHost *:80>
    ServerName files.example.com
    DocumentRoot /srv/www
    Redirect 301 /old https://example.com/new
</VirtualHost>
";
        let imported = import(conf);
        assert!(imported
            .flow
            .contains("route /old ~> redirect(to=\"https://example.com/new\", status=301)"));
        assert!(imported.flow.contains("route /* ~> files(\"/srv/www\")"));
    }
}
