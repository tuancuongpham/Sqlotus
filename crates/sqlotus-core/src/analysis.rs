//! Editor intelligence beyond completion: code-analysis diagnostics, hover
//! text, parameter hints and "what object is under the cursor". The checks
//! made before a script runs are in [`crate::safety`].

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use crate::catalog::{Catalog, DbObject, ObjectDefinition, ObjectKind};
use crate::keywords::STATEMENT_KEYWORDS;
use crate::lexer::{Token, TokenKind, significant, tokenize, word};
use crate::scope::{Scope, TableRef, analyze_statement, document_symbols};
use crate::statements::{Span, split_batches, split_statements, tokens_in};
use crate::{Dialect, keywords};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// Byte range in the source.
    pub start: usize,
    pub end: usize,
    pub severity: Severity,
    pub code: String,
    pub message: String,
    /// A safe rewrite that resolves the problem, when there is one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<Fix>,
}

/// Replace `start..end` (bytes) with `text`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Fix {
    pub title: String,
    pub start: usize,
    pub end: usize,
    pub text: String,
}

pub(crate) fn diag(t: &Token<'_>, severity: Severity, code: &str, message: String) -> Diagnostic {
    Diagnostic {
        start: t.start,
        end: t.end(),
        severity,
        code: code.into(),
        message,
        fix: None,
    }
}

const SYSTEM_SCHEMAS: &[&str] = &[
    "sys",
    "information_schema",
    "pg_catalog",
    "mysql",
    "performance_schema",
    "guest",
    "dbo_sys",
];

/// Objects created by the document itself (`CREATE TABLE x`, ...).
fn created_objects(t: &[Token<'_>]) -> HashSet<String> {
    let mut out = HashSet::new();
    for i in 0..t.len() {
        if word(t.get(i)) != "CREATE" {
            continue;
        }
        let mut k = i + 1;
        while matches!(
            word(t.get(k)).as_str(),
            "OR" | "ALTER" | "REPLACE" | "TEMP" | "TEMPORARY" | "UNLOGGED"
        ) {
            k += 1;
        }
        if !matches!(
            word(t.get(k)).as_str(),
            "TABLE" | "VIEW" | "PROCEDURE" | "PROC" | "FUNCTION" | "SYNONYM"
        ) {
            continue;
        }
        k += 1;
        if word(t.get(k)) == "IF" {
            k += 3; // IF NOT EXISTS
        }
        let mut last = None;
        while let Some(n) = t.get(k).filter(|n| n.is_name()) {
            last = Some(n.ident_value());
            if t.get(k + 1).is_some_and(|d| d.kind == TokenKind::Dot) {
                k += 2;
            } else {
                break;
            }
        }
        if let Some(n) = last {
            out.insert(n.to_lowercase());
        }
    }
    out
}

pub(crate) fn resolve<'c>(cat: &'c Catalog, r: &TableRef) -> Option<&'c DbObject> {
    if r.local || r.name.is_empty() {
        return None;
    }
    cat.find(r.schema.as_deref(), &r.name)
}

/// Code analysis for the whole document.
pub fn diagnostics(src: &str, dialect: Dialect, catalog: Option<&Catalog>) -> Vec<Diagnostic> {
    let tokens = tokenize(src, dialect);
    let all = significant(&tokens);
    let mut out = Vec::new();
    let created = created_objects(&all);
    let catalog = catalog.filter(|c| !c.objects.is_empty());
    let variable_types: HashMap<String, String> = document_symbols(src, dialect)
        .variables
        .into_iter()
        .filter_map(|v| Some((v.name.to_lowercase(), v.data_type?)))
        .collect();

    // per statement
    let statements = split_statements(src, dialect);
    for span in &statements {
        let stmt_tokens = tokens_in(&tokens, span);
        let t = significant(stmt_tokens);
        for i in crate::safety::unfiltered_modifications(&t) {
            out.push(diag(
                &t[i],
                Severity::Warning,
                "SQ001",
                format!("{} without a WHERE clause affects every row", t[i].upper()),
            ));
        }
        select_star(&t, &mut out);
        null_comparison(&t, &mut out);
        let scope = analyze_statement(stmt_tokens, dialect);
        crate::rules::not_in_subquery(&t, &scope, catalog, &mut out);
        crate::rules::non_sargable(&t, &scope, catalog, &mut out);
        if let Some(cat) = catalog {
            crate::rules::implicit_conversions(&t, &scope, cat, &variable_types, dialect, &mut out);
        }
        if dialect == Dialect::MsSql {
            deprecated_types(&t, &mut out);
            unicode_without_n(&t, &mut out);
        }
        if let Some(cat) = catalog {
            invalid_objects(&scope, cat, &created, dialect, &mut out);
            invalid_columns(&t, &scope, cat, &mut out);
        }
    }
    if dialect != Dialect::Postgres {
        unused_variables(src, dialect, &tokens, &statements, &mut out);
    }
    out.sort_by_key(|d| d.start);
    out
}

