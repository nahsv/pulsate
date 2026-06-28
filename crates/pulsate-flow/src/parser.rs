//! The recursive-descent Flow parser.
//!
//! Consumes the token stream from [`crate::lexer`] and produces an [`Ast`]. The
//! grammar is the one in `docs/04-configuration.md#lexical--grammar-overview`:
//! statements are blocks, directives, or routes; arguments are positional or
//! `name=value`; routes are `~>`-separated pipelines. The parser is
//! domain-agnostic — it does not know which keywords are valid, only the shape —
//! so the keyword set lives entirely in `pulsate-config`.
//!
//! On the first malformed token the parser returns a span-anchored
//! [`Diagnostic`] (`PLS-CFG-0001`). Cross-reference and invariant checks (which
//! benefit from reporting many problems at once) happen later in validation.

use pulsate_core::Code;

use crate::ast::{Arg, Ast, Block, Directive, MatchKind, Matcher, Predicate, Route, Step, Stmt};
use crate::diag::Diagnostic;
use crate::lexer::{Token, TokenKind};
use crate::span::Span;
use crate::value::{type_atom, type_env, Spanned, Value};

/// Parse a token stream into an [`Ast`].
///
/// # Errors
/// Returns a [`Diagnostic`] on the first syntax error.
pub fn parse(tokens: Vec<Token>) -> Result<Ast, Diagnostic> {
    Parser { tokens, pos: 0 }.parse_document()
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> &Token {
        // The lexer always terminates with Eof, so this never indexes OOB.
        &self.tokens[self.pos.min(self.tokens.len() - 1)]
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    fn next_kind(&self) -> &TokenKind {
        let i = (self.pos + 1).min(self.tokens.len() - 1);
        &self.tokens[i].kind
    }

    fn bump(&mut self) -> Token {
        let t = self.tokens[self.pos.min(self.tokens.len() - 1)].clone();
        if self.pos < self.tokens.len() - 1 {
            self.pos += 1;
        }
        t
    }

    fn skip_seps(&mut self) {
        while matches!(self.peek_kind(), TokenKind::Sep) {
            self.pos += 1;
        }
    }

    fn parse_document(&mut self) -> Result<Ast, Diagnostic> {
        let stmts = self.parse_stmts(false)?;
        Ok(Ast { stmts })
    }

    /// Parse statements until EOF, or until (and consuming) a `}` when
    /// `in_block` is set.
    fn parse_stmts(&mut self, in_block: bool) -> Result<Vec<Stmt>, Diagnostic> {
        let mut stmts = Vec::new();
        loop {
            self.skip_seps();
            match self.peek_kind() {
                TokenKind::Eof => {
                    if in_block {
                        return Err(self.err(
                            Code::CFG_SYNTAX,
                            "unexpected end of input: missing closing `}`",
                        ));
                    }
                    break;
                }
                TokenKind::RBrace => {
                    if in_block {
                        self.bump();
                        break;
                    }
                    return Err(self.err(Code::CFG_SYNTAX, "unexpected `}` with no matching block"));
                }
                _ => stmts.push(self.parse_stmt()?),
            }
        }
        Ok(stmts)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, Diagnostic> {
        let TokenKind::Atom(kw) = self.peek_kind().clone() else {
            return Err(self.err(Code::CFG_SYNTAX, "expected a keyword to start a statement"));
        };
        if kw == "route" {
            return Ok(Stmt::Route(self.parse_route()?));
        }

        let kw_tok = self.bump();
        let keyword = Spanned::new(kw, kw_tok.span);
        let args = self.parse_space_args()?;

        if matches!(self.peek_kind(), TokenKind::LBrace) {
            self.bump(); // {
            let body = self.parse_stmts(true)?;
            let end = self.prev_span().end();
            let span = span_to(keyword.span, end);
            Ok(Stmt::Block(Block {
                keyword,
                args,
                body,
                span,
            }))
        } else {
            let end = args.last().map_or(keyword.span.end(), |a| a.span().end());
            let span = span_to(keyword.span, end);
            Ok(Stmt::Directive(Directive {
                keyword,
                args,
                span,
            }))
        }
    }

    /// Space-separated arguments, terminating at a separator, brace, or EOF.
    fn parse_space_args(&mut self) -> Result<Vec<Arg>, Diagnostic> {
        let mut args = Vec::new();
        while matches!(
            self.peek_kind(),
            TokenKind::Atom(_) | TokenKind::Str(_) | TokenKind::Env(_) | TokenKind::LBracket
        ) {
            args.push(self.parse_arg()?);
        }
        Ok(args)
    }

    /// One argument: `name=value` if an atom is immediately followed by `=`,
    /// else a positional value.
    fn parse_arg(&mut self) -> Result<Arg, Diagnostic> {
        if matches!(self.peek_kind(), TokenKind::Atom(_))
            && matches!(self.next_kind(), TokenKind::Equals)
        {
            let name_tok = self.bump();
            let TokenKind::Atom(name) = name_tok.kind.clone() else {
                unreachable!("checked Atom above");
            };
            self.bump(); // =
            let value = self.parse_value()?;
            Ok(Arg::Named {
                name: Spanned::new(name, name_tok.span),
                value,
            })
        } else {
            Ok(Arg::Positional(self.parse_value()?))
        }
    }

    fn parse_value(&mut self) -> Result<Spanned<Value>, Diagnostic> {
        match self.peek_kind().clone() {
            TokenKind::Str(s) => {
                let t = self.bump();
                Ok(Spanned::new(Value::Str(s), t.span))
            }
            TokenKind::Env(inner) => {
                let t = self.bump();
                Ok(Spanned::new(type_env(&inner), t.span))
            }
            TokenKind::Atom(raw) => {
                let t = self.bump();
                Ok(Spanned::new(type_atom(&raw), t.span))
            }
            TokenKind::LBracket => self.parse_array(),
            _ => Err(self.err(Code::CFG_TYPE_MISMATCH, "expected a value")),
        }
    }

    fn parse_array(&mut self) -> Result<Spanned<Value>, Diagnostic> {
        let open = self.bump(); // [
        let mut items = Vec::new();
        loop {
            self.skip_seps();
            if matches!(self.peek_kind(), TokenKind::RBracket) {
                break;
            }
            items.push(self.parse_value()?);
            self.skip_seps();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
            } else {
                break;
            }
        }
        if !matches!(self.peek_kind(), TokenKind::RBracket) {
            return Err(self.err(Code::CFG_SYNTAX, "expected `]` to close array"));
        }
        let close = self.bump();
        let span = span_to(open.span, close.span.end());
        Ok(Spanned::new(Value::Array(items), span))
    }

    fn parse_route(&mut self) -> Result<Route, Diagnostic> {
        let route_kw = self.bump(); // `route`
        let matcher = self.parse_matcher()?;
        let mut steps = Vec::new();
        loop {
            self.skip_seps();
            if matches!(self.peek_kind(), TokenKind::Arrow) {
                self.bump();
                self.skip_seps();
                steps.push(self.parse_step()?);
            } else {
                break;
            }
        }
        let end = steps.last().map_or(matcher.span.end(), |s| s.span.end());
        Ok(Route {
            span: span_to(route_kw.span, end),
            matcher,
            steps,
        })
    }

    fn parse_matcher(&mut self) -> Result<Matcher, Diagnostic> {
        // `= /path` exact; `~ pattern` regex; otherwise a bare prefix/glob path.
        let (kind, start_span) = match self.peek_kind() {
            TokenKind::Equals => {
                let t = self.bump();
                (MatchKind::Exact, t.span)
            }
            TokenKind::Atom(a) if a == "~" => {
                let t = self.bump();
                (MatchKind::Regex, t.span)
            }
            _ => {
                // Prefix: the start span is the pattern's own span.
                let span = self.peek().span;
                (MatchKind::Prefix, span)
            }
        };

        let TokenKind::Atom(pat) = self.peek_kind().clone() else {
            return Err(self.err(Code::CFG_SYNTAX, "expected a route pattern"));
        };
        let pat_tok = self.bump();
        let pattern = Spanned::new(pat, pat_tok.span);

        let mut predicates = Vec::new();
        let mut end = pat_tok.span.end();
        if matches!(self.peek_kind(), TokenKind::LBracket) {
            let close = self.parse_predicates(&mut predicates)?;
            end = close;
        }

        Ok(Matcher {
            kind,
            pattern,
            predicates,
            span: span_to(start_span, end),
        })
    }

    /// Parse `[key=value, ...]` predicates; returns the end offset of the `]`.
    fn parse_predicates(&mut self, out: &mut Vec<Predicate>) -> Result<usize, Diagnostic> {
        self.bump(); // [
        loop {
            self.skip_seps();
            if matches!(self.peek_kind(), TokenKind::RBracket) {
                break;
            }
            let TokenKind::Atom(key) = self.peek_kind().clone() else {
                return Err(self.err(Code::CFG_SYNTAX, "expected a predicate key like `method`"));
            };
            let key_tok = self.bump();
            let mut value = "*".to_string(); // presence by default
            let mut end = key_tok.span.end();
            if matches!(self.peek_kind(), TokenKind::Equals) {
                self.bump();
                let v = self.parse_value()?;
                value = value_to_lexeme(&v.node);
                end = v.span.end();
            }
            out.push(Predicate {
                key,
                value,
                span: span_to(key_tok.span, end),
            });
            self.skip_seps();
            if matches!(self.peek_kind(), TokenKind::Comma) {
                self.bump();
            } else {
                break;
            }
        }
        if !matches!(self.peek_kind(), TokenKind::RBracket) {
            return Err(self.err(Code::CFG_SYNTAX, "expected `]` to close predicates"));
        }
        let close = self.bump();
        Ok(close.span.end())
    }

    fn parse_step(&mut self) -> Result<Step, Diagnostic> {
        let TokenKind::Atom(name) = self.peek_kind().clone() else {
            return Err(self.err(Code::CFG_SYNTAX, "expected a middleware or handler name"));
        };
        let name_tok = self.bump();
        let mut args = Vec::new();
        let mut has_parens = false;
        let mut end = name_tok.span.end();
        if matches!(self.peek_kind(), TokenKind::LParen) {
            has_parens = true;
            self.bump(); // (
            loop {
                self.skip_seps();
                if matches!(self.peek_kind(), TokenKind::RParen) {
                    break;
                }
                args.push(self.parse_arg()?);
                self.skip_seps();
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    self.bump();
                } else {
                    break;
                }
            }
            if !matches!(self.peek_kind(), TokenKind::RParen) {
                return Err(self.err(Code::CFG_SYNTAX, "expected `)` to close arguments"));
            }
            end = self.bump().span.end();
        }
        Ok(Step {
            name: Spanned::new(name, name_tok.span),
            args,
            has_parens,
            span: span_to(name_tok.span, end),
        })
    }

    fn prev_span(&self) -> Span {
        self.tokens[self.pos.saturating_sub(1)].span
    }

    fn err(&self, code: Code, message: impl Into<String>) -> Diagnostic {
        Diagnostic::error(code, message, self.peek().span)
    }
}

