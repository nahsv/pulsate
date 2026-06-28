//! The Flow abstract syntax tree.
//!
//! A faithful, untyped-by-domain tree: blocks, directives, and routes with
//! typed [`Value`]s and spans everywhere. Domain meaning (what a `site` or
//! `upstream` block *means*) is applied later by `pulsate-config` when it lowers
//! the AST into the typed config model. Keeping the AST domain-agnostic means
//! the parser never needs to know the full keyword set
//! (`docs/02-architecture.md#configuration-loading`).

use crate::span::Span;
use crate::value::{Spanned, Value};

/// A parsed Flow document: a flat list of top-level statements.
#[derive(Debug, Clone, PartialEq)]
pub struct Ast {
    /// The top-level statements, in source order.
    pub stmts: Vec<Stmt>,
}

/// A single statement.
#[derive(Debug, Clone, PartialEq)]
pub enum Stmt {
    /// `keyword [args] { ... }`
    Block(Block),
    /// `keyword [args]`
    Directive(Directive),
    /// `route <matcher> ~> step ~> ...`
    Route(Route),
}

impl Stmt {
    /// The statement's full source span.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Stmt::Block(b) => b.span,
            Stmt::Directive(d) => d.span,
            Stmt::Route(r) => r.span,
        }
    }
}

/// A `keyword [args] { body }` block.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    /// The leading keyword (e.g. `site`, `upstream`, `p8`).
    pub keyword: Spanned<String>,
    /// Positional/named arguments before the brace (e.g. `site example.com`).
    pub args: Vec<Arg>,
    /// The nested statements.
    pub body: Vec<Stmt>,
    /// The full span from keyword to closing brace.
    pub span: Span,
}

/// A `keyword [args]` directive (no body).
#[derive(Debug, Clone, PartialEq)]
pub struct Directive {
    /// The leading keyword (e.g. `workers`, `target`, `tls`).
    pub keyword: Spanned<String>,
    /// Its arguments.
    pub args: Vec<Arg>,
    /// The full span.
    pub span: Span,
}

/// A positional or named argument.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// A bare value.
    Positional(Spanned<Value>),
    /// `name=value`.
    Named {
        /// The argument name.
        name: Spanned<String>,
        /// Its value.
        value: Spanned<Value>,
    },
}

impl Arg {
    /// The argument's span.
    #[must_use]
    pub fn span(&self) -> Span {
        match self {
            Arg::Positional(v) => v.span,
            Arg::Named { name, value } => Span::new(
                name.span.offset,
                value.span.end() - name.span.offset,
                name.span.line,
                name.span.col,
            ),
        }
    }
}

/// How a route matcher interprets its pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchKind {
    /// Bare path: prefix/glob match (`/api/*`).
    Prefix,
    /// `= /healthz`: exact path.
    Exact,
    /// `~ ^/u/(?<id>\d+)$`: regex.
    Regex,
}

/// A route matcher: a kind, a pattern, and optional `[key=value]` predicates.
#[derive(Debug, Clone, PartialEq)]
pub struct Matcher {
    /// How `pattern` is interpreted.
    pub kind: MatchKind,
    /// The path/regex pattern.
    pub pattern: Spanned<String>,
    /// Refinements like `[method=POST]`, `[host=admin.example.com]`.
    pub predicates: Vec<Predicate>,
    /// The matcher's span.
    pub span: Span,
}

/// A `key=value` route predicate from a `[...]` refinement.
#[derive(Debug, Clone, PartialEq)]
pub struct Predicate {
    /// The predicate key (`method`, `host`, `header.x-canary`, `query.q`).
    pub key: String,
    /// The expected value (`*` means "present").
    pub value: String,
    /// The predicate's span.
    pub span: Span,
}

/// A route: `route <matcher> ~> step ~> ... ~> handler`.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    /// The matcher selecting requests.
    pub matcher: Matcher,
    /// The pipeline steps (middleware then a terminal handler). The parser does
    /// not distinguish them; `pulsate-config` classifies by name.
    pub steps: Vec<Step>,
    /// The full route span.
    pub span: Span,
}

/// One step in a route pipeline: a middleware or a handler, `name(args)`.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// The step name (`compress`, `proxy`, `plugin.geoblock`).
    pub name: Spanned<String>,
    /// Its arguments, if it had a `(...)` list.
    pub args: Vec<Arg>,
    /// Whether a `(...)` argument list was present (distinguishes `compress`
    /// from `compress()`).
    pub has_parens: bool,
    /// The step's span.
    pub span: Span,
}
