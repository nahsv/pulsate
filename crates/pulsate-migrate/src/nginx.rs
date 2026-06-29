//! The nginx importer: a small brace/semicolon parser plus a mapping to Flow.

use crate::{Builder, Fidelity, Import};

/// A parsed nginx directive: `name args... ;` or `name args... { block }`.
struct Node {
    name: String,
    args: Vec<String>,
    block: Vec<Node>,
}

/// Import an `nginx.conf` into Flow.
pub fn import(text: &str) -> Import {
    let tokens = tokenize(text);
    let (nodes, _) = parse_block(&tokens, 0);
    let mut b = Builder::default();
    b.line("# Imported from nginx by `pulsate import nginx`. Review the notes.");
    map(&nodes, &mut b);
    b.finish()
}

/// Split nginx source into words plus the standalone tokens `{ } ;`.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '#' => {
                while let Some(&n) = chars.peek() {
                    if n == '\n' {
                        break;
                    }
                    chars.next();
                }
            }
            '{' | '}' | ';' => {
                push_word(&mut tokens, &mut word);
                tokens.push(c.to_string());
            }
            c if c.is_whitespace() => push_word(&mut tokens, &mut word),
            c => word.push(c),
        }
    }
    push_word(&mut tokens, &mut word);
    tokens
}

fn push_word(tokens: &mut Vec<String>, word: &mut String) {
    if !word.is_empty() {
        tokens.push(std::mem::take(word));
    }
}

/// Parse a brace block starting at `pos`, returning the nodes and the index past
/// the closing `}` (or end of input at the top level).
fn parse_block(tokens: &[String], mut pos: usize) -> (Vec<Node>, usize) {
    let mut nodes = Vec::new();
    let mut head: Vec<String> = Vec::new();
    while pos < tokens.len() {
        match tokens[pos].as_str() {
            "}" => return (nodes, pos + 1),
            ";" => {
                if let Some((name, args)) = split_head(&mut head) {
                    nodes.push(Node {
                        name,
                        args,
                        block: Vec::new(),
                    });
                }
                pos += 1;
            }
            "{" => {
                let (block, next) = parse_block(tokens, pos + 1);
                if let Some((name, args)) = split_head(&mut head) {
                    nodes.push(Node { name, args, block });
                }
                pos = next;
            }
            _ => {
                head.push(tokens[pos].clone());
                pos += 1;
            }
        }
    }
    (nodes, pos)
}

fn split_head(head: &mut Vec<String>) -> Option<(String, Vec<String>)> {
    if head.is_empty() {
        return None;
    }
    let parts = std::mem::take(head);
    let mut it = parts.into_iter();
    let name = it.next()?;
    Some((name, it.collect()))
}

fn map(nodes: &[Node], b: &mut Builder) {
    for node in nodes {
        match node.name.as_str() {
            // `http {}` / `stream {}` wrap the interesting blocks.
            "http" | "stream" | "events" => map(&node.block, b),
            "upstream" => map_upstream(node, b),
            "server" => map_server(node, b),
            other => b.note(
                other,
                Fidelity::Dropped,
                "top-level directive not translated",
            ),
        }
    }
}

fn map_upstream(node: &Node, b: &mut Builder) {
    let name = node.args.first().cloned().unwrap_or_default();
    b.open(&format!("upstream {name} {{"));
    for child in &node.block {
        if child.name == "server" {
            if let Some(addr) = child.args.first() {
                b.line(&format!("target {}", with_scheme(addr)));
            }
        } else {
            b.note(
                &child.name,
                Fidelity::Manual,
                "upstream directive needs manual translation",
            );
        }
    }
    b.close();
    b.note(
        "upstream",
        Fidelity::Exact,
        "mapped to a Flow upstream pool",
    );
}

