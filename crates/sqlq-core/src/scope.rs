//! Lightweight analysis of which tables, aliases, CTEs, temp tables and
//! variables are visible at a point in a script. Works on tokens only, so it
//! copes with incomplete statements.

use crate::Dialect;
use crate::keywords;
use crate::lexer::{Token, TokenKind, tokenize};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TableRef {
    /// Database part of a three-part name.
    pub database: Option<String>,
    pub schema: Option<String>,
    pub name: String,
    pub alias: Option<String>,
    /// Columns known from the query text (derived tables, CTEs, temp
    /// tables). Empty means "look the table up in the catalog".
    pub columns: Vec<String>,
    pub local: bool,
    /// Byte offset of the reference in the source.
    pub start: usize,
    /// Byte range of the object name token (the last name part).
    pub name_start: usize,
    pub name_end: usize,
    /// Followed by `(`: a table-valued function call.
    pub is_call: bool,
    /// Keyword that introduced the reference (`FROM`, `JOIN`, `INTO`,
    /// `UPDATE`, ...), or `,` for comma-separated FROM lists.
    pub via: String,
}

impl TableRef {
    /// The name used to qualify columns: alias if any, else the table name.
    pub fn visible_name(&self) -> &str {
        self.alias.as_deref().unwrap_or(&self.name)
    }

