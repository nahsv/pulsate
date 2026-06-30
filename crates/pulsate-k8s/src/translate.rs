//! Pure translation: Gateway API objects → Pulsate Flow config text.
//!
//! This module is deliberately free of any Kubernetes I/O: it takes already-read
//! [`GatewayClass`], [`Gateway`], and [`HTTPRoute`] objects and produces the Flow
//! source string that [`crate::reconcile`] feeds to
//! [`pulsate_config::ConfigStore::reload`]. Keeping it pure is what lets the
//! golden tests assert the produced config without a live cluster.
//!
//! ## Mapping
//! - A [`GatewayClass`] is managed when its `controllerName` equals the
//!   controller name passed in (see [`crate::CONTROLLER_NAME`]).
//! - Each managed [`Gateway`] listener contributes a Flow `site` keyed by its
//!   hostname; `HTTPS` listeners get `tls auto`, everything else `tls off`.
//! - Each attached [`HTTPRoute`] rule becomes one Flow `route` per path match,
//!   forwarding to a generated `upstream` whose weighted targets are the rule's
//!   backend `Service`s (`http://<svc>.<ns>.svc:<port>`).
//!
//! Output is deterministic: upstreams and sites are emitted in sorted order and
//! routes within a site are ordered by match precedence (exact before longer
//! prefix), so equal inputs always yield byte-identical Flow text.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::crd::{Gateway, GatewayClass, HTTPRoute};

/// Default backend port when a `backendRef` omits one.
const DEFAULT_BACKEND_PORT: u16 = 80;

/// Translate the managed Gateway API objects into Flow config source.
///
/// `classes`/`gateways`/`routes` are the full sets observed in the cluster; this
/// function selects the ones it manages (`classes` whose `controllerName` matches
/// `controller_name`, the `gateways` of those classes, and the `routes` attached
/// to those gateways) and renders them. Resources outside that selection are
/// ignored, so passing extra objects is harmless.
#[must_use]
pub fn to_flow(
    classes: &[GatewayClass],
    gateways: &[Gateway],
    routes: &[HTTPRoute],
    controller_name: &str,
) -> String {
    // Class names this controller owns.
    let managed_classes: Vec<&str> = classes
        .iter()
        .filter(|c| c.spec.controller_name == controller_name)
        .filter_map(|c| c.metadata.name.as_deref())
        .collect();

    // Gateways belonging to a managed class, keyed by (namespace, name).
    let managed_gateways: BTreeMap<(String, String), &Gateway> = gateways
        .iter()
        .filter(|g| managed_classes.contains(&g.spec.gateway_class_name.as_str()))
        .filter_map(|g| {
            let name = g.metadata.name.clone()?;
            let ns = g.metadata.namespace.clone().unwrap_or_default();
            Some(((ns, name), g))
        })
        .collect();

    let mut sites: BTreeMap<String, SiteAcc> = BTreeMap::new();
    let mut upstreams: BTreeMap<String, String> = BTreeMap::new();

    for route in routes {
        let route_ns = route.metadata.namespace.clone().unwrap_or_default();
        let route_name = route.metadata.name.clone().unwrap_or_default();

        // Reject the whole route if any field that reaches the generated Flow
        // text is not strictly safe. Emitting unsafe Flow would let a tenant
        // inject directives (`tls off`, extra `site`/`route` blocks) or SSRF
        // targets that are recompiled and published cluster-wide (security C1).
        if !route_is_valid(route) {
            tracing::warn!(
                namespace = %route_ns,
                name = %route_name,
                "skipping HTTPRoute: a field failed strict Flow-safety validation"
            );
            continue;
        }

        // The managed gateways this route attaches to via its parentRefs.
        let parents: Vec<&Gateway> = route
            .spec
            .parent_refs
            .iter()
            .filter_map(|p| {
                let ns = p.namespace.clone().unwrap_or_else(|| route_ns.clone());
                managed_gateways.get(&(ns, p.name.clone())).copied()
            })
            .collect();
        if parents.is_empty() {
            continue;
        }

        for (rule_idx, rule) in route.spec.rules.iter().enumerate() {
            if rule.backend_refs.is_empty() {
                // No backend to proxy to: nothing to emit for this rule.
                continue;
            }

            let up_name = upstream_name(&route_ns, &route_name, rule_idx);
            let up_block = upstream_block(&up_name, rule, &route_ns);
            let route_lines = rule_route_lines(rule, &up_name);
            if route_lines.is_empty() {
                continue;
            }

            // Place the rule on every host its parent gateways expose.
            for gw in &parents {
                for (host, is_https) in hosts_for(gw, &route.spec.hostnames) {
                    let acc = sites.entry(host).or_default();
                    acc.https |= is_https;
                    for (key, line) in &route_lines {
                        acc.routes.insert(key.clone(), line.clone());
                    }
                    upstreams.insert(up_name.clone(), up_block.clone());
                }
            }
        }
    }

    render(&upstreams, &sites)
}

