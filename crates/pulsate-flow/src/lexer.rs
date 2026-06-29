//! The hand-written Flow lexer.
//!
//! Turns source text into a flat token stream, each token carrying a [`Span`].
//! The lexer is intentionally simple (`docs/04-configuration.md`): it recognizes
//! the structural punctuation, the `~>` flow operator, quoted strings, and
//! `${env}` references as whole tokens, and emits everything else as an `Atom`
//! (a bare run of non-structural characters). Typing an atom into a duration,
//! size, ref, etc. happens later, in [`crate::value`], where context is known.

use crate::diag::Diagnostic;
use crate::span::Span;
use pulsate_core::Code;

/// The kind of a lexed token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `[`
    LBracket,
    /// `]`
    RBracket,
    /// `,`
    Comma,
    /// `=`
    Equals,
    /// `~>` — the flow/pipeline operator.
    Arrow,
    /// A statement separator: a run of newlines and/or `;`. Collapsed to one
    /// token so blank lines and trailing `;` do not produce empty statements.
    Sep,
    /// A `"double quoted"` string, with escapes already resolved.
    Str(String),
    /// A `${VAR}` or `${VAR:-default}` environment reference (raw inner text).
    Env(String),
    /// A bare run of characters: identifiers, hosts, paths, `@refs`, numbers,
    /// `secret://x`, durations, sizes, regex matchers, etc.
    Atom(String),
    /// End of input.
    Eof,
}

/// A token with its source span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// What was lexed.
    pub kind: TokenKind,
    /// Where it was lexed from.
    pub span: Span,
}

/// Lex the full source into tokens, terminating with [`TokenKind::Eof`].
///
/// # Errors
/// Returns a [`Diagnostic`] (`PLS-CFG-0001`) on an unterminated string or
/// malformed `${...}`.
pub fn lex(text: &str) -> Result<Vec<Token>, Diagnostic> {
    Lexer::new(text).run()
}

struct Lexer<'a> {
    bytes: &'a [u8],
    text: &'a str,
    pos: usize,
    line: u32,
    col: u32,
}