    pub fn matches_qualifier(&self, q: &str) -> bool {
        match &self.alias {
            Some(a) => a.eq_ignore_ascii_case(q),
            None => self.name.eq_ignore_ascii_case(q),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalTable {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    pub data_type: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    pub tables: Vec<TableRef>,
    pub ctes: Vec<LocalTable>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocSymbols {
    pub variables: Vec<Variable>,
    pub local_tables: Vec<LocalTable>,
}

fn significant<'a>(tokens: &[Token<'a>]) -> Vec<Token<'a>> {
    tokens.iter().filter(|t| !t.is_trivia()).copied().collect()
}

fn is_word(t: Option<&Token<'_>>, w: &str) -> bool {
    t.is_some_and(|t| t.kind == TokenKind::Word && t.text.eq_ignore_ascii_case(w))
}

/// Index of the `)` matching the `(` at `open`, or the end.
fn matching_paren(t: &[Token<'_>], open: usize) -> usize {
    let mut depth = 0i32;
    for (i, tok) in t.iter().enumerate().skip(open) {
        match tok.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    t.len()
}

fn is_alias_candidate(t: &Token<'_>, dialect: Dialect) -> bool {
    match t.kind {
        TokenKind::QuotedIdent => true,
        TokenKind::Word => {
            !keywords::is_reserved(&t.upper(), dialect) && !is_clause_word(&t.upper())
        }
        _ => false,
    }
}

/// Words that follow a table reference and must not be taken as its alias,
/// even in dialects where they are not reserved.
fn is_clause_word(u: &str) -> bool {
    matches!(
        u,
        "WHERE"
            | "JOIN"
            | "INNER"
            | "LEFT"
            | "RIGHT"
            | "FULL"
            | "CROSS"
            | "OUTER"
            | "ON"
            | "GROUP"
            | "ORDER"
            | "HAVING"
            | "UNION"
            | "LIMIT"
            | "OFFSET"
            | "SET"
            | "USING"
            | "WINDOW"
            | "NATURAL"
            | "OPTION"
            | "FOR"
            | "WITH"
            | "RETURNING"
            | "OUTPUT"
            | "VALUES"
            | "SELECT"
            | "FETCH"
            | "PIVOT"
            | "UNPIVOT"
            | "APPLY"
            | "LATERAL"
            | "STRAIGHT_JOIN"
            | "WHEN"
            | "AS"
    )
}

/// Analyze one statement's tokens.
pub fn analyze_statement(tokens: &[Token<'_>], dialect: Dialect) -> Scope {
    let t = significant(tokens);
    let mut scope = Scope::default();
    analyze_sig(&t, dialect, &mut scope);
    scope.tables.sort_by_key(|r| r.start);
    // CTE references resolve to the CTE's columns
    for r in &mut scope.tables {
        if r.schema.is_none() && r.columns.is_empty() {
            if let Some(c) = scope
                .ctes
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&r.name))
            {
                r.columns = c.columns.clone();
                r.local = true;
            }
        }
    }
    scope
}

/// Walk significant tokens collecting table references. Subqueries are
/// descended into, so tables of inner queries are visible too (good enough
/// for completion, which cannot tell scopes apart in broken SQL anyway).
fn analyze_sig(t: &[Token<'_>], dialect: Dialect, scope: &mut Scope) {
    let mut depth = 0i32;
    let mut from_depths: Vec<i32> = Vec::new();
    let mut i = 0;
    while i < t.len() {
        let tok = &t[i];
        match tok.kind {
            TokenKind::LParen => {
                depth += 1;
                i += 1;
                continue;
            }
            TokenKind::RParen => {
                from_depths.retain(|d| *d != depth);
                depth -= 1;
                i += 1;
                continue;
            }
            TokenKind::Comma if from_depths.contains(&depth) => {
                i = table_refs(t, i + 1, dialect, scope, ",");
                continue;
            }
            TokenKind::Word => {}
            _ => {
                i += 1;
                continue;
            }
        }
        let upper = tok.upper();
        match upper.as_str() {
            "WITH" if t.get(i + 1).is_some_and(|n| n.kind != TokenKind::LParen) => {
                i = parse_ctes(t, i + 1, dialect, scope);
            }
            "FROM" | "JOIN" | "APPLY" | "UPDATE" | "INTO" | "USING" | "STRAIGHT_JOIN" => {
                if upper == "FROM" && !from_depths.contains(&depth) {
                    from_depths.push(depth);
                }
                let via = if upper == "INTO"
                    && i > 0
                    && t[i - 1].upper() != "INSERT"
                    && t[i - 1].upper() != "MERGE"
                {
                    // SELECT ... INTO creates the table
                    "SELECT INTO".to_string()
                } else {
                    upper.clone()
                };
                i = table_refs(t, i + 1, dialect, scope, &via).max(i + 1);
            }
            "WHERE" | "GROUP" | "ORDER" | "HAVING" | "SELECT" | "UNION" | "EXCEPT"
            | "INTERSECT" | "SET" | "LIMIT" | "WINDOW" | "OPTION" | "VALUES" => {
                from_depths.retain(|d| *d != depth);
                i += 1;
            }
            _ => i += 1,
        }
    }
}

/// Parse one table reference at `k`; derived tables are also analyzed.
fn table_refs(t: &[Token<'_>], k: usize, dialect: Dialect, scope: &mut Scope, via: &str) -> usize {
    let Some((mut r, next)) = parse_table_ref(t, k, dialect) else {
        return k;
    };
    r.via = via.to_string();
    if t.get(k).is_some_and(|x| x.kind == TokenKind::LParen) {
        let close = matching_paren(t, k).min(t.len());
        analyze_sig(&t[k + 1..close], dialect, scope);
    }
    scope.tables.push(r);
    next
}

fn parse_ctes(t: &[Token<'_>], mut i: usize, dialect: Dialect, scope: &mut Scope) -> usize {
    if is_word(t.get(i), "RECURSIVE") {
        i += 1;
    }
    loop {
        let Some(name_tok) = t.get(i).filter(|n| n.is_name()) else {
            return i;
        };
        let name = name_tok.ident_value();
        i += 1;
        let mut columns = Vec::new();
        if t.get(i).is_some_and(|n| n.kind == TokenKind::LParen) {
            let close = matching_paren(t, i);
            columns = t[i + 1..close.min(t.len())]
                .iter()
                .filter(|x| x.is_name())
                .map(|x| x.ident_value())
                .collect();
            i = close + 1;
        }
        if !is_word(t.get(i), "AS") {
            scope.ctes.push(LocalTable { name, columns });
            return i;
        }
        i += 1;
        if is_word(t.get(i), "MATERIALIZED") || is_word(t.get(i), "NOT") {
            i += if is_word(t.get(i), "NOT") { 2 } else { 1 };
        }
        if t.get(i).is_some_and(|n| n.kind == TokenKind::LParen) {
            let close = matching_paren(t, i);
            if columns.is_empty() {
                columns = select_list_names(&t[i + 1..close.min(t.len())], dialect);
            }
            analyze_sig(&t[i + 1..close.min(t.len())], dialect, scope);
            i = close + 1;
        }
        scope.ctes.push(LocalTable { name, columns });
        if t.get(i).is_some_and(|n| n.kind == TokenKind::Comma) {
            i += 1;
            continue;
        }
        return i;
    }
}

/// Parse `[schema.]name [AS] [alias]` or `( subquery ) [AS] alias` at `i`.
fn parse_table_ref(t: &[Token<'_>], mut i: usize, dialect: Dialect) -> Option<(TableRef, usize)> {
    let first = t.get(i)?;
    let start = first.start;
    let (mut name_start, mut name_end) = (first.start, first.end());
    let mut is_call = false;
    let mut database = None;
    let (schema, name, columns, local) = match first.kind {
        TokenKind::LParen => {
            let close = matching_paren(t, i);
            let cols = select_list_names(&t[i + 1..close.min(t.len())], dialect);
            i = close + 1;
            (None, String::new(), cols, true)
        }
        TokenKind::Word | TokenKind::QuotedIdent | TokenKind::Variable => {
            if first.kind == TokenKind::Word && keywords::is_reserved(&first.upper(), dialect) {
                return None;
            }
            let mut parts = vec![first.ident_value()];
            i += 1;
            while t.get(i).is_some_and(|d| d.kind == TokenKind::Dot) {
                match t.get(i + 1).filter(|n| n.is_name()) {
                    Some(n) => {
                        parts.push(n.ident_value());
                        (name_start, name_end) = (n.start, n.end());
                        i += 2;
                    }
                    None => {
                        // `dbo.` while typing: keep the qualifier, empty name
                        parts.push(String::new());
                        i += 1;
                        break;
                    }
                }
            }
            // table-valued function arguments
            if t.get(i).is_some_and(|n| n.kind == TokenKind::LParen) {
                i = matching_paren(t, i) + 1;
                is_call = true;
            }
            let name = parts.pop().unwrap_or_default();
            let schema = parts.pop();
            database = parts.pop();
            let local = name.starts_with('#') || name.starts_with('@');
            (schema, name, Vec::new(), local)
        }
        _ => return None,
    };
    let mut alias = None;
    if is_word(t.get(i), "AS") {
        if let Some(a) = t.get(i + 1).filter(|n| n.is_name()) {
            alias = Some(a.ident_value());
            i += 2;
        } else {
            i += 1;
        }
    } else if let Some(a) = t.get(i).filter(|n| is_alias_candidate(n, dialect)) {
        alias = Some(a.ident_value());
        i += 1;
    }
    let name = if name.is_empty()
        && alias.is_some()
        && schema.is_none()
        && first.kind == TokenKind::LParen
    {
        alias.clone().unwrap_or_default()
    } else {
        name
    };
    Some((
        TableRef {
            database,
            name_start,
            name_end,
            is_call,
            via: String::new(),
            schema,
            name,
            alias,
            columns,
            local,
            start,
        },
        i,
    ))
}

/// Output column names of the first SELECT list in `t` (best effort).
pub fn select_list_names(t: &[Token<'_>], dialect: Dialect) -> Vec<String> {
    let t: Vec<Token<'_>> = t.iter().filter(|x| !x.is_trivia()).copied().collect();
    let Some(sel) = t.iter().position(|x| is_word(Some(x), "SELECT")) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    let mut item: Vec<Token<'_>> = Vec::new();
    let mut depth = 0i32;
    let mut i = sel + 1;
    // skip DISTINCT / TOP n
    while let Some(x) = t.get(i) {
        let u = x.upper();
        if matches!(u.as_str(), "DISTINCT" | "ALL") {
            i += 1;
        } else if u == "TOP" {
            i += 1;
            if t.get(i).is_some_and(|n| n.kind == TokenKind::LParen) {
                i = matching_paren(&t, i) + 1;
            } else {
                i += 1;
            }
        } else {
            break;
        }
    }
    let mut finish = |item: &mut Vec<Token<'_>>| {
        if let Some(n) = item_name(item, dialect) {
            names.push(n);
        }
        item.clear();
    };
    while let Some(x) = t.get(i) {
        match x.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            TokenKind::Comma if depth == 0 => {
                finish(&mut item);
                i += 1;
                continue;
            }
            TokenKind::Word if depth == 0 => {
                let u = x.upper();
                if matches!(
                    u.as_str(),
                    "FROM"
                        | "INTO"
                        | "WHERE"
                        | "UNION"
                        | "GROUP"
                        | "ORDER"
                        | "EXCEPT"
                        | "INTERSECT"
                ) {
                    break;
                }
            }
            _ => {}
        }
        if depth < 0 {
            break;
        }
        item.push(*x);
        i += 1;
    }
    finish(&mut item);
    names
}

fn item_name(item: &[Token<'_>], dialect: Dialect) -> Option<String> {
    let last = item.last()?;
    // T-SQL `alias = expr`
    if item.len() > 2 && item[0].is_name() && item[1].text == "=" {
        return Some(item[0].ident_value());
    }
    if !last.is_name()
        || (last.kind == TokenKind::Word && keywords::is_reserved(&last.upper(), dialect))
    {
        return None;
    }
    Some(last.ident_value())
}

/// Variables and local tables declared anywhere in the document.
pub fn document_symbols(src: &str, dialect: Dialect) -> DocSymbols {
    let tokens = tokenize(src, dialect);
    let t = significant(&tokens);
    let mut syms = DocSymbols::default();
    for (i, tok) in t.iter().enumerate() {
        if tok.kind == TokenKind::Variable && !tok.text.starts_with("@@") {
            let ty = t
                .get(i + 1)
                .filter(|n| n.kind == TokenKind::Word && keywords::is_data_type(&n.upper()))
                .map(|n| n.text.to_string());
            match syms
                .variables
                .iter_mut()
                .find(|v| v.name.eq_ignore_ascii_case(tok.text))
            {
                Some(v) if v.data_type.is_none() => v.data_type = ty,
                Some(_) => {}
                None => syms.variables.push(Variable {
                    name: tok.text.to_string(),
                    data_type: ty,
                }),
            }
        }
        // CREATE TABLE #t (...)  /  DECLARE @t TABLE (...)
        let table_def =
            if is_word(Some(tok), "TABLE") && i > 0 && t[i - 1].kind == TokenKind::Variable {
                Some((t[i - 1].text.to_string(), i + 1))
            } else if is_word(Some(tok), "TABLE")
                && t.get(i + 1).is_some_and(|n| n.text.starts_with('#'))
                && t.get(i + 2).is_some_and(|n| n.kind == TokenKind::LParen)
            {
                Some((t[i + 1].text.to_string(), i + 2))
            } else {
                None
            };
        if let Some((name, open)) = table_def {
            if t.get(open).is_some_and(|n| n.kind == TokenKind::LParen) {
                let close = matching_paren(&t, open);
                let columns = column_defs(&t[open + 1..close.min(t.len())]);
                syms.local_tables.push(LocalTable { name, columns });
            }
        }
        // SELECT ... INTO #t
        if is_word(Some(tok), "INTO") && t.get(i + 1).is_some_and(|n| n.text.starts_with('#')) {
            let sel = t[..i].iter().rposition(|x| is_word(Some(x), "SELECT"));
            if let Some(sel) = sel {
                let columns = select_list_names(&t[sel..i], dialect);
                syms.local_tables.push(LocalTable {
                    name: t[i + 1].text.to_string(),
                    columns,
                });
            }
        }
    }
    syms
}

fn column_defs(t: &[Token<'_>]) -> Vec<String> {
    let mut cols = Vec::new();
    let mut depth = 0;
    let mut expect_name = true;
    for x in t {
        match x.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            TokenKind::Comma if depth == 0 => expect_name = true,
            _ if expect_name && depth == 0 => {
                expect_name = false;
                let u = x.upper();
                let constraint = matches!(
                    u.as_str(),
                    "CONSTRAINT" | "PRIMARY" | "FOREIGN" | "UNIQUE" | "CHECK" | "INDEX" | "KEY"
                );
                if x.is_name() && !constraint {
                    cols.push(x.ident_value());
                }
            }
            _ => {}
        }
    }
    cols
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(src: &str) -> Scope {
        analyze_statement(&tokenize(src, Dialect::MsSql), Dialect::MsSql)
    }

    #[test]
    fn tables_and_aliases() {
        let s = scope(
            "SELECT * FROM dbo.Orders o JOIN [Sales].[Customer] AS c ON c.Id = o.CustomerId, Items",
        );
        let got: Vec<_> = s
            .tables
            .iter()
            .map(|t| (t.schema.clone(), t.name.clone(), t.alias.clone()))
            .collect();
        assert_eq!(
            got,
            vec![
                (Some("dbo".into()), "Orders".into(), Some("o".into())),
                (Some("Sales".into()), "Customer".into(), Some("c".into())),
                (None, "Items".into(), None),
            ]
        );
    }

    #[test]
    fn alias_is_not_a_keyword() {
        let s = scope("SELECT * FROM t WHERE x = 1");
        assert_eq!(s.tables[0].alias, None);
    }

    #[test]
    fn derived_table_columns() {
        let s = scope("SELECT x. FROM (SELECT a, b AS bb, total = 1 + 2, t.c FROM t) x");
        assert_eq!(s.tables[0].name, "x");
        assert_eq!(s.tables[0].columns, vec!["a", "bb", "total", "c"]);
    }

    #[test]
    fn cte_columns_resolve() {
        let s = scope("WITH c AS (SELECT id, name FROM u) SELECT * FROM c JOIN d ON 1=1");
        assert_eq!(s.ctes[0].columns, vec!["id", "name"]);
        let c = s.tables.iter().find(|t| t.name == "c").unwrap();
        assert_eq!(c.columns, vec!["id", "name"]);
        assert!(
            s.tables
                .iter()
                .find(|t| t.name == "d")
                .unwrap()
                .columns
                .is_empty()
        );
    }

    #[test]
    fn update_and_insert_targets() {
        let s = scope("UPDATE o SET a = 1 FROM dbo.Orders o");
        assert_eq!(s.tables.len(), 2);
        let s = scope("SELECT * FROM a WHERE x IN (SELECT y FROM b), c");
        let names: Vec<_> = s.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
        let s = scope("INSERT INTO dbo.T (a) SELECT 1");
        assert_eq!(s.tables[0].name, "T");
    }

    #[test]
    fn doc_symbols() {
        let src = "DECLARE @id INT, @t TABLE (Id INT, Name NVARCHAR(10), PRIMARY KEY (Id));\nCREATE TABLE #tmp (A INT, B INT);\nSELECT x, y INTO #s FROM z";
        let d = document_symbols(src, Dialect::MsSql);
        assert_eq!(
            d.variables[0],
            Variable {
                name: "@id".into(),
                data_type: Some("INT".into())
            }
        );
        let tables: Vec<_> = d
            .local_tables
            .iter()
            .map(|t| (t.name.as_str(), t.columns.clone()))
            .collect();
        assert_eq!(
            tables,
            vec![
                ("@t", vec!["Id".to_string(), "Name".to_string()]),
                ("#tmp", vec!["A".to_string(), "B".to_string()]),
                ("#s", vec!["x".to_string(), "y".to_string()]),
            ]
        );
    }
}
