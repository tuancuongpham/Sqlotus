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

/// A stretch of a MySQL script between `DELIMITER` lines, with the statement
/// terminator in force there.
struct Region<'a> {
    start: usize,
    end: usize,
    delimiter: &'a str,
}

/// MySQL client `DELIMITER x` lines (dumps, procedure and trigger scripts).
/// The server does not know them, so the script is cut there. `None` when
/// the script has none. `DELIMITER` counts only at the start of a line that
/// follows a finished statement, so a column named `delimiter` stays a column.
fn delimiter_regions<'a>(src: &'a str, tokens: &[Token<'a>]) -> Option<Vec<Region<'a>>> {
    let mut regions = Vec::new();
    let mut start = 0;
    let mut delimiter = ";";
    let mut at_boundary = true;
    let mut skip_until = 0;
    for (i, t) in tokens.iter().enumerate() {
        if t.start < skip_until || t.is_trivia() {
            continue;
        }
        let line_start =
            i == 0 || (tokens[i - 1].kind == TokenKind::Whitespace && (i == 1 || tokens[i - 1].text.contains('\n')));
        if t.kind == TokenKind::Word && t.text.eq_ignore_ascii_case("DELIMITER") && line_start && at_boundary {
            let line_end = src[t.end()..].find('\n').map_or(src.len(), |n| t.end() + n);
            if let Some(next) = src[t.end()..line_end].split_whitespace().next() {
                regions.push(Region {
                    start,
                    end: t.start,
                    delimiter,
                });
                delimiter = next;
                start = line_end;
                skip_until = line_end;
                continue;
            }
        }
        at_boundary = src[..t.end()].ends_with(delimiter);
    }
    if regions.is_empty() {
        return None;
    }
    regions.push(Region {
        start,
        end: src.len(),
        delimiter,
    });
    Some(regions)
}

/// The statements of a region whose terminator is not `;`: cut at each
/// terminator outside strings, quoted names and comments (the terminator is
/// left out, as the client does).
fn region_pieces(src: &str, tokens: &[Token<'_>], r: &Region<'_>) -> Vec<Span> {
    let mut out = Vec::new();
    let mut piece = r.start;
    let mut from = r.start;
    while let Some(found) = src[from..r.end].find(r.delimiter) {
        let at = from + found;
        let i = tokens.partition_point(|t| t.end() <= at);
        let protected = tokens.get(i).filter(|t| {
            t.start <= at
                && matches!(
                    t.kind,
                    TokenKind::String | TokenKind::QuotedIdent | TokenKind::LineComment | TokenKind::BlockComment
                )
        });
        if let Some(t) = protected {
            from = t.end().min(r.end);
            continue;
        }
        out.extend(trim_span(src, piece, at));
        piece = at + r.delimiter.len();
        from = piece;
    }
    out.extend(trim_span(src, piece, r.end));
    out
}

/// Split a script into batches. T-SQL splits on `GO`; a MySQL script with
/// `DELIMITER` lines runs each custom-terminated statement on its own; other
/// scripts are one multi-statement batch.
pub fn split_batches(src: &str, dialect: Dialect) -> Vec<Batch> {
    let mut batches = Vec::new();
    let mut start = 0;
    if dialect == Dialect::MySql {
        let tokens = tokenize(src, dialect);
        let Some(regions) = delimiter_regions(src, &tokens) else {
            push_trimmed(&mut batches, src, 0, src.len(), 1);
            return batches;
        };
        for r in &regions {
            if r.delimiter == ";" {
                push_trimmed(&mut batches, src, r.start, r.end, 1);
            } else {
                batches.extend(
                    region_pieces(src, &tokens, r)
                        .into_iter()
                        .map(|span| Batch { span, repeat: 1 }),
                );
            }
        }
        return batches;
    }
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
    if dialect == Dialect::MySql
        && let Some(regions) = delimiter_regions(src, &tokens)
    {
        let mut spans = Vec::new();
        for r in &regions {
            if r.delimiter == ";" {
                spans.extend(split_range(src, &tokens, &[], r.start, r.end));
            } else {
                spans.extend(region_pieces(src, &tokens, r));
            }
        }
        return spans;
    }
    let gos: Vec<usize> = if dialect == Dialect::MsSql {
        go_lines(&tokens).into_iter().map(|g| g.0).collect()
    } else {
        Vec::new()
    };
    split_range(src, &tokens, &gos, 0, src.len())
}

/// [`split_statements`] over the tokens in `from..to`.
fn split_range(src: &str, tokens: &[Token<'_>], gos: &[usize], from: usize, to: usize) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut start = from;
    let mut depth = 0i32;
    let mut skip_until = 0usize;
    for (i, t) in tokens.iter().enumerate() {
        if t.start < from || t.start < skip_until {
            continue;
        }
        if t.start >= to {
            break;
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
    if let Some(s) = trim_span(src, start.min(to), to) {
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

    fn batch_texts(src: &str, d: Dialect) -> Vec<&str> {
        split_batches(src, d).iter().map(|b| b.span.text(src)).collect()
    }

    #[test]
    fn go_edge_cases() {
        // GO inside block comments (also nested) and multi-line strings is text
        let src = "select 1 /* outer /* inner\nGO\n*/ still comment\nGO\n*/\nselect 'a\nGO\nb'";
        assert_eq!(split_batches(src, Dialect::MsSql).len(), 1);
        // CRLF, lowercase, a trailing comment and a leading blank line
        let src = "\r\nselect 1\r\ngo -- run it\r\nselect 2\r\n";
        assert_eq!(batch_texts(src, Dialect::MsSql), vec!["select 1", "select 2"]);
        // GO with a count, and an empty batch between two GOs
        let b = split_batches("select 1\nGO 2\nGO\nselect 2", Dialect::MsSql);
        assert_eq!(b.iter().map(|b| b.repeat).collect::<Vec<_>>(), vec![2, 1]);
        // `GO` followed by more code on its line is not a separator
        assert_eq!(split_batches("select 1\nGO select 2", Dialect::MsSql).len(), 1);
        // other dialects have no GO
        assert_eq!(split_batches("select 1\nGO\nselect 2", Dialect::Postgres).len(), 1);
    }

    #[test]
    fn mysql_delimiter_scripts() {
        let src = "CREATE TABLE t (a int);\n\
                   DELIMITER //\n\
                   CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 'x // y';\nEND //\n\
                   CREATE TRIGGER tr BEFORE INSERT ON t FOR EACH ROW BEGIN SET NEW.a = 1; END//\n\
                   DELIMITER ;\n\
                   SELECT 2; SELECT 3;";
        assert_eq!(
            batch_texts(src, Dialect::MySql),
            vec![
                "CREATE TABLE t (a int);",
                "CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 'x // y';\nEND",
                "CREATE TRIGGER tr BEFORE INSERT ON t FOR EACH ROW BEGIN SET NEW.a = 1; END",
                "SELECT 2; SELECT 3;",
            ]
        );
        // statements follow the delimiter too, so Shift+F5 runs the whole procedure
        let st: Vec<_> = split_statements(src, Dialect::MySql)
            .iter()
            .map(|s| s.text(src))
            .collect();
        assert_eq!(st.len(), 5, "{st:?}");
        assert!(st[1].starts_with("CREATE PROCEDURE") && st[1].ends_with("END"));
        assert_eq!(st[3..], ["SELECT 2;", "SELECT 3;"]);
        let at = statement_at(src, Dialect::MySql, src.find("SELECT 1").unwrap()).unwrap();
        assert!(at.text(src).starts_with("CREATE PROCEDURE"));
    }

    #[test]
    fn mysql_dump_style_delimiters() {
        // mysqldump: `;;`, versioned comments, `$$` glued to END
        let src = "DELIMITER ;;\n/*!50003 CREATE*/ /*!50003 TRIGGER t1 BEFORE INSERT ON t FOR EACH ROW SET NEW.a = 1 */;;\nDELIMITER ;\n\
                   delimiter $$\nCREATE FUNCTION f() RETURNS int DETERMINISTIC BEGIN RETURN 1; END$$\ndelimiter ;";
        assert_eq!(
            batch_texts(src, Dialect::MySql),
            vec![
                "/*!50003 CREATE*/ /*!50003 TRIGGER t1 BEFORE INSERT ON t FOR EACH ROW SET NEW.a = 1 */",
                "CREATE FUNCTION f() RETURNS int DETERMINISTIC BEGIN RETURN 1; END",
            ]
        );
    }

    #[test]
    fn delimiter_word_that_is_not_a_directive() {
        // a column named delimiter at the start of a line inside a statement
        let src = "SELECT a,\ndelimiter FROM t;";
        assert_eq!(batch_texts(src, Dialect::MySql), vec![src]);
        // the delimiter inside a comment or a string does not end a statement
        let src = "DELIMITER //\nSELECT 1 -- not here //\n, '//' //\nDELIMITER ;";
        assert_eq!(
            batch_texts(src, Dialect::MySql),
            vec!["SELECT 1 -- not here //\n, '//'"]
        );
        // only MySQL has DELIMITER
        assert_eq!(split_batches("DELIMITER //\nSELECT 1 //", Dialect::Postgres).len(), 1);
    }

    #[test]
    fn mysql_double_dash_needs_a_space() {
        let src = "SELECT 5--2;\nSELECT 1; -- comment ; here\nSELECT 3;";
        let st: Vec<_> = split_statements(src, Dialect::MySql)
            .iter()
            .map(|s| s.text(src))
            .collect();
        assert_eq!(st, vec!["SELECT 5--2;", "SELECT 1;", "-- comment ; here\nSELECT 3;"]);
        // SQL Server and PostgreSQL: `--` always starts a comment
        assert_eq!(split_statements("SELECT 5--2;\nSELECT 3;", Dialect::MsSql).len(), 1);
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