/// A span from `start` to an absolute end offset, preserving start line/col.
fn span_to(start: Span, end: usize) -> Span {
    Span::new(
        start.offset,
        end.saturating_sub(start.offset),
        start.line,
        start.col,
    )
}

/// Render a value back to the lexeme used as a predicate's expected text.
fn value_to_lexeme(v: &Value) -> String {
    match v {
        Value::Str(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Int(i) => i.to_string(),
        _ => format!("{v:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lexer::lex;

    fn parse_str(s: &str) -> Result<Ast, Diagnostic> {
        parse(lex(s).unwrap())
    }

    #[test]
    fn parses_a_block_with_nested_directives() {
        let ast = parse_str("pulsate {\n  workers 0\n  http { port 80 }\n}").unwrap();
        assert_eq!(ast.stmts.len(), 1);
        let Stmt::Block(b) = &ast.stmts[0] else {
            panic!("expected block");
        };
        assert_eq!(b.keyword.node, "pulsate");
        assert_eq!(b.body.len(), 2);
    }

    #[test]
    fn parses_directive_with_positional_and_named_args() {
        let ast = parse_str("target http://10.0.0.11:8080 weight=3").unwrap();
        let Stmt::Directive(d) = &ast.stmts[0] else {
            panic!("expected directive");
        };
        assert_eq!(d.keyword.node, "target");
        assert_eq!(d.args.len(), 2);
        assert!(matches!(d.args[0], Arg::Positional(_)));
        match &d.args[1] {
            Arg::Named { name, value } => {
                assert_eq!(name.node, "weight");
                assert_eq!(value.node, Value::Int(3));
            }
            Arg::Positional(_) => panic!("expected named"),
        }
    }

    #[test]
    fn parses_a_multiline_route_pipeline() {
        let src = "route /api/* ~> cors(origins=[\"https://a\"])\n    ~> rate_limit(100/min, key=ip)\n    ~> proxy(@api)";
        let ast = parse_str(src).unwrap();
        let Stmt::Route(r) = &ast.stmts[0] else {
            panic!("expected route");
        };
        assert_eq!(r.matcher.kind, MatchKind::Prefix);
        assert_eq!(r.matcher.pattern.node, "/api/*");
        assert_eq!(r.steps.len(), 3);
        assert_eq!(r.steps[2].name.node, "proxy");
        assert_eq!(r.steps[2].args.len(), 1);
    }

    #[test]
    fn parses_exact_and_predicate_matchers() {
        let ast = parse_str("route = /healthz ~> respond(status=200)").unwrap();
        let Stmt::Route(r) = &ast.stmts[0] else {
            panic!("route");
        };
        assert_eq!(r.matcher.kind, MatchKind::Exact);

        let ast = parse_str("route /api/* [method=POST] ~> proxy(@api)").unwrap();
        let Stmt::Route(r) = &ast.stmts[0] else {
            panic!("route");
        };
        assert_eq!(r.matcher.predicates.len(), 1);
        assert_eq!(r.matcher.predicates[0].key, "method");
        assert_eq!(r.matcher.predicates[0].value, "POST");
    }

    #[test]
    fn parses_arrays_and_refs() {
        let ast = parse_str("route /* ~> proxy(split=[@v1, @v2])").unwrap();
        let Stmt::Route(r) = &ast.stmts[0] else {
            panic!("route");
        };
        let Arg::Named { name, value } = &r.steps[0].args[0] else {
            panic!("named");
        };
        assert_eq!(name.node, "split");
        let Value::Array(items) = &value.node else {
            panic!("expected array");
        };
        let refs: Vec<&Value> = items.iter().map(|i| &i.node).collect();
        assert_eq!(
            refs,
            vec![&Value::Ref("v1".into()), &Value::Ref("v2".into())]
        );
    }

    #[test]
    fn unclosed_block_is_an_error() {
        let err = parse_str("pulsate {\n  workers 0\n").unwrap_err();
        assert_eq!(err.code(), Code::CFG_SYNTAX);
    }
}
