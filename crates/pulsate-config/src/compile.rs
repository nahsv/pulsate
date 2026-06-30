//! Lowering the Flow AST into the typed [`Config`], validating it, and building
//! a deterministic [`ConfigSnapshot`].
//!
//! This is the control-plane half of `docs/02-architecture.md#configuration-loading`:
//! `source → AST → typed Config → validate → ConfigSnapshot`. Validation is
//! *total before publish* — a config that produces any error diagnostic never
//! becomes a snapshot, so the running snapshot is untouched on a bad reload.

use pulsate_core::{Code, ConfigSnapshot, SnapshotId};
use pulsate_flow::ast::{Arg, Ast, Step, Stmt};
use pulsate_flow::value::Value;
use pulsate_flow::{Diagnostic, Severity, Span};

use crate::model::{
    Breaker, CacheDef, Config, Handler, Host, MatchKind as ModelMatchKind, MwSpec, NameRef, Named,
    RefUse, Retry, RouteDef, Site, Target, TlsMode, Upstream, WafDef,
};

/// Terminal handler keywords.
const HANDLERS: &[&str] = &["proxy", "files", "redirect", "respond", "grpc", "ws"];

/// `flow_version`s this binary understands.
const SUPPORTED_FLOW_VERSIONS: &[&str] = &["1"];

/// A successfully compiled configuration: the typed model, its snapshot, and any
/// non-fatal warnings.
#[derive(Debug, Clone)]
pub struct Compiled {
    /// The validated typed config.
    pub config: Config,
    /// The published-ready immutable snapshot.
    pub snapshot: ConfigSnapshot,
    /// Warnings (deprecations, etc.) that did not block compilation.
    pub warnings: Vec<Diagnostic>,
}

/// Compile Flow source into a [`Compiled`] config at the given `generation`.
///
/// # Errors
/// Returns all collected error [`Diagnostic`]s if parsing, lowering, or
/// validation fails. The caller renders them against the source.
pub fn compile(name: &str, text: &str, generation: u64) -> Result<Compiled, Vec<Diagnostic>> {
    let ast = pulsate_flow::parse(name, text).map_err(|d| vec![d])?;

    let mut diags = Vec::new();
    let config = lower(&ast, &mut diags);
    validate(&config, &mut diags);

    let (errors, warnings): (Vec<_>, Vec<_>) = diags
        .into_iter()
        .partition(|d| d.severity() == Severity::Error);
    if !errors.is_empty() {
        return Err(errors);
    }

    let snapshot = build_snapshot(&config, generation);
    Ok(Compiled {
        config,
        snapshot,
        warnings,
    })
}

/// Lower the AST into a typed [`Config`], emitting structural diagnostics
/// (handler counts, unsupported version) as it goes.
fn lower(ast: &Ast, diags: &mut Vec<Diagnostic>) -> Config {
    let mut config = Config {
        flow_version: None,
        upstreams: Vec::new(),
        caches: Vec::new(),
        wafs: Vec::new(),
        user_sets: Vec::new(),
        sites: Vec::new(),
    };

    for stmt in &ast.stmts {
        match stmt {
            Stmt::Directive(d) if d.keyword.node == "flow_version" => {
                if let Some(Value::Str(v)) = first_positional(&d.args) {
                    config.flow_version = Some(v.clone());
                    if !SUPPORTED_FLOW_VERSIONS.contains(&v.as_str()) {
                        diags.push(
                            Diagnostic::error(
                                Code::CFG_TYPE_MISMATCH,
                                format!("unsupported flow_version {v:?}"),
                                d.span,
                            )
                            .with_help("this binary supports flow_version \"1\""),
                        );
                    }
                }
            }
            Stmt::Block(b) => match b.keyword.node.as_str() {
                "upstream" => config.upstreams.push(lower_upstream(b)),
                "cache" => config.caches.push(lower_cache(b)),
                "waf" => config.wafs.push(lower_waf(b)),
                "users" => push_named(&mut config.user_sets, b),
                "site" => config.sites.push(lower_site(b, diags)),
                _ => {}
            },
            _ => {}
        }
    }
    config
}

