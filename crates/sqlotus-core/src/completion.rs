//! Context-aware completion: what to suggest at a cursor position.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::catalog::{Catalog, DbObject, ObjectKind};
use crate::format::KeywordCase;
use crate::lexer::{Token, TokenKind, tokenize};
use crate::scope::{DocSymbols, Scope, TableRef, analyze_statement, document_symbols};
use crate::script;
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
    /// Values for snippet variables `$USER$`, `$MACHINE$`, `$DBNAME$`, `$SERVER$`.
    pub snippet_vars: std::collections::HashMap<String, String>,
    /// Databases on the server, offered as `Db.` qualifiers.
    #[serde(skip)]
    pub databases: Vec<String>,
}

impl Default for CompletionOptions {
    fn default() -> Self {
        Self {
            keyword_case: KeywordCase::Upper,
            qualify_objects: true,
            assign_aliases: true,
            max_items: 400,
            snippet_vars: Default::default(),
            databases: Vec::new(),
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
        /// `INSERT INTO |`: tables expand to a full INSERT statement.
        insert: bool,
    },
    /// `GROUP BY |`: offer the non-aggregated SELECT items.
    GroupBy,
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
    /// Catalogs of other databases on the same server (`Db.schema.object`).
    others: &'a [&'a Catalog],
    opts: &'a CompletionOptions,
    scope: Scope,
    doc: DocSymbols,
    /// Significant tokens of the current statement before the cursor word.
    before: Vec<Token<'a>>,
    /// The word being typed.
    prefix: String,
    replace_start: usize,
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
    complete_with_others(src, offset, dialect, catalog, &[], snippets, opts)
}