/// Accumulated routes and TLS posture for a single Flow `site`.
#[derive(Default)]
struct SiteAcc {
    /// Whether any contributing listener terminated TLS (`HTTPS`).
    https: bool,
    /// Routes keyed by precedence so output is deterministic and de-duplicated.
    routes: BTreeMap<RouteKey, String>,
}

/// A total ordering key giving exact matches precedence over prefixes, and
/// longer prefixes precedence over shorter ones; ties break on the literal text.
/// Tuple: `(kind_rank, inverse_path_len, pattern, method, upstream)`.
type RouteKey = (u8, usize, String, String, String);

/// The hosts (and their TLS posture) a route maps to under one gateway.
fn hosts_for(gw: &Gateway, route_hostnames: &[String]) -> Vec<(String, bool)> {
    let mut out = Vec::new();
    for listener in &gw.spec.listeners {
        // Listener hostnames are also untrusted CRD strings that land in `site`
        // headers; drop any listener whose hostname is not DNS-valid (C1).
        if let Some(lh) = listener.hostname.as_deref() {
            if !is_valid_hostname(lh) {
                continue;
            }
        }
        let is_https = listener.protocol.eq_ignore_ascii_case("HTTPS");
        match (listener.hostname.as_deref(), route_hostnames.is_empty()) {
            // Listener pins a hostname, route is unconstrained: use the listener's.
            (Some(lh), true) => out.push((lh.to_string(), is_https)),
            // Both constrain: intersect.
            (Some(lh), false) => {
                for rh in route_hostnames {
                    if host_matches(lh, rh) {
                        out.push((rh.clone(), is_https));
                    }
                }
            }
            // Listener matches all hosts; the route's hostnames win.
            (None, false) => {
                for rh in route_hostnames {
                    out.push((rh.clone(), is_https));
                }
            }
            // Neither constrains: a catch-all site.
            (None, true) => out.push((":default".to_string(), is_https)),
        }
    }
    out
}

/// Whether a route hostname is served by a listener hostname. Supports an exact
/// match or a single leading `*.` wildcard on the listener side (Gateway API
/// listener hostnames may be wildcards; route hostnames are concrete).
fn host_matches(listener: &str, route_host: &str) -> bool {
    if let Some(suffix) = listener.strip_prefix("*.") {
        route_host
            .strip_suffix(suffix)
            .is_some_and(|p| p.ends_with('.') && p.len() > 1)
    } else {
        listener == route_host
    }
}

/// Build the route line(s) for one rule, keyed for deterministic ordering. One
/// line is produced per path match (an empty match list means "match `/`").
fn rule_route_lines(rule: &crate::crd::HTTPRouteRule, upstream: &str) -> Vec<(RouteKey, String)> {
    let mut lines = Vec::new();
    if rule.matches.is_empty() {
        let (key, line) = route_line(MatchKind::Prefix, "/*", None, upstream);
        lines.push((key, line));
        return lines;
    }
    for m in &rule.matches {
        let (kind, pattern) = match &m.path {
            Some(p) if p.match_type.eq_ignore_ascii_case("Exact") => {
                (MatchKind::Exact, p.value.clone())
            }
            Some(p) => (MatchKind::Prefix, prefix_to_glob(&p.value)),
            None => (MatchKind::Prefix, "/*".to_string()),
        };
        lines.push(route_line(kind, &pattern, m.method.as_deref(), upstream));
    }
    lines
}

/// How a route pattern is matched, mirroring the Flow matcher kinds.
#[derive(Clone, Copy)]
enum MatchKind {
    /// `route = /path` — an exact path.
    Exact,
    /// `route /prefix/*` — a prefix glob.
    Prefix,
}

/// Render one Flow `route` line plus its ordering key.
fn route_line(
    kind: MatchKind,
    pattern: &str,
    method: Option<&str>,
    upstream: &str,
) -> (RouteKey, String) {
    let (kind_rank, matcher) = match kind {
        MatchKind::Exact => (0_u8, format!("= {pattern}")),
        MatchKind::Prefix => (1_u8, pattern.to_string()),
    };
    let method_up = method.map(str::to_ascii_uppercase);
    let predicate = match &method_up {
        Some(m) => format!(" [method={m}]"),
        None => String::new(),
    };
    let line = format!("route {matcher}{predicate} ~> proxy(@{upstream})");
    let key: RouteKey = (
        kind_rank,
        usize::MAX - pattern.len(),
        pattern.to_string(),
        method_up.unwrap_or_default(),
        upstream.to_string(),
    );
    (key, line)
}

/// Convert a Gateway API `PathPrefix` value into a Flow prefix glob.
///
/// `/` becomes `/*`; `/api` and `/api/` both become `/api/*`. This is a small
/// approximation: Gateway API `PathPrefix` also matches the bare prefix segment
/// itself, whereas the Flow glob matches sub-paths — see the deferred-work note
/// in the crate docs.
fn prefix_to_glob(value: &str) -> String {
    let trimmed = value.trim_end_matches('/');
    if trimmed.is_empty() {
        "/*".to_string()
    } else {
        format!("{trimmed}/*")
    }
}