fn push_named(out: &mut Vec<Named>, block: &pulsate_flow::ast::Block) {
    if let Some(Value::Str(name)) = first_positional(&block.args) {
        out.push(Named {
            name: name.clone(),
            span: block.keyword.span,
        });
    }
}

/// Lower an `upstream <name> { target ...; policy ...; retry {...}; breaker {...} }`
/// block into a typed [`Upstream`].
fn lower_upstream(block: &pulsate_flow::ast::Block) -> Upstream {
    let name = match first_positional(&block.args) {
        Some(Value::Str(s)) => s.clone(),
        _ => String::new(),
    };
    let mut targets = Vec::new();
    let mut policy = "round_robin".to_string();
    let mut retry = None;
    let mut breaker = None;

    for stmt in &block.body {
        match stmt {
            Stmt::Directive(d) if d.keyword.node == "target" => {
                if let Some(url) = first_positional(&d.args).and_then(resolve_str) {
                    let weight = match named_value(&d.args, "weight") {
                        Some(Value::Int(i)) => u32::try_from(*i).unwrap_or(1),
                        _ => 1,
                    };
                    targets.push(Target { url, weight });
                }
            }
            Stmt::Directive(d) if d.keyword.node == "policy" => {
                if let Some(p) = first_positional_str(&d.args) {
                    policy = p;
                }
            }
            Stmt::Block(b) if b.keyword.node == "retry" => retry = Some(lower_retry(b)),
            Stmt::Block(b) if b.keyword.node == "breaker" => breaker = Some(lower_breaker(b)),
            _ => {}
        }
    }

    Upstream {
        name,
        span: block.keyword.span,
        targets,
        policy,
        retry,
        breaker,
    }
}

