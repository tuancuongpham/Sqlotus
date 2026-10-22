//! Splitting a script into batches (T-SQL `GO`) and statements.

use serde::Serialize;

use std::ops::Range;

use crate::Dialect;
use crate::lexer::{Token, TokenKind, significant, tokenize};

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
pub(crate) fn go_lines(tokens: &[Token<'_>]) -> Vec<(usize, u32, usize)> {
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
pub(crate) struct Region<'a> {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) delimiter: &'a str,
}

/// MySQL client `DELIMITER x` lines (dumps, procedure and trigger scripts).
/// The server does not know them, so the script is cut there. `None` when
/// the script has none. `DELIMITER` counts only at the start of a line that
/// follows a finished statement, so a column named `delimiter` stays a column.
pub(crate) fn delimiter_regions<'a>(src: &'a str, tokens: &[Token<'a>]) -> Option<Vec<Region<'a>>> {
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
pub(crate) fn region_pieces(src: &str, tokens: &[Token<'_>], r: &Region<'_>) -> Vec<Span> {
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

/// Whether `text` holds anything but whitespace and comments: a batch or
/// statement of comments alone is not sent (MySQL calls it an empty query).
/// MySQL runs `/*!50003 CREATE */` comments (dumps), so they are code.
fn has_code(text: &str, dialect: Dialect) -> bool {
    tokenize(text, dialect).iter().any(|t| {
        !t.is_trivia() || (dialect == Dialect::MySql && t.kind == TokenKind::BlockComment && t.text.starts_with("/*!"))
    })
}

/// Split a script into batches. T-SQL splits on `GO`; a MySQL script with
/// `DELIMITER` lines runs each custom-terminated statement on its own; other
/// scripts are one multi-statement batch. Batches of comments alone are left
/// out.
pub fn split_batches(src: &str, dialect: Dialect) -> Vec<Batch> {
    let mut batches = all_batches(src, dialect);
    batches.retain(|b| has_code(b.span.text(src), dialect));
    batches
}

fn all_batches(src: &str, dialect: Dialect) -> Vec<Batch> {
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

/// Words after which a statement keyword goes on with the statement: `AS`
/// (CREATE VIEW v AS, CREATE TABLE t AS), `FOR` (DECLARE c CURSOR FOR),
/// `THEN` (MERGE ... WHEN MATCHED THEN), `DO` (CREATE EVENT, ON CONFLICT DO),
/// set operators (SELECT ... UNION SELECT ...; also UNION ALL / DISTINCT).
const CONTINUED_AFTER: &[&str] = &[
    "AS",
    "FOR",
    "THEN",
    "ELSE",
    "DO",
    "INSTEAD",
    "ALSO",
    "UNION",
    "EXCEPT",
    "INTERSECT",
    "MINUS",
];

/// Statements that are not whole until one more statement keyword: a CTE
/// before its main statement, IF and WHILE before their body, INSERT before
/// its rows (VALUES, SELECT, EXEC), EXPLAIN before what it explains.
const NEED_MORE: &[&str] = &["WITH", "IF", "WHILE", "INSERT", "REPLACE", "EXPLAIN"];

/// Whether `word` is one of `words` (upper case), in any case.
fn one_of(word: &str, words: &[&str]) -> bool {
    words.iter().any(|w| word.eq_ignore_ascii_case(w))
}

/// Split into statements: on `;`, on `GO` lines, and where a blank line is
/// followed by a statement keyword outside parentheses (T-SQL scripts often
/// omit semicolons) — unless the statement before is not whole yet: `WITH x
/// AS (...)` and its DELETE, `IF ...` and its body, `INSERT INTO t (...)` and
/// its SELECT, `UNION ALL` and the next SELECT are one statement. Comments
/// alone are not a statement.
pub fn split_statements(src: &str, dialect: Dialect) -> Vec<Span> {
    let mut spans = all_statements(src, dialect);
    spans.retain(|s| has_code(s.text(src), dialect));
    spans
}

fn all_statements(src: &str, dialect: Dialect) -> Vec<Span> {
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
    // the statement read so far: whether it has code, its first word, whether
    // it still needs a statement keyword to be whole, CASE ... END nesting
    // (whose ELSE is no IF's), and the two words just before (empty after
    // anything else)
    let mut code = false;
    let mut first = "";
    let mut needs = false;
    let mut cases = 0u32;
    let (mut prev, mut prev2) = ("", "");
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
                (code, first, needs, cases, prev, prev2) = (false, "", false, 0, "", "");
                continue;
            }
            TokenKind::Word if gos.contains(&i) => {
                if let Some(s) = trim_span(src, start, t.start) {
                    spans.push(s);
                }
                let line_end = src[t.start..].find('\n').map_or(src.len(), |n| t.start + n);
                start = line_end;
                skip_until = line_end;
                depth = 0;
                (code, first, needs, cases, prev, prev2) = (false, "", false, 0, "", "");
                continue;
            }
            TokenKind::Word if depth == 0 => {
                let word = t.text;
                let statement = one_of(word, STATEMENT_STARTS);
                let blank_before = i > 0
                    && tokens[i - 1].kind == TokenKind::Whitespace
                    && tokens[i - 1].text.matches('\n').count() >= 2;
                let goes_on = needs
                    || one_of(prev, CONTINUED_AFTER)
                    || (one_of(prev, &["ALL", "DISTINCT"])
                        && one_of(prev2, &["UNION", "EXCEPT", "INTERSECT", "MINUS"]));
                if blank_before && statement && code && !goes_on {
                    if let Some(s) = trim_span(src, start, t.start) {
                        spans.push(s);
                    }
                    start = t.start;
                    (first, cases) = ("", 0);
                }
                // UPDATE(col) and REPLACE(...) are functions
                let call = one_of(word, &["UPDATE", "REPLACE"])
                    && tokens[i + 1..]
                        .iter()
                        .find(|n| !n.is_trivia())
                        .is_some_and(|n| n.kind == TokenKind::LParen);
                if first.is_empty() {
                    first = word;
                    needs = one_of(word, NEED_MORE);
                } else if needs && (statement || word.eq_ignore_ascii_case("VALUES")) && !call {
                    // the main statement of a CTE, the body of IF, the rows of INSERT
                    needs = one_of(word, NEED_MORE);
                } else if word.eq_ignore_ascii_case("ELSE") && cases == 0 {
                    needs = true;
                }
                if word.eq_ignore_ascii_case("CASE") && !prev.eq_ignore_ascii_case("END") {
                    cases += 1;
                } else if word.eq_ignore_ascii_case("END") && cases > 0 {
                    cases -= 1;
                }
            }
            _ => {}
        }
        if !t.is_trivia() {
            code = true;
            prev2 = prev;
            prev = if t.kind == TokenKind::Word { t.text } else { "" };
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

/// Kinds of object whose definition carries a body that the server stores
/// rather than runs.
const MODULE_KINDS: &[&str] = &["PROCEDURE", "PROC", "FUNCTION", "TRIGGER", "VIEW", "EVENT", "RULE"];

/// Whether the significant tokens `t` start a routine, trigger or view
/// definition: CREATE or ALTER, modifiers such as `OR ALTER`, `OR REPLACE`,
/// `DEFINER = u@h`, `ALGORITHM = x`, `SQL SECURITY x`, `CONSTRAINT` or
/// `TEMP`, then the kind of object (so `ALTER TABLE t DISABLE TRIGGER` is not
/// one).
pub(crate) fn defines_module(t: &[Token<'_>]) -> bool {
    let word = |i: usize| {
        t.get(i)
            .filter(|x| x.kind == TokenKind::Word)
            .map(Token::upper)
            .unwrap_or_default()
    };
    if !matches!(word(0).as_str(), "CREATE" | "ALTER") {
        return false;
    }
    let reach = t.len().min(32);
    let mut i = 1;
    while i < reach {
        match word(i).as_str() {
            k if MODULE_KINDS.contains(&k) => return true,
            "OR" if matches!(word(i + 1).as_str(), "ALTER" | "REPLACE") => i += 2,
            "ALGORITHM" => i += 3,
            "SQL" if word(i + 1) == "SECURITY" => i += 3,
            "AGGREGATE" | "CONSTRAINT" | "TEMP" | "TEMPORARY" | "RECURSIVE" | "MATERIALIZED" => i += 1,
            // DEFINER = user@host or CURRENT_USER: up to the next keyword
            "DEFINER" => {
                i += 1;
                while i < reach
                    && !matches!(word(i).as_str(), k if MODULE_KINDS.contains(&k) || k == "ALGORITHM" || k == "SQL")
                {
                    i += 1;
                }
            }
            _ => return false,
        }
    }
    false
}

/// BEGIN ... END nesting in procedural code. CASE ... END expressions and
/// MySQL's END IF / LOOP / WHILE / REPEAT / CASE close no block; T-SQL BEGIN
/// TRAN / DIALOG / CONVERSATION, MySQL BEGIN WORK and PostgreSQL BEGIN (a
/// transaction, unlike BEGIN ATOMIC) open none.
#[derive(Debug, Default)]
pub(crate) struct Blocks {
    pub(crate) depth: i32,
    cases: i32,
}

impl Blocks {
    /// Take in the significant token `t[k]`.
    pub(crate) fn read(&mut self, dialect: Dialect, t: &[Token<'_>], k: usize) {
        if t[k].kind != TokenKind::Word {
            return;
        }
        let key = |i: Option<usize>| {
            i.and_then(|i| t.get(i)).map_or(String::new(), |x| {
                if x.kind == TokenKind::Word {
                    x.upper()
                } else {
                    x.text.to_string()
                }
            })
        };
        let (prev, next) = (key(k.checked_sub(1)), key(Some(k + 1)));
        match t[k].upper().as_str() {
            "CASE" if prev != "END" => self.cases += 1,
            "BEGIN" if Self::opens(dialect, &next) => self.depth += 1,
            "END" if dialect == Dialect::MySql && matches!(next.as_str(), "IF" | "LOOP" | "WHILE" | "REPEAT") => {}
            "END" if self.cases > 0 => self.cases -= 1,
            "END" if next != "CONVERSATION" => self.depth = (self.depth - 1).max(0),
            _ => {}
        }
    }

    fn opens(dialect: Dialect, next: &str) -> bool {
        match dialect {
            Dialect::Postgres => next == "ATOMIC",
            Dialect::MsSql => !matches!(
                next,
                "TRAN" | "TRANSACTION" | "DISTRIBUTED" | "DIALOG" | "CONVERSATION" | ";"
            ),
            Dialect::MySql => !matches!(next, "WORK" | ";"),
        }
    }
}

/// The tokens of `tokens` (in source order, with or without trivia) that lie
/// inside `span`, found by binary search: callers going through every
/// statement or batch of a long script stay linear.
pub(crate) fn tokens_in<'t, 'a>(tokens: &'t [Token<'a>], span: &Span) -> &'t [Token<'a>] {
    let from = tokens.partition_point(|t| t.start < span.start);
    let to = tokens.partition_point(|t| t.end() <= span.end).max(from);
    &tokens[from..to]
}

/// Token ranges of routine, trigger and view definitions in the significant
/// tokens `t`, given its batches and its statement starts (sorted). A T-SQL
/// definition must start its batch and takes the rest of it; elsewhere a
/// definition ends with the END of its BEGIN ... END body, or at its `;`
/// when it has none.
pub(crate) fn definition_ranges(
    t: &[Token<'_>],
    batches: &[Range<usize>],
    starts: &[usize],
    dialect: Dialect,
) -> Vec<Range<usize>> {
    let mut out: Vec<Range<usize>> = Vec::new();
    for b in batches {
        let first = starts.partition_point(|&s| s < b.start);
        let in_batch = &starts[first..starts.partition_point(|&s| s < b.end)];
        for (k, &s) in in_batch.iter().enumerate() {
            if out.last().is_some_and(|r| s < r.end) {
                continue;
            }
            if defines_module(&t[s..b.end]) {
                // never shorter than its statement: between two MySQL
                // DELIMITER lines the whole piece is one, `;` or not
                let statement_end = in_batch.get(k + 1).copied().unwrap_or(b.end);
                let end = match dialect {
                    Dialect::MsSql => b.end,
                    _ => (s + body_end(&t[s..b.end], dialect)).max(statement_end),
                };
                out.push(s..end);
            }
            if dialect == Dialect::MsSql {
                break;
            }
        }
    }
    out
}

/// Where the definition that starts `t` ends (exclusive): after the END
/// that closes its body, or after its `;` when it has no BEGIN ... END.
fn body_end(t: &[Token<'_>], dialect: Dialect) -> usize {
    let mut blocks = Blocks::default();
    let mut parens = 0i32;
    for k in 0..t.len() {
        match t[k].kind {
            TokenKind::LParen => parens += 1,
            TokenKind::RParen => parens -= 1,
            TokenKind::Semicolon if blocks.depth == 0 && parens <= 0 => return k + 1,
            TokenKind::Word => {
                let open = blocks.depth > 0;
                blocks.read(dialect, t, k);
                if open && blocks.depth == 0 {
                    return k + 1;
                }
            }
            _ => {}
        }
    }
    t.len()
}

/// What "execute current statement" runs: [`statement_at`], widened to the
/// whole T-SQL batch for CREATE / ALTER PROCEDURE, FUNCTION, TRIGGER and
/// VIEW (the rest of the batch is their body), to the whole MySQL routine,
/// trigger or event (a statement of its body would run on its own), and to
/// the whole block for a statement inside BEGIN ... END, TRY / CATCH,
/// IF / ELSE or WHILE (PostgreSQL: BEGIN ATOMIC ... END).
pub fn executable_statement_at(src: &str, dialect: Dialect, offset: usize) -> Option<Span> {
    let span = statement_at(src, dialect, offset)?;
    if dialect == Dialect::MySql {
        return Some(mysql_definition_around(src, span).unwrap_or(span));
    }
    let tokens: Vec<Token<'_>> = tokenize(src, dialect).into_iter().filter(|t| !t.is_trivia()).collect();
    let batch = split_batches(src, dialect)
        .into_iter()
        .map(|b| b.span)
        .find(|b| b.start <= span.start && span.end <= b.end)
        .unwrap_or(Span {
            start: 0,
            end: src.len(),
        });
    let statements: Vec<Span> = split_statements(src, dialect)
        .into_iter()
        .filter(|s| batch.start <= s.start && s.end <= batch.end)
        .collect();
    // a module definition takes the rest of its batch
    if dialect == Dialect::MsSql
        && statements
            .first()
            .is_some_and(|s| defines_module(tokens_in(&tokens, s)))
    {
        return Some(batch);
    }
    // statements joined into top-level units: a block stays together, ELSE
    // continues an IF and BEGIN CATCH continues a TRY
    let mut units: Vec<Span> = Vec::new();
    let mut blocks = Blocks::default();
    for s in &statements {
        let t = tokens_in(&tokens, s);
        let word = |i: usize| t.get(i).filter(|x| x.kind == TokenKind::Word).map(Token::upper);
        let continues = word(0).as_deref() == Some("ELSE")
            || (word(0).as_deref() == Some("BEGIN") && word(1).as_deref() == Some("CATCH"));
        match units.last_mut() {
            Some(u) if blocks.depth > 0 || continues => u.end = s.end,
            _ => units.push(*s),
        }
        for k in 0..t.len() {
            blocks.read(dialect, t, k);
        }
    }
    units
        .into_iter()
        .find(|u| u.start <= span.start && span.end <= u.end)
        .or(Some(span))
}

/// The MySQL routine, trigger or event definition that statement `span`
/// belongs to: from the start of the statement holding its first token to
/// the end of the statement holding its last (comments around it included).
fn mysql_definition_around(src: &str, span: Span) -> Option<Span> {
    let dialect = Dialect::MySql;
    let t = significant(&tokenize(src, dialect));
    let index = |offset: usize| t.partition_point(|x| x.start < offset);
    let statements = split_statements(src, dialect);
    let starts: Vec<usize> = statements.iter().map(|s| index(s.start)).collect();
    let batches: Vec<Range<usize>> = split_batches(src, dialect)
        .iter()
        .map(|b| index(b.span.start)..index(b.span.end))
        .collect();
    let at = index(span.start);
    let def = definition_ranges(&t, &batches, &starts, dialect)
        .into_iter()
        .find(|r| r.start <= at && at < r.end)?;
    let statement_of = |token: &Token<'_>| {
        statements
            .iter()
            .find(|s| s.start <= token.start && token.start < s.end)
    };
    // the `;` after END belongs to the definition
    let last = match t.get(def.end) {
        Some(x) if x.kind == TokenKind::Semicolon => x,
        _ => &t[def.end - 1],
    };
    let first = &t[def.start];
    Some(Span {
        start: statement_of(first).map_or(first.start, |s| s.start.min(first.start)),
        end: statement_of(last).map_or(last.end(), |s| s.end.max(last.end())),
    })
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

    #[test]
    fn execute_current_statement_takes_whole_blocks() {
        let src = "CREATE PROCEDURE dbo.p AS\nBEGIN\n    SELECT 1;\n    SELECT 2;\nEND\nGO\n\
                   BEGIN TRY\n    SELECT 1/0;\nEND TRY\nBEGIN CATCH\n    SELECT ERROR_MESSAGE();\nEND CATCH\nGO\n\
                   IF 1 = 1\nBEGIN\n    PRINT 'a';\n    PRINT CASE WHEN 1 = 1 THEN 'b' END;\nEND\nELSE\n    PRINT 'c';\n\
                   SELECT 3;\nGO\nCREATE OR ALTER PROC dbo.q AS SELECT 1; SELECT 2;\nGO\nBEGIN TRAN;\nUPDATE t SET a = 1;";
        let run = |needle: &str| {
            let at = src.find(needle).unwrap() + 1;
            executable_statement_at(src, Dialect::MsSql, at)
                .unwrap()
                .text(src)
                .to_string()
        };
        let proc = "CREATE PROCEDURE dbo.p AS\nBEGIN\n    SELECT 1;\n    SELECT 2;\nEND";
        assert_eq!(run("CREATE PROCEDURE"), proc);
        assert_eq!(run("SELECT 2"), proc);
        let try_catch = "BEGIN TRY\n    SELECT 1/0;\nEND TRY\nBEGIN CATCH\n    SELECT ERROR_MESSAGE();\nEND CATCH";
        assert_eq!(run("SELECT 1/0"), try_catch);
        assert_eq!(run("ERROR_MESSAGE"), try_catch);
        let if_else =
            "IF 1 = 1\nBEGIN\n    PRINT 'a';\n    PRINT CASE WHEN 1 = 1 THEN 'b' END;\nEND\nELSE\n    PRINT 'c';";
        assert_eq!(run("PRINT 'a'"), if_else);
        assert_eq!(run("PRINT 'c'"), if_else);
        assert_eq!(run("SELECT 3"), "SELECT 3;");
        // a module without BEGIN ... END: the rest of the batch is its body
        assert_eq!(
            run("CREATE OR ALTER PROC"),
            "CREATE OR ALTER PROC dbo.q AS SELECT 1; SELECT 2;"
        );
        // BEGIN TRAN is not a block
        assert_eq!(run("UPDATE t"), "UPDATE t SET a = 1;");
        // PostgreSQL: BEGIN ATOMIC bodies, while BEGIN; is a transaction
        let pg = "BEGIN;\nCREATE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; SELECT 2; END;\nCOMMIT;";
        let at = pg.find("SELECT 2").unwrap();
        assert_eq!(
            executable_statement_at(pg, Dialect::Postgres, at).unwrap().text(pg),
            "CREATE FUNCTION f() RETURNS int LANGUAGE sql BEGIN ATOMIC SELECT 1; SELECT 2; END;"
        );
        assert_eq!(
            executable_statement_at(pg, Dialect::Postgres, 2).unwrap().text(pg),
            "BEGIN;"
        );
    }

    fn sig(src: &str, d: Dialect) -> Vec<Token<'_>> {
        tokenize(src, d).into_iter().filter(|t| !t.is_trivia()).collect()
    }

    #[test]
    fn blank_lines_inside_a_statement_do_not_cut_it() {
        let ms = Dialect::MsSql;
        let run = |src: &str, needle: &str, d: Dialect| {
            let at = src.find(needle).unwrap() + 1;
            executable_statement_at(src, d, at).unwrap().text(src).to_string()
        };
        // the DELETE alone would empty dbo.Orders, not the rows the CTE picks
        let cte =
            "WITH Orders AS (\n    SELECT * FROM dbo.Orders WHERE Year < 2020\n)\n\nDELETE FROM Orders\n\nSELECT 1";
        for d in [ms, Dialect::Postgres, Dialect::MySql] {
            assert_eq!(
                run(cte, "DELETE", d),
                "WITH Orders AS (\n    SELECT * FROM dbo.Orders WHERE Year < 2020\n)\n\nDELETE FROM Orders",
                "{d:?}"
            );
            assert_eq!(run(cte, "SELECT 1", d), "SELECT 1");
        }
        // ... nor would a body without its IF, WHILE or ELSE
        let guarded = "IF EXISTS (SELECT 1 FROM dbo.Flags)\n\n    UPDATE dbo.Orders SET a = 1\n\nELSE\n\n    DELETE FROM dbo.Orders\n\nPRINT 'done'";
        let whole = "IF EXISTS (SELECT 1 FROM dbo.Flags)\n\n    UPDATE dbo.Orders SET a = 1\n\nELSE\n\n    DELETE FROM dbo.Orders";
        assert_eq!(run(guarded, "UPDATE", ms), whole);
        assert_eq!(run(guarded, "DELETE", ms), whole);
        assert_eq!(run(guarded, "PRINT", ms), "PRINT 'done'");
        let looped = "WHILE @@ROWCOUNT > 0\n\n    DELETE TOP (100) FROM dbo.Logs";
        assert_eq!(run(looped, "DELETE", ms), looped);
        let block = "IF @x = 1\n\nBEGIN\n    DELETE FROM dbo.Orders WHERE Id = 1;\nEND";
        assert_eq!(run(block, "DELETE", ms), block);
        // INSERT and its rows, set operators, CREATE ... AS, a cursor's query
        let insert = "INSERT INTO dbo.Archive (Id)\n\nSELECT Id FROM dbo.Orders";
        assert_eq!(run(insert, "SELECT", ms), insert);
        let union = "SELECT 1\n\nUNION ALL\n\nSELECT 2";
        assert_eq!(run(union, "SELECT 2", ms), union);
        let ctas = "CREATE TABLE archive AS\n\nSELECT * FROM orders";
        assert_eq!(run(ctas, "SELECT", Dialect::Postgres), ctas);
        let cursor = "DECLARE c CURSOR FOR\n\nSELECT Id FROM dbo.Orders";
        assert_eq!(run(cursor, "SELECT", ms), cursor);
        // whole statements still split at a blank line, a CASE's ELSE included
        let two = "SELECT CASE WHEN 1 = 1 THEN 'a' ELSE 'b' END\n\nSELECT 2\n\nINSERT INTO t VALUES (1)\n\nSELECT 3";
        assert_eq!(split_statements(two, ms).len(), 4);
        assert_eq!(split_statements("(SELECT 1)\n\nSELECT 2", ms).len(), 2);
        // ALL ends this statement: only after UNION does it go on
        assert_eq!(
            split_statements("UPDATE STATISTICS dbo.t WITH FULLSCAN, ALL\n\nSELECT 1", ms).len(),
            2
        );
    }

    #[test]
    fn execute_current_statement_takes_whole_mysql_routines() {
        let my = Dialect::MySql;
        let src = "SELECT 1;\nCREATE PROCEDURE clear_staging()\nBEGIN\n  DELETE FROM staging;\n  IF 1 THEN\n    INSERT INTO log VALUES (1);\n  END IF;\nEND;\nSELECT 2;";
        let proc = "CREATE PROCEDURE clear_staging()\nBEGIN\n  DELETE FROM staging;\n  IF 1 THEN\n    INSERT INTO log VALUES (1);\n  END IF;\nEND;";
        let run = |needle: &str| {
            let at = src.find(needle).unwrap() + 1;
            executable_statement_at(src, my, at).unwrap().text(src).to_string()
        };
        // a statement of the body never runs on its own
        for needle in ["CREATE", "DELETE", "INSERT", "END IF", "END;\nSELECT"] {
            assert_eq!(run(needle), proc, "{needle}");
        }
        assert_eq!(run("SELECT 1"), "SELECT 1;");
        assert_eq!(run("SELECT 2"), "SELECT 2;");
        let trigger = "CREATE TRIGGER t BEFORE DELETE ON orders FOR EACH ROW BEGIN DELETE FROM audit; END";
        let at = trigger.find("FROM audit").unwrap();
        assert_eq!(executable_statement_at(trigger, my, at).unwrap().text(trigger), trigger);
        // with DELIMITER the client already keeps the routine whole
        let delimited = "DELIMITER //\nCREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2; END //\nDELIMITER ;";
        let at = delimited.find("SELECT 2").unwrap();
        assert_eq!(
            executable_statement_at(delimited, my, at).unwrap().text(delimited),
            "CREATE PROCEDURE p() BEGIN SELECT 1; SELECT 2; END"
        );
        // ... also with no BEGIN, where `;` ends the routine's single statement
        let piece = "DELIMITER //\nCREATE PROCEDURE p() SELECT 1; SELECT 2 //\nDELIMITER ;";
        let at = piece.find("SELECT 1").unwrap();
        assert_eq!(
            executable_statement_at(piece, my, at).unwrap().text(piece),
            "CREATE PROCEDURE p() SELECT 1; SELECT 2"
        );
        // a comment ending the routine's last statement stays with it, as it does
        // with the statement under the cursor
        let open = "CREATE PROCEDURE p()\nBEGIN\n  SELECT 1;\n  SELECT 2 # last";
        let at = open.find("SELECT 2").unwrap();
        assert_eq!(executable_statement_at(open, my, at).unwrap().text(open), open);
    }

    #[test]
    fn module_definitions_are_recognised() {
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        for (src, d) in [
            ("CREATE PROCEDURE dbo.p AS SELECT 1", ms),
            ("create or alter proc p as select 1", ms),
            ("ALTER TRIGGER dbo.t ON dbo.x AFTER INSERT AS SELECT 1", ms),
            (
                "CREATE OR REPLACE FUNCTION f() RETURNS int AS $$ SELECT 1 $$ LANGUAGE sql",
                pg,
            ),
            (
                "CREATE CONSTRAINT TRIGGER t AFTER INSERT ON x FOR EACH ROW EXECUTE FUNCTION f()",
                pg,
            ),
            ("CREATE OR REPLACE TEMP VIEW v AS SELECT 1", pg),
            ("CREATE DEFINER=`root`@`%` PROCEDURE p() BEGIN SELECT 1; END", my),
            (
                "CREATE DEFINER = CURRENT_USER TRIGGER t BEFORE INSERT ON x FOR EACH ROW SET NEW.a = 1",
                my,
            ),
            ("CREATE ALGORITHM=MERGE SQL SECURITY INVOKER VIEW v AS SELECT 1", my),
            ("CREATE EVENT e ON SCHEDULE EVERY 1 DAY DO DELETE FROM log", my),
        ] {
            assert!(defines_module(&sig(src, d)), "{src}");
        }
        for (src, d) in [
            ("ALTER TABLE dbo.t DISABLE TRIGGER ALL", ms),
            ("CREATE TABLE trigger_log (a int)", ms),
            ("CREATE UNIQUE INDEX ix ON t (a)", ms),
            ("DROP PROCEDURE dbo.p", ms),
            ("CREATE TEMPORARY TABLE w (a int)", my),
            ("SELECT 'CREATE PROCEDURE'", pg),
        ] {
            assert!(!defines_module(&sig(src, d)), "{src}");
        }
    }

    #[test]
    fn blocks_follow_begin_and_end() {
        let depth_after = |src: &str, d: Dialect| {
            let t = sig(src, d);
            let mut b = Blocks::default();
            (0..t.len()).for_each(|k| b.read(d, &t, k));
            b.depth
        };
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        assert_eq!(depth_after("BEGIN SELECT CASE WHEN 1 = 1 THEN 2 END", ms), 1);
        assert_eq!(depth_after("BEGIN TRY SELECT 1 END TRY BEGIN CATCH", ms), 1);
        assert_eq!(depth_after("BEGIN TRAN; BEGIN DIALOG @h", ms), 0);
        assert_eq!(depth_after("BEGIN END CONVERSATION @h", ms), 1);
        // MySQL: END IF / END CASE close no block
        assert_eq!(
            depth_after(
                "BEGIN IF a THEN CASE b WHEN 1 THEN SET c = 1; END CASE; END IF; WHILE x DO SET y = 1; END WHILE;",
                my
            ),
            1
        );
        assert_eq!(depth_after("BEGIN WORK;", my), 0);
        assert_eq!(depth_after("BEGIN; SELECT 1;", pg), 0);
        assert_eq!(depth_after("BEGIN ATOMIC SELECT 1;", pg), 1);
    }
}