/// [`complete`] with the catalogs of other databases on the server, used for
/// three-part names (`OtherDb.dbo.Table`).
pub fn complete_with_others(
    src: &str,
    offset: usize,
    dialect: Dialect,
    catalog: Option<&Catalog>,
    others: &[&Catalog],
    snippets: &[Snippet],
    opts: &CompletionOptions,
) -> CompletionResult {
    let offset = floor_char_boundary(src, offset.min(src.len()));
    let tokens = tokenize(src, dialect);
    let current = tokens.iter().find(|t| t.start < offset && offset <= t.end());

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
        Some(t) if matches!(t.kind, TokenKind::Word | TokenKind::QuotedIdent | TokenKind::Variable) => {
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
    let before = if before.last().is_some_and(|t| t.kind == TokenKind::Semicolon) {
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
        others,
        opts,
        scope,
        doc: document_symbols(src, dialect),
        before,
        prefix: prefix.to_string(),
        replace_start,
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
                Context::Relation { alias, join, insert } => cx.relations(alias, join, insert),
                Context::GroupBy => {
                    cx.group_by();
                    cx.general(snippets);
                }
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
                        insert: false,
                    };
                }
                "JOIN" => {
                    return Context::Relation {
                        alias: true,
                        join: true,
                        insert: false,
                    };
                }
                "INSERT" | "INTO" => {
                    let insert = last.upper() == "INSERT" || self.last_upper(1) == "INSERT";
                    return Context::Relation {
                        alias: false,
                        join: false,
                        insert,
                    };
                }
                "UPDATE" | "TABLE" | "USING" | "TRUNCATE" => {
                    return Context::Relation {
                        alias: false,
                        join: false,
                        insert: false,
                    };
                }
                "BY" if self.last_upper(1) == "GROUP" => return Context::GroupBy,
                "EXEC" | "EXECUTE" | "CALL" => return Context::Procedure,
                "ON" if self.after_join() => return Context::JoinCondition,
                _ => {}
            }
        }
        if matches!(last.kind, TokenKind::LParen | TokenKind::Comma) && self.in_insert_column_list() {
            return Context::InsertColumns;
        }
        if last.kind == TokenKind::Comma && self.clause_keyword() == "FROM" {
            return Context::Relation {
                alias: true,
                join: false,
                insert: false,
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
            ..Default::default()
        })
    }

    // -- providers ----------------------------------------------------------

    fn keywords(&mut self, statement_start: bool) {
        const STARTS: &[&str] = &[
            "SELECT", "INSERT", "UPDATE", "DELETE", "WITH", "CREATE", "ALTER", "DROP", "DECLARE", "SET", "EXEC",
            "BEGIN", "IF", "MERGE", "TRUNCATE",
        ];
        let words = keywords::completion_keywords(self.dialect);
        for w in words {
            let rank = if statement_start && STARTS.contains(&w) { 2 } else { 7 };
            let k = self.kw(w);
            self.push(k.clone(), CompletionKind::Keyword, "keyword", k, rank);
        }
        for phrase in KEYWORD_PHRASES {
            let k = self.kw(phrase);
            self.push(k.clone(), CompletionKind::Keyword, "keyword", k, 6);
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
        let vars = self.opts.snippet_vars.clone();
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
            // the word being typed right after a table looks like its alias
            if v.is_empty() || v.eq_ignore_ascii_case(&self.prefix) {
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

    /// The catalog of `database` (`None` or the current one: the main catalog).
    fn catalog_for(&self, database: Option<&str>) -> Option<&'a Catalog> {
        match database {
            None => self.catalog,
            Some(db) if self.catalog.is_some_and(|c| c.database.eq_ignore_ascii_case(db)) => self.catalog,
            Some(db) => self
                .others
                .iter()
                .copied()
                .find(|c| c.database.eq_ignore_ascii_case(db)),
        }
    }

    fn is_other_database(&self, name: &str) -> bool {
        self.catalog.is_none_or(|c| !c.database.eq_ignore_ascii_case(name))
            && (self.opts.databases.iter().any(|d| d.eq_ignore_ascii_case(name))
                || self.others.iter().any(|c| c.database.eq_ignore_ascii_case(name)))
    }

    fn table_columns(&self, t: &TableRef) -> Vec<(String, String)> {
        if !t.columns.is_empty() {
            return t.columns.iter().map(|c| (c.clone(), String::new())).collect();
        }
        if let Some(local) = self
            .doc
            .local_tables
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(&t.name))
        {
            return local.columns.iter().map(|c| (c.clone(), String::new())).collect();
        }
        if let Some(c) = self.scope.ctes.iter().find(|c| c.name.eq_ignore_ascii_case(&t.name)) {
            return c.columns.iter().map(|c| (c.clone(), String::new())).collect();
        }
        self.catalog_for(t.database.as_deref())
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
        // Qualifying with the default schema is a SQL Server convention
        // (`dbo.`); in Postgres/MySQL it is just noise (`public.`, `mydb.`).
        let use_schema = force_schema
            || (self.opts.qualify_objects && self.dialect == Dialect::MsSql)
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

    /// Item for a table after `INSERT INTO`: the whole column list and a
    /// commented VALUES block, as SQL Prompt does.
    fn insert_item(&mut self, o: &DbObject, name: String, rank: u8) {
        let insertable = o.columns.iter().any(|c| c.is_insertable());
        let insert_text = if insertable {
            script::insert_tail(o, &name, self.dialect, 4, true)
        } else {
            crate::snippet::escape_snippet(&name)
        };
        self.items.push(CompletionItem {
            sort_text: format!("{rank}{}", o.name.to_lowercase()),
            label: o.name.clone(),
            kind: Self::object_kind(o.kind),
            detail: if insertable {
                format!("{}.{} \u{2014} insert all columns", o.schema, o.name)
            } else {
                format!("{}.{}", o.schema, o.name)
            },
            insert_text,
            is_snippet: true,
            replace_start: None,
            filter_text: None,
        });
    }

    /// Column of the replaced text relative to its line's indentation.
    fn insert_column(&self) -> usize {
        let line_start = self.src[..self.replace_start].rfind('\n').map_or(0, |i| i + 1);
        let prefix = &self.src[line_start..self.replace_start];
        let indent = prefix.chars().take_while(|c| c.is_whitespace()).count();
        prefix.chars().count() - indent
    }

    fn exec_item(&mut self, o: &DbObject, name: String) {
        let col = self.insert_column();
        self.items.push(CompletionItem {
            sort_text: format!("0{}", o.name.to_lowercase()),
            label: o.name.clone(),
            kind: CompletionKind::Procedure,
            detail: format!("{}.{}", o.schema, o.name),
            insert_text: script::exec_tail(o, &name, self.dialect, col, true),
            is_snippet: true,
            replace_start: None,
            filter_text: None,
        });
    }

    /// `GROUP BY |`: the SELECT items that are not aggregates, as one item.
    fn group_by(&mut self) {
        let items = group_by_items(&self.before, self.src);
        if items.is_empty() {
            return;
        }
        let text = items.join(", ");
        self.items.push(CompletionItem {
            sort_text: "0".into(),
            label: text.clone(),
            kind: CompletionKind::Expansion,
            detail: "non-aggregated columns".into(),
            insert_text: text,
            is_snippet: false,
            replace_start: None,
            filter_text: None,
        });
    }

    fn relations(&mut self, alias: bool, join: bool, insert: bool) {
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
                if insert {
                    let name = self.qualified_name(o, false);
                    let rank = if o.kind == ObjectKind::Table { 2 } else { 3 };
                    self.insert_item(o, name, rank);
                    continue;
                }
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
            for db in &self.opts.databases {
                if !db.eq_ignore_ascii_case(&cat.database) {
                    self.push(db.clone(), CompletionKind::Schema, "database", self.quote(db), 5);
                }
            }
            for s in &cat.schemas {
                self.push(s.clone(), CompletionKind::Schema, "schema", self.quote(s), 4);
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
            .filter_map(|t| cat.find(t.schema.as_deref(), &t.name).map(|o| (t.clone(), o)))
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
                    let insert = format!("{} {alias} ON {cond}", self.qualified_name(target, false));
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
                let cond = join_condition(newest.visible_name(), ncols, r.visible_name(), rcols, self.dialect);
                self.push(cond.clone(), CompletionKind::Join, format!("FK {}", fk.name), cond, 0);
            }
        }
    }

    fn procedures(&mut self) {
        let Some(cat) = self.catalog else { return };
        for o in cat.objects.iter().filter(|o| o.kind == ObjectKind::Procedure) {
            let name = self.qualified_name(o, false);
            self.exec_item(o, name);
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
        let insert_ctx =
            kw_before == "INSERT" || (kw_before == "INTO" && j >= 2 && self.before[j - 2].upper() == "INSERT");

        match parts.as_slice() {
            [q] => {
                if !relation_ctx
                    && !proc_ctx
                    && let Some(t) = self.scope.tables.iter().find(|t| t.matches_qualifier(q)).cloned()
                {
                    self.columns(Some(&t));
                    return;
                }
                // `OtherDb.` -> its schemas
                if self.is_other_database(q) {
                    if let Some(other) = self.catalog_for(Some(q)) {
                        for sch in &other.schemas {
                            self.push(
                                sch.clone(),
                                CompletionKind::Schema,
                                format!("schema in {q}"),
                                self.quote(sch),
                                0,
                            );
                        }
                    }
                    return;
                }
                if let Some(cat) = self.catalog {
                    if cat.is_schema(q) {
                        self.schema_objects(cat, q, relation_ctx, proc_ctx, insert_ctx);
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
                            ..Default::default()
                        };
                        self.columns(Some(&t));
                    }
                }
            }
            [a, b] => {
                if let Some(cat) = self.catalog_for(Some(a))
                    && cat.is_schema(b)
                    && (cat.database.eq_ignore_ascii_case(a) || self.is_other_database(a))
                {
                    self.schema_objects(cat, b, relation_ctx, proc_ctx, insert_ctx);
                    return;
                }
                let t = TableRef {
                    schema: Some(a.clone()),
                    name: b.clone(),
                    alias: None,
                    columns: Vec::new(),
                    local: false,
                    start: 0,
                    ..Default::default()
                };
                self.columns(Some(&t));
            }
            [db, s, t] => {
                let t = TableRef {
                    database: Some(db.clone()),
                    schema: Some(s.clone()),
                    name: t.clone(),
                    alias: None,
                    columns: Vec::new(),
                    local: false,
                    start: 0,
                    ..Default::default()
                };
                self.columns(Some(&t));
            }
            _ => {}
        }
    }

    fn schema_objects(&mut self, cat: &'a Catalog, schema: &str, relation_ctx: bool, proc_ctx: bool, insert_ctx: bool) {
        let taken = self.existing_aliases();
        let alias =
            relation_ctx && self.opts.assign_aliases && self.last_upper(2) != "UPDATE" && self.last_upper(2) != "INTO";
        for o in cat.objects.iter().filter(|o| o.schema.eq_ignore_ascii_case(schema)) {
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
            if insert_ctx && o.kind.is_relation() {
                self.insert_item(o, self.quote(&o.name), 0);
                continue;
            }
            if proc_ctx {
                self.exec_item(o, self.quote(&o.name));
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
        let qualified = n >= 3 && self.before[n - 2].kind == TokenKind::Dot && self.before[n - 3].is_name();
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

const KEYWORD_PHRASES: &[&str] = &[
    "INNER JOIN",
    "LEFT JOIN",
    "LEFT OUTER JOIN",
    "RIGHT JOIN",
    "FULL OUTER JOIN",
    "CROSS JOIN",
    "CROSS APPLY",
    "OUTER APPLY",
    "GROUP BY",
    "ORDER BY",
    "PARTITION BY",
    "UNION ALL",
    "IS NULL",
    "IS NOT NULL",
    "NOT EXISTS",
    "NOT IN",
    "INSERT INTO",
    "DELETE FROM",
    "BEGIN TRANSACTION",
    "COMMIT TRANSACTION",
    "ROLLBACK TRANSACTION",
    "BEGIN TRY",
    "BEGIN CATCH",
    "SET NOCOUNT ON",
];

const AGGREGATES: &[&str] = &[
    "COUNT",
    "COUNT_BIG",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "STRING_AGG",
    "ARRAY_AGG",
    "GROUP_CONCAT",
    "JSON_AGG",
    "JSONB_AGG",
    "STDEV",
    "STDEVP",
    "STDDEV",
    "VAR",
    "VARP",
    "VARIANCE",
    "LISTAGG",
    "BOOL_AND",
    "BOOL_OR",
    "CHECKSUM_AGG",
    "GROUPING",
];

/// SELECT items of the query before `GROUP BY` that are not aggregates,
/// without their aliases.
fn group_by_items(before: &[Token<'_>], src: &str) -> Vec<String> {
    // `before` ends with GROUP BY; find the SELECT of this query level
    let end = before.len().saturating_sub(2);
    let mut depth = 0i32;
    let mut select = None;
    let mut from = None;
    for i in (0..end).rev() {
        match before[i].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => depth -= 1,
            TokenKind::Word if depth == 0 => match before[i].upper().as_str() {
                "FROM" => from = Some(i),
                "SELECT" => {
                    select = Some(i);
                    break;
                }
                _ => {}
            },
            _ => {}
        }
        if depth < 0 {
            break;
        }
    }
    let (Some(sel), Some(from)) = (select, from) else {
        return Vec::new();
    };
    let mut i = sel + 1;
    while i < from && matches!(before[i].upper().as_str(), "DISTINCT" | "ALL") {
        i += 1;
    }
    if i < from && before[i].upper() == "TOP" {
        i += 1;
        if before.get(i).is_some_and(|t| t.kind == TokenKind::LParen) {
            while i < from && before[i].kind != TokenKind::RParen {
                i += 1;
            }
        }
        i += 1;
    }
    let mut items = Vec::new();
    let mut current: Vec<Token<'_>> = Vec::new();
    let mut depth = 0i32;
    for t in &before[i.min(from)..from] {
        match t.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            TokenKind::Comma if depth == 0 => {
                items.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(*t);
    }
    items.push(current);
    items
        .into_iter()
        .filter_map(|item| {
            let is_aggregate = item
                .windows(2)
                .any(|w| w[1].kind == TokenKind::LParen && AGGREGATES.contains(&w[0].upper().as_str()));
            if item.is_empty() || is_aggregate {
                return None;
            }
            let mut toks: &[Token<'_>] = &item;
            // `alias = expr` (T-SQL)
            if toks.len() > 2 && toks[0].is_name() && toks[1].text == "=" {
                toks = &toks[2..];
            }
            if let Some(p) = toks.iter().rposition(|t| t.upper() == "AS") {
                // `expr AS alias`
                toks = &toks[..p];
            } else if toks.len() >= 2
                && toks[toks.len() - 1].is_name()
                && (toks[toks.len() - 2].is_name() || toks[toks.len() - 2].kind == TokenKind::RParen)
            {
                // `expr alias`
                toks = &toks[..toks.len() - 1];
            }
            let (first, last) = (toks.first()?, toks.last()?);
            if toks.len() == 1 && matches!(first.kind, TokenKind::Number | TokenKind::String) {
                return None;
            }
            if toks.iter().any(|t| t.text == "*") {
                return None;
            }
            Some(src[first.start..last.end()].to_string())
        })
        .collect()
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
            Some(p) => (c.is_uppercase() && p.is_lowercase()) || (p == '_' || p == ' ') && c.is_alphanumeric(),
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
            let target = it.filter_text.as_deref().unwrap_or(&it.label).to_lowercase();
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
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.sort_text.cmp(&b.1.sort_text)));
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
            is_computed: false,
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
                        Column {
                            is_identity: true,
                            is_primary_key: true,
                            ..col("Id", "int")
                        },
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
        assert_eq!(r.items[0].insert_text, "dbo.Customers c ON c.Id = o.CustomerId");
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
        assert!(r.items.iter().any(|i| i.label == "Total" && i.insert_text == "Total"));
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
        let t = &r.items[0].insert_text;
        assert!(t.starts_with("dbo.GetOrders @CustomerId = ${1:0}"), "{t}");
        assert!(t.ends_with("-- int"), "{t}");
    }

    #[test]
    fn insert_into_expands_columns() {
        let r = run("INSERT INTO dbo.Ord|");
        let t = &r.items[0].insert_text;
        assert!(
            t.starts_with(
                "Orders
(
    CustomerId,
    Total
)
VALUES
(   ${1:NULL}, -- CustomerId - int"
            ),
            "{t}"
        );
        let r = run("INSERT INTO Ord|");
        assert!(
            r.items[0].insert_text.starts_with(
                "dbo.Orders
(
"
            ),
            "{}",
            r.items[0].insert_text
        );
        // SELECT ... INTO is not an INSERT
        let r = run("SELECT * INTO Ord|");
        assert!(!r.items.iter().any(|i| i.insert_text.contains("VALUES")));
    }

    #[test]
    fn group_by_suggests_non_aggregates() {
        let r = run(
            "SELECT o.CustomerId, YEAR(o.Total) AS y, COUNT(*) AS n, SUM(o.Total) total FROM dbo.Orders o GROUP BY |",
        );
        assert_eq!(r.items[0].insert_text, "o.CustomerId, YEAR(o.Total)");
    }

    #[test]
    fn keyword_phrases() {
        let r = run("SELECT * FROM a inn|");
        assert_eq!(r.items[0].label, "INNER JOIN");
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
    fn postgres_does_not_qualify_default_schema() {
        let mut c = cat();
        c.default_schema = "dbo".into();
        let src = "SELECT * FROM Cus";
        let r = complete(
            src,
            src.len(),
            Dialect::Postgres,
            Some(&c),
            &[],
            &CompletionOptions::default(),
        );
        assert_eq!(r.items[0].insert_text, "\"Customers\" c");
        let src = "SELECT * FROM Reg";
        let r = complete(
            src,
            src.len(),
            Dialect::Postgres,
            Some(&c),
            &[],
            &CompletionOptions::default(),
        );
        assert_eq!(r.items[0].insert_text, "sales.\"Regions\" r");
    }

    #[test]
    fn other_database_objects_and_columns() {
        let mut other = cat();
        other.database = "Archive".into();
        other.schemas = vec!["dbo".into(), "old".into()];
        let main = cat();
        let opts = CompletionOptions {
            databases: vec!["Shop".into(), "Archive".into()],
            ..Default::default()
        };
        let run = |src: &str| {
            let offset = src.find('|').unwrap();
            let src = src.replacen('|', "", 1);
            complete_with_others(&src, offset, Dialect::MsSql, Some(&main), &[&other], &[], &opts)
        };
        let r = run("SELECT * FROM Archive.|");
        let labels: Vec<_> = r.items.iter().map(|i| i.label.as_str()).collect();
        assert_eq!(labels, vec!["dbo", "old"]);
        let r = run("SELECT * FROM Archive.dbo.Ord|");
        assert_eq!(r.items[0].label, "Orders");
        let r = run("SELECT a.| FROM Archive.dbo.Orders a");
        assert!(r.items.iter().any(|i| i.label == "CustomerId"));
        let r = run("SELECT * FROM Arch|");
        assert!(r.items.iter().any(|i| i.label == "Archive" && i.detail == "database"));
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