fn map_server(node: &Node, b: &mut Builder) {
    let mut hosts: Vec<String> = Vec::new();
    let mut ssl = false;
    for child in &node.block {
        match child.name.as_str() {
            "server_name" => hosts.clone_from(&child.args),
            "listen" if child.args.iter().any(|a| a == "ssl") => ssl = true,
            _ => {}
        }
    }
    if hosts.is_empty() || hosts == ["_"] {
        hosts = vec![":default".to_string()];
    }

    b.open(&format!("site {} {{", hosts.join(" ")));
    if ssl {
        b.line("tls auto");
        b.note(
            "listen ssl",
            Fidelity::Approximate,
            "TLS mapped to `tls auto` (ACME); manual certs need a tls{} block",
        );
    }
    for child in &node.block {
        if child.name == "location" {
            map_location(child, b);
        }
    }
    b.close();
}

fn map_location(loc: &Node, b: &mut Builder) {
    // `location = /path` (exact) | `location /path` (prefix) | `location ~ re`.
    let (matcher, exact) = match loc.args.split_first() {
        Some((m, rest)) if m == "=" => (rest.first().cloned().unwrap_or_default(), true),
        Some((m, _)) if m == "~" || m == "~*" => {
            b.note(
                "location ~",
                Fidelity::Manual,
                "regex location needs a manual Flow regex route",
            );
            return;
        }
        Some((p, _)) => (p.clone(), false),
        None => return,
    };

    let route = if exact {
        format!("route = {matcher}")
    } else if matcher == "/" {
        "route /*".to_string()
    } else {
        format!("route {}/*", matcher.trim_end_matches('/'))
    };

    // Pick the terminal handler from the block.
    let mut handler = None;
    let mut try_root = None;
    for d in &loc.block {
        match d.name.as_str() {
            "proxy_pass" => {
                if let Some(target) = d.args.first() {
                    handler = Some(format!("proxy({})", upstream_ref(target)));
                    b.note("proxy_pass", Fidelity::Exact, "mapped to proxy()");
                }
            }
            "return" => {
                handler = Some(map_return(&d.args, b));
            }
            "root" | "alias" => try_root = d.args.first().cloned(),
            _ => {}
        }
    }
    if handler.is_none() {
        if let Some(root) = try_root {
            handler = Some(format!("files(\"{root}\")"));
            b.note("root", Fidelity::Exact, "mapped to files()");
        }
    }

    match handler {
        Some(h) => b.line(&format!("{route} ~> {h}")),
        None => b.note(
            "location",
            Fidelity::Manual,
            "no recognized handler (proxy_pass/root/return) in this location",
        ),
    }
}

fn map_return(args: &[String], b: &mut Builder) -> String {
    // `return 301 https://...` → redirect; `return 204` → respond.
    match args {
        [code, url] if url.contains("://") || url.starts_with('/') => {
            b.note("return", Fidelity::Exact, "mapped to redirect()");
            format!("redirect(to=\"{url}\", status={code})")
        }
        [code, ..] => {
            b.note("return", Fidelity::Approximate, "mapped to respond()");
            format!("respond(status={code})")
        }
        _ => {
            b.note("return", Fidelity::Manual, "unrecognized return form");
            "respond(status=200)".to_string()
        }
    }
}

/// If `target` is a bare `http://name` matching no scheme/host:port, treat a
/// scheme-less host as an upstream reference; otherwise keep the URL.
fn upstream_ref(target: &str) -> String {
    // `proxy_pass http://backend;` where `backend` is an upstream name → `@backend`.
    if let Some(host) = target.strip_prefix("http://") {
        if !host.contains(':') && !host.contains('.') && !host.contains('/') {
            return format!("@{host}");
        }
    }
    target.to_string()
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
    fn tokenizer_separates_structure() {
        let t = tokenize("server { listen 80; }");
        assert_eq!(t, vec!["server", "{", "listen", "80", ";", "}"]);
    }

    #[test]
    fn exact_and_prefix_locations() {
        let conf = "server { server_name a.com; location = /h { return 204; } location /api { proxy_pass http://x:1; } }";
        let imported = import(conf);
        assert!(imported.flow.contains("route = /h ~> respond(status=204)"));
        assert!(imported.flow.contains("route /api/* ~> proxy(http://x:1)"));
    }
}
