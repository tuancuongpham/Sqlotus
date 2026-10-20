//! Context-aware completion: what to suggest at a cursor position.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, DbObject, ObjectKind};
use crate::format::KeywordCase;
use crate::lexer::{Token, TokenKind, tokenize};
use crate::scope::{DocSymbols, Scope, TableRef, analyze_statement, document_symbols};
use crate::snippet::{Snippet, to_monaco};
use crate::statements::statement_at;
use crate::{Dialect, keywords};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum CompletionKind {
    Keyword,
    Function,
    Schema,
    Table,
    View,
    TableFunction,
    ScalarFunction,
    Procedure,
    Column,
    Alias,
    LocalTable,
    Variable,
    Snippet,
    Join,
    Expansion,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    pub detail: String,
    pub insert_text: String,
    /// `insert_text` uses snippet syntax (`$1`, `${1:x}`, `$0`).
    pub is_snippet: bool,
    pub sort_text: String,
    /// Overrides the result's replace range start (byte offset) when the
    /// item replaces more than the current word, e.g. `t.*` expansion.
    pub replace_start: Option<usize>,
    pub filter_text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionResult {
    /// Byte range of the text the selected item replaces.
    pub replace_start: usize,
    pub replace_end: usize,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompletionOptions {
    pub keyword_case: KeywordCase,
    /// Insert `schema.object` rather than `object`.
    pub qualify_objects: bool,
    /// Append a generated alias after tables in FROM/JOIN.
    pub assign_aliases: bool,
    pub max_items: usize,
}

impl Default for CompletionOptions {
    fn default() -> Self {
        Self {
            keyword_case: KeywordCase::Upper,
            qualify_objects: true,
            assign_aliases: true,
            max_items: 400,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Context {
    /// After FROM/JOIN/UPDATE/INTO/...: relation names. `alias` says whether
    /// to append a generated alias; `join` whether FK-based joins apply.
    Relation {
        alias: bool,
        join: bool,
    },
    Procedure,
    /// `INSERT INTO t (|`
    InsertColumns,
    /// Right after `ON` of a join.
    JoinCondition,
    General,
}

struct Ctx<'a> {
    src: &'a str,
    dialect: Dialect,
    catalog: Option<&'a Catalog>,
    opts: &'a CompletionOptions,
    scope: Scope,
    doc: DocSymbols,
    /// Significant tokens of the current statement before the cursor word.
    before: Vec<Token<'a>>,
    /// The word being typed.
    prefix: String,
    items: Vec<CompletionItem>,
}

/// Compute completions for `src` with the cursor at byte `offset`.
pub fn complete(
    src: &str,
    offset: usize,
    dialect: Dialect,
    catalog: Option<&Catalog>,
    snippets: &[Snippet],
    opts: &CompletionOptions,
) -> CompletionResult {
    let offset = floor_char_boundary(src, offset.min(src.len()));
    let tokens = tokenize(src, dialect);
    let current = tokens
        .iter()
        .find(|t| t.start < offset && offset <= t.end());

    let empty = CompletionResult {
        replace_start: offset,
        replace_end: offset,
        items: Vec::new(),
    };
    if let Some(t) = current {
        let in_literal = match t.kind {
            TokenKind::String | TokenKind::LineComment => offset < t.end() || !string_closed(t),
            TokenKind::BlockComment => offset < t.end() || !t.text.ends_with("*/"),
            TokenKind::QuotedIdent => false,
            _ => false,
        };
        if in_literal {
            return empty;
        }
    }

    // the word being typed
    let (replace_start, replace_end, prefix) = match current {
        Some(t)
            if matches!(
                t.kind,
                TokenKind::Word | TokenKind::QuotedIdent | TokenKind::Variable
            ) =>
        {
            (t.start, t.end(), &src[t.start..offset])
        }
        Some(t) if t.kind == TokenKind::Operator && t.text == "@" => (t.start, t.end(), "@"),
        _ => (offset, offset, ""),
    };

    let stmt = statement_at(src, dialect, replace_start).filter(|s| s.start <= replace_start);
    let stmt_tokens: Vec<Token<'_>> = match stmt {
        Some(s) => tokens
            .iter()
            .filter(|t| t.start >= s.start && t.end() <= s.end.max(offset))
            .copied()
            .collect(),
        None => Vec::new(),
    };
    let before: Vec<Token<'_>> = stmt_tokens
        .iter()
        .filter(|t| !t.is_trivia() && t.end() <= replace_start)
        .copied()
        .collect();
    // a statement that already ended (`;`) before the cursor does not apply
    let before = if before
        .last()
        .is_some_and(|t| t.kind == TokenKind::Semicolon)
    {
        Vec::new()
    } else {
        before
    };
    let scope = if before.is_empty() {
        Scope::default()
    } else {
        analyze_statement(&stmt_tokens, dialect)
    };

    let mut cx = Ctx {
        src,
        dialect,
        catalog,
        opts,
        scope,
        doc: document_symbols(src, dialect),
        before,
        prefix: prefix.to_string(),
        items: Vec::new(),
    };

    let unquoted_prefix = crate::lexer::unquote(prefix);
    if prefix.starts_with('@') && dialect != Dialect::Postgres {
        cx.variables();
    } else if let Some(last) = cx.before.last().copied() {
        match last.kind {
            TokenKind::Dot => cx.qualified(),
            TokenKind::Operator if last.text == "*" && prefix.is_empty() => {
                cx.expansion(last);
                cx.general(snippets);
            }
            _ => match cx.context() {
                Context::Relation { alias, join } => cx.relations(alias, join),
                Context::Procedure => cx.procedures(),
                Context::InsertColumns => cx.insert_columns(),
                Context::JoinCondition => {
                    cx.join_conditions();
                    cx.columns(None);
                    cx.aliases();
                }
                Context::General => cx.general(snippets),
            },
        }
    } else {
        cx.keywords(true);
        cx.snippets(snippets);
    }

    let mut items = filter_and_rank(cx.items, &unquoted_prefix, opts.max_items);
    dedupe(&mut items);
    CompletionResult {
        replace_start,
        replace_end,
        items,
    }
}

fn string_closed(t: &Token<'_>) -> bool {
    t.kind == TokenKind::String && t.text.len() >= 2 && t.text.ends_with('\'')
}

fn floor_char_boundary(s: &str, mut i: usize) -> usize {
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

impl<'a> Ctx<'a> {
    fn kw(&self, s: &str) -> String {
        match self.opts.keyword_case {
            KeywordCase::Upper | KeywordCase::Preserve => s.to_ascii_uppercase(),
            KeywordCase::Lower => s.to_ascii_lowercase(),
        }
    }

    fn push(
        &mut self,
        label: impl Into<String>,
        kind: CompletionKind,
        detail: impl Into<String>,
        insert: impl Into<String>,
        rank: u8,
    ) {
        let label = label.into();
        self.items.push(CompletionItem {
            sort_text: format!("{rank}{}", label.to_lowercase()),
            label,
            kind,
            detail: detail.into(),
            insert_text: insert.into(),
            is_snippet: false,
            replace_start: None,
            filter_text: None,
        });
    }

    fn quote(&self, name: &str) -> String {
        self.dialect.quote_ident(name)
    }

    fn last_upper(&self, back: usize) -> String {
        self.before
            .len()
            .checked_sub(back + 1)
            .map(|i| self.before[i].upper())
            .unwrap_or_default()
    }

    // -- context detection ------------------------------------------------

    fn context(&self) -> Context {
        let last = *self.before.last().expect("non-empty");
        if last.kind == TokenKind::Word {
            match last.upper().as_str() {
                "FROM" | "APPLY" | "STRAIGHT_JOIN" => {
                    return Context::Relation {
                        alias: true,
                        join: false,
                    };
                }
                "JOIN" => {
                    return Context::Relation {
                        alias: true,
                        join: true,
                    };
                }
                "UPDATE" | "INTO" | "TABLE" | "USING" | "TRUNCATE" => {
                    return Context::Relation {
                        alias: false,
                        join: false,
                    };
                }
                "EXEC" | "EXECUTE" | "CALL" => return Context::Procedure,
                "ON" if self.after_join() => return Context::JoinCondition,
                _ => {}
            }
        }
        if matches!(last.kind, TokenKind::LParen | TokenKind::Comma) && self.in_insert_column_list()
        {
            return Context::InsertColumns;
        }
        if last.kind == TokenKind::Comma && self.clause_keyword() == "FROM" {
            return Context::Relation {
                alias: true,
                join: false,
            };
        }
        Context::General
    }

    /// The most recent clause keyword at paren depth 0 before the cursor.
    fn clause_keyword(&self) -> String {
        let mut depth = 0i32;
        for t in self.before.iter().rev() {
            match t.kind {
                TokenKind::RParen => depth += 1,
                TokenKind::LParen => depth -= 1,
                TokenKind::Word if depth <= 0 => {
                    let u = t.upper();
                    if matches!(
                        u.as_str(),
                        "SELECT"
                            | "FROM"
                            | "WHERE"
                            | "JOIN"
                            | "ON"
                            | "BY"
                            | "HAVING"
                            | "SET"
                            | "VALUES"
                            | "INTO"
                            | "UPDATE"
                    ) {
                        return u;
                    }
                }
                _ => {}
            }
        }
        String::new()
    }

    fn after_join(&self) -> bool {
        let mut depth = 0i32;
        for t in self.before.iter().rev().skip(1) {
            match t.kind {
                TokenKind::RParen => depth += 1,
                TokenKind::LParen => depth -= 1,
                TokenKind::Word if depth <= 0 => {
                    let u = t.upper();
                    if u == "JOIN" {
                        return true;
                    }
                    if matches!(u.as_str(), "SELECT" | "WHERE" | "FROM" | "ON") {
                        return false;
                    }
                }
                _ => {}
            }
        }
        false
    }

    /// `INSERT INTO t (a, |`: the innermost open paren follows `INTO name`.
    fn in_insert_column_list(&self) -> bool {
        self.insert_target().is_some()
    }

    fn insert_target(&self) -> Option<TableRef> {
        let mut depth = 0i32;
        let open = self.before.iter().rposition(|t| {
            match t.kind {
                TokenKind::RParen => depth += 1,
                TokenKind::LParen => {
                    if depth == 0 {
                        return true;
                    }
                    depth -= 1;
                }
                _ => {}
            }
            false
        })?;
        // walk back over the (possibly qualified) name
        let mut j = open;
        let mut parts = Vec::new();
        while j > 0 && self.before[j - 1].is_name() {
            parts.push(self.before[j - 1].ident_value());
            j -= 1;
            if j > 0 && self.before[j - 1].kind == TokenKind::Dot {
                j -= 1;
            } else {
                break;
            }
        }
        if parts.is_empty() || j == 0 {
            return None;
        }
        let kw = self.before[j - 1].upper();
        if kw != "INTO" && kw != "INSERT" {
            return None;
        }
        parts.reverse();
        let name = parts.pop()?;
        Some(TableRef {
            schema: parts.pop(),
            name,
            alias: None,
            columns: Vec::new(),
            local: false,
            start: 0,
        })
    }

    // -- providers ----------------------------------------------------------

    fn keywords(&mut self, statement_start: bool) {
        const STARTS: &[&str] = &[
            "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "CREATE", "ALTER", "DROP", "DECLARE",
            "SET", "EXEC", "BEGIN", "IF", "MERGE", "TRUNCATE",
        ];
        let words = keywords::completion_keywords(self.dialect);
        for w in words {
            let rank = if statement_start && STARTS.contains(&w) {
                2
            } else {
                7
            };
            let k = self.kw(w);
            self.push(k.clone(), CompletionKind::Keyword, "keyword", k, rank);
        }
    }

    fn functions(&mut self) {
        for f in keywords::functions() {
            let name = self.kw(f);
            let mut item = CompletionItem {
                sort_text: format!("6{}", name.to_lowercase()),
                label: name.clone(),
                kind: CompletionKind::Function,
                detail: "built-in function".into(),
                insert_text: format!("{name}($0)"),
                is_snippet: true,
                replace_start: None,
                filter_text: None,
            };
            if matches!(
                *f,
                "GETDATE" | "NOW" | "NEWID" | "SCOPE_IDENTITY" | "GETUTCDATE" | "SYSDATETIME"
            ) {
                item.insert_text = format!("{name}()");
                item.is_snippet = false;
            }
            self.items.push(item);
        }
    }

    fn snippets(&mut self, snippets: &[Snippet]) {
        let vars = Default::default();
        for s in snippets {
            self.items.push(CompletionItem {
                sort_text: format!("1{}", s.prefix.to_lowercase()),
                label: s.prefix.clone(),
                kind: CompletionKind::Snippet,
                detail: s.description.clone(),
                insert_text: to_monaco(s, &vars),
                is_snippet: true,
                replace_start: None,
                filter_text: None,
            });
        }
    }

    fn variables(&mut self) {
        let vars = self.doc.variables.clone();
        for v in vars {
            if v.data_type.is_none() && v.name.eq_ignore_ascii_case(&self.prefix) {
                continue;
            }
            let detail = v.data_type.clone().unwrap_or_else(|| "variable".into());
            self.push(v.name.clone(), CompletionKind::Variable, detail, v.name, 0);
        }
        if self.dialect == Dialect::MsSql {
            for g in [
                "@@ROWCOUNT",
                "@@ERROR",
                "@@IDENTITY",
                "@@TRANCOUNT",
                "@@SPID",
                "@@VERSION",
                "@@SERVERNAME",
            ] {
                self.push(g, CompletionKind::Variable, "system variable", g, 5);
            }
        }
    }

    fn general(&mut self, snippets: &[Snippet]) {
        self.columns(None);
        self.aliases();
        if self.dialect != Dialect::Postgres {
            self.variables();
        }
        self.functions();
        self.keywords(false);
        self.snippets(snippets);
    }

    fn aliases(&mut self) {
        let tables = self.scope.tables.clone();
        for t in tables {
            let v = t.visible_name().to_string();
            if v.is_empty() {
                continue;
            }
            let detail = match (&t.alias, &t.schema) {
                (Some(_), Some(s)) => format!("alias of {s}.{}", t.name),
                (Some(_), None) => format!("alias of {}", t.name),
                _ => "table".into(),
            };
            let q = self.quote(&v);
            self.push(v, CompletionKind::Alias, detail, q, 1);
        }
    }

    fn table_columns(&self, t: &TableRef) -> Vec<(String, String)> {
        if !t.columns.is_empty() {
            return t
                .columns
                .iter()
                .map(|c| (c.clone(), String::new()))
                .collect();
        }
        if let Some(local) = self
            .doc
            .local_tables
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(&t.name))
        {
            return local
                .columns
                .iter()
                .map(|c| (c.clone(), String::new()))
                .collect();
        }
        if let Some(c) = self
            .scope
            .ctes
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(&t.name))
        {
            return c
                .columns
                .iter()
                .map(|c| (c.clone(), String::new()))
                .collect();
        }
        self.catalog
            .and_then(|cat| cat.find(t.schema.as_deref(), &t.name))
            .map(|o| {
                o.columns
                    .iter()
                    .map(|c| (c.name.clone(), c.data_type.clone()))
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Columns of tables in scope; `only` restricts to one table.
    fn columns(&mut self, only: Option<&TableRef>) {
        let tables: Vec<TableRef> = match only {
            Some(t) => vec![t.clone()],
            None => self.scope.tables.clone(),
        };
        let per_table: Vec<(TableRef, Vec<(String, String)>)> = tables
            .into_iter()
            .map(|t| {
                let c = self.table_columns(&t);
                (t, c)
            })
            .collect();
        let mut seen: HashSet<String> = HashSet::new();
        let mut dup: HashSet<String> = HashSet::new();
        for (_, cols) in &per_table {
            for (c, _) in cols {
                if !seen.insert(c.to_lowercase()) {
                    dup.insert(c.to_lowercase());
                }
            }
        }
        let qualify_all = only.is_none() && per_table.len() > 1;
        for (t, cols) in per_table {
            let vis = t.visible_name().to_string();
            for (c, ty) in cols {
                let detail = if ty.is_empty() {
                    vis.clone()
                } else {
                    format!("{vis} · {ty}")
                };
                let insert = if only.is_none() && qualify_all && dup.contains(&c.to_lowercase()) {
                    format!("{}.{}", self.quote(&vis), self.quote(&c))
                } else {
                    self.quote(&c)
                };
                self.push(c, CompletionKind::Column, detail, insert, 0);
            }
        }
    }

    fn object_kind(kind: ObjectKind) -> CompletionKind {
        match kind {
            ObjectKind::Table | ObjectKind::Synonym => CompletionKind::Table,
            ObjectKind::View => CompletionKind::View,
            ObjectKind::TableFunction => CompletionKind::TableFunction,
            ObjectKind::ScalarFunction => CompletionKind::ScalarFunction,
            ObjectKind::Procedure => CompletionKind::Procedure,
            ObjectKind::Trigger | ObjectKind::Sequence => CompletionKind::Table,
        }
    }

    fn qualified_name(&self, o: &DbObject, force_schema: bool) -> String {
        let cat = self.catalog;
        let use_schema = force_schema
            || self.opts.qualify_objects
            || cat.is_some_and(|c| !o.schema.eq_ignore_ascii_case(&c.default_schema));
        if use_schema && !o.schema.is_empty() {
            format!("{}.{}", self.quote(&o.schema), self.quote(&o.name))
        } else {
            self.quote(&o.name)
        }
    }

    fn existing_aliases(&self) -> HashSet<String> {
        self.scope
            .tables
            .iter()
            .map(|t| t.visible_name().to_lowercase())
            .collect()
    }

    fn relations(&mut self, alias: bool, join: bool) {
        let alias = alias && self.opts.assign_aliases;
        let taken = self.existing_aliases();
        if join {
            self.fk_joins(&taken);
        }
        // CTEs and local tables
        let locals: Vec<String> = self
            .scope
            .ctes
            .iter()
            .map(|c| c.name.clone())
            .chain(self.doc.local_tables.iter().map(|l| l.name.clone()))
            .collect();
        for name in locals {
            let mut insert = self.quote(&name);
            if alias && !name.starts_with('@') {
                insert = format!("{insert} {}", make_alias(&name, &taken));
            }
            self.push(name, CompletionKind::LocalTable, "local", insert, 0);
        }
        if let Some(cat) = self.catalog {
            for o in cat.objects.iter().filter(|o| o.kind.is_relation()) {
                let mut insert = self.qualified_name(o, false);
                if alias {
                    insert = format!("{insert} {}", make_alias(&o.name, &taken));
                }
                let rank = if o.kind == ObjectKind::Table { 2 } else { 3 };
                self.push(
                    o.name.clone(),
                    Self::object_kind(o.kind),
                    format!("{}.{}", o.schema, o.name),
                    insert,
                    rank,
                );
            }
            for s in &cat.schemas {
                self.push(
                    s.clone(),
                    CompletionKind::Schema,
                    "schema",
                    self.quote(s),
                    4,
                );
            }
        }
        self.keywords(false);
    }

    /// After `JOIN`: tables with a foreign key to a table already in scope,
    /// inserted together with the ON clause.
    fn fk_joins(&mut self, taken: &HashSet<String>) {
        let Some(cat) = self.catalog else { return };
        let in_scope: Vec<(TableRef, &DbObject)> = self
            .scope
            .tables
            .iter()
            .filter_map(|t| {
                cat.find(t.schema.as_deref(), &t.name)
                    .map(|o| (t.clone(), o))
            })
            .collect();
        for target in cat.objects.iter().filter(|o| o.kind == ObjectKind::Table) {
            for (r, ro) in &in_scope {
                for fk in cat.foreign_keys_between(target, ro) {
                    let alias = make_alias(&target.name, taken);
                    let target_is_from = fk.from_table.eq_ignore_ascii_case(&target.name)
                        && fk.from_schema.eq_ignore_ascii_case(&target.schema);
                    let (tcols, rcols) = if target_is_from {
                        (&fk.from_columns, &fk.to_columns)
                    } else {
                        (&fk.to_columns, &fk.from_columns)
                    };
                    let cond = join_condition(&alias, tcols, r.visible_name(), rcols, self.dialect);
                    let insert =
                        format!("{} {alias} ON {cond}", self.qualified_name(target, false));
                    let label = format!("{} ON {cond}", target.name);
                    self.items.push(CompletionItem {
                        sort_text: format!("0{}", label.to_lowercase()),
                        label,
                        kind: CompletionKind::Join,
                        detail: format!("FK {}", fk.name),
                        insert_text: insert,
                        is_snippet: false,
                        replace_start: None,
                        filter_text: Some(target.name.clone()),
                    });
                }
            }
        }
    }

    /// After `ON`: conditions from foreign keys between the newest joined
    /// table and earlier ones.
    fn join_conditions(&mut self) {
        let Some(cat) = self.catalog else { return };
        let tables = self.scope.tables.clone();
        let Some((newest, earlier)) = tables.split_last() else {
            return;
        };
        let Some(no) = cat.find(newest.schema.as_deref(), &newest.name) else {
            return;
        };
        for r in earlier {
            let Some(ro) = cat.find(r.schema.as_deref(), &r.name) else {
                continue;
            };
            for fk in cat.foreign_keys_between(no, ro) {
                let newest_is_from = fk.from_table.eq_ignore_ascii_case(&no.name);
                let (ncols, rcols) = if newest_is_from {
                    (&fk.from_columns, &fk.to_columns)
                } else {
                    (&fk.to_columns, &fk.from_columns)
                };
                let cond = join_condition(
                    newest.visible_name(),
                    ncols,
                    r.visible_name(),
                    rcols,
                    self.dialect,
                );
                self.push(
                    cond.clone(),
                    CompletionKind::Join,
                    format!("FK {}", fk.name),
                    cond,
                    0,
                );
            }
        }
    }

    fn procedures(&mut self) {
        let Some(cat) = self.catalog else { return };
        for o in cat
            .objects
            .iter()
            .filter(|o| o.kind == ObjectKind::Procedure)
        {
            let name = self.qualified_name(o, false);
            let params: Vec<String> = o
                .params
                .iter()
                .enumerate()
                .map(|(i, p)| {
                    let n = crate::snippet::escape_snippet(&p.name);
                    if self.dialect == Dialect::MsSql {
                        format!("{n} = ${{{}:{n}}}", i + 1)
                    } else {
                        format!("${{{}:{n}}}", i + 1)
                    }
                })
                .collect();
            let insert = match (self.dialect, params.is_empty()) {
                (_, true) => crate::snippet::escape_snippet(&name),
                (Dialect::MsSql, false) => format!(
                    "{} {}",
                    crate::snippet::escape_snippet(&name),
                    params.join(", ")
                ),
                _ => format!(
                    "{}({})",
                    crate::snippet::escape_snippet(&name),
                    params.join(", ")
                ),
            };
            self.items.push(CompletionItem {
                sort_text: format!("0{}", o.name.to_lowercase()),
                label: o.name.clone(),
                kind: CompletionKind::Procedure,
                detail: format!("{}.{}", o.schema, o.name),
                insert_text: insert,
                is_snippet: true,
                replace_start: None,
                filter_text: None,
            });
        }
    }

    fn insert_columns(&mut self) {
        if let Some(t) = self.insert_target() {
            self.columns(Some(&t));
        }
    }

    /// After `qualifier.`: columns of an alias/table, or objects of a schema.
    fn qualified(&mut self) {
        let mut parts = Vec::new();
        let mut j = self.before.len();
        while j >= 2 && self.before[j - 1].kind == TokenKind::Dot && self.before[j - 2].is_name() {
            parts.push(self.before[j - 2].ident_value());
            j -= 2;
        }
        parts.reverse();
        let kw_before = if j > 0 {
            self.before[j - 1].upper()
        } else {
            String::new()
        };
        let relation_ctx = matches!(
            kw_before.as_str(),
            "FROM" | "JOIN" | "APPLY" | "UPDATE" | "INTO" | "TABLE" | "USING"
        ) || (self
            .before
            .get(j.wrapping_sub(1))
            .is_some_and(|t| t.kind == TokenKind::Comma)
            && self.clause_keyword() == "FROM");
        let proc_ctx = matches!(kw_before.as_str(), "EXEC" | "EXECUTE" | "CALL");

        match parts.as_slice() {
            [q] => {
                if !relation_ctx && !proc_ctx {
                    if let Some(t) = self
                        .scope
                        .tables
                        .iter()
                        .find(|t| t.matches_qualifier(q))
                        .cloned()
                    {
                        self.columns(Some(&t));
                        return;
                    }
                }
                if let Some(cat) = self.catalog {
                    if cat.is_schema(q) {
                        self.schema_objects(q, relation_ctx, proc_ctx);
                        return;
                    }
                    if !relation_ctx {
                        let t = TableRef {
                            schema: None,
                            name: q.clone(),
                            alias: None,
                            columns: Vec::new(),
                            local: false,
                            start: 0,
                        };
                        self.columns(Some(&t));
                    }
                }
            }
            [a, b] => {
                if let Some(cat) = self.catalog {
                    if cat.database.eq_ignore_ascii_case(a) && cat.is_schema(b) {
                        self.schema_objects(b, relation_ctx, proc_ctx);
                        return;
                    }
                }
                let t = TableRef {
                    schema: Some(a.clone()),
                    name: b.clone(),
                    alias: None,
                    columns: Vec::new(),
                    local: false,
                    start: 0,
                };
                self.columns(Some(&t));
            }
            [_, s, t] => {
                let t = TableRef {
                    schema: Some(s.clone()),
                    name: t.clone(),
                    alias: None,
                    columns: Vec::new(),
                    local: false,
                    start: 0,
                };
                self.columns(Some(&t));
            }
            _ => {}
        }
    }

    fn schema_objects(&mut self, schema: &str, relation_ctx: bool, proc_ctx: bool) {
        let Some(cat) = self.catalog else { return };
        let taken = self.existing_aliases();
        let alias = relation_ctx
            && self.opts.assign_aliases
            && self.last_upper(2) != "UPDATE"
            && self.last_upper(2) != "INTO";
        for o in cat
            .objects
            .iter()
            .filter(|o| o.schema.eq_ignore_ascii_case(schema))
        {
            let wanted = if proc_ctx {
                o.kind == ObjectKind::Procedure
            } else if relation_ctx {
                o.kind.is_relation()
            } else {
                o.kind != ObjectKind::Procedure && o.kind != ObjectKind::Trigger
            };
            if !wanted {
                continue;
            }
            let mut insert = self.quote(&o.name);
            if alias {
                insert = format!("{insert} {}", make_alias(&o.name, &taken));
            }
            self.push(
                o.name.clone(),
                Self::object_kind(o.kind),
                format!("{}.{}", o.schema, o.name),
                insert,
                0,
            );
        }
    }

    /// `SELECT *|` or `SELECT t.*|`: offer the explicit column list.
    fn expansion(&mut self, star: Token<'a>) {
        let n = self.before.len();
        let qualified =
            n >= 3 && self.before[n - 2].kind == TokenKind::Dot && self.before[n - 3].is_name();
        let (tables, start): (Vec<TableRef>, usize) = if qualified {
            let q = self.before[n - 3].ident_value();
            match self.scope.tables.iter().find(|t| t.matches_qualifier(&q)) {
                Some(t) => (vec![t.clone()], self.before[n - 3].start),
                None => return,
            }
        } else {
            (self.scope.tables.clone(), star.start)
        };
        let multi = tables.len() > 1 || qualified;
        let mut cols = Vec::new();
        for t in &tables {
            for (c, _) in self.table_columns(t) {
                cols.push(if multi {
                    format!("{}.{}", self.quote(t.visible_name()), self.quote(&c))
                } else {
                    self.quote(&c)
                });
            }
        }
        if cols.is_empty() {
            return;
        }
        let filter = self.src[start..star.end()].to_string();
        self.items.push(CompletionItem {
            label: format!("{filter} → {} columns", cols.len()),
            kind: CompletionKind::Expansion,
            detail: "expand wildcard".into(),
            insert_text: cols.join(", "),
            is_snippet: false,
            sort_text: "0".into(),
            replace_start: Some(start),
            filter_text: Some(filter),
        });
    }
}

fn join_condition(a: &str, acols: &[String], b: &str, bcols: &[String], d: Dialect) -> String {
    acols
        .iter()
        .zip(bcols)
        .map(|(x, y)| {
            format!(
                "{}.{} = {}.{}",
                d.quote_ident(a),
                d.quote_ident(x),
                d.quote_ident(b),
                d.quote_ident(y)
            )
        })
        .collect::<Vec<_>>()
        .join(" AND ")
}

/// Alias from the capitals / word starts of a name: `OrderDetails` -> `od`,
/// `order_items` -> `oi`, made unique against `taken`.
pub fn make_alias(name: &str, taken: &HashSet<String>) -> String {
    let name = name.trim_start_matches(['#', '@']);
    let mut alias = String::new();
    let mut prev: Option<char> = None;
    for c in name.chars() {
        let boundary = match prev {
            None => true,
            Some(p) => {
                (c.is_uppercase() && p.is_lowercase())
                    || (p == '_' || p == ' ') && c.is_alphanumeric()
            }
        };
        if boundary && c.is_alphanumeric() {
            alias.extend(c.to_lowercase());
        }
        prev = Some(c);
    }
    if alias.is_empty() {
        alias.push('t');
    }
    if keywords::is_reserved(&alias.to_ascii_uppercase(), Dialect::MsSql) {
        alias.push('1');
    }
    if !taken.contains(&alias) {
        return alias;
    }
    (2..)
        .map(|n| format!("{alias}{n}"))
        .find(|a| !taken.contains(a))
        .expect("infinite")
}

/// Keep items matching `prefix` (subsequence, case-insensitive), best first.
fn filter_and_rank(items: Vec<CompletionItem>, prefix: &str, max: usize) -> Vec<CompletionItem> {
    let p = prefix.to_lowercase();
    let mut scored: Vec<(u8, CompletionItem)> = items
        .into_iter()
        .filter_map(|it| {
            if p.is_empty() {
                return Some((0, it));
            }
            let target = it
                .filter_text
                .as_deref()
                .unwrap_or(&it.label)
                .to_lowercase();
            let score = if target.starts_with(&p) {
                0
            } else if target.contains(&p) {
                1
            } else if is_subsequence(&p, &target) {
                2
            } else {
                return None;
            };
            Some((score, it))
        })
        .collect();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.1.sort_text.cmp(&b.1.sort_text))
    });
    scored.truncate(max);
    scored
        .into_iter()
        .enumerate()
        .map(|(i, (_, mut it))| {
            it.sort_text = format!("{i:05}");
            it
        })
        .collect()
}

fn is_subsequence(needle: &str, hay: &str) -> bool {
    let mut it = hay.chars();
    needle.chars().all(|c| it.any(|h| h == c))
}

fn dedupe(items: &mut Vec<CompletionItem>) {
    let mut seen = HashSet::new();
    items.retain(|i| seen.insert((i.label.to_lowercase(), i.insert_text.clone())));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Column, ForeignKey, Param};

    fn col(n: &str, t: &str) -> Column {
        Column {
            name: n.into(),
            data_type: t.into(),
            nullable: false,
            is_primary_key: false,
            is_identity: false,
        }
    }

    fn cat() -> Catalog {
        Catalog {
            database: "Shop".into(),
            default_schema: "dbo".into(),
            schemas: vec!["dbo".into(), "sales".into()],
            objects: vec![
                DbObject {
                    schema: "dbo".into(),
                    name: "Customers".into(),
                    kind: ObjectKind::Table,
                    columns: vec![col("Id", "int"), col("Name", "nvarchar(100)")],
                    params: vec![],
                },
                DbObject {
                    schema: "dbo".into(),
                    name: "Orders".into(),
                    kind: ObjectKind::Table,
                    columns: vec![
                        col("Id", "int"),
                        col("CustomerId", "int"),
                        col("Total", "money"),
                    ],
                    params: vec![],
                },
                DbObject {
                    schema: "sales".into(),
                    name: "Regions".into(),
                    kind: ObjectKind::View,
                    columns: vec![col("Code", "char(2)")],
                    params: vec![],
                },
                DbObject {
                    schema: "dbo".into(),
                    name: "GetOrders".into(),
                    kind: ObjectKind::Procedure,
                    columns: vec![],
                    params: vec![Param {
                        name: "@CustomerId".into(),
                        data_type: "int".into(),
                        is_output: false,
                        has_default: false,
                    }],
                },
            ],
            foreign_keys: vec![ForeignKey {
                name: "FK_Orders_Customers".into(),
                from_schema: "dbo".into(),
                from_table: "Orders".into(),
                from_columns: vec!["CustomerId".into()],
                to_schema: "dbo".into(),
                to_table: "Customers".into(),
                to_columns: vec!["Id".into()],
            }],
        }
    }

    /// `|` marks the cursor.
    fn run(src_with_cursor: &str) -> CompletionResult {
        let offset = src_with_cursor.find('|').expect("cursor marker");
        let src = src_with_cursor.replacen('|', "", 1);
        let c = cat();
        complete(
            &src,
            offset,
            Dialect::MsSql,
            Some(&c),
            &crate::snippet::builtin_snippets(),
            &CompletionOptions::default(),
        )
    }

    fn labels(r: &CompletionResult) -> Vec<String> {
        r.items.iter().map(|i| i.label.clone()).collect()
    }

    #[test]
    fn alias_dot_gives_columns() {
        let r = run("SELECT o.| FROM dbo.Orders o");
        assert_eq!(labels(&r), vec!["CustomerId", "Id", "Total"]);
    }

    #[test]
    fn prefix_filters_and_sets_range() {
        let r = run("SELECT o.Cu| FROM dbo.Orders o");
        assert_eq!(labels(&r), vec!["CustomerId"]);
        assert_eq!((r.replace_start, r.replace_end), (9, 11));
    }

    #[test]
    fn from_suggests_tables_with_alias() {
        let r = run("SELECT * FROM Ord|");
        let first = &r.items[0];
        assert_eq!(first.label, "Orders");
        assert_eq!(first.insert_text, "dbo.Orders o");
    }

    #[test]
    fn schema_dot_lists_objects() {
        let r = run("SELECT * FROM sales.|");
        assert_eq!(labels(&r), vec!["Regions"]);
        assert_eq!(r.items[0].insert_text, "Regions r");
    }

    #[test]
    fn join_suggests_fk_join() {
        let r = run("SELECT * FROM dbo.Orders o JOIN |");
        assert_eq!(r.items[0].kind, CompletionKind::Join);
        assert_eq!(
            r.items[0].insert_text,
            "dbo.Customers c ON c.Id = o.CustomerId"
        );
    }

    #[test]
    fn on_suggests_condition() {
        let r = run("SELECT * FROM dbo.Orders o JOIN dbo.Customers c ON |");
        assert_eq!(r.items[0].label, "c.Id = o.CustomerId");
    }

    #[test]
    fn columns_from_all_tables_qualify_duplicates() {
        let r = run("SELECT | FROM dbo.Orders o JOIN dbo.Customers c ON c.Id = o.CustomerId");
        let mut id: Vec<_> = r
            .items
            .iter()
            .filter(|i| i.label == "Id")
            .map(|i| i.insert_text.clone())
            .collect();
        id.sort();
        assert_eq!(id, vec!["c.Id", "o.Id"]);
        assert!(
            r.items
                .iter()
                .any(|i| i.label == "Total" && i.insert_text == "Total")
        );
    }

    #[test]
    fn star_expansion() {
        let r = run("SELECT *| FROM dbo.Customers");
        assert_eq!(r.items[0].kind, CompletionKind::Expansion);
        assert_eq!(r.items[0].insert_text, "Id, Name");
        assert_eq!(r.items[0].replace_start, Some(7));
        let r = run("SELECT c.*| FROM dbo.Customers c");
        assert_eq!(r.items[0].insert_text, "c.Id, c.Name");
        assert_eq!(r.items[0].replace_start, Some(7));
    }

    #[test]
    fn insert_column_list() {
        let r = run("INSERT INTO dbo.Orders (|");
        assert_eq!(labels(&r), vec!["CustomerId", "Id", "Total"]);
    }

    #[test]
    fn exec_inserts_params() {
        let r = run("EXEC Get|");
        assert_eq!(
            r.items[0].insert_text,
            "dbo.GetOrders @CustomerId = ${1:@CustomerId}"
        );
    }

    #[test]
    fn variables() {
        let r = run("DECLARE @count INT; SELECT @c|");
        assert_eq!(r.items[0].label, "@count");
        assert_eq!(r.items[0].detail, "INT");
    }

    #[test]
    fn statement_start_offers_snippets_and_keywords() {
        let r = run("ss|");
        assert_eq!(r.items[0].label, "ssf");
        let r = run("sel|");
        assert_eq!(r.items[0].label, "SELECT");
    }

    #[test]
    fn nothing_inside_strings_or_comments() {
        assert!(run("SELECT 'ab|c' FROM t").items.is_empty());
        assert!(run("-- comm|ent").items.is_empty());
    }

    #[test]
    fn cte_columns() {
        let r = run("WITH x AS (SELECT Id, Total FROM dbo.Orders) SELECT x.| FROM x");
        assert_eq!(labels(&r), vec!["Id", "Total"]);
    }

    #[test]
    fn temp_table_columns() {
        let r = run("CREATE TABLE #t (A INT, B INT);\nSELECT t.| FROM #t t");
        assert_eq!(labels(&r), vec!["A", "B"]);
    }

    #[test]
    fn alias_generation() {
        let taken = HashSet::from(["o".to_string()]);
        assert_eq!(make_alias("OrderDetails", &taken), "od");
        assert_eq!(make_alias("orders", &taken), "o2");
        assert_eq!(make_alias("order_items", &taken), "oi");
        assert_eq!(make_alias("#tmp", &taken), "t");
    }
}