fn select_star(t: &[Token<'_>], out: &mut Vec<Diagnostic>) {
    for (i, tok) in t.iter().enumerate() {
        if tok.text != "*" || tok.kind != TokenKind::Operator || i == 0 {
            continue;
        }
        let prev = &t[i - 1];
        let star_position = matches!(prev.upper().as_str(), "SELECT" | "DISTINCT")
            || prev.kind == TokenKind::Comma
            || (prev.kind == TokenKind::Dot && i >= 2 && t[i - 2].is_name());
        if !star_position {
            continue;
        }
        // find the SELECT this * belongs to; EXISTS (SELECT *) is fine
        let mut depth = 0i32;
        let mut sel = None;
        for k in (0..i).rev() {
            match t[k].kind {
                TokenKind::RParen => depth += 1,
                TokenKind::LParen => {
                    if depth == 0 {
                        break;
                    }
                    depth -= 1;
                }
                TokenKind::Word if depth == 0 && t[k].upper() == "SELECT" => {
                    sel = Some(k);
                    break;
                }
                _ => {}
            }
        }
        let Some(sel) = sel else { continue };
        let in_exists = sel >= 2 && t[sel - 1].kind == TokenKind::LParen && word(t.get(sel - 2)) == "EXISTS";
        if !in_exists {
            out.push(diag(
                tok,
                Severity::Info,
                "SQ002",
                "SELECT * — list the columns explicitly (Tab after * expands them)".into(),
            ));
        }
    }
}

fn null_comparison(t: &[Token<'_>], out: &mut Vec<Diagnostic>) {
    for (i, tok) in t.iter().enumerate() {
        if !matches!(tok.text, "=" | "<>" | "!=") {
            continue;
        }
        let next_null = word(t.get(i + 1)) == "NULL";
        let prev_null = i > 0 && word(t.get(i - 1)) == "NULL";
        if !next_null && !prev_null {
            continue;
        }
        let context = t[..i]
            .iter()
            .rev()
            .filter(|x| x.kind == TokenKind::Word)
            .map(|x| x.upper())
            .find(|w| {
                matches!(
                    w.as_str(),
                    "WHERE"
                        | "ON"
                        | "HAVING"
                        | "WHEN"
                        | "AND"
                        | "OR"
                        | "IF"
                        | "WHILE"
                        | "SET"
                        | "SELECT"
                        | "VALUES"
                        | "DECLARE"
                        | "UPDATE"
                )
            })
            .unwrap_or_default();
        if matches!(
            context.as_str(),
            "WHERE" | "ON" | "HAVING" | "WHEN" | "AND" | "OR" | "IF" | "WHILE"
        ) {
            let fix = if tok.text == "=" { "IS NULL" } else { "IS NOT NULL" };
            let mut d = diag(
                tok,
                Severity::Warning,
                "SQ003",
                format!("Comparison with NULL is never true; use {fix}"),
            );
            // `col = NULL` -> `col IS NULL` (`NULL = col` would need reordering)
            if next_null {
                d.fix = Some(Fix {
                    title: format!("Use {fix}"),
                    start: tok.start,
                    end: t[i + 1].end(),
                    text: fix.into(),
                });
            }
            out.push(d);
        }
    }
}

/// `'Nguyễn'` without the N prefix: SQL Server converts the literal to the
/// database code page first, so characters outside it become `?`.
fn unicode_without_n(t: &[Token<'_>], out: &mut Vec<Diagnostic>) {
    for tok in t {
        if tok.kind == TokenKind::String && tok.text.starts_with('\'') && !tok.text.is_ascii() {
            let mut d = diag(
                tok,
                Severity::Warning,
                "SQ008",
                "Unicode text without the N prefix may be stored as '?' — write N'…'".into(),
            );
            d.fix = Some(Fix {
                title: "Add the N prefix".into(),
                start: tok.start,
                end: tok.start,
                text: "N".into(),
            });
            out.push(d);
        }
    }
}

fn deprecated_types(t: &[Token<'_>], out: &mut Vec<Diagnostic>) {
    let first = word(t.first());
    if !matches!(first.as_str(), "CREATE" | "ALTER" | "DECLARE") {
        return;
    }
    for (i, tok) in t.iter().enumerate().skip(2) {
        let u = word(Some(tok));
        let replacement = match u.as_str() {
            "TEXT" => "varchar(max)",
            "NTEXT" => "nvarchar(max)",
            "IMAGE" => "varbinary(max)",
            _ => continue,
        };
        let prev = &t[i - 1];
        let is_type = prev.kind == TokenKind::Variable
            || (prev.is_name() && matches!(t[i - 2].kind, TokenKind::LParen | TokenKind::Comma));
        if is_type {
            out.push(diag(
                tok,
                Severity::Warning,
                "SQ004",
                format!("{} is deprecated; use {replacement}", u.to_lowercase()),
            ));
        }
    }
}

fn unused_variables(src: &str, dialect: Dialect, tokens: &[Token<'_>], statements: &[Span], out: &mut Vec<Diagnostic>) {
    let all = significant(tokens);
    for batch in split_batches(src, dialect) {
        let t = tokens_in(&all, &batch.span);
        let mut uses: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, x) in t.iter().enumerate() {
            if x.kind == TokenKind::Variable && !x.text.starts_with("@@") {
                uses.entry(x.text.to_lowercase()).or_default().push(i);
            }
        }
        let routine = matches!(word(t.first()).as_str(), "CREATE" | "ALTER");
        for (_, idx) in uses {
            let i = idx[0];
            let prev = if i > 0 { word(t.get(i - 1)) } else { String::new() };
            let declared = prev == "DECLARE" || (i > 0 && t[i - 1].kind == TokenKind::Comma && in_declare(t, i));
            let message = if declared {
                // a DECLAREd variable that is only ever assigned is not used either
                let assigned = idx[1..].iter().any(|&k| is_assignment(t, k));
                if idx[1..].iter().any(|&k| !is_assignment(t, k)) {
                    continue;
                }
                if assigned {
                    "is assigned but never read"
                } else {
                    "is declared but never used"
                }
            } else if routine && idx.len() == 1 && is_parameter(t, i) {
                // parameters: an assignment may be the point (OUTPUT), so only a parameter that never appears
                "is declared but never used"
            } else {
                continue;
            };
            let mut d = diag(&t[i], Severity::Info, "SQ005", format!("{} {message}", t[i].text));
            if declared && idx.len() == 1 {
                d.fix = remove_declaration(src, statements, t, i);
            }
            out.push(d);
        }
    }
}

/// Remove `DECLARE @v type [= value]` when it declares only `@v` and holds
/// no other statement; the line goes too when nothing else is on it.
fn remove_declaration(src: &str, statements: &[Span], t: &[Token<'_>], i: usize) -> Option<Fix> {
    let span = *statements
        .get(statements.partition_point(|s| s.end < t[i].end()))
        .filter(|s| s.start <= t[i].start)?;
    let stmt = tokens_in(t, &span);
    let lone = stmt.len() >= 3
        && word(stmt.first()) == "DECLARE"
        && stmt[1].start == t[i].start
        && stmt[2..].iter().all(|x| {
            x.kind != TokenKind::Comma
                && x.kind != TokenKind::Variable
                && !STATEMENT_KEYWORDS.contains(&x.upper().as_str())
        });
    if !lone {
        return None;
    }
    let line_start = src[..span.start].rfind('\n').map_or(0, |n| n + 1);
    let line_end = src[span.end..].find('\n').map_or(src.len(), |n| span.end + n + 1);
    let alone = src[line_start..span.start].trim().is_empty() && src[span.end..line_end].trim().is_empty();
    let (start, end) = if alone {
        (line_start, line_end)
    } else {
        (span.start, span.end)
    };
    Some(Fix {
        title: format!("Remove the declaration of {}", t[i].text),
        start,
        end,
        text: String::new(),
    })
}

/// `@v` at `i` is only written: `SET @v = …` (or `+=` etc.), `SELECT @v = …`,
/// or `UPDATE … SET …, @v = …`.
fn is_assignment(t: &[Token<'_>], i: usize) -> bool {
    const ASSIGN: &[&str] = &["=", "+=", "-=", "*=", "/=", "%=", "&=", "|=", "^="];
    if !t
        .get(i + 1)
        .is_some_and(|n| n.kind == TokenKind::Operator && ASSIGN.contains(&n.text))
    {
        return false;
    }
    let prev = if i > 0 { word(t.get(i - 1)) } else { return false };
    if prev == "SET" || prev == "SELECT" {
        return true;
    }
    // in a SELECT list or an UPDATE's SET list: walk back to the clause keyword
    if t[i - 1].kind != TokenKind::Comma {
        return false;
    }
    let mut depth = 0i32;
    for k in (0..i - 1).rev() {
        match t[k].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => depth -= 1,
            TokenKind::Semicolon => return false,
            TokenKind::Word if depth == 0 => {
                let w = t[k].upper();
                if w == "SELECT" || w == "SET" {
                    return true;
                }
                if STATEMENT_KEYWORDS.contains(&w.as_str()) || matches!(w.as_str(), "FROM" | "WHERE" | "AS") {
                    return false;
                }
            }
            _ => {}
        }
    }
    false
}

/// Inside `DECLARE a, b, ...` (walk back over the list to DECLARE).
pub(crate) fn in_declare(t: &[Token<'_>], i: usize) -> bool {
    let mut depth = 0i32;
    for k in (0..i).rev() {
        match t[k].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => depth -= 1,
            TokenKind::Word if depth == 0 => {
                let w = t[k].upper();
                if w == "DECLARE" {
                    return true;
                }
                // `DECLARE @a AS int, @b ...`: an AS right after a variable is part of the list
                let type_as = w == "AS" && k > 0 && t[k - 1].kind == TokenKind::Variable;
                if STATEMENT_KEYWORDS.contains(&w.as_str()) || (w == "AS" && !type_as) {
                    return false;
                }
            }
            TokenKind::Semicolon => return false,
            _ => {}
        }
    }
    false
}

/// A parameter in a CREATE PROCEDURE/FUNCTION header (before the body's AS).
pub(crate) fn is_parameter(t: &[Token<'_>], i: usize) -> bool {
    !t[..i].iter().any(|x| matches!(x.upper().as_str(), "AS" | "BEGIN"))
}

fn invalid_objects(
    scope: &Scope,
    cat: &Catalog,
    created: &HashSet<String>,
    dialect: Dialect,
    out: &mut Vec<Diagnostic>,
) {
    let aliases: HashSet<String> = scope
        .tables
        .iter()
        .filter_map(|r| r.alias.as_ref())
        .map(|a| a.to_lowercase())
        .collect();
    let ctes: HashSet<String> = scope.ctes.iter().map(|c| c.name.to_lowercase()).collect();
    for r in &scope.tables {
        let lname = r.name.to_lowercase();
        // SQLCMD variables (`$(Schema).Orders`) name an object only when the script runs
        let sqlcmd = r.name.contains("$(") || r.schema.as_ref().is_some_and(|s| s.contains("$("));
        let skip = r.local
            || sqlcmd
            || r.name.is_empty()
            || r.via == "SELECT INTO"
            || (r.is_call && r.schema.is_none())
            || r.database
                .as_ref()
                .is_some_and(|d| !d.eq_ignore_ascii_case(&cat.database))
            || r.schema.as_ref().is_some_and(|s| {
                SYSTEM_SCHEMAS.iter().any(|x| x.eq_ignore_ascii_case(s))
                    || (dialect == Dialect::MySql && !cat.is_schema(s))
            })
            || lname.starts_with("pg_")
            || (dialect == Dialect::MsSql && matches!(lname.as_str(), "inserted" | "deleted"))
            || lname.starts_with("sys")
            || ctes.contains(&lname)
            || aliases.contains(&lname)
            || created.contains(&lname)
            || keywords::is_function(&r.name.to_ascii_uppercase());
        if skip || resolve(cat, r).is_some() {
            continue;
        }
        let shown = match &r.schema {
            Some(s) => format!("{s}.{}", r.name),
            None => r.name.clone(),
        };
        out.push(Diagnostic {
            start: r.name_start,
            end: r.name_end,
            severity: Severity::Warning,
            code: "SQ006".into(),
            message: format!("Invalid object name '{shown}' (reload objects if it was just created)"),
            fix: None,
        });
    }
}

fn invalid_columns(t: &[Token<'_>], scope: &Scope, cat: &Catalog, out: &mut Vec<Diagnostic>) {
    for i in 0..t.len().saturating_sub(2) {
        let (q, dot, c) = (&t[i], &t[i + 1], &t[i + 2]);
        if !q.is_name() || dot.kind != TokenKind::Dot || !c.is_name() {
            continue;
        }
        if i > 0 && t[i - 1].kind == TokenKind::Dot {
            continue; // part of a longer name
        }
        if t.get(i + 3)
            .is_some_and(|n| matches!(n.kind, TokenKind::Dot | TokenKind::LParen))
        {
            continue; // db.schema.table or method call
        }
        let qv = q.ident_value();
        let Some(r) = scope.tables.iter().find(|r| r.matches_qualifier(&qv)) else {
            continue;
        };
        if r.alias.is_none() && cat.is_schema(&qv) {
            continue;
        }
        let Some(obj) = resolve(cat, r) else { continue };
        if obj.columns.is_empty() {
            continue;
        }
        let col = c.ident_value();
        // the table name itself in `FROM dbo.Orders` style refs is not a column use
        if c.start == r.name_start || col.contains("$(") {
            continue;
        }
        if !obj.columns.iter().any(|x| x.name.eq_ignore_ascii_case(&col)) {
            out.push(diag(
                c,
                Severity::Warning,
                "SQ007",
                format!("Invalid column name '{col}' for {}.{}", obj.schema, obj.name),
            ));
        }
    }
}

/// A problem found in a stored object's definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectProblem {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
    /// 1-based line in the definition.
    pub line: usize,
    pub code: String,
    pub message: String,
}

/// "Find invalid objects": views, procedures, functions and triggers whose
/// definitions reference tables or columns that no longer exist.
pub fn invalid_objects_in_definitions(
    definitions: &[ObjectDefinition],
    dialect: Dialect,
    catalog: &Catalog,
) -> Vec<ObjectProblem> {
    let mut out = Vec::new();
    for d in definitions {
        for diag in diagnostics(&d.definition, dialect, Some(catalog)) {
            if diag.code != "SQ006" && diag.code != "SQ007" {
                continue;
            }
            out.push(ObjectProblem {
                schema: d.schema.clone(),
                name: d.name.clone(),
                kind: d.kind,
                line: d.definition[..diag.start].matches('\n').count() + 1,
                code: diag.code,
                message: diag.message,
            });
        }
    }
    out.sort_by(|a, b| (&a.schema, &a.name, a.line).cmp(&(&b.schema, &b.name, b.line)));
    out
}

// ---------------------------------------------------------------------------
// hover, parameter hints, object under cursor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoverInfo {
    pub start: usize,
    pub end: usize,
    /// Markdown.
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ObjectAt {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
}

struct Located<'a> {
    tokens: Vec<Token<'a>>,
    /// index in `tokens` of the name under the cursor
    idx: usize,
    scope: Scope,
}

fn locate<'a>(src: &'a str, offset: usize, dialect: Dialect) -> Option<Located<'a>> {
    let all = tokenize(src, dialect);
    let span = crate::statements::statement_at(src, dialect, offset)?;
    let stmt: Vec<Token<'a>> = all
        .iter()
        .filter(|t| t.start >= span.start && t.end() <= span.end)
        .copied()
        .collect();
    let scope = analyze_statement(&stmt, dialect);
    let tokens = significant(&stmt);
    let idx = tokens
        .iter()
        .position(|t| t.start <= offset && offset <= t.end() && (t.is_name() || t.kind == TokenKind::Variable))?;
    Some(Located { tokens, idx, scope })
}

/// Qualifier parts before the token at `idx`: `a.b.|c|` -> ["a", "b"].
fn qualifiers(t: &[Token<'_>], idx: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut k = idx;
    while k >= 2 && t[k - 1].kind == TokenKind::Dot && t[k - 2].is_name() {
        parts.push(t[k - 2].ident_value());
        k -= 2;
    }
    parts.reverse();
    parts
}

fn kind_label(k: ObjectKind) -> &'static str {
    match k {
        ObjectKind::Table => "table",
        ObjectKind::View => "view",
        ObjectKind::Procedure => "procedure",
        ObjectKind::ScalarFunction => "function",
        ObjectKind::TableFunction => "table-valued function",
        ObjectKind::Trigger => "trigger",
        ObjectKind::Synonym => "synonym",
        ObjectKind::Sequence => "sequence",
    }
}

/// `text` as literal Markdown: what Markdown reads as emphasis, a link, a web
/// address, code, a table cell or an entity is escaped, and line breaks are
/// spaces. Names come from the database: a table `_Archive_` lost its
/// underscores in a hover, and one named `[x](https://…)` became a link.
fn md(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in crate::text::one_line(text).chars() {
        if matches!(
            c,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '|' | '~' | '&' | ':' | '.'
        ) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `text` as a Markdown code span, whatever backticks it holds.
fn code(text: &str) -> String {
    let text = crate::text::one_line(text);
    let fence = "`".repeat(text.split(|c| c != '`').map(str::len).max().unwrap_or(0) + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

fn describe(o: &DbObject) -> String {
    let mut s = format!("**{}.{}** — {}\n\n", md(&o.schema), md(&o.name), kind_label(o.kind));
    if !o.columns.is_empty() {
        s.push_str("| column | type | |\n|---|---|---|\n");
        for c in o.columns.iter().take(40) {
            let mut flags = Vec::new();
            if c.is_primary_key {
                flags.push("PK");
            }
            if c.is_identity {
                flags.push("identity");
            }
            if c.is_computed {
                flags.push("computed");
            }
            if !c.nullable {
                flags.push("not null");
            }
            s.push_str(&format!(
                "| {} | {} | {} |\n",
                md(&c.name),
                md(&c.data_type),
                flags.join(", ")
            ));
        }
        if o.columns.len() > 40 {
            s.push_str(&format!("\n…and {} more columns\n", o.columns.len() - 40));
        }
    }
    if !o.params.is_empty() {
        // a line of its own could end the code block early
        s.push_str("```\n");
        for p in &o.params {
            s.push_str(&format!(
                "{} {}{}\n",
                crate::text::one_line(&p.name),
                crate::text::one_line(&p.data_type),
                if p.is_output { " OUTPUT" } else { "" }
            ));
        }
        s.push_str("```\n");
    }
    s
}

/// Resolve the name under the cursor to a catalog object (through aliases).
pub fn object_at(src: &str, offset: usize, dialect: Dialect, catalog: &Catalog) -> Option<ObjectAt> {
    let loc = locate(src, offset, dialect)?;
    let tok = loc.tokens[loc.idx];
    let quals = qualifiers(&loc.tokens, loc.idx);
    let name = tok.ident_value();
    let obj = match quals.as_slice() {
        [] => {
            let via_alias = loc
                .scope
                .tables
                .iter()
                .find(|r| r.alias.as_ref().is_some_and(|a| a.eq_ignore_ascii_case(&name)))
                .and_then(|r| resolve(catalog, r));
            via_alias.or_else(|| catalog.find(None, &name))
        }
        [.., s] => catalog.find(Some(s), &name),
    }?;
    Some(ObjectAt {
        schema: obj.schema.clone(),
        name: obj.name.clone(),
        kind: obj.kind,
    })
}

pub fn hover(src: &str, offset: usize, dialect: Dialect, catalog: Option<&Catalog>) -> Option<HoverInfo> {
    let loc = locate(src, offset, dialect)?;
    let tok = loc.tokens[loc.idx];
    let range = |contents: String| {
        Some(HoverInfo {
            start: tok.start,
            end: tok.end(),
            contents,
        })
    };

    if tok.kind == TokenKind::Variable {
        let syms = document_symbols(src, dialect);
        let v = syms.variables.iter().find(|v| v.name.eq_ignore_ascii_case(tok.text))?;
        return range(code(&format!(
            "{} {}",
            v.name,
            v.data_type.as_deref().unwrap_or("(type unknown)")
        )));
    }
    let cat = catalog?;
    let name = tok.ident_value();
    let quals = qualifiers(&loc.tokens, loc.idx);
    let next_is_dot = loc.tokens.get(loc.idx + 1).is_some_and(|n| n.kind == TokenKind::Dot);

    // alias / table used as a qualifier, or an alias on its own
    if quals.is_empty() {
        if let Some(r) = loc
            .scope
            .tables
            .iter()
            .find(|r| r.matches_qualifier(&name) || r.alias.as_ref().is_some_and(|a| a.eq_ignore_ascii_case(&name)))
        {
            if let Some(o) = resolve(cat, r) {
                return range(describe(o));
            }
            if !r.columns.is_empty() {
                return range(format!(
                    "**{}** — derived table / CTE\n\ncolumns: {}",
                    md(r.visible_name()),
                    r.columns.iter().map(|c| md(c)).collect::<Vec<_>>().join(", ")
                ));
            }
        }
        if next_is_dot && cat.is_schema(&name) {
            return range(format!("schema **{}**", md(&name)));
        }
    }
    // qualified column: alias.column / table.column
    if let Some(q) = quals.last()
        && let Some(r) = loc.scope.tables.iter().find(|r| r.matches_qualifier(q))
        && let Some(o) = resolve(cat, r)
        && let Some(c) = o.columns.iter().find(|c| c.name.eq_ignore_ascii_case(&name))
    {
        return range(format!(
            "column **{}.{}.{}**\n\n{}{}{}",
            md(&o.schema),
            md(&o.name),
            md(&c.name),
            code(&c.data_type),
            if c.nullable { " null" } else { " not null" },
            if c.is_primary_key { " · primary key" } else { "" }
        ));
    }
    // object by (schema.)name
    let schema = quals.last().map(String::as_str);
    if let Some(o) = cat.find(schema, &name) {
        return range(describe(o));
    }
    // unqualified column of a table in scope
    if quals.is_empty() {
        for r in &loc.scope.tables {
            if let Some(o) = resolve(cat, r)
                && let Some(c) = o.columns.iter().find(|c| c.name.eq_ignore_ascii_case(&name))
            {
                return range(format!(
                    "column **{}.{}.{}**\n\n{}",
                    md(&o.schema),
                    md(&o.name),
                    md(&c.name),
                    code(&c.data_type)
                ));
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SignatureInfo {
    pub label: String,
    pub parameters: Vec<String>,
    pub active_parameter: usize,
}

const BUILTIN_SIGNATURES: &[(&str, &[&str])] = &[
    ("DATEADD", &["datepart", "number", "date"]),
    ("DATEDIFF", &["datepart", "startdate", "enddate"]),
    ("DATEPART", &["datepart", "date"]),
    ("DATENAME", &["datepart", "date"]),
    ("DATEFROMPARTS", &["year", "month", "day"]),
    ("EOMONTH", &["start_date", "[month_to_add]"]),
    ("CONVERT", &["data_type", "expression", "[style]"]),
    ("TRY_CONVERT", &["data_type", "expression", "[style]"]),
    ("CAST", &["expression AS data_type"]),
    ("ISNULL", &["check_expression", "replacement_value"]),
    ("IFNULL", &["expr1", "expr2"]),
    ("NULLIF", &["expr1", "expr2"]),
    ("COALESCE", &["expression1", "expression2", "..."]),
    ("IIF", &["boolean_expression", "true_value", "false_value"]),
    ("SUBSTRING", &["expression", "start", "length"]),
    (
        "CHARINDEX",
        &["expression_to_find", "expression_to_search", "[start_location]"],
    ),
    ("REPLACE", &["string", "pattern", "replacement"]),
    ("STUFF", &["string", "start", "length", "replace_with"]),
    ("LEFT", &["string", "length"]),
    ("RIGHT", &["string", "length"]),
    ("LEN", &["string"]),
    ("ROUND", &["numeric_expression", "length", "[function]"]),
    ("FORMAT", &["value", "format", "[culture]"]),
    ("CONCAT", &["string1", "string2", "..."]),
    ("CONCAT_WS", &["separator", "string1", "string2", "..."]),
    ("STRING_AGG", &["expression", "separator"]),
    ("STRING_SPLIT", &["string", "separator", "[enable_ordinal]"]),
    ("ROW_NUMBER", &[]),
    ("LAG", &["expression", "[offset]", "[default]"]),
    ("LEAD", &["expression", "[offset]", "[default]"]),
    ("DATE_TRUNC", &["field", "source"]),
    ("TO_CHAR", &["value", "format"]),
    ("DATE_FORMAT", &["date", "format"]),
    ("JSON_VALUE", &["expression", "path"]),
];

/// Parameter hints inside `fn(...|` or after `EXEC proc ...|`.
pub fn signature_help(src: &str, offset: usize, dialect: Dialect, catalog: Option<&Catalog>) -> Option<SignatureInfo> {
    let all = tokenize(src, dialect);
    let t: Vec<Token<'_>> = significant(&all).into_iter().filter(|x| x.end() <= offset).collect();
    // innermost unclosed '('
    let mut depth = 0i32;
    let mut commas = 0usize;
    for k in (0..t.len()).rev() {
        match t[k].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen if depth > 0 => depth -= 1,
            TokenKind::Comma if depth == 0 => commas += 1,
            TokenKind::Semicolon => break,
            TokenKind::LParen => {
                let name_tok = k.checked_sub(1).map(|p| t[p])?;
                if !name_tok.is_name() {
                    return None;
                }
                let name = name_tok.ident_value();
                let quals = qualifiers(&t, k - 1);
                if let Some(o) = catalog
                    .and_then(|c| c.find(quals.last().map(String::as_str), &name))
                    .filter(|o| o.kind.is_routine())
                {
                    return Some(routine_signature(o, commas));
                }
                let upper = name.to_ascii_uppercase();
                let (_, params) = BUILTIN_SIGNATURES.iter().find(|(n, _)| *n == upper)?;
                let params: Vec<String> = params.iter().map(|p| p.to_string()).collect();
                let active = commas.min(params.len().saturating_sub(1));
                return Some(SignatureInfo {
                    label: format!("{upper}({})", params.join(", ")),
                    parameters: params,
                    active_parameter: active,
                });
            }
            TokenKind::Word if depth == 0 && matches!(t[k].upper().as_str(), "EXEC" | "EXECUTE") => {
                let name_tok = t.get(k + 1).filter(|n| n.is_name())?;
                let mut j = k + 1;
                let mut schema = None;
                let mut name = name_tok.ident_value();
                while t.get(j + 1).is_some_and(|d| d.kind == TokenKind::Dot) {
                    schema = Some(name);
                    name = t.get(j + 2)?.ident_value();
                    j += 2;
                }
                if offset <= t.get(j)?.end() {
                    return None; // still typing the procedure name
                }
                let o = catalog?
                    .find(schema.as_deref(), &name)
                    .filter(|o| o.kind == ObjectKind::Procedure)?;
                return Some(routine_signature(o, commas));
            }
            TokenKind::Word if depth == 0 && STATEMENT_KEYWORDS.contains(&t[k].upper().as_str()) => {
                return None;
            }
            _ => {}
        }
    }
    None
}

fn routine_signature(o: &DbObject, commas: usize) -> SignatureInfo {
    let params: Vec<String> = o
        .params
        .iter()
        .map(|p| format!("{} {}{}", p.name, p.data_type, if p.is_output { " OUTPUT" } else { "" }))
        .collect();
    SignatureInfo {
        label: format!("{}.{}({})", o.schema, o.name, params.join(", ")),
        active_parameter: commas.min(params.len().saturating_sub(1)),
        parameters: params,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Column, Param};

    fn col(n: &str, t: &str) -> Column {
        Column {
            name: n.into(),
            data_type: t.into(),
            nullable: true,
            is_primary_key: n == "Id",
            is_identity: false,
            is_computed: false,
        }
    }

    fn cat() -> Catalog {
        Catalog {
            database: "Shop".into(),
            default_schema: "dbo".into(),
            schemas: vec!["dbo".into()],
            objects: vec![
                DbObject {
                    schema: "dbo".into(),
                    name: "Orders".into(),
                    kind: ObjectKind::Table,
                    columns: vec![col("Id", "int"), col("CustomerId", "int"), col("Total", "money")],
                    params: vec![],
                },
                DbObject {
                    schema: "dbo".into(),
                    name: "GetOrders".into(),
                    kind: ObjectKind::Procedure,
                    columns: vec![],
                    params: vec![
                        Param {
                            name: "@CustomerId".into(),
                            data_type: "int".into(),
                            is_output: false,
                            has_default: false,
                        },
                        Param {
                            name: "@From".into(),
                            data_type: "date".into(),
                            is_output: false,
                            has_default: true,
                        },
                    ],
                },
            ],
            foreign_keys: vec![],
        }
    }

    fn codes(src: &str) -> Vec<String> {
        diagnostics(src, Dialect::MsSql, Some(&cat()))
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn update_delete_without_where() {
        assert_eq!(codes("UPDATE dbo.Orders SET Total = 0"), vec!["SQ001"]);
        assert_eq!(codes("DELETE FROM dbo.Orders"), vec!["SQ001"]);
        assert!(codes("UPDATE dbo.Orders SET Total = 0 WHERE Id = 1").is_empty());
        assert!(codes("DELETE o FROM dbo.Orders o WHERE o.Id = 1").is_empty());
        assert!(codes("CREATE TABLE x (a int REFERENCES dbo.Orders(Id) ON DELETE CASCADE)").is_empty());
    }

    #[test]
    fn select_star_and_null_comparison() {
        assert_eq!(codes("SELECT * FROM dbo.Orders"), vec!["SQ002"]);
        assert!(codes("SELECT COUNT(*) FROM dbo.Orders WHERE EXISTS (SELECT * FROM dbo.Orders)").is_empty());
        assert_eq!(codes("SELECT Id FROM dbo.Orders WHERE Total = NULL"), vec!["SQ003"]);
        assert!(codes("UPDATE dbo.Orders SET Total = NULL WHERE Id = 1").is_empty());
    }

    #[test]
    fn invalid_objects_and_columns() {
        assert_eq!(codes("SELECT Id FROM dbo.Ordrs"), vec!["SQ006"]);
        // SQLCMD variables name objects only when the script runs
        assert!(
            codes(":setvar S dbo\nSELECT o.Id, o.$(Col) FROM $(S).Orders o JOIN dbo.Orders_$(Env) x ON 1 = 1")
                .is_empty()
        );
        assert_eq!(codes("SELECT o.Totl FROM dbo.Orders o"), vec!["SQ007"]);
        assert!(codes("SELECT o.Total FROM dbo.Orders o JOIN #t t ON t.x = o.Id").is_empty());
        assert!(codes("WITH c AS (SELECT Id FROM dbo.Orders) SELECT c.Id FROM c").is_empty());
        assert!(codes("CREATE TABLE dbo.NewOne (a int); INSERT INTO dbo.NewOne (a) VALUES (1)").is_empty());
        assert!(
            codes("SELECT name FROM sys.objects; SELECT * INTO dbo.Copy FROM dbo.Orders")
                .iter()
                .all(|c| c == "SQ002")
        );
        assert!(codes("UPDATE o SET Total = 1 FROM dbo.Orders o WHERE o.Id = 1").is_empty());
        assert!(codes("SELECT value FROM STRING_SPLIT('a,b', ',')").is_empty());
        assert!(codes("SELECT * FROM OtherDb.dbo.Unknown").iter().all(|c| c == "SQ002"));
    }

    #[test]
    fn unicode_literals_need_n_prefix() {
        assert_eq!(codes("SELECT Id FROM dbo.Orders WHERE Total = 'Nguyễn'"), vec!["SQ008"]);
        assert!(codes("SELECT Id FROM dbo.Orders WHERE Total = N'Nguyễn' OR Total = 'abc'").is_empty());
        let pg = diagnostics("SELECT 'Nguyễn'", Dialect::Postgres, None);
        assert!(pg.is_empty());
    }

    #[test]
    fn deprecated_and_unused() {
        assert_eq!(codes("DECLARE @t TABLE (a text)"), vec!["SQ005", "SQ004"]);
        assert_eq!(codes("DECLARE @a int, @b int; SELECT @a"), vec!["SQ005"]);
        assert!(codes("CREATE PROCEDURE p @x int AS SELECT @x").is_empty());
        assert_eq!(codes("CREATE PROCEDURE p @x int, @y int AS SELECT @x"), vec!["SQ005"]);
    }

    #[test]
    fn assigned_but_never_read() {
        let msgs = |s: &str| {
            diagnostics(s, Dialect::MsSql, None)
                .into_iter()
                .filter(|d| d.code == "SQ005")
                .map(|d| d.message)
                .collect::<Vec<_>>()
        };
        // only assigned, in every assignment form
        assert_eq!(
            msgs("DECLARE @a int; SET @a = 1"),
            vec!["@a is assigned but never read"]
        );
        assert_eq!(
            msgs("DECLARE @n int = 0; SET @n += 1"),
            vec!["@n is assigned but never read"]
        );
        assert_eq!(
            msgs("DECLARE @c int; SELECT @c = COUNT(*) FROM t"),
            vec!["@c is assigned but never read"]
        );
        assert_eq!(
            msgs("DECLARE @c int; SELECT a, @c = b FROM t"),
            vec!["@c is assigned but never read"]
        );
        assert_eq!(
            msgs("DECLARE @v int; UPDATE t SET a = 1, @v = a"),
            vec!["@v is assigned but never read"]
        );
        assert_eq!(
            msgs(
                "DECLARE @a varchar(10) SET @a = 'x'
DECLARE @b varchar(10) SET @b = 'y'"
            ),
            vec!["@a is assigned but never read", "@b is assigned but never read"]
        );
        // DECLARE ... AS type, the form used in many scripts
        assert_eq!(
            msgs("DECLARE @Current AS VARCHAR(100) SET @Current = 'a'"),
            vec!["@Current is assigned but never read"]
        );
        assert_eq!(
            msgs("DECLARE @a AS int, @b AS int; SELECT @a"),
            vec!["@b is declared but never used"]
        );
        // read somewhere: not reported
        assert!(msgs("DECLARE @a int; SET @a = 1; SELECT @a").is_empty());
        assert!(msgs("DECLARE @a int; SET @a = @a + 1").is_empty());
        assert!(msgs("DECLARE @a int = 1; IF @a = 1 PRINT 'x'").is_empty());
        assert!(msgs("DECLARE @a int = 1; SELECT x FROM t WHERE y = 2 AND @a = 1").is_empty());
        assert!(msgs("DECLARE @a int = 1; SELECT CASE WHEN @a = 1 THEN 1 END").is_empty());
        assert!(msgs("DECLARE @a int = 1; EXEC p @x = 1, @y = @a").is_empty());
        assert!(msgs("DECLARE @a int = 1; EXEC p @a = 5, @b = @a").is_empty());
        assert!(msgs("DECLARE @t TABLE (a int); INSERT INTO @t VALUES (1); SELECT * FROM @t").is_empty());
        // separate batches are separate scopes
        assert_eq!(
            msgs(
                "DECLARE @a int = 1
GO
DECLARE @a int = 2 SELECT @a"
            ),
            vec!["@a is declared but never used"]
        );
        // an assigned OUTPUT parameter is the point of the procedure
        assert!(msgs("CREATE PROCEDURE p @r int OUTPUT AS SET @r = 1").is_empty());
    }

    #[test]
    fn invalid_objects_in_database() {
        let defs = vec![
            ObjectDefinition {
                schema: "dbo".into(),
                name: "vOk".into(),
                kind: ObjectKind::View,
                definition: "CREATE VIEW dbo.vOk AS SELECT o.Total FROM dbo.Orders o".into(),
            },
            ObjectDefinition {
                schema: "dbo".into(),
                name: "pBroken".into(),
                kind: ObjectKind::Procedure,
                definition: "CREATE PROCEDURE dbo.pBroken AS
SELECT o.Amount FROM dbo.Orders o
SELECT 1 FROM dbo.Gone"
                    .into(),
            },
            ObjectDefinition {
                schema: "dbo".into(),
                name: "tr".into(),
                kind: ObjectKind::Trigger,
                definition: "CREATE TRIGGER dbo.tr ON dbo.Orders AFTER INSERT AS SELECT i.Id FROM inserted i".into(),
            },
        ];
        let found = invalid_objects_in_definitions(&defs, Dialect::MsSql, &cat());
        let got: Vec<_> = found
            .iter()
            .map(|p| (p.name.as_str(), p.line, p.code.as_str()))
            .collect();
        assert_eq!(got, vec![("pBroken", 2, "SQ007"), ("pBroken", 3, "SQ006")]);
    }

    #[test]
    fn hover_and_object_at() {
        let c = cat();
        let src = "SELECT o.Total FROM dbo.Orders o";
        let h = hover(src, 10, Dialect::MsSql, Some(&c)).unwrap();
        assert!(
            h.contents.contains("dbo.Orders.Total") && h.contents.contains("money"),
            "{}",
            h.contents
        );
        let h = hover(src, 26, Dialect::MsSql, Some(&c)).unwrap();
        assert!(h.contents.contains("| CustomerId | int |"), "{}", h.contents);
        assert_eq!(object_at(src, 7, Dialect::MsSql, &c).unwrap().name, "Orders");
        let h = hover("DECLARE @n nvarchar(10); SELECT @n", 33, Dialect::MsSql, None).unwrap();
        assert!(h.contents.contains("nvarchar"));
    }

    /// Names come from the database: a hover shows them as they are, never
    /// as Markdown (emphasis, a link, a broken table).
    #[test]
    fn hover_shows_names_as_they_are() {
        let mut c = cat();
        c.objects.push(DbObject {
            schema: "dbo".into(),
            name: "_Archive_".into(),
            kind: ObjectKind::Table,
            columns: vec![col("[x](https://e.example)", "int"), col("a|b", "enum('`')")],
            params: vec![],
        });
        let src = "SELECT * FROM dbo._Archive_";
        let h = hover(src, src.len() - 2, Dialect::MsSql, Some(&c)).unwrap();
        assert!(
            h.contents.starts_with("**dbo.\\_Archive\\_** — table"),
            "{}",
            h.contents
        );
        assert!(
            h.contents.contains("| \\[x\\](https\\://e\\.example) | int |"),
            "{}",
            h.contents
        );
        assert!(h.contents.contains("| a\\|b | enum('\\`') |"), "{}", h.contents);
        assert_eq!(code("enum('`')"), "``enum('`')``");
        assert_eq!(code("`x"), "`` `x ``");
        assert_eq!(md("a\nb"), "a b");
    }

    #[test]
    fn parameter_hints() {
        let c = cat();
        let s = signature_help("SELECT DATEADD(day, 1, ", 23, Dialect::MsSql, Some(&c)).unwrap();
        assert_eq!(s.active_parameter, 2);
        assert_eq!(s.parameters[2], "date");
        let src = "EXEC dbo.GetOrders @CustomerId = 1, ";
        let s = signature_help(src, src.len(), Dialect::MsSql, Some(&c)).unwrap();
        assert_eq!(s.active_parameter, 1);
        assert!(s.label.starts_with("dbo.GetOrders(@CustomerId int"));
        assert!(signature_help("SELECT a FROM t", 15, Dialect::MsSql, Some(&c)).is_none());
    }
}
