//! Token-stream SQL formatter.
//!
//! The formatter never parses into an AST: it walks the significant tokens,
//! keeps a small stack of layout blocks (statement, subquery, column list,
//! BEGIN/END) and decides where line breaks go. That keeps it tolerant of
//! half-written or dialect-specific SQL. As a safety net the result is
//! re-tokenized and compared with the input; if anything other than
//! whitespace and keyword case changed, the original text is returned.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::keywords;
use crate::lexer::{Token, TokenKind, tokenize};
use crate::statements::{delimiter_regions, go_lines, region_pieces, split_batches};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KeywordCase {
    Upper,
    Lower,
    Preserve,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ListLayout {
    /// `SELECT a,` / `       b` — items aligned after the clause keyword.
    Aligned,
    /// Clause keyword on its own line, items indented below it.
    Stacked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CommaStyle {
    Trailing,
    Leading,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FormatOptions {
    pub keyword_case: KeywordCase,
    pub indent_size: usize,
    pub list_layout: ListLayout,
    pub comma_style: CommaStyle,
    /// Indent JOIN clauses one level under FROM.
    pub indent_joins: bool,
    /// Put `ON` of a join on its own line.
    pub on_new_line: bool,
    /// Break before AND/OR in WHERE, HAVING and ON.
    pub and_or_new_line: bool,
    pub blank_line_between_statements: bool,
}

impl Default for FormatOptions {
    fn default() -> Self {
        Self {
            keyword_case: KeywordCase::Upper,
            indent_size: 4,
            list_layout: ListLayout::Aligned,
            comma_style: CommaStyle::Trailing,
            indent_joins: true,
            on_new_line: true,
            and_or_new_line: true,
            blank_line_between_statements: true,
        }
    }
}

/// Format `src`. Returns the input unchanged if formatting would alter
/// anything but whitespace and keyword case, or the script's batches.
///
/// Client lines (`GO [n]`, MySQL `DELIMITER x`) are not SQL: they keep their
/// own line, and the SQL between them is formatted on its own, each
/// statement of a `DELIMITER` region followed by its terminator. Line breaks
/// follow the document (CRLF stays CRLF).
pub fn format_sql(src: &str, dialect: Dialect, opts: &FormatOptions) -> String {
    let eol = if src.contains("\r\n") { "\r\n" } else { "\n" };
    let blank = if opts.blank_line_between_statements {
        format!("{eol}{eol}")
    } else {
        eol.to_string()
    };
    let mut out = String::new();
    let mut prev: Option<&Part<'_>> = None;
    let parts = script_parts(src, dialect);
    for part in &parts {
        let text = match part {
            Part::Sql { text, terminator } => {
                let code = format_code(text, dialect, opts, eol);
                if code.is_empty() {
                    continue;
                }
                match terminator {
                    // a comment ending the code would swallow the terminator
                    Some((t, _)) if ends_in_line_comment(&code, dialect) => format!("{code}{eol}{t}"),
                    // `end$$` is one word to MySQL: keep the terminator where it was
                    Some((t, true)) => format!("{code}{t}"),
                    Some((t, false)) => format!("{code} {t}"),
                    None => code,
                }
            }
            Part::Line(line) => {
                let (word, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
                let word = match opts.keyword_case {
                    KeywordCase::Upper => word.to_ascii_uppercase(),
                    KeywordCase::Lower => word.to_ascii_lowercase(),
                    KeywordCase::Preserve => word.to_string(),
                };
                match rest.trim() {
                    "" => word,
                    rest => format!("{word} {rest}"),
                }
            }
        };
        match (prev, part) {
            (None, _) => {}
            // after GO the next batch starts like a new statement
            (Some(Part::Line(l)), Part::Sql { .. }) if is_go_line(l) => out.push_str(&blank),
            (Some(Part::Sql { .. }), Part::Sql { .. }) => out.push_str(&blank),
            _ => out.push_str(eol),
        }
        out.push_str(&text);
        prev = Some(part);
    }
    if src.ends_with('\n') {
        out.push_str(eol);
    }
    if same_tokens(src, &out, dialect) && same_batches(src, &out, dialect) {
        out
    } else {
        src.to_string()
    }
}

/// A stretch of a script for [`format_sql`].
enum Part<'a> {
    /// SQL to format, and the custom terminator that followed it (right
    /// after the code, or after a space).
    Sql {
        text: &'a str,
        terminator: Option<(&'a str, bool)>,
    },
    /// A client line kept as it is: `GO [n]` or `DELIMITER x`.
    Line(&'a str),
}

fn script_parts(src: &str, dialect: Dialect) -> Vec<Part<'_>> {
    let sql = |text| Part::Sql { text, terminator: None };
    let tokens = tokenize(src, dialect);
    let mut parts = Vec::new();
    match dialect {
        Dialect::MsSql => {
            let mut from = 0;
            for (i, _, line_end) in go_lines(&tokens) {
                parts.push(sql(&src[from..tokens[i].start]));
                parts.push(Part::Line(src[tokens[i].start..line_end].trim()));
                from = line_end;
            }
            parts.push(sql(&src[from..]));
        }
        Dialect::MySql => match delimiter_regions(src, &tokens) {
            None => parts.push(sql(src)),
            Some(regions) => {
                for (k, r) in regions.iter().enumerate() {
                    if k > 0 {
                        // the DELIMITER line that opened this region
                        parts.push(Part::Line(src[regions[k - 1].end..r.start].trim()));
                    }
                    if r.delimiter == ";" {
                        parts.push(sql(&src[r.start..r.end]));
                        continue;
                    }
                    for span in region_pieces(src, &tokens, r) {
                        let after = &src[span.end..r.end];
                        let ended = after.trim_start().starts_with(r.delimiter);
                        parts.push(Part::Sql {
                            text: span.text(src),
                            terminator: ended.then_some((r.delimiter, after.starts_with(r.delimiter))),
                        });
                    }
                }
            }
        },
        Dialect::Postgres => parts.push(sql(src)),
    }
    parts
}

fn is_go_line(line: &str) -> bool {
    line.split_whitespace()
        .next()
        .is_some_and(|w| w.eq_ignore_ascii_case("GO"))
}

fn ends_in_line_comment(code: &str, dialect: Dialect) -> bool {
    tokenize(code, dialect)
        .iter()
        .rfind(|t| t.kind != TokenKind::Whitespace)
        .is_some_and(|t| t.kind == TokenKind::LineComment)
}

/// The same batches (count, repeat counts and code) in both scripts.
fn same_batches(a: &str, b: &str, dialect: Dialect) -> bool {
    let (x, y) = (split_batches(a, dialect), split_batches(b, dialect));
    x.len() == y.len()
        && x.iter()
            .zip(&y)
            .all(|(p, q)| p.repeat == q.repeat && same_tokens(p.span.text(a), q.span.text(b), dialect))
}

/// Format SQL without client lines; the result has no leading or trailing
/// whitespace.
fn format_code(src: &str, dialect: Dialect, opts: &FormatOptions, eol: &'static str) -> String {
    let tokens = tokenize(src, dialect);
    let items = build_items(&tokens);
    let mut f = Formatter::new(&items, dialect, opts, eol);
    f.run();
    f.out.buf.trim().to_string()
}

/// True when both texts have the same significant tokens, comparing words
/// case-insensitively and comments without trailing whitespace.
pub fn same_tokens(a: &str, b: &str, dialect: Dialect) -> bool {
    fn sig(src: &str, d: Dialect) -> Vec<Token<'_>> {
        tokenize(src, d)
            .into_iter()
            .filter(|t| t.kind != TokenKind::Whitespace)
            .collect()
    }
    let (ta, tb) = (sig(a, dialect), sig(b, dialect));
    ta.len() == tb.len()
        && ta.iter().zip(&tb).all(|(x, y)| {
            x.kind == y.kind
                && match x.kind {
                    TokenKind::Word => x.text.eq_ignore_ascii_case(y.text),
                    TokenKind::LineComment | TokenKind::BlockComment => x.text.trim_end() == y.text.trim_end(),
                    _ => x.text == y.text,
                }
        })
}

// ---------------------------------------------------------------------------
// Items: significant tokens with multi-word keywords merged
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Item<'a> {
    kind: TokenKind,
    text: Cow<'a, str>,
    upper: String,
    newlines_before: usize,
    merged: bool,
}

const MULTI_WORD: &[&[&str]] = &[
    &["WHEN", "NOT", "MATCHED", "BY", "SOURCE"],
    &["WHEN", "NOT", "MATCHED", "BY", "TARGET"],
    &["BEGIN", "DISTRIBUTED", "TRANSACTION"],
    &["ON", "DUPLICATE", "KEY", "UPDATE"],
    &["WHEN", "NOT", "MATCHED"],
    &["LEFT", "OUTER", "JOIN"],
    &["RIGHT", "OUTER", "JOIN"],
    &["FULL", "OUTER", "JOIN"],
    &["CREATE", "OR", "ALTER"],
    &["CREATE", "OR", "REPLACE"],
    &["WHEN", "MATCHED"],
    &["LEFT", "JOIN"],
    &["RIGHT", "JOIN"],
    &["FULL", "JOIN"],
    &["INNER", "JOIN"],
    &["CROSS", "JOIN"],
    &["NATURAL", "JOIN"],
    &["CROSS", "APPLY"],
    &["OUTER", "APPLY"],
    &["GROUP", "BY"],
    &["ORDER", "BY"],
    &["PARTITION", "BY"],
    &["UNION", "ALL"],
    &["UNION", "DISTINCT"],
    &["EXCEPT", "ALL"],
    &["INTERSECT", "ALL"],
    &["INSERT", "INTO"],
    &["DELETE", "FROM"],
    &["MERGE", "INTO"],
    &["ON", "CONFLICT"],
    &["BEGIN", "TRY"],
    &["END", "TRY"],
    &["BEGIN", "CATCH"],
    &["END", "CATCH"],
    &["BEGIN", "TRANSACTION"],
    &["BEGIN", "TRAN"],
    &["COMMIT", "TRANSACTION"],
    &["COMMIT", "TRAN"],
    &["ROLLBACK", "TRANSACTION"],
    &["ROLLBACK", "TRAN"],
    &["END", "IF"],
    &["END", "LOOP"],
    &["END", "WHILE"],
];

fn build_items<'a>(tokens: &[Token<'a>]) -> Vec<Item<'a>> {
    let mut raw: Vec<Item<'a>> = Vec::new();
    let mut newlines = 0usize;
    for t in tokens {
        if t.kind == TokenKind::Whitespace {
            newlines += t.text.matches('\n').count();
            continue;
        }
        raw.push(Item {
            kind: t.kind,
            text: Cow::Borrowed(t.text),
            upper: if t.kind == TokenKind::Word {
                t.upper()
            } else {
                String::new()
            },
            newlines_before: newlines,
            merged: false,
        });
        newlines = 0;
    }

    let mut out = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        let seq = MULTI_WORD.iter().find(|seq| {
            seq.iter().enumerate().all(|(k, w)| {
                raw.get(i + k)
                    .is_some_and(|it| it.kind == TokenKind::Word && it.upper == *w)
            })
        });
        match seq {
            Some(seq) => {
                let parts = &raw[i..i + seq.len()];
                out.push(Item {
                    kind: TokenKind::Word,
                    text: Cow::Owned(parts.iter().map(|p| p.text.as_ref()).collect::<Vec<_>>().join(" ")),
                    upper: seq.join(" "),
                    newlines_before: parts[0].newlines_before,
                    merged: true,
                });
                i += seq.len();
            }
            None => {
                out.push(raw[i].clone());
                i += 1;
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Output buffer with deferred newlines
// ---------------------------------------------------------------------------

struct Out {
    buf: String,
    col: usize,
    line_indent: usize,
    at_line_start: bool,
    pending: Option<(usize, bool)>,
    /// The document's line break.
    eol: &'static str,
}

impl Out {
    fn new(eol: &'static str) -> Self {
        Self {
            buf: String::new(),
            col: 0,
            line_indent: 0,
            at_line_start: true,
            pending: None,
            eol,
        }
    }

    /// Ask for a line break before the next write. Later requests replace
    /// the indent of earlier ones; a blank-line request is sticky.
    fn newline(&mut self, indent: usize, blank: bool) {
        if self.buf.is_empty() {
            return;
        }
        self.pending = Some(match self.pending {
            Some((_, b)) => (indent, b || blank),
            None => (indent, blank),
        });
    }

    fn flush(&mut self) {
        if let Some((indent, blank)) = self.pending.take() {
            let trimmed = self.buf.trim_end_matches([' ', '\t']).len();
            self.buf.truncate(trimmed);
            self.buf.push_str(self.eol);
            if blank {
                self.buf.push_str(self.eol);
            }
            self.col = indent;
            self.line_indent = indent;
            self.at_line_start = true;
        }
    }

    /// Write `text`, returning the column it starts at.
    fn write(&mut self, text: &str, space: bool) -> usize {
        self.flush();
        if self.at_line_start {
            if !self.buf.is_empty() || self.col > 0 {
                self.buf.extend(std::iter::repeat_n(' ', self.line_indent));
            }
            self.col = self.line_indent;
        } else if space {
            self.buf.push(' ');
            self.col += 1;
        }
        self.at_line_start = false;
        let start = self.col;
        self.push_text(text);
        start
    }

    /// Write a trailing comment on the current line without consuming a
    /// pending line break.
    fn write_trailing(&mut self, text: &str) {
        if !self.buf.is_empty() && !self.buf.ends_with([' ', '\n']) {
            self.buf.push(' ');
            self.col += 1;
        }
        self.at_line_start = false;
        self.push_text(text);
    }

    fn push_text(&mut self, text: &str) {
        self.buf.push_str(text);
        match text.rfind('\n') {
            Some(nl) => self.col = text[nl + 1..].chars().count(),
            None => self.col += text.chars().count(),
        }
    }
}

// ---------------------------------------------------------------------------
// Layout state
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clause {
    None,
    Select,
    Into,
    From,
    Join,
    On,
    Where,
    GroupBy,
    Having,
    OrderBy,
    Set,
    Values,
    Update,
    Delete,
    Insert,
    SetOp,
    With,
    Returning,
    Output,
    Limit,
    Declare,
    Params,
    Columns,
    Using,
    Other,
}

impl Clause {
    fn is_list(self) -> bool {
        matches!(
            self,
            Clause::Select
                | Clause::From
                | Clause::GroupBy
                | Clause::OrderBy
                | Clause::Set
                | Clause::Values
                | Clause::Returning
                | Clause::Output
                | Clause::Declare
                | Clause::Params
                | Clause::Columns
                | Clause::With
        )
    }

    /// In stacked layout these clauses put their content on the next line.
    fn stacks(self) -> bool {
        matches!(
            self,
            Clause::Select
                | Clause::From
                | Clause::Where
                | Clause::GroupBy
                | Clause::Having
                | Clause::OrderBy
                | Clause::Set
                | Clause::Values
                | Clause::Returning
                | Clause::Output
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Root,
    Subquery { close_col: usize },
    Columns { close_col: usize },
    BeginEnd,
}

/// State of SELECT modifiers (`DISTINCT`, `TOP (n) PERCENT WITH TIES`) that
/// stay on the SELECT line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mods {
    Off,
    Open,
    TopArg,
    TopParen(usize),
    AfterTop,
}

#[derive(Debug, Clone)]
struct Block {
    kind: BlockKind,
    base: usize,
    clause: Clause,
    content_col: Option<usize>,
    inline_depth: usize,
    case_stack: Vec<usize>,
    join_col: usize,
    pending_break: bool,
    mods: Mods,
    between: bool,
}

impl Block {
    fn new(kind: BlockKind, base: usize) -> Self {
        Self {
            kind,
            base,
            clause: Clause::None,
            content_col: None,
            inline_depth: 0,
            case_stack: Vec::new(),
            join_col: base,
            pending_break: false,
            mods: Mods::Off,
            between: false,
        }
    }

    fn statement_level(&self) -> bool {
        matches!(self.kind, BlockKind::Root | BlockKind::BeginEnd)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Paren {
    Inline,
    Block,
}

#[derive(Debug, Default, Clone, Copy)]
struct Stmt {
    create_table: bool,
    columns_opened: bool,
    routine: bool,
    body_started: bool,
    merge: bool,
}

const OBJECT_KINDS: &[&str] = &[
    "TABLE",
    "VIEW",
    "PROC",
    "PROCEDURE",
    "FUNCTION",
    "TRIGGER",
    "INDEX",
    "SCHEMA",
    "DATABASE",
    "TYPE",
    "SEQUENCE",
    "SYNONYM",
    "USER",
    "ROLE",
    "LOGIN",
    "STATISTICS",
    "EXTENSION",
    "MATERIALIZED",
    "TEMPORARY",
    "TEMP",
    "IF",
];

const STATEMENT_WORDS: &[&str] = &[
    "DECLARE",
    "IF",
    "WHILE",
    "RETURN",
    "PRINT",
    "EXEC",
    "EXECUTE",
    "USE",
    "TRUNCATE",
    "RAISERROR",
    "THROW",
    "BREAK",
    "CONTINUE",
    "GOTO",
    "WAITFOR",
    "COMMIT",
    "COMMIT TRAN",
    "COMMIT TRANSACTION",
    "ROLLBACK",
    "ROLLBACK TRAN",
    "ROLLBACK TRANSACTION",
    "BEGIN TRAN",
    "BEGIN TRANSACTION",
    "BEGIN DISTRIBUTED TRANSACTION",
    "OPEN",
    "CLOSE",
    "DEALLOCATE",
    "GRANT",
    "REVOKE",
    "DENY",
    "CALL",
    "SHOW",
    "CREATE",
    "CREATE OR ALTER",
    "CREATE OR REPLACE",
    "ALTER",
    "DROP",
    "EXPLAIN",
    "VACUUM",
    "COPY",
    "DO",
    "RAISE",
    "PERFORM",
    "LOCK",
    "MERGE",
    "MERGE INTO",
    "ELSE",
    "END",
    "END IF",
    "END LOOP",
    "END WHILE",
];

struct Formatter<'a, 'o> {
    items: &'a [Item<'a>],
    dialect: Dialect,
    opts: &'o FormatOptions,
    out: Out,
    blocks: Vec<Block>,
    parens: Vec<Paren>,
    stmt: Stmt,
    prev: Option<usize>,
    no_space: bool,
}

impl<'a, 'o> Formatter<'a, 'o> {
    fn new(items: &'a [Item<'a>], dialect: Dialect, opts: &'o FormatOptions, eol: &'static str) -> Self {
        Self {
            items,
            dialect,
            opts,
            out: Out::new(eol),
            blocks: vec![Block::new(BlockKind::Root, 0)],
            parens: Vec::new(),
            stmt: Stmt::default(),
            prev: None,
            no_space: false,
        }
    }

    fn top(&mut self) -> &mut Block {
        self.blocks.last_mut().expect("root block always present")
    }

    fn top_ref(&self) -> &Block {
        self.blocks.last().expect("root block always present")
    }

    fn indent(&self) -> usize {
        self.opts.indent_size
    }

    fn prev_item(&self) -> Option<&Item<'a>> {
        self.prev.map(|p| &self.items[p])
    }

    fn prev_upper(&self) -> &str {
        self.prev_item().map_or("", |p| p.upper.as_str())
    }

    fn next_sig(&self, i: usize) -> Option<&Item<'a>> {
        self.items[i + 1..]
            .iter()
            .find(|it| !matches!(it.kind, TokenKind::LineComment | TokenKind::BlockComment))
    }

    fn next_upper(&self, i: usize) -> &str {
        self.next_sig(i).map_or("", |n| n.upper.as_str())
    }

    fn run(&mut self) {
        for i in 0..self.items.len() {
            let it = &self.items[i];
            match it.kind {
                TokenKind::LineComment | TokenKind::BlockComment => {
                    self.comment(i);
                    continue;
                }
                TokenKind::Comma => self.comma(i),
                TokenKind::Semicolon => self.semicolon(),
                TokenKind::LParen => self.lparen(i),
                TokenKind::RParen => self.rparen(i),
                TokenKind::Word => self.word(i),
                TokenKind::Variable => self.variable(i),
                TokenKind::Operator => self.operator(i),
                _ => {
                    let text = it.text.clone();
                    self.emit(i, &text);
                }
            }
            self.prev = Some(i);
        }
    }

    // -- spacing ------------------------------------------------------------

    fn space_before(&mut self, i: usize) -> bool {
        if std::mem::take(&mut self.no_space) {
            return false;
        }
        let cur = &self.items[i];
        let Some(prev) = self.prev_item() else {
            return false;
        };
        match cur.kind {
            TokenKind::Comma | TokenKind::Semicolon | TokenKind::RParen | TokenKind::Dot => {
                return false;
            }
            _ => {}
        }
        match prev.kind {
            TokenKind::LParen | TokenKind::Dot => return false,
            _ => {}
        }
        // `label:` (a space there is a syntax error in T-SQL), `a[1:2]`
        if cur.text == "::" || prev.text == "::" || prev.text == "[" || cur.text == "]" || cur.text == ":" {
            return false;
        }
        if cur.text == "[" && (prev.kind == TokenKind::RParen || prev.kind.is_name_like()) {
            return false;
        }
        if cur.kind == TokenKind::LParen {
            return !self.is_call(prev);
        }
        true
    }

    /// Whether a `(` right after `prev` belongs to a function call (no space).
    fn is_call(&self, prev: &Item<'_>) -> bool {
        let clause = self.top_ref().clause;
        match prev.kind {
            TokenKind::Word => {
                let u = prev.upper.as_str();
                if keywords::is_function(u) || keywords::is_data_type(u) || u == "IDENTITY" {
                    return true;
                }
                if prev.merged || keywords::is_reserved(u, self.dialect) {
                    return false;
                }
                !matches!(clause, Clause::Insert | Clause::Into | Clause::With)
            }
            TokenKind::QuotedIdent => !matches!(clause, Clause::Insert | Clause::Into | Clause::With),
            _ => false,
        }
    }

    // -- writing ------------------------------------------------------------

    /// Write a content token, applying pending stacked breaks and recording
    /// the clause's content column. Returns the start column.
    fn emit(&mut self, i: usize, text: &str) -> usize {
        let is_mod = self.check_modifier(i);
        let space = self.space_before(i);
        let indent = self.indent();
        let b = self.top();
        if b.pending_break && !is_mod {
            b.pending_break = false;
            let col = b.base + indent;
            self.out.newline(col, false);
        }
        let start = self.out.write(text, space);
        let b = self.top();
        if !is_mod && b.content_col.is_none() {
            b.content_col = Some(start);
        }
        start
    }

    fn check_modifier(&mut self, i: usize) -> bool {
        let it = &self.items[i];
        let depth = self.parens.len();
        let b = self.blocks.last_mut().expect("root");
        match b.mods {
            Mods::Off => false,
            Mods::Open => match it.upper.as_str() {
                "DISTINCT" | "ALL" => true,
                "TOP" => {
                    b.mods = Mods::TopArg;
                    true
                }
                _ => {
                    b.mods = Mods::Off;
                    false
                }
            },
            Mods::TopArg => match it.kind {
                TokenKind::Number | TokenKind::Variable => {
                    b.mods = Mods::AfterTop;
                    true
                }
                TokenKind::LParen => {
                    b.mods = Mods::TopParen(depth + 1);
                    true
                }
                _ => {
                    b.mods = Mods::Off;
                    false
                }
            },
            Mods::TopParen(n) => {
                if it.kind == TokenKind::RParen && depth == n {
                    b.mods = Mods::AfterTop;
                }
                true
            }
            Mods::AfterTop => {
                if matches!(it.upper.as_str(), "PERCENT" | "WITH" | "TIES") {
                    true
                } else {
                    b.mods = Mods::Off;
                    false
                }
            }
        }
    }

    fn keyword_text(&self, text: &str) -> String {
        match self.opts.keyword_case {
            KeywordCase::Upper => text.to_ascii_uppercase(),
            KeywordCase::Lower => text.to_ascii_lowercase(),
            KeywordCase::Preserve => text.to_string(),
        }
    }

    /// Display text for a word: keywords get the configured case.
    fn word_text(&self, i: usize) -> String {
        let it = &self.items[i];
        if self.is_keyword(i) {
            self.keyword_text(&it.text)
        } else {
            it.text.to_string()
        }
    }

    fn is_keyword(&self, i: usize) -> bool {
        let it = &self.items[i];
        if it.merged {
            return true;
        }
        if it.text.starts_with('#') {
            return false;
        }
        let prev = self.prev_item();
        let next = self.items.get(i + 1);
        if prev.is_some_and(|p| p.kind == TokenKind::Dot) || next.is_some_and(|n| n.kind == TokenKind::Dot) {
            return false;
        }
        let u = it.upper.as_str();
        if keywords::is_reserved(u, self.dialect) {
            return true;
        }
        let next_paren = next.is_some_and(|n| n.kind == TokenKind::LParen);
        if next_paren && (keywords::is_function(u) || keywords::is_data_type(u)) {
            return true;
        }
        if keywords::is_data_type(u) {
            let prev_kind = prev.map(|p| p.kind);
            let prev_u = prev.map_or("", |p| p.upper.as_str());
            if prev_kind == Some(TokenKind::Variable) || matches!(prev_u, "AS" | "RETURNS") {
                return true;
            }
            if u == "MAX" && prev_kind == Some(TokenKind::LParen) {
                return true;
            }
            // column definition: `( name TYPE` or `, name TYPE`
            if self.top_ref().clause == Clause::Columns
                && prev.is_some_and(|p| p.kind.is_name_like())
                && i >= 2
                && matches!(self.items[i - 2].kind, TokenKind::LParen | TokenKind::Comma)
            {
                return true;
            }
        }
        false
    }

    // -- token handlers -----------------------------------------------------

    fn comment(&mut self, i: usize) {
        let it = &self.items[i];
        let is_line = it.kind == TokenKind::LineComment;
        let text = it.text.trim_end().to_string();
        if it.newlines_before == 0 && !self.out.buf.is_empty() {
            self.out.write_trailing(&text);
            if is_line && self.out.pending.is_none() {
                let indent = self.out.line_indent;
                self.out.newline(indent, false);
            }
            return;
        }
        let indent = match self.out.pending {
            Some((ind, _)) => ind,
            None => {
                let b = self.top_ref();
                b.content_col.unwrap_or(b.base)
            }
        };
        let blank = it.newlines_before >= 2 && self.top_ref().statement_level();
        self.out.newline(indent, blank);
        self.out.write(&text, false);
        self.out.newline(indent, false);
    }

    fn comma(&mut self, i: usize) {
        let indent = self.indent();
        let style = self.opts.comma_style;
        let b = self.top_ref();
        let breaks = b.inline_depth == 0 && b.case_stack.is_empty() && b.clause.is_list();
        if !breaks {
            self.emit(i, ",");
            return;
        }
        let col = b.content_col.unwrap_or(b.base + indent);
        match style {
            CommaStyle::Trailing => {
                self.out.write(",", false);
                self.out.newline(col, false);
            }
            CommaStyle::Leading => {
                self.out.newline(col.saturating_sub(2), false);
                self.out.write(", ", false);
                self.no_space = true;
            }
        }
    }

    fn semicolon(&mut self) {
        self.out.write(";", false);
        let blank = self.opts.blank_line_between_statements;
        let b = self.top();
        if b.inline_depth > 0 || !b.statement_level() {
            return;
        }
        b.clause = Clause::None;
        b.content_col = None;
        b.pending_break = false;
        b.mods = Mods::Off;
        b.case_stack.clear();
        let (base, root) = (b.base, b.kind == BlockKind::Root);
        self.stmt = Stmt::default();
        self.out.newline(base, blank && root);
    }

    fn lparen(&mut self, i: usize) {
        let next = self.next_upper(i).to_string();
        let indent = self.indent();
        let prev_upper = self.prev_upper().to_string();
        let b = self.top_ref();
        let columns =
            (self.stmt.create_table && !self.stmt.columns_opened && b.kind == BlockKind::Root && b.inline_depth == 0)
                || prev_upper == "TABLE";
        if columns {
            self.stmt.columns_opened = true;
            self.out.write("(", true);
            let close_col = self.out.line_indent;
            let mut blk = Block::new(BlockKind::Columns { close_col }, close_col + indent);
            blk.clause = Clause::Columns;
            blk.content_col = Some(close_col + indent);
            self.blocks.push(blk);
            self.parens.push(Paren::Block);
            self.out.newline(close_col + indent, false);
            return;
        }
        if matches!(next.as_str(), "SELECT" | "WITH") {
            self.emit(i, "(");
            let close_col = self.out.line_indent;
            self.blocks
                .push(Block::new(BlockKind::Subquery { close_col }, close_col + indent));
            self.parens.push(Paren::Block);
            return;
        }
        self.emit(i, "(");
        self.parens.push(Paren::Inline);
        self.top().inline_depth += 1;
    }

    fn rparen(&mut self, i: usize) {
        match self.parens.last().copied() {
            Some(Paren::Block) => {
                self.parens.pop();
                while let Some(b) = self.blocks.pop() {
                    match b.kind {
                        BlockKind::Subquery { close_col } | BlockKind::Columns { close_col } => {
                            self.out.newline(close_col, false);
                            break;
                        }
                        BlockKind::Root => {
                            self.blocks.push(b);
                            break;
                        }
                        BlockKind::BeginEnd => {}
                    }
                }
                self.out.write(")", false);
            }
            Some(Paren::Inline) => {
                // check_modifier must see the depth before the pop
                self.emit(i, ")");
                self.parens.pop();
                let b = self.top();
                b.inline_depth = b.inline_depth.saturating_sub(1);
            }
            None => {
                self.emit(i, ")");
            }
        }
    }

    fn variable(&mut self, i: usize) {
        let indent = self.indent();
        let stmt = self.stmt;
        let b = self.top();
        if stmt.routine && !stmt.body_started && b.inline_depth == 0 && b.clause != Clause::Params {
            b.clause = Clause::Params;
            b.content_col = Some(b.base + indent);
            let col = b.base + indent;
            self.out.newline(col, false);
            self.out.write(&self.items[i].text, false);
            return;
        }
        let text = self.items[i].text.clone();
        self.emit(i, &text);
    }

    fn operator(&mut self, i: usize) {
        let text = self.items[i].text.clone();
        let unary = matches!(text.as_ref(), "-" | "+" | "~")
            && match self.prev_item() {
                None => true,
                Some(p) => match p.kind {
                    TokenKind::Operator | TokenKind::LParen | TokenKind::Comma => true,
                    TokenKind::Word => {
                        let u = p.upper.as_str();
                        (p.merged || keywords::is_reserved(u, self.dialect)) && !keywords::is_function(u)
                    }
                    _ => false,
                },
            };
        self.emit(i, &text);
        if unary {
            self.no_space = true;
        }
    }

    fn word(&mut self, i: usize) {
        let item = &self.items[i];
        let up = item.upper.clone();
        let text = self.word_text(i);
        let in_case = !self.top_ref().case_stack.is_empty();
        let indent = self.indent();

        if up == "CASE" {
            let start = self.emit(i, &text);
            self.top().case_stack.push(start);
            return;
        }
        if in_case {
            let case_col = *self.top_ref().case_stack.last().expect("in case");
            if up == "WHEN" || up.starts_with("WHEN ") || up == "ELSE" {
                self.out.newline(case_col + indent, false);
                self.out.write(&text, false);
                return;
            }
            if up == "END" {
                self.top().case_stack.pop();
                self.out.newline(case_col, false);
                self.out.write(&text, false);
                return;
            }
        }
        if self.dialect == Dialect::MsSql && up == "GO" && (item.newlines_before > 0 || self.prev.is_none()) {
            self.go(&text);
            return;
        }
        if self.top_ref().inline_depth > 0 {
            self.emit(i, &text);
            return;
        }

        let clause = self.top_ref().clause;
        let prev_up = self.prev_upper().to_string();
        let prev_kind = self.prev_item().map(|p| p.kind);
        let next_up = self.next_upper(i).to_string();
        let next_is_paren = self.next_sig(i).is_some_and(|n| n.kind == TokenKind::LParen);

        match up.as_str() {
            "SELECT" => return self.clause(i, &text, Clause::Select, true),
            "FROM" => return self.clause(i, &text, Clause::From, false),
            "WHERE" => return self.clause(i, &text, Clause::Where, false),
            "GROUP BY" => return self.clause(i, &text, Clause::GroupBy, false),
            "HAVING" => return self.clause(i, &text, Clause::Having, false),
            "ORDER BY" => return self.clause(i, &text, Clause::OrderBy, false),
            "UNION" | "UNION ALL" | "UNION DISTINCT" | "EXCEPT" | "EXCEPT ALL" | "INTERSECT" | "INTERSECT ALL" => {
                self.clause(i, &text, Clause::SetOp, false);
                let base = self.top_ref().base;
                self.out.newline(base, false);
                return;
            }
            "INSERT" | "INSERT INTO" => return self.clause(i, &text, Clause::Insert, true),
            // VALUES(col) is a function inside MySQL's ON DUPLICATE KEY UPDATE
            "VALUES"
                if !matches!(
                    clause,
                    Clause::Set | Clause::Select | Clause::Where | Clause::On | Clause::Having
                ) =>
            {
                return self.clause(i, &text, Clause::Values, false);
            }
            "UPDATE" | "DELETE" | "DELETE FROM" if prev_up != "ON" => {
                let c = if up == "UPDATE" { Clause::Update } else { Clause::Delete };
                return self.clause(i, &text, c, true);
            }
            "SET" if !matches!(prev_up.as_str(), "DELETE" | "UPDATE") => {
                return self.clause(i, &text, Clause::Set, true);
            }
            "ON DUPLICATE KEY UPDATE" => return self.clause(i, &text, Clause::Set, false),
            "ON CONFLICT" => return self.clause(i, &text, Clause::Other, false),
            "INTO" if clause == Clause::Select => {
                return self.clause(i, &text, Clause::Into, false);
            }
            "USING" if self.stmt.merge => return self.clause(i, &text, Clause::Using, false),
            "OUTPUT"
                if matches!(
                    clause,
                    Clause::Insert
                        | Clause::Update
                        | Clause::Set
                        | Clause::Delete
                        | Clause::Where
                        | Clause::Using
                        | Clause::Other
                ) =>
            {
                return self.clause(i, &text, Clause::Output, false);
            }
            "RETURNING" => return self.clause(i, &text, Clause::Returning, false),
            "LIMIT" | "OFFSET" | "FETCH" if clause != Clause::None => {
                return self.clause(i, &text, Clause::Limit, false);
            }
            "OPTION" if next_is_paren => return self.clause(i, &text, Clause::Other, false),
            "WINDOW" if self.dialect != Dialect::MsSql => {
                return self.clause(i, &text, Clause::Other, false);
            }
            "FOR"
                if matches!(next_up.as_str(), "XML" | "JSON" | "UPDATE" | "BROWSE" | "SHARE")
                    && clause != Clause::None =>
            {
                return self.clause(i, &text, Clause::Other, false);
            }
            "WITH" if !next_is_paren && self.is_cte_start() => {
                return self.clause(i, &text, Clause::With, true);
            }
            _ if up.starts_with("WHEN ") && self.stmt.merge => {
                return self.clause(i, &text, Clause::Other, false);
            }
            _ if is_join(&up) => return self.join(i, &text),
            "ON" if matches!(clause, Clause::Join | Clause::Using) => return self.on(i, &text),
            "AND" | "OR" => return self.logical(i, &text),
            "BETWEEN" => {
                self.top().between = true;
                self.emit(i, &text);
                return;
            }
            "BEGIN" | "BEGIN TRY" | "BEGIN CATCH"
                if !(up == "BEGIN" && (next_up.is_empty() || matches!(next_up.as_str(), "WORK" | "TRANSACTION")))
                    && self.next_sig(i).is_none_or(|n| n.kind != TokenKind::Semicolon) =>
            {
                return self.begin_block(i, &text);
            }
            "END" | "END TRY" | "END CATCH" if self.top_ref().kind == BlockKind::BeginEnd => {
                return self.end_block(&text);
            }
            "AS" if self.stmt.routine && !self.stmt.body_started && self.top_ref().kind == BlockKind::Root => {
                self.stmt.body_started = true;
                let base = self.top_ref().base;
                self.top().clause = Clause::None;
                self.out.newline(base, false);
                self.out.write(&text, false);
                let base = self.top_ref().base;
                self.out.newline(base, false);
                return;
            }
            "RETURNS" if self.stmt.routine && !self.stmt.body_started => {
                return self.clause(i, &text, Clause::Other, false);
            }
            _ => {}
        }

        if STATEMENT_WORDS.contains(&up.as_str()) && self.is_statement_start(i, &up, &prev_up, prev_kind, &next_up) {
            self.statement(i, &text, &up, &next_up);
            return;
        }
        self.emit(i, &text);
    }

    fn is_cte_start(&self) -> bool {
        match self.prev_item() {
            None => true,
            Some(p) => match p.kind {
                TokenKind::Semicolon | TokenKind::LParen => true,
                TokenKind::Word => matches!(p.upper.as_str(), "AS" | "GO" | "BEGIN" | "ELSE" | "THEN" | "END"),
                _ => false,
            },
        }
    }

    fn is_statement_start(
        &self,
        i: usize,
        up: &str,
        prev_up: &str,
        prev_kind: Option<TokenKind>,
        next_up: &str,
    ) -> bool {
        let it = &self.items[i];
        if !it.merged && !keywords::is_reserved(up, self.dialect) {
            return false;
        }
        if matches!(
            prev_kind,
            Some(TokenKind::Dot | TokenKind::Operator | TokenKind::Comma | TokenKind::LParen)
        ) {
            return false;
        }
        match up {
            "ALTER" => next_up != "COLUMN",
            "DROP" => OBJECT_KINDS.contains(&next_up),
            "IF" => {
                !OBJECT_KINDS.contains(&prev_up)
                    && prev_up != "ELSE"
                    && !(self.dialect == Dialect::MySql
                        && self.items.get(i + 1).is_some_and(|n| n.kind == TokenKind::LParen))
            }
            "ELSE" | "END" => self.top_ref().case_stack.is_empty(),
            _ => true,
        }
    }

    fn statement(&mut self, i: usize, text: &str, up: &str, next_up: &str) {
        let blank = self.items[i].newlines_before >= 2 && self.top_ref().statement_level();
        if up.starts_with("CREATE") || up == "ALTER" {
            self.stmt = Stmt::default();
            match next_up {
                "PROC" | "PROCEDURE" | "FUNCTION" | "TRIGGER" | "VIEW" => self.stmt.routine = true,
                "TABLE" if up.starts_with("CREATE") => self.stmt.create_table = true,
                _ => {}
            }
        } else if up.starts_with("MERGE") {
            self.stmt = Stmt {
                merge: true,
                ..Stmt::default()
            };
        } else if up == "DECLARE" {
            self.stmt = Stmt::default();
        }
        let b = self.top();
        b.clause = if up == "DECLARE" { Clause::Declare } else { Clause::None };
        b.content_col = None;
        b.pending_break = false;
        b.mods = Mods::Off;
        let base = b.base;
        self.out.newline(base, blank);
        self.out.write(text, false);
    }

    fn clause(&mut self, i: usize, text: &str, clause: Clause, stmt_start: bool) {
        let stacked = self.opts.list_layout == ListLayout::Stacked;
        let blank = stmt_start && self.items[i].newlines_before >= 2 && self.top_ref().statement_level();
        let b = self.top();
        let base = b.base;
        b.clause = clause;
        b.content_col = None;
        b.between = false;
        b.mods = if clause == Clause::Select {
            Mods::Open
        } else {
            Mods::Off
        };
        b.pending_break = stacked && clause.stacks();
        self.out.newline(base, blank);
        self.out.write(text, false);
    }

    fn join(&mut self, _i: usize, text: &str) {
        let extra = if self.opts.indent_joins { self.indent() } else { 0 };
        let b = self.top();
        let col = b.base + extra;
        b.clause = Clause::Join;
        b.join_col = col;
        b.content_col = None;
        b.pending_break = false;
        self.out.newline(col, false);
        self.out.write(text, false);
    }

    fn on(&mut self, i: usize, text: &str) {
        let indent = self.indent();
        let on_new_line = self.opts.on_new_line;
        let b = self.top();
        let col = if b.clause == Clause::Using {
            b.base + indent
        } else {
            b.join_col + indent
        };
        b.clause = Clause::On;
        b.content_col = None;
        if on_new_line {
            self.out.newline(col, false);
            self.out.write(text, false);
        } else {
            self.emit(i, text);
            self.top().content_col = None;
        }
    }

    fn logical(&mut self, i: usize, text: &str) {
        let indent = self.indent();
        let enabled = self.opts.and_or_new_line;
        let b = self.top();
        if std::mem::take(&mut b.between) {
            self.emit(i, text);
            return;
        }
        let breaks = enabled
            && b.case_stack.is_empty()
            && matches!(b.clause, Clause::Where | Clause::Having | Clause::On | Clause::Join);
        if breaks {
            let col = b.content_col.unwrap_or(b.base + indent);
            self.out.newline(col, false);
            self.out.write(text, false);
        } else {
            self.emit(i, text);
        }
    }

    fn begin_block(&mut self, i: usize, text: &str) {
        let indent = self.indent();
        let blank = self.items[i].newlines_before >= 2 && self.top_ref().statement_level();
        let b = self.top();
        b.clause = Clause::None;
        b.pending_break = false;
        let base = b.base;
        self.out.newline(base, blank);
        self.out.write(text, false);
        self.blocks.push(Block::new(BlockKind::BeginEnd, base + indent));
        self.out.newline(base + indent, false);
    }

    fn end_block(&mut self, text: &str) {
        self.blocks.pop();
        let b = self.top();
        b.clause = Clause::None;
        let base = b.base;
        self.out.newline(base, false);
        self.out.write(text, false);
    }

    fn go(&mut self, text: &str) {
        self.blocks.truncate(1);
        self.blocks[0] = Block::new(BlockKind::Root, 0);
        self.parens.clear();
        self.stmt = Stmt::default();
        self.out.newline(0, false);
        self.out.write(text, false);
        self.out.newline(0, true);
    }
}

fn is_join(up: &str) -> bool {
    up == "JOIN" || up == "STRAIGHT_JOIN" || up.ends_with(" JOIN") || up.ends_with(" APPLY")
}

// TokenKind is Copy, so taking self by value is fine
#[allow(clippy::wrong_self_convention)]
trait NameLike {
    fn is_name_like(self) -> bool;
}

impl NameLike for TokenKind {
    fn is_name_like(self) -> bool {
        matches!(self, TokenKind::Word | TokenKind::QuotedIdent)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fmt(src: &str) -> String {
        format_sql(src, Dialect::MsSql, &FormatOptions::default())
    }

    fn fmt_with(src: &str, d: Dialect, f: impl FnOnce(&mut FormatOptions)) -> String {
        let mut o = FormatOptions::default();
        f(&mut o);
        format_sql(src, d, &o)
    }

    #[test]
    fn simple_select_aligned() {
        assert_eq!(
            fmt("select a, b, count(*) from dbo.t where x = 1 and y in (1,2) group by a, b order by a desc"),
            "SELECT a,\n       b,\n       COUNT(*)\nFROM dbo.t\nWHERE x = 1\n      AND y IN (1, 2)\nGROUP BY a,\n         b\nORDER BY a DESC"
        );
    }

    #[test]
    fn joins_and_on() {
        assert_eq!(
            fmt(
                "select o.id from dbo.Orders o inner join dbo.Customers c on c.id = o.customer_id and c.active = 1 left join x on x.id=o.id"
            ),
            "SELECT o.id\nFROM dbo.Orders o\n    INNER JOIN dbo.Customers c\n        ON c.id = o.customer_id\n           AND c.active = 1\n    LEFT JOIN x\n        ON x.id = o.id"
        );
    }

    #[test]
    fn stacked_layout_with_distinct_top() {
        let out = fmt_with(
            "select distinct top (10) a, b from t where a = 1",
            Dialect::MsSql,
            |o| {
                o.list_layout = ListLayout::Stacked;
            },
        );
        assert_eq!(
            out,
            "SELECT DISTINCT TOP (10)\n    a,\n    b\nFROM\n    t\nWHERE\n    a = 1"
        );
    }

    #[test]
    fn leading_commas() {
        let out = fmt_with("select a, b from t", Dialect::MsSql, |o| {
            o.comma_style = CommaStyle::Leading;
            o.list_layout = ListLayout::Stacked;
        });
        assert_eq!(out, "SELECT\n    a\n  , b\nFROM\n    t");
    }

    #[test]
    fn subquery_is_indented() {
        assert_eq!(
            fmt("select * from (select a from t) x where x.a in (select b from u)"),
            "SELECT *\nFROM (\n    SELECT a\n    FROM t\n) x\nWHERE x.a IN (\n    SELECT b\n    FROM u\n)"
        );
    }

    #[test]
    fn case_expression() {
        assert_eq!(
            fmt("select case when a = 1 then 'x' else 'y' end as k from t"),
            "SELECT CASE\n           WHEN a = 1 THEN 'x'\n           ELSE 'y'\n       END AS k\nFROM t"
        );
    }

    #[test]
    fn between_and_does_not_break() {
        assert_eq!(
            fmt("select a from t where d between 1 and 2 and e = 3"),
            "SELECT a\nFROM t\nWHERE d BETWEEN 1 AND 2\n      AND e = 3"
        );
    }

    #[test]
    fn multiple_statements_and_go() {
        assert_eq!(
            fmt("select 1; select 2\ngo\nselect 3"),
            "SELECT 1;\n\nSELECT 2\nGO\n\nSELECT 3"
        );
    }

    #[test]
    fn insert_values() {
        assert_eq!(
            fmt("insert into dbo.t (a, b) values (1, 'x'), (2, 'y')"),
            "INSERT INTO dbo.t (a, b)\nVALUES (1, 'x'),\n       (2, 'y')"
        );
    }

    #[test]
    fn update_set_from() {
        assert_eq!(
            fmt("update t set a = 1, b = 2 where id = @id"),
            "UPDATE t\nSET a = 1,\n    b = 2\nWHERE id = @id"
        );
    }

    #[test]
    fn create_table_columns() {
        assert_eq!(
            fmt("create table dbo.t (id int not null primary key, name nvarchar(50) null)"),
            "CREATE TABLE dbo.t (\n    id INT NOT NULL PRIMARY KEY,\n    name NVARCHAR(50) NULL\n)"
        );
    }

    #[test]
    fn procedure_with_params_and_body() {
        assert_eq!(
            fmt("create procedure dbo.p @a int, @b varchar(max) as begin set nocount on; select @a end"),
            "CREATE PROCEDURE dbo.p\n    @a INT,\n    @b VARCHAR(MAX)\nAS\nBEGIN\n    SET NOCOUNT ON;\n    SELECT @a\nEND"
        );
    }

    #[test]
    fn if_else_blocks() {
        assert_eq!(
            fmt("if @x = 1 begin print 'a' end else begin print 'b' end"),
            "IF @x = 1\nBEGIN\n    PRINT 'a'\nEND\nELSE\nBEGIN\n    PRINT 'b'\nEND"
        );
    }

    #[test]
    fn cte() {
        assert_eq!(
            fmt("with c as (select a from t) select a from c"),
            "WITH c AS (\n    SELECT a\n    FROM t\n)\nSELECT a\nFROM c"
        );
    }

    #[test]
    fn sqlcmd_lines_and_variables_are_kept() {
        assert_eq!(
            fmt(":setvar Schema \"dbo\"\n:on error exit\nselect a from $(Schema).Orders_$(Env) where b = '$(x)'\ngo"),
            ":setvar Schema \"dbo\"\n:on error exit\nSELECT a\nFROM $(Schema).Orders_$(Env)\nWHERE b = '$(x)'\nGO"
        );
    }

    #[test]
    fn comments_are_kept() {
        assert_eq!(
            fmt("-- header\nselect a, -- first\n b /* inline */ from t"),
            "-- header\nSELECT a, -- first\n       b /* inline */\nFROM t"
        );
    }

    #[test]
    fn lower_case_keywords_and_identifiers_untouched() {
        let out = fmt_with("SELECT [Select], Date FROM T", Dialect::MsSql, |o| {
            o.keyword_case = KeywordCase::Lower
        });
        assert_eq!(out, "select [Select],\n       Date\nfrom T");
    }

    #[test]
    fn postgres_casts_and_limit() {
        assert_eq!(
            format_sql(
                "select x::int, now() from t order by 1 limit 5",
                Dialect::Postgres,
                &FormatOptions::default()
            ),
            "SELECT x::int,\n       NOW()\nFROM t\nORDER BY 1\nLIMIT 5"
        );
    }

    #[test]
    fn mysql_on_duplicate_key() {
        assert_eq!(
            format_sql(
                "insert into t (a) values (1) on duplicate key update a = values(a)",
                Dialect::MySql,
                &FormatOptions::default()
            ),
            "INSERT INTO t (a)\nVALUES (1)\nON DUPLICATE KEY UPDATE a = VALUES (a)"
        );
    }

    #[test]
    fn unary_minus() {
        assert_eq!(
            fmt("select -1, a - 1 from t where x = -2"),
            "SELECT -1,\n       a - 1\nFROM t\nWHERE x = -2"
        );
    }

    #[test]
    fn idempotent() {
        let srcs = [
            "select a, b from dbo.t t inner join u on u.id = t.id where t.x = 1 and (t.y = 2 or t.z = 3)",
            "create procedure p @a int as begin select case when @a > 1 then 1 else 0 end end",
            "with c as (select 1 as a) select * from c; select 2",
        ];
        for s in srcs {
            let once = fmt(s);
            assert_eq!(fmt(&once), once, "not idempotent for {s}");
        }
    }

    #[test]
    fn incomplete_sql_never_loses_tokens() {
        for s in [
            "select a, from",
            "select (a from t",
            "select a) from t",
            "begin end end",
            "select 'unterminated",
        ] {
            let out = fmt(s);
            assert!(same_tokens(s, &out, Dialect::MsSql), "{s:?} -> {out:?}");
        }
    }
}