impl<'a> Lexer<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            text,
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn run(mut self) -> Result<Vec<Token>, Diagnostic> {
        let mut tokens = Vec::new();
        loop {
            self.skip_horizontal();
            if self.pos >= self.bytes.len() {
                tokens.push(Token {
                    kind: TokenKind::Eof,
                    span: self.here(0),
                });
                return Ok(tokens);
            }
            // Statement separators: a run of newlines / `;` (with interleaved
            // horizontal trivia and comments) collapses to one `Sep`.
            if matches!(self.peek(), Some(b'\n' | b';')) {
                let start = self.pos;
                let line = self.line;
                let col = self.col;
                self.consume_separator_run();
                tokens.push(Token {
                    kind: TokenKind::Sep,
                    span: Span::new(start, self.pos - start, line, col),
                });
                continue;
            }
            tokens.push(self.next_token()?);
        }
    }

    /// Consume a maximal run of separators (`\n`, `;`) plus any horizontal
    /// trivia and line comments between them.
    fn consume_separator_run(&mut self) {
        loop {
            match self.peek() {
                Some(b'\n' | b';' | b' ' | b'\t' | b'\r') => {
                    self.bump();
                }
                Some(b'#') => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => break,
            }
        }
    }

    /// A span starting at the current position with the given length.
    fn here(&self, len: usize) -> Span {
        Span::new(self.pos, len, self.line, self.col)
    }

    fn bump(&mut self) -> u8 {
        let b = self.bytes[self.pos];
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        b
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn peek2(&self) -> Option<u8> {
        self.bytes.get(self.pos + 1).copied()
    }

    /// Skip horizontal whitespace and `#` line comments, but NOT newlines or
    /// `;` — those are statement separators emitted as [`TokenKind::Sep`].
    fn skip_horizontal(&mut self) {
        while let Some(b) = self.peek() {
            match b {
                b' ' | b'\t' | b'\r' => {
                    self.bump();
                }
                b'#' => {
                    while let Some(c) = self.peek() {
                        if c == b'\n' {
                            break;
                        }
                        self.bump();
                    }
                }
                _ => break,
            }
        }
    }

    fn next_token(&mut self) -> Result<Token, Diagnostic> {
        let start = self.pos;
        let line = self.line;
        let col = self.col;
        let b = self.peek().expect("next_token called at EOF");

        // Single-character structural punctuation.
        let single = match b {
            b'{' => Some(TokenKind::LBrace),
            b'}' => Some(TokenKind::RBrace),
            b'(' => Some(TokenKind::LParen),
            b')' => Some(TokenKind::RParen),
            b'[' => Some(TokenKind::LBracket),
            b']' => Some(TokenKind::RBracket),
            b',' => Some(TokenKind::Comma),
            b'=' => Some(TokenKind::Equals),
            _ => None,
        };
        if let Some(kind) = single {
            self.bump();
            return Ok(Token {
                kind,
                span: Span::new(start, 1, line, col),
            });
        }

        // The `~>` flow operator.
        if b == b'~' && self.peek2() == Some(b'>') {
            self.bump();
            self.bump();
            return Ok(Token {
                kind: TokenKind::Arrow,
                span: Span::new(start, 2, line, col),
            });
        }

        if b == b'"' {
            return self.lex_string(start, line, col);
        }

        if b == b'$' && self.peek2() == Some(b'{') {
            return self.lex_env(start, line, col);
        }

        Ok(self.lex_atom(start, line, col))
    }

    fn lex_string(&mut self, start: usize, line: u32, col: u32) -> Result<Token, Diagnostic> {
        self.bump(); // opening quote
        let mut value = String::new();
        loop {
            let Some(b) = self.peek() else {
                return Err(Diagnostic::error(
                    Code::CFG_SYNTAX,
                    "unterminated string literal",
                    Span::new(start, self.pos - start, line, col),
                )
                .with_help("add a closing `\"`"));
            };
            match b {
                b'"' => {
                    self.bump();
                    let span = Span::new(start, self.pos - start, line, col);
                    return Ok(Token {
                        kind: TokenKind::Str(value),
                        span,
                    });
                }
                b'\\' => {
                    self.bump();
                    match self.peek() {
                        Some(b'n') => value.push('\n'),
                        Some(b't') => value.push('\t'),
                        Some(b'r') => value.push('\r'),
                        Some(b'"') => value.push('"'),
                        Some(b'\\') => value.push('\\'),
                        Some(other) => value.push(other as char),
                        None => continue,
                    }
                    self.bump();
                }
                _ => {
                    // Multi-byte UTF-8 is copied through verbatim.
                    let ch_start = self.pos;
                    self.bump();
                    while self.peek().is_some_and(|c| c & 0b1100_0000 == 0b1000_0000) {
                        self.bump();
                    }
                    value.push_str(&self.text[ch_start..self.pos]);
                }
            }
        }
    }

    fn lex_env(&mut self, start: usize, line: u32, col: u32) -> Result<Token, Diagnostic> {
        self.bump(); // $
        self.bump(); // {
        let inner_start = self.pos;
        loop {
            let Some(b) = self.peek() else {
                return Err(Diagnostic::error(
                    Code::CFG_SYNTAX,
                    "unterminated `${...}` environment reference",
                    Span::new(start, self.pos - start, line, col),
                )
                .with_help("add a closing `}`"));
            };
            if b == b'}' {
                let inner = self.text[inner_start..self.pos].to_string();
                self.bump(); // }
                let span = Span::new(start, self.pos - start, line, col);
                return Ok(Token {
                    kind: TokenKind::Env(inner),
                    span,
                });
            }
            self.bump();
        }
    }

    fn lex_atom(&mut self, start: usize, line: u32, col: u32) -> Token {
        while let Some(b) = self.peek() {
            // Atoms end at whitespace, comments, or structural punctuation.
            let stop = matches!(
                b,
                b' ' | b'\t'
                    | b'\r'
                    | b'\n'
                    | b'#'
                    | b'{'
                    | b'}'
                    | b'('
                    | b')'
                    | b'['
                    | b']'
                    | b','
                    | b'='
                    | b'"'
                    | b';'
            ) || (b == b'~' && self.peek2() == Some(b'>'));
            if stop {
                break;
            }
            self.bump();
        }
        let raw = self.text[start..self.pos].to_string();
        Token {
            kind: TokenKind::Atom(raw),
            span: Span::new(start, self.pos - start, line, col),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<TokenKind> {
        lex(text).unwrap().into_iter().map(|t| t.kind).collect()
    }

    #[test]
    fn lexes_a_route_pipeline() {
        let k = kinds("route /api/* ~> compress ~> proxy(@api)");
        assert_eq!(
            k,
            vec![
                TokenKind::Atom("route".into()),
                TokenKind::Atom("/api/*".into()),
                TokenKind::Arrow,
                TokenKind::Atom("compress".into()),
                TokenKind::Arrow,
                TokenKind::Atom("proxy".into()),
                TokenKind::LParen,
                TokenKind::Atom("@api".into()),
                TokenKind::RParen,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn comments_are_trivia_newlines_are_separators() {
        let k = kinds("pulsate {\n  # a comment\n  workers 0\n}");
        assert_eq!(
            k,
            vec![
                TokenKind::Atom("pulsate".into()),
                TokenKind::LBrace,
                TokenKind::Sep,
                TokenKind::Atom("workers".into()),
                TokenKind::Atom("0".into()),
                TokenKind::Sep,
                TokenKind::RBrace,
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn semicolons_are_separators() {
        let k = kinds("user \"pulsate\"; group \"pulsate\"");
        assert_eq!(
            k,
            vec![
                TokenKind::Atom("user".into()),
                TokenKind::Str("pulsate".into()),
                TokenKind::Sep,
                TokenKind::Atom("group".into()),
                TokenKind::Str("pulsate".into()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn strings_with_escapes() {
        let toks = lex(r#"respond(body="a\n\"b\"")"#).unwrap();
        let s = toks.iter().find_map(|t| match &t.kind {
            TokenKind::Str(s) => Some(s.clone()),
            _ => None,
        });
        assert_eq!(s, Some("a\n\"b\"".to_string()));
    }

    #[test]
    fn env_reference_is_one_token() {
        let k = kinds("port ${HTTPS_PORT:-443}");
        assert_eq!(k[1], TokenKind::Env("HTTPS_PORT:-443".into()));
    }

    #[test]
    fn unterminated_string_is_an_error() {
        let err = lex("respond(body=\"oops)").unwrap_err();
        assert_eq!(err.code(), Code::CFG_SYNTAX);
    }

    #[test]
    fn spans_track_line_and_col() {
        let toks = lex("a\n  bb").unwrap();
        assert_eq!((toks[0].span.line, toks[0].span.col), (1, 1));
        assert_eq!(toks[1].kind, TokenKind::Sep);
        assert_eq!((toks[2].span.line, toks[2].span.col), (2, 3));
    }
}