/// Whether every CRD string field on `route` that reaches the generated Flow
/// text is strictly safe to interpolate. Any failure means the whole route is
/// skipped (security C1) rather than emitting unsafe config.
fn route_is_valid(route: &HTTPRoute) -> bool {
    for host in &route.spec.hostnames {
        if !is_valid_hostname(host) {
            return false;
        }
    }
    for rule in &route.spec.rules {
        for m in &rule.matches {
            if let Some(path) = &m.path {
                if !is_valid_path_value(&path.value) {
                    return false;
                }
            }
            if let Some(method) = &m.method {
                if !is_valid_method(method) {
                    return false;
                }
            }
        }
        for backend in &rule.backend_refs {
            if !is_dns_label(&backend.name) {
                return false;
            }
            if let Some(ns) = &backend.namespace {
                if !is_dns_label(ns) {
                    return false;
                }
            }
        }
    }
    true
}

/// A hostname is valid when it is a sequence of RFC 1123 DNS labels (optionally
/// prefixed with a single `*.` wildcard, as Gateway API listener hostnames may
/// be). This forbids whitespace, newlines, and every Flow metacharacter as a
/// side effect of the strict label alphabet.
fn is_valid_hostname(host: &str) -> bool {
    let host = host.strip_prefix("*.").unwrap_or(host);
    if host.is_empty() || host.len() > 253 {
        return false;
    }
    host.split('.').all(is_dns_label)
}

/// A single RFC 1123 DNS label: 1–63 lowercase-alphanumeric chars plus `-`, not
/// leading or trailing with `-`. Used for hostnames, Service names, namespaces.
fn is_dns_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    if bytes.is_empty() || bytes.len() > 63 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() || !bytes[bytes.len() - 1].is_ascii_alphanumeric() {
        return false;
    }
    label
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// An HTTP method is valid when it is one or more ASCII letters (`^[A-Za-z]+$`);
/// it is upper-cased before emission so the Flow predicate sees `^[A-Z]+$`.
fn is_valid_method(method: &str) -> bool {
    !method.is_empty() && method.bytes().all(|b| b.is_ascii_alphabetic())
}

/// A path-match value is valid when it is an absolute path with no whitespace,
/// control characters, or Flow metacharacters that could break out of the
/// matcher token.
fn is_valid_path_value(value: &str) -> bool {
    value.starts_with('/') && !has_unsafe_char(value)
}

/// Whether `s` contains a character that is unsafe to interpolate into Flow
/// source: whitespace, control characters, or a Flow metacharacter
/// (`{` `}` `(` `)` `"` `~` `>` `<` `#` `@`). `~>` is covered by rejecting `~`.
fn has_unsafe_char(s: &str) -> bool {
    s.chars().any(|c| {
        c.is_whitespace()
            || c.is_control()
            || matches!(c, '{' | '}' | '(' | ')' | '"' | '~' | '>' | '<' | '#' | '@')
    })
}

/// A deterministic, lexically valid upstream identifier for a rule.
fn upstream_name(namespace: &str, route: &str, rule_idx: usize) -> String {
    format!("up_{}_{}_{rule_idx}", sanitize(namespace), sanitize(route))
}

/// Lower-case and replace every non-alphanumeric byte with `_` so the result is
/// a valid Flow identifier / `@reference`.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect()
}

/// Render an `upstream <name> { target ... }` block for a rule's backends.
fn upstream_block(name: &str, rule: &crate::crd::HTTPRouteRule, route_ns: &str) -> String {
    let mut body = String::new();
    for backend in &rule.backend_refs {
        let ns = backend.namespace.as_deref().unwrap_or(route_ns);
        let port = backend.port.unwrap_or(DEFAULT_BACKEND_PORT);
        let url = format!("http://{}.{ns}.svc:{port}", backend.name);
        let weight = backend.weight.unwrap_or(1);
        if weight == 1 {
            let _ = writeln!(body, "  target {url}");
        } else {
            let _ = writeln!(body, "  target {url} weight={weight}");
        }
    }
    format!("upstream {name} {{\n{body}}}")
}

/// Assemble the final Flow source from the sorted upstreams and sites.
fn render(upstreams: &BTreeMap<String, String>, sites: &BTreeMap<String, SiteAcc>) -> String {
    let mut out =
        String::from("# Generated by the pulsate-k8s Gateway API controller. Do not edit.\n");

    for block in upstreams.values() {
        out.push('\n');
        out.push_str(block);
        out.push('\n');
    }

    for (host, acc) in sites {
        if acc.routes.is_empty() {
            continue;
        }
        out.push('\n');
        let _ = writeln!(out, "site {host} {{");
        out.push_str(if acc.https {
            "  tls auto\n"
        } else {
            "  tls off\n"
        });
        for line in acc.routes.values() {
            out.push_str("  ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("}\n");
    }

    out
}
