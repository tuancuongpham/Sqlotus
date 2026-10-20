//! Whole-document refactorings in the spirit of SQL Prompt's actions:
//! expand wildcards, qualify object names, insert semicolons, remove square
//! brackets, apply keyword casing. Each returns the new text; callers apply
//! it as one undoable edit.

use std::collections::HashSet;

use crate::catalog::{Catalog, ObjectKind};
use crate::format::KeywordCase;
use crate::lexer::{Token, TokenKind, tokenize, unquote};
use crate::scope::analyze_statement;
use crate::statements::split_statements;
use crate::{Dialect, keywords};

/// (start, end, replacement) byte edits, applied back to front.
fn apply(src: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = src.to_string();
    let mut last_start = usize::MAX;
    for (s, e, text) in edits {
        if e > last_start {
            continue; // overlapping edit
        }
        out.replace_range(s..e, &text);
        last_start = s;
    }
    out
}

fn sig<'a>(tokens: &[Token<'a>]) -> Vec<Token<'a>> {
    tokens.iter().filter(|t| !t.is_trivia()).copied().collect()
}

/// Paren depth before each token.
fn depths(t: &[Token<'_>]) -> Vec<i32> {
    let mut d = 0;
    t.iter()
        .map(|x| {
            let before = d;
            match x.kind {
                TokenKind::LParen => d += 1,
                TokenKind::RParen => d -= 1,
                _ => {}
            }
            if x.kind == TokenKind::RParen {
                d
            } else {
                before
            }
        })
        .collect()
}

/// Index of the `(` enclosing token `i`, if any.
fn enclosing_paren(t: &[Token<'_>], i: usize) -> Option<usize> {
    let mut depth = 0i32;
    for k in (0..i).rev() {
        match t[k].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => {
                if depth == 0 {
                    return Some(k);
                }
                depth -= 1;
            }
            _ => {}
        }
    }
    None
}

/// Replace `*` and `alias.*` in SELECT lists with explicit column lists.
pub fn expand_wildcards(src: &str, dialect: Dialect, catalog: Option<&Catalog>) -> String {
    let tokens = tokenize(src, dialect);
    let mut edits = Vec::new();
    for span in split_statements(src, dialect) {
        let stmt: Vec<Token<'_>> = tokens
            .iter()
            .filter(|t| t.start >= span.start && t.end() <= span.end)
            .copied()
            .collect();
        let scope = analyze_statement(&stmt, dialect);
        let t = sig(&stmt);
        let depth = depths(&t);
        let cols_of = |r: &crate::scope::TableRef| -> Option<Vec<String>> {
            if !r.columns.is_empty() {
                return Some(r.columns.clone());
            }
            let cte = scope
                .ctes
                .iter()
                .find(|c| c.name.eq_ignore_ascii_case(&r.name));
            if let Some(c) = cte.filter(|c| !c.columns.is_empty()) {
                return Some(c.columns.clone());
            }
            let o = catalog?.find(r.schema.as_deref(), &r.name)?;
            (!o.columns.is_empty()).then(|| o.columns.iter().map(|c| c.name.clone()).collect())
        };
        for (i, tok) in t.iter().enumerate() {
            if tok.text != "*" || tok.kind != TokenKind::Operator || i == 0 {
                continue;
            }
            let prev = &t[i - 1];
            let qualified = prev.kind == TokenKind::Dot && i >= 2 && t[i - 2].is_name();
            let bare = matches!(prev.upper().as_str(), "SELECT" | "DISTINCT")
                || prev.kind == TokenKind::Comma;
            if !qualified && !bare {
                continue;
            }
            if qualified {
                let q = t[i - 2].ident_value();
                let Some(r) = scope.tables.iter().find(|r| r.matches_qualifier(&q)) else {
                    continue;
                };
                let Some(cols) = cols_of(r) else { continue };
                let prefix = &src[t[i - 2].start..t[i - 2].end()];
                let list: Vec<String> = cols
                    .iter()
                    .map(|c| format!("{prefix}.{}", dialect.quote_ident(c)))
                    .collect();
                edits.push((t[i - 2].start, tok.end(), list.join(", ")));
                continue;
            }
            // tables of this query level: same enclosing parenthesis and depth
            let level = enclosing_paren(&t, i);
            let in_exists = level.is_some_and(|p| p >= 1 && t[p - 1].upper() == "EXISTS");
            if in_exists {
                continue;
            }
            let tables: Vec<&crate::scope::TableRef> = scope
                .tables
                .iter()
                .filter(|r| {
                    t.iter()
                        .position(|x| x.start == r.start)
                        .is_some_and(|k| depth[k] == depth[i] && enclosing_paren(&t, k) == level)
                })
                .collect();
            if tables.is_empty() {
                continue;
            }
            let mut list = Vec::new();
            let multi = tables.len() > 1;
            let mut ok = true;
            for r in &tables {
                match cols_of(r) {
                    Some(cols) => {
                        for c in cols {
                            let c = dialect.quote_ident(&c);
                            list.push(if multi {
                                format!("{}.{c}", dialect.quote_ident(r.visible_name()))
                            } else {
                                c
                            });
                        }
                    }
                    None => ok = false,
                }
            }
            if ok {
                edits.push((tok.start, tok.end(), list.join(", ")));
            }
        }
    }
    apply(src, edits)
}

/// Add the schema to unqualified table and procedure names that resolve
/// in the catalog.
pub fn qualify_object_names(src: &str, dialect: Dialect, catalog: &Catalog) -> String {
    let tokens = tokenize(src, dialect);
    let mut edits = Vec::new();
    let mut seen = HashSet::new();
    for span in split_statements(src, dialect) {
        let stmt: Vec<Token<'_>> = tokens
            .iter()
            .filter(|t| t.start >= span.start && t.end() <= span.end)
            .copied()
            .collect();
        let scope = analyze_statement(&stmt, dialect);
        let aliases: HashSet<String> = scope
            .tables
            .iter()
            .filter_map(|r| r.alias.clone())
            .map(|a| a.to_lowercase())
            .collect();
        for r in &scope.tables {
            if r.schema.is_some()
                || r.local
                || r.name.is_empty()
                || aliases.contains(&r.name.to_lowercase())
            {
                continue;
            }
            if scope
                .ctes
                .iter()
                .any(|c| c.name.eq_ignore_ascii_case(&r.name))
            {
                continue;
            }
            if let Some(o) = catalog.find(None, &r.name)
                && seen.insert(r.name_start)
            {
                edits.push((
                    r.name_start,
                    r.name_start,
                    format!("{}.", dialect.quote_ident(&o.schema)),
                ));
            }
        }
        let t = sig(&stmt);
        for (i, tok) in t.iter().enumerate() {
            if !matches!(tok.upper().as_str(), "EXEC" | "EXECUTE" | "CALL") {
                continue;
            }
            let Some(n) = t.get(i + 1).filter(|n| n.is_name()) else {
                continue;
            };
            if t.get(i + 2).is_some_and(|d| d.kind == TokenKind::Dot) {
                continue;
            }
            if let Some(o) = catalog
                .find(None, &n.ident_value())
                .filter(|o| o.kind == ObjectKind::Procedure)
                && seen.insert(n.start)
            {
                edits.push((
                    n.start,
                    n.start,
                    format!("{}.", dialect.quote_ident(&o.schema)),
                ));
            }
        }
    }
    apply(src, edits)
}

/// Terminate statements with `;` where it is safe to do so.
pub fn insert_semicolons(src: &str, dialect: Dialect) -> String {
    let tokens = tokenize(src, dialect);
    let mut edits = Vec::new();
    for span in split_statements(src, dialect) {
        let last = tokens
            .iter()
            .rfind(|t| t.start >= span.start && t.end() <= span.end && !t.is_trivia());
        let Some(last) = last else { continue };
        let skip = match last.kind {
            TokenKind::Semicolon
            | TokenKind::Comma
            | TokenKind::LParen
            | TokenKind::Operator
            | TokenKind::Dot => true,
            TokenKind::Word => matches!(
                last.upper().as_str(),
                "BEGIN" | "AS" | "THEN" | "ELSE" | "DO" | "TRY" | "CATCH" | "GO"
            ),
            _ => false,
        };
        if !skip {
            edits.push((last.end(), last.end(), ";".to_string()));
        }
    }
    apply(src, edits)
}

/// `[Name]` -> `Name` where brackets are not needed (SQL Server).
pub fn remove_brackets(src: &str, dialect: Dialect) -> String {
    if dialect != Dialect::MsSql {
        return src.to_string();
    }
    let edits = tokenize(src, dialect)
        .into_iter()
        .filter(|t| {
            t.kind == TokenKind::QuotedIdent && t.text.starts_with('[') && t.text.ends_with(']')
        })
        .filter_map(|t| {
            let inner = unquote(t.text);
            (!dialect.needs_quoting(&inner)).then(|| (t.start, t.end(), inner))
        })
        .collect();
    apply(src, edits)
}

/// Change the case of keywords (and built-in function names before `(`)
/// without touching layout.
pub fn apply_keyword_case(src: &str, dialect: Dialect, case: KeywordCase) -> String {
    if case == KeywordCase::Preserve {
        return src.to_string();
    }
    let tokens = tokenize(src, dialect);
    let sig_idx: Vec<usize> = (0..tokens.len())
        .filter(|&i| !tokens[i].is_trivia())
        .collect();
    let mut edits = Vec::new();
    for (n, &i) in sig_idx.iter().enumerate() {
        let t = &tokens[i];
        if t.kind != TokenKind::Word || t.text.starts_with('#') {
            continue;
        }
        let prev = n.checked_sub(1).map(|p| &tokens[sig_idx[p]]);
        let next = sig_idx.get(n + 1).map(|&k| &tokens[k]);
        if prev.is_some_and(|p| p.kind == TokenKind::Dot)
            || next.is_some_and(|x| x.kind == TokenKind::Dot)
        {
            continue;
        }
        let u = t.upper();
        let is_kw = keywords::is_reserved(&u, dialect)
            || (next.is_some_and(|x| x.kind == TokenKind::LParen)
                && (keywords::is_function(&u) || keywords::is_data_type(&u)));
        if !is_kw {
            continue;
        }
        let new = match case {
            KeywordCase::Upper => u,
            KeywordCase::Lower => t.text.to_ascii_lowercase(),
            KeywordCase::Preserve => continue,
        };
        if new != t.text {
            edits.push((t.start, t.end(), new));
        }
    }
    apply(src, edits)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Column, DbObject};

    fn cat() -> Catalog {
        let col = |n: &str| Column {
            name: n.into(),
            data_type: "int".into(),
            nullable: true,
            is_primary_key: false,
            is_identity: false,
            is_computed: false,
        };
        Catalog {
            database: "Shop".into(),
            default_schema: "dbo".into(),
            schemas: vec!["dbo".into(), "sales".into()],
            objects: vec![
                DbObject {
                    schema: "dbo".into(),
                    name: "Orders".into(),
                    kind: ObjectKind::Table,
                    columns: vec![col("Id"), col("Total")],
                    params: vec![],
                },
                DbObject {
                    schema: "sales".into(),
                    name: "Regions".into(),
                    kind: ObjectKind::Table,
                    columns: vec![col("Code")],
                    params: vec![],
                },
                DbObject {
                    schema: "dbo".into(),
                    name: "GetOrders".into(),
                    kind: ObjectKind::Procedure,
                    columns: vec![],
                    params: vec![],
                },
            ],
            foreign_keys: vec![],
        }
    }

    #[test]
    fn wildcards() {
        let c = cat();
        assert_eq!(
            expand_wildcards("SELECT * FROM Orders", Dialect::MsSql, Some(&c)),
            "SELECT Id, Total FROM Orders"
        );
        assert_eq!(
            expand_wildcards(
                "SELECT o.*, r.Code FROM Orders o, sales.Regions r",
                Dialect::MsSql,
                Some(&c)
            ),
            "SELECT o.Id, o.Total, r.Code FROM Orders o, sales.Regions r"
        );
        assert_eq!(
            expand_wildcards(
                "SELECT * FROM (SELECT Id AS a FROM Orders) x",
                Dialect::MsSql,
                Some(&c)
            ),
            "SELECT a FROM (SELECT Id AS a FROM Orders) x"
        );
        let keep = "SELECT 1 WHERE EXISTS (SELECT * FROM Orders)";
        assert_eq!(expand_wildcards(keep, Dialect::MsSql, Some(&c)), keep);
        assert_eq!(
            expand_wildcards("SELECT COUNT(*) FROM Orders", Dialect::MsSql, Some(&c)),
            "SELECT COUNT(*) FROM Orders"
        );
    }

    #[test]
    fn qualify() {
        assert_eq!(
            qualify_object_names(
                "SELECT * FROM Orders o JOIN Regions r ON 1=1; EXEC GetOrders",
                Dialect::MsSql,
                &cat()
            ),
            "SELECT * FROM dbo.Orders o JOIN sales.Regions r ON 1=1; EXEC dbo.GetOrders"
        );
        let keep = "WITH Orders AS (SELECT 1 AS x) SELECT * FROM Orders";
        assert_eq!(qualify_object_names(keep, Dialect::MsSql, &cat()), keep);
    }

    #[test]
    fn semicolons() {
        assert_eq!(
            insert_semicolons(
                "SELECT 1\n\nSELECT 2 -- two\nGO\nUPDATE t SET a = 1;",
                Dialect::MsSql
            ),
            "SELECT 1;\n\nSELECT 2; -- two\nGO\nUPDATE t SET a = 1;"
        );
    }

    #[test]
    fn brackets_and_case() {
        assert_eq!(
            remove_brackets(
                "SELECT [Id], [Order], [my col] FROM [dbo].[Orders]",
                Dialect::MsSql
            ),
            "SELECT Id, [Order], [my col] FROM dbo.Orders"
        );
        assert_eq!(
            apply_keyword_case(
                "select count(*), t.select from t where x is null",
                Dialect::MsSql,
                KeywordCase::Upper
            ),
            "SELECT COUNT(*), t.select FROM t WHERE x IS NULL"
        );
    }
}
