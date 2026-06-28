//! Compiling the typed [`Config`] into the data-plane [`Router`] and the
//! upstream [`Registry`].
//!
//! This is a control-plane → data-plane handoff: the control plane owns the
//! typed model; the router and registry are the immutable artifacts the data
//! plane matches and forwards against. Reference-bearing middleware (`waf(@name)`,
//! `cache(@name)`) are resolved against the config here, and per-cache stores are
//! shared across every route that references them.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use pulsate_cache::{CacheConfig, CacheLayer, MemoryStore};
use pulsate_pipeline::{Cors, Mw};
use pulsate_proxy::{BreakerPolicy, Policy, Registry, RetryPolicy, Upstream as PUpstream};
use pulsate_router::{Handler as RHandler, MatchKind as RKind, Route, Router, SiteRoutes};
use pulsate_waf::{Cidr, IpAcl, Mode, RateLimiter, WafEngine};

use crate::model::{Config, Handler, MatchKind, MwSpec, RouteDef};

/// Build the upstream registry (data-plane pools) for a validated [`Config`].
#[must_use]
pub fn build_upstreams(config: &Config) -> Registry {
    let mut registry = Registry::new();
    for up in &config.upstreams {
        let targets = up
            .targets
            .iter()
            .map(|t| (t.url.clone(), t.weight))
            .collect::<Vec<_>>();
        let retry = up
            .retry
            .as_ref()
            .map_or_else(RetryPolicy::default, |r| RetryPolicy {
                attempts: r.attempts,
                retry_on_status: r.retry_on_status.clone(),
                on_connect_error: r.on_connect_error,
            });
        let breaker = up
            .breaker
            .as_ref()
            .map_or_else(BreakerPolicy::default, |b| BreakerPolicy {
                consecutive_failures: b.consecutive_failures,
                open_for: Duration::from_secs(b.open_for_secs),
            });
        registry.insert(PUpstream::new(
            up.name.clone(),
            targets,
            Policy::parse(&up.policy),
            retry,
            breaker,
        ));
    }
    registry
}

/// Build the routing table for a validated [`Config`].
#[must_use]
pub fn build_router(config: &Config) -> Router {
    // One shared cache layer per named cache, so every referencing route hits the
    // same store.
    let cache_layers: HashMap<String, CacheLayer> = config
        .caches
        .iter()
        .map(|c| {
            let cfg = CacheConfig {
                default_ttl: Duration::from_secs(c.default_ttl_secs),
                methods: c.methods.clone(),
                vary: c.vary.clone(),
                stale_while_revalidate: Duration::from_secs(c.swr_secs),
                ..CacheConfig::default()
            };
            (
                c.name.clone(),
                CacheLayer::new(Arc::new(MemoryStore::new()), cfg),
            )
        })
        .collect();

    let sites = config
        .sites
        .iter()
        .map(|site| SiteRoutes {
            hosts: site.hosts.iter().map(|h| h.pattern.clone()).collect(),
            routes: site
                .routes
                .iter()
                .filter_map(|r| build_route(r, config, &cache_layers))
                .collect(),
        })
        .collect();
    Router::new(sites)
}

fn build_route(
    def: &RouteDef,
    config: &Config,
    cache_layers: &HashMap<String, CacheLayer>,
) -> Option<Route> {
    let mut middleware = Vec::new();
    let mut cache = None;

    for spec in &def.mw_specs {
        match spec {
            MwSpec::StripPrefix(p) => middleware.push(Mw::StripPrefix(p.clone())),
            MwSpec::Cors {
                origins,
                methods,
                credentials,
            } => middleware.push(Mw::Cors(Cors {
                origins: origins.clone(),
                methods: methods.clone(),
                credentials: *credentials,
            })),
            MwSpec::RateLimit {
                count,
                per_secs,
                key,
            } => middleware.push(Mw::RateLimit {
                limiter: Arc::new(RateLimiter::new(*count, Duration::from_secs(*per_secs))),
                key: key.clone(),
            }),
            MwSpec::Waf(name) => {
                if let Some(waf) = config.waf(name) {
                    if let Some(acl) = build_ip_acl(&waf.ip_deny, &waf.ip_allow) {
                        middleware.push(Mw::Ip(Arc::new(acl)));
                    }
                    let mode = if waf.mode == "detect" {
                        Mode::Detect
                    } else {
                        Mode::Block
                    };
                    middleware.push(Mw::Waf(Arc::new(WafEngine::new(mode))));
                }
            }
            MwSpec::Cache(name) => cache = cache_layers.get(name).cloned(),
        }
    }

    Some(Route {
        kind: map_kind(def.kind),
        pattern: def.pattern.clone(),
        method: def.method.clone(),
        middleware,
        cache,
        handler: map_handler(def.handler.as_ref()?),
    })
}

/// Build an [`IpAcl`] from deny/allow CIDR strings, or `None` if neither has any
/// valid entries.
fn build_ip_acl(deny: &[String], allow: &[String]) -> Option<IpAcl> {
    let mut acl = IpAcl::new();
    let mut any = false;
    for cidr in allow {
        if let Ok(c) = cidr.parse::<Cidr>() {
            acl = acl.allow(c);
            any = true;
        }
    }
    for cidr in deny {
        if let Ok(c) = cidr.parse::<Cidr>() {
            acl = acl.deny(c);
            any = true;
        }
    }
    any.then_some(acl)
}

fn map_kind(k: MatchKind) -> RKind {
    match k {
        MatchKind::Prefix => RKind::Prefix,
        MatchKind::Exact => RKind::Exact,
        MatchKind::Regex => RKind::Regex,
    }
}

fn map_handler(h: &Handler) -> RHandler {
    match h {
        Handler::Files { root, try_files } => RHandler::Files {
            root: root.clone(),
            try_files: try_files.clone(),
        },
        Handler::Respond { status, body } => RHandler::Respond {
            status: *status,
            body: body.clone(),
        },
        Handler::Redirect { to, status } => RHandler::Redirect {
            to: to.clone(),
            status: *status,
        },
        Handler::Proxy { upstream, target } => RHandler::Proxy {
            upstream: upstream.clone(),
            target: target.clone(),
        },
        Handler::Other(n) => RHandler::Other(n.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile;

    #[test]
    fn builds_a_router_that_matches() {
        let src = "site app.com {\n  route = /healthz ~> respond(status=204)\n  route /* ~> files(\"/srv\")\n}";
        let compiled = compile("t.flow", src, 0).unwrap();
        let router = build_router(&compiled.config);
        assert_eq!(router.site_count(), 1);

        let health = router.route("app.com", "/healthz", "GET").unwrap();
        assert!(matches!(
            health.handler,
            RHandler::Respond { status: 204, .. }
        ));
        let any = router.route("app.com", "/index.html", "GET").unwrap();
        assert!(matches!(any.handler, RHandler::Files { .. }));
    }

    #[test]
    fn resolves_cache_and_waf_references() {
        let src = "\
            cache assets { default_ttl 5m; methods [GET] }\n\
            waf strict { mode block; ip { deny [\"10.0.0.0/8\"] } }\n\
            site app.com {\n\
              route /a/* ~> cache(@assets) ~> waf(@strict) ~> respond(status=200)\n\
            }";
        let compiled = compile("t.flow", src, 0).unwrap();
        let router = build_router(&compiled.config);
        let route = router.route("app.com", "/a/x", "GET").unwrap();
        assert!(route.cache.is_some(), "cache layer resolved");
        // waf(@strict) compiles to an IP ACL + a WAF engine.
        assert_eq!(route.middleware.len(), 2);
    }
}
