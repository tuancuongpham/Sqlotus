//! Splitting a script into batches (T-SQL `GO`) and statements.

use serde::Serialize;

use crate::Dialect;
use crate::lexer::{Token, TokenKind, tokenize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

impl Span {
    pub fn contains(&self, offset: usize) -> bool {
        self.start <= offset && offset <= self.end
    }

    pub fn text<'a>(&self, src: &'a str) -> &'a str {
        &src[self.start..self.end]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Batch {
    pub span: Span,
    /// `GO 5` runs the batch five times.
    pub repeat: u32,
}

/// Index of every `GO` separator token, with its repeat count and the byte
/// offset where its line ends.
fn go_lines(tokens: &[Token<'_>]) -> Vec<(usize, u32, usize)> {
    let mut found = Vec::new();
    for (i, t) in tokens.iter().enumerate() {
        if t.kind != TokenKind::Word || !t.text.eq_ignore_ascii_case("GO") {
            continue;
        }
        let line_start =
            i == 0 || (tokens[i - 1].kind == TokenKind::Whitespace && (i == 1 || tokens[i - 1].text.contains('\n')));
        if !line_start {
            continue;
        }
        let mut repeat = 1u32;
        let mut j = i + 1;
        let mut ok = true;
        let mut line_end = t.end();
        while let Some(n) = tokens.get(j) {
            match n.kind {
                TokenKind::Whitespace if n.text.contains('\n') => break,
                TokenKind::Whitespace | TokenKind::LineComment => {}
                TokenKind::Number => repeat = n.text.parse().unwrap_or(1),
                _ => {
                    ok = false;
                    break;
                }
            }
            line_end = n.end();
            j += 1;
        }
        if ok {
            found.push((i, repeat.max(1), line_end));
        }
    }
    found
}

/// Split a script into batches. Only T-SQL has `GO`; other dialects return
/// the whole text as one batch.
pub fn split_batches(src: &str, dialect: Dialect) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut start = 0;
    if dialect == Dialect::MsSql {
        let tokens = tokenize(src, dialect);
        for (i, repeat, line_end) in go_lines(&tokens) {
            push_trimmed(&mut batches, src, start, tokens[i].start, repeat);
            start = line_end;
        }
    }
    push_trimmed(&mut batches, src, start, src.len(), 1);
    batches
}

fn push_trimmed(out: &mut Vec<Batch>, src: &str, start: usize, end: usize, repeat: u32) {
    if let Some(span) = trim_span(src, start, end) {
        out.push(Batch { span, repeat });
    }
}

fn trim_span(src: &str, start: usize, end: usize) -> Option<Span> {
    let text = &src[start..end];
    let lead = text.len() - text.trim_start().len();
    let trail = text.len() - text.trim_end().len();
    (lead + trail < text.len()).then(|| Span {
        start: start + lead,
        end: end - trail,
    })
}

const STATEMENT_STARTS: &[&str] = &[
    "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "MERGE", "CREATE", "ALTER", "DROP", "DECLARE", "EXEC", "EXECUTE",
    "SET", "IF", "TRUNCATE", "USE", "PRINT", "BEGIN", "CALL", "SHOW", "EXPLAIN",
];

/// Split into statements: on `;`, on `GO` lines, and where a blank line is
/// followed by a statement keyword outside parentheses (T-SQL scripts often
/// omit semicolons).
pub fn split_statements(src: &str, dialect: Dialect) -> Vec<Span> {
    let tokens = tokenize(src, dialect);
    let gos: Vec<usize> = if dialect == Dialect::MsSql {
        go_lines(&tokens).into_iter().map(|g| g.0).collect()
    } else {
        Vec::new()
    };
    let mut spans = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    let mut skip_until = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        if t.start < skip_until {
            continue;
        }
        match t.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth = (depth - 1).max(0),
            TokenKind::Semicolon => {
                if let Some(s) = trim_span(src, start, t.end()) {
                    spans.push(s);
                }
                start = t.end();
                depth = 0;
            }
            TokenKind::Word if gos.contains(&i) => {
                if let Some(s) = trim_span(src, start, t.start) {
                    spans.push(s);
                }
                let line_end = src[t.start..].find('\n').map_or(src.len(), |n| t.start + n);
                start = line_end;
                skip_until = line_end;
                depth = 0;
            }
            TokenKind::Word if depth == 0 && i > 0 => {
                let blank_before =
                    tokens[i - 1].kind == TokenKind::Whitespace && tokens[i - 1].text.matches('\n').count() >= 2;
                if blank_before && STATEMENT_STARTS.iter().any(|k| t.text.eq_ignore_ascii_case(k)) {
                    if let Some(s) = trim_span(src, start, t.start) {
                        spans.push(s);
                    }
                    start = t.start;
                }
            }
            _ => {}
        }
    }
    if let Some(s) = trim_span(src, start, src.len()) {
        spans.push(s);
    }
    spans
}

/// The statement containing `offset`, or the one just before it when the
/// cursor sits in whitespace after a statement.
pub fn statement_at(src: &str, dialect: Dialect, offset: usize) -> Option<Span> {
    let spans = split_statements(src, dialect);
    spans
        .iter()
        .find(|s| s.contains(offset))
        .or_else(|| spans.iter().rev().find(|s| s.end <= offset))
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batches_split_on_go() {
        let src = "select 1\nGO\nselect 2\ngo 3\n  \nselect 3";
        let b = split_batches(src, Dialect::MsSql);
        let texts: Vec<_> = b.iter().map(|x| (x.span.text(src), x.repeat)).collect();
        assert_eq!(texts, vec![("select 1", 1), ("select 2", 3), ("select 3", 1)]);
    }

    #[test]
    fn go_inside_text_is_not_a_separator() {
        let src = "select 'GO'\nselect go_live from t";
        assert_eq!(split_batches(src, Dialect::MsSql).len(), 1);
    }

    #[test]
    fn statements_split() {
        let src = "select 1; select 2\n\nupdate t set a = 1\nwhere b = (select 2\n\nfrom x)";
        let s: Vec<_> = split_statements(src, Dialect::MsSql)
            .iter()
            .map(|s| s.text(src))
            .collect();
        assert_eq!(
            s,
            vec![
                "select 1;",
                "select 2",
                "update t set a = 1\nwhere b = (select 2\n\nfrom x)"
            ]
        );
    }

    #[test]
    fn statement_at_cursor() {
        let src = "select 1;\nselect a from t;";
        let at = statement_at(src, Dialect::MsSql, 15).unwrap();
        assert_eq!(at.text(src), "select a from t;");
    }
}