fn lower_retry(block: &pulsate_flow::ast::Block) -> Retry {
    let mut attempts = 1;
    let mut retry_on_status = Vec::new();
    let mut on_connect_error = true;
    for stmt in &block.body {
        let Stmt::Directive(d) = stmt else { continue };
        match d.keyword.node.as_str() {
            "attempts" => {
                if let Some(Value::Int(i)) = first_positional(&d.args) {
                    attempts = u32::try_from(*i).unwrap_or(1);
                }
            }
            "on" => {
                if let Some(Value::Array(items)) = first_positional(&d.args) {
                    for item in items {
                        match &item.node {
                            Value::Int(i) => {
                                if let Ok(s) = u16::try_from(*i) {
                                    retry_on_status.push(s);
                                }
                            }
                            Value::Str(s) if s == "connect_error" => on_connect_error = true,
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }
    Retry {
        attempts,
        retry_on_status,
        on_connect_error,
    }
}

fn lower_breaker(block: &pulsate_flow::ast::Block) -> Breaker {
    let mut consecutive_failures = 5;
    let mut open_for_secs = 15;
    for stmt in &block.body {
        let Stmt::Directive(d) = stmt else { continue };
        match d.keyword.node.as_str() {
            "min_requests" | "consecutive" => {
                if let Some(Value::Int(i)) = first_positional(&d.args) {
                    consecutive_failures = u32::try_from(*i).unwrap_or(5);
                }
            }
            "open_for" => {
                if let Some(Value::Duration(dur)) = first_positional(&d.args) {
                    open_for_secs = dur.as_secs();
                }
            }
            _ => {}
        }
    }
    Breaker {
        consecutive_failures,
        open_for_secs,
    }
}

fn lower_site(block: &pulsate_flow::ast::Block, diags: &mut Vec<Diagnostic>) -> Site {
    let hosts: Vec<Host> = block
        .args
        .iter()
        .filter_map(|a| match a {
            Arg::Positional(v) => match &v.node {
                Value::Str(s) => Some(Host {
                    pattern: s.clone(),
                    span: v.span,
                }),
                _ => None,
            },
            Arg::Named { .. } => None,
        })
        .collect();

    let mut tls = TlsMode::Auto; // secure by default
    let mut routes = Vec::new();
    for stmt in &block.body {
        match stmt {
            Stmt::Directive(d) if d.keyword.node == "tls" => {
                tls = match first_positional(&d.args) {
                    Some(Value::Bool(false)) => TlsMode::Off,
                    Some(Value::Str(s)) if s == "off" => TlsMode::Off,
                    Some(Value::Str(s)) if s == "auto" => TlsMode::Auto,
                    _ => TlsMode::Manual,
                };
            }
            Stmt::Block(b) if b.keyword.node == "tls" => tls = TlsMode::Manual,
            Stmt::Route(r) => routes.push(lower_route(r, diags)),
            _ => {}
        }
    }

    Site {
        hosts,
        tls,
        routes,
        span: block.span,
    }
}

fn lower_route(route: &pulsate_flow::ast::Route, diags: &mut Vec<Diagnostic>) -> RouteDef {
    // Indices of steps whose name is a known terminal handler.
    let handler_idxs: Vec<usize> = route
        .steps
        .iter()
        .enumerate()
        .filter(|(_, s)| HANDLERS.contains(&s.name.node.as_str()))
        .map(|(i, _)| i)
        .collect();

    let terminal_idx: Option<usize> = match handler_idxs.len() {
        1 => Some(handler_idxs[0]),
        0 => match route
            .steps
            .iter()
            .rposition(|s| s.name.node.starts_with("plugin."))
        {
            Some(i) if i == route.steps.len().saturating_sub(1) => Some(i),
            _ => {
                diags.push(
                    Diagnostic::error(
                        Code::CFG_MISSING_FIELD,
                        "route has no terminal handler",
                        route.span,
                    )
                    .with_help("end the pipeline with a handler, e.g. `~> proxy(@api)`"),
                );
                None
            }
        },
        _ => {
            diags.push(
                Diagnostic::error(
                    Code::CFG_MULTI_HANDLER,
                    "a route has more than one terminal handler",
                    route.steps[handler_idxs[1]].span,
                )
                .with_help("a route has exactly one handler; make earlier steps middleware"),
            );
            Some(handler_idxs[0])
        }
    };

    let handler = terminal_idx.map(|i| build_handler(&route.steps[i]));
    let mw_steps: Vec<&Step> = route
        .steps
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != terminal_idx)
        .map(|(_, s)| s)
        .collect();
    let middleware: Vec<String> = mw_steps.iter().map(|s| s.name.node.clone()).collect();
    let mw_specs: Vec<MwSpec> = mw_steps
        .iter()
        .filter_map(|s| build_middleware(s))
        .collect();

    let mut refs = Vec::new();
    for step in &route.steps {
        collect_refs(&step.args, &mut refs);
    }

    let method = route
        .matcher
        .predicates
        .iter()
        .find(|p| p.key == "method")
        .map(|p| p.value.to_ascii_uppercase());

    RouteDef {
        kind: map_kind(route.matcher.kind),
        pattern: route.matcher.pattern.node.clone(),
        method,
        middleware,
        mw_specs,
        handler,
        refs,
        span: route.span,
    }
}

/// Compile a middleware step into an [`MwSpec`], if it is one of the supported
/// built-ins. Unrecognized steps (e.g. `compress`, `headers` with a `set={...}`
/// map) are dropped here.
fn build_middleware(step: &Step) -> Option<MwSpec> {
    match step.name.node.as_str() {
        "strip_prefix" => Some(MwSpec::StripPrefix(first_positional_str(&step.args)?)),
        "cors" => Some(MwSpec::Cors {
            origins: named_str_array(&step.args, "origins"),
            methods: named_str_array(&step.args, "methods"),
            credentials: matches!(
                named_value(&step.args, "credentials"),
                Some(Value::Bool(true))
            ),
        }),
        "rate_limit" => {
            let (count, per_secs) = match first_positional(&step.args) {
                Some(Value::Rate { count, per }) => (*count, rate_window_secs(*per)),
                _ => return None,
            };
            let key = named_str(&step.args, "key").unwrap_or_else(|| "ip".to_string());
            Some(MwSpec::RateLimit {
                count,
                per_secs,
                key,
            })
        }
        "waf" => Some(MwSpec::Waf(first_ref(&step.args)?)),
        "cache" => Some(MwSpec::Cache(first_ref(&step.args)?)),
        _ => None,
    }
}

fn rate_window_secs(per: pulsate_flow::value::RateWindow) -> u64 {
    use pulsate_flow::value::RateWindow;
    match per {
        RateWindow::Second => 1,
        RateWindow::Minute => 60,
        RateWindow::Hour => 3600,
    }
}

fn first_ref(args: &[Arg]) -> Option<String> {
    args.iter().find_map(|a| match a {
        Arg::Positional(v) | Arg::Named { value: v, .. } => match &v.node {
            Value::Ref(r) => Some(r.clone()),
            _ => None,
        },
    })
}

/// Lower a `cache <name> { default_ttl ...; methods [...]; vary [...]; ... }` block.
fn lower_cache(block: &pulsate_flow::ast::Block) -> CacheDef {
    let name = first_positional_str(&block.args).unwrap_or_default();
    let mut def = CacheDef {
        name,
        span: block.keyword.span,
        default_ttl_secs: 60,
        methods: vec!["GET".into(), "HEAD".into()],
        vary: Vec::new(),
        swr_secs: 0,
    };
    for stmt in &block.body {
        let Stmt::Directive(d) = stmt else { continue };
        match d.keyword.node.as_str() {
            "default_ttl" => {
                if let Some(Value::Duration(dur)) = first_positional(&d.args) {
                    def.default_ttl_secs = dur.as_secs();
                }
            }
            "methods" => def.methods = positional_str_array(&d.args),
            "vary" => def.vary = positional_str_array(&d.args),
            "stale_while_revalidate" => {
                if let Some(Value::Duration(dur)) = first_positional(&d.args) {
                    def.swr_secs = dur.as_secs();
                }
            }
            _ => {}
        }
    }
    def
}

/// Lower a `waf <name> { mode ...; ip { deny [...]; allow [...] } }` block.
fn lower_waf(block: &pulsate_flow::ast::Block) -> WafDef {
    let name = first_positional_str(&block.args).unwrap_or_default();
    let mut def = WafDef {
        name,
        span: block.keyword.span,
        mode: "block".into(),
        ip_deny: Vec::new(),
        ip_allow: Vec::new(),
    };
    for stmt in &block.body {
        match stmt {
            Stmt::Directive(d) if d.keyword.node == "mode" => {
                if let Some(m) = first_positional_str(&d.args) {
                    def.mode = m;
                }
            }
            Stmt::Block(b) if b.keyword.node == "ip" => {
                for inner in &b.body {
                    let Stmt::Directive(d) = inner else { continue };
                    match d.keyword.node.as_str() {
                        "deny" => def.ip_deny = positional_str_array(&d.args),
                        "allow" => def.ip_allow = positional_str_array(&d.args),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    def
}

/// Collect the string items of a directive's first positional array argument.
fn positional_str_array(args: &[Arg]) -> Vec<String> {
    match first_positional(args) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| match &i.node {
                Value::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn map_kind(k: pulsate_flow::ast::MatchKind) -> ModelMatchKind {
    match k {
        pulsate_flow::ast::MatchKind::Prefix => ModelMatchKind::Prefix,
        pulsate_flow::ast::MatchKind::Exact => ModelMatchKind::Exact,
        pulsate_flow::ast::MatchKind::Regex => ModelMatchKind::Regex,
    }
}

/// Build a typed [`Handler`] from its terminal step's arguments.
fn build_handler(step: &Step) -> Handler {
    let name = step.name.node.as_str();
    match name {
        "files" => Handler::Files {
            root: first_positional_str(&step.args).unwrap_or_default(),
            try_files: named_str_array(&step.args, "try"),
        },
        "respond" => Handler::Respond {
            status: status_arg(&step.args, 200),
            body: named_str(&step.args, "body").unwrap_or_default(),
        },
        "redirect" => Handler::Redirect {
            to: named_str(&step.args, "to")
                .or_else(|| first_positional_str(&step.args))
                .unwrap_or_default(),
            status: status_arg(&step.args, 308),
        },
        "proxy" => {
            let mut upstream = None;
            let mut target = None;
            for arg in &step.args {
                let v = match arg {
                    Arg::Positional(v) | Arg::Named { value: v, .. } => &v.node,
                };
                match v {
                    Value::Ref(r) if upstream.is_none() => upstream = Some(r.clone()),
                    Value::Str(s) if target.is_none() => target = Some(s.clone()),
                    _ => {}
                }
            }
            Handler::Proxy { upstream, target }
        }
        other => Handler::Other(other.to_string()),
    }
}

/// Recursively collect every `@ref` used in an argument list.
fn collect_refs(args: &[Arg], out: &mut Vec<RefUse>) {
    for arg in args {
        let value = match arg {
            Arg::Positional(v) | Arg::Named { value: v, .. } => v,
        };
        collect_refs_value(&value.node, value.span, out);
    }
}

fn collect_refs_value(value: &Value, span: Span, out: &mut Vec<RefUse>) {
    match value {
        Value::Ref(name) => out.push(RefUse {
            name: name.clone(),
            span,
        }),
        Value::Array(items) => {
            for item in items {
                collect_refs_value(&item.node, item.span, out);
            }
        }
        _ => {}
    }
}

/// Cross-cutting validation: duplicate names, dangling references, host
/// collisions. Appends diagnostics; never mutates the config.
fn validate(config: &Config, diags: &mut Vec<Diagnostic>) {
    check_dup_names(
        config.upstreams.iter().map(|u| (u.name.as_str(), u.span)),
        "upstream",
        diags,
    );
    check_dup_names(
        config.caches.iter().map(|c| (c.name.as_str(), c.span)),
        "cache",
        diags,
    );
    check_dup_names(
        config.wafs.iter().map(|w| (w.name.as_str(), w.span)),
        "waf",
        diags,
    );
    check_duplicates(&config.user_sets, "users", diags);

    let defined = config.defined_names();
    for site in &config.sites {
        for route in &site.routes {
            for r in &route.refs {
                if !defined.iter().any(|n| n.name == r.name.as_str()) {
                    let mut diag = Diagnostic::error(
                        Code::CFG_UNKNOWN_REF,
                        format!("no definition named `{}`", r.name),
                        r.span,
                    );
                    if let Some(suggestion) = nearest(&r.name, &defined) {
                        diag = diag.with_help(format!("did you mean `@{suggestion}`?"));
                    }
                    diags.push(diag);
                }
            }
        }
    }

    // `cors(origins=["*"], credentials=true)` is forbidden by the Fetch spec and
    // is a credential-leak footgun; reject it at compile time (LOW).
    for site in &config.sites {
        for route in &site.routes {
            for mw in &route.mw_specs {
                if let MwSpec::Cors {
                    origins,
                    credentials,
                    ..
                } = mw
                {
                    if *credentials && origins.iter().any(|o| o == "*") {
                        diags.push(
                            Diagnostic::error(
                                Code::CFG_INVALID_CORS,
                                "cors(credentials=true) cannot be combined with a `*` origin",
                                route.span,
                            )
                            .with_help("list explicit origins, or set credentials=false"),
                        );
                    }
                }
            }
        }
    }

    check_host_collisions(config, diags);
}

fn check_duplicates(items: &[Named], kind: &str, diags: &mut Vec<Diagnostic>) {
    check_dup_names(items.iter().map(|n| (n.name.as_str(), n.span)), kind, diags);
}

/// Flag any name that appears earlier in the sequence.
fn check_dup_names<'a>(
    names: impl Iterator<Item = (&'a str, Span)>,
    kind: &str,
    diags: &mut Vec<Diagnostic>,
) {
    let mut seen: Vec<&str> = Vec::new();
    for (name, span) in names {
        if seen.contains(&name) {
            diags.push(
                Diagnostic::error(
                    Code::CFG_DUPLICATE,
                    format!("duplicate {kind} named `{name}`"),
                    span,
                )
                .with_help("each name must be unique; remove or rename the duplicate"),
            );
        } else {
            seen.push(name);
        }
    }
}

fn check_host_collisions(config: &Config, diags: &mut Vec<Diagnostic>) {
    let mut seen: Vec<(&str, Span)> = Vec::new();
    for site in &config.sites {
        for host in &site.hosts {
            if seen.iter().any(|(h, _)| *h == host.pattern) {
                diags.push(
                    Diagnostic::error(
                        Code::CFG_HOST_COLLISION,
                        format!("host `{}` is claimed by more than one site", host.pattern),
                        host.span,
                    )
                    .with_help("two sites cannot bind the same host:port"),
                );
            } else {
                seen.push((&host.pattern, host.span));
            }
        }
    }
}

/// Build a deterministic snapshot. The identity is a content hash of the typed
/// config, so equivalent configs share an id.
fn build_snapshot(config: &Config, generation: u64) -> ConfigSnapshot {
    let digest = fnv1a(&format!("{config:?}"));
    ConfigSnapshot::builder(SnapshotId::from_digest(digest), generation).build()
}

/// FNV-1a 64-bit, for content-addressable snapshot ids.
fn fnv1a(s: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in s.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The defined name closest to `name` within edit distance 2, for "did you mean".
fn nearest<'a>(name: &str, defined: &[NameRef<'a>]) -> Option<&'a str> {
    defined
        .iter()
        .map(|n| (levenshtein(name, n.name), n.name))
        .filter(|(d, _)| *d <= 2)
        .min_by_key(|(d, _)| *d)
        .map(|(_, n)| n)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

fn first_positional(args: &[Arg]) -> Option<&Value> {
    args.iter().find_map(|a| match a {
        Arg::Positional(v) => Some(&v.node),
        Arg::Named { .. } => None,
    })
}

fn first_positional_str(args: &[Arg]) -> Option<String> {
    match first_positional(args)? {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    }
}

/// Resolve a value to a string, expanding `${ENV:-default}` references against
/// the process environment at load time.
fn resolve_str(v: &Value) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.clone()),
        Value::Env { var, default } => std::env::var(var).ok().or_else(|| default.clone()),
        _ => None,
    }
}

fn named_value<'a>(args: &'a [Arg], key: &str) -> Option<&'a Value> {
    args.iter().find_map(|a| match a {
        Arg::Named { name, value } if name.node == key => Some(&value.node),
        _ => None,
    })
}

fn named_str(args: &[Arg], key: &str) -> Option<String> {
    match named_value(args, key)? {
        Value::Str(s) => Some(s.clone()),
        _ => None,
    }
}

fn named_str_array(args: &[Arg], key: &str) -> Vec<String> {
    match named_value(args, key) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|i| match &i.node {
                Value::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// Read a `status` argument (named or first positional integer), clamped into a
/// `u16`; falls back to `default`.
fn status_arg(args: &[Arg], default: u16) -> u16 {
    let int = match named_value(args, "status") {
        Some(Value::Int(i)) => Some(*i),
        _ => match first_positional(args) {
            Some(Value::Int(i)) => Some(*i),
            _ => None,
        },
    };
    int.and_then(|i| u16::try_from(i).ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn errors(text: &str) -> Vec<Diagnostic> {
        compile("t.flow", text, 1).err().unwrap_or_default()
    }

    #[test]
    fn compiles_a_valid_config() {
        let src = r"
            upstream api { target http://127.0.0.1:8080 }
            site app.example.com {
              tls auto
              route /api/* ~> compress ~> proxy(@api)
            }
        ";
        let compiled = compile("t.flow", src, 1).expect("compiles");
        assert_eq!(compiled.config.upstreams.len(), 1);
        assert_eq!(compiled.config.sites.len(), 1);
        let route = &compiled.config.sites[0].routes[0];
        assert_eq!(route.handler.as_ref().map(Handler::name), Some("proxy"));
        assert_eq!(route.middleware, vec!["compress"]);
    }

    #[test]
    fn unknown_reference_suggests_nearest() {
        let src = r"
            upstream api { target http://127.0.0.1:8080 }
            site s.example.com { route /* ~> proxy(@apii) }
        ";
        let errs = errors(src);
        assert_eq!(errs.len(), 1);
        assert_eq!(errs[0].code(), Code::CFG_UNKNOWN_REF);
        assert!(errs[0]
            .render(&pulsate_flow::Source::new("t.flow", src))
            .contains("did you mean `@api`?"));
    }

    #[test]
    fn route_without_handler_is_rejected() {
        let errs = errors("site s.com { route /api/* ~> compress }");
        assert!(errs.iter().any(|d| d.code() == Code::CFG_MISSING_FIELD));
    }

    #[test]
    fn two_handlers_in_one_route_is_rejected() {
        let errs = errors("upstream a { target http://x:1 }\nsite s.com { route /* ~> proxy(@a) ~> files(\"/srv\") }");
        assert!(errs.iter().any(|d| d.code() == Code::CFG_MULTI_HANDLER));
    }

    #[test]
    fn duplicate_upstream_is_rejected() {
        let errs = errors("upstream a { target http://x:1 }\nupstream a { target http://y:2 }");
        assert!(errs.iter().any(|d| d.code() == Code::CFG_DUPLICATE));
    }

    #[test]
    fn host_collision_is_rejected() {
        let src = "site dup.com { route /* ~> respond(status=200) }\nsite dup.com { route /* ~> respond(status=200) }";
        let errs = errors(src);
        assert!(errs.iter().any(|d| d.code() == Code::CFG_HOST_COLLISION));
    }

    #[test]
    fn snapshot_is_deterministic_for_equivalent_configs() {
        let src = "site a.com { route /* ~> respond(status=200) }";
        let a = compile("t.flow", src, 1).unwrap();
        let b = compile("t.flow", src, 9).unwrap();
        // Same content → same id, regardless of generation.
        assert_eq!(a.snapshot.id(), b.snapshot.id());
        assert_ne!(a.snapshot.generation(), b.snapshot.generation());
    }

    #[test]
    fn unsupported_flow_version_is_rejected() {
        let errs = errors("flow_version \"2\"\nsite s.com { route /* ~> respond(status=200) }");
        assert!(errs.iter().any(|d| d.code() == Code::CFG_TYPE_MISMATCH));
    }

    #[test]
    fn cors_wildcard_with_credentials_is_rejected() {
        let src = "site s.com { route /* ~> cors(origins=[\"*\"], credentials=true) ~> respond(status=200) }";
        let errs = errors(src);
        assert!(errs.iter().any(|d| d.code() == Code::CFG_INVALID_CORS));
    }

    #[test]
    fn cors_explicit_origin_with_credentials_is_allowed() {
        let src = "site s.com { route /* ~> cors(origins=[\"https://app.example.com\"], credentials=true) ~> respond(status=200) }";
        assert!(compile("t.flow", src, 0).is_ok());
    }
}
