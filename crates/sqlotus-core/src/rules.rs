//! Code-analysis rules about query performance and correctness that need
//! the statement scope (and, for some, the catalog):
//!
//! - SQ009 `NOT IN (SELECT …)` over a nullable column
//! - SQ010 a function wrapped around a column in WHERE / ON / HAVING
//! - SQ011 comparisons that force an implicit conversion of a column

use std::collections::HashMap;

use crate::Dialect;
use crate::analysis::{Diagnostic, Severity, diag, resolve, word};
use crate::catalog::{Catalog, Column};
use crate::lexer::{Token, TokenKind};
use crate::scope::Scope;

const COMPARISON: &[&str] = &["=", "<>", "!=", "<", ">", "<=", ">="];

fn is_comparison(t: Option<&Token<'_>>) -> bool {
    t.is_some_and(|t| {
        (t.kind == TokenKind::Operator && COMPARISON.contains(&t.text))
            || (t.kind == TokenKind::Word && matches!(t.upper().as_str(), "LIKE" | "BETWEEN" | "IN"))
    })
}

/// Lower-case type name without length / precision: `varchar(50)` → `varchar`.
fn base_type(data_type: &str) -> String {
    data_type.split('(').next().unwrap_or("").trim().to_ascii_lowercase()
}

fn is_text_type(t: &str) -> bool {
    matches!(
        t,
        "char" | "varchar" | "nchar" | "nvarchar" | "text" | "ntext" | "character varying" | "character" | "tinytext"
            | "mediumtext" | "longtext"
    )
}

fn is_non_unicode_text(t: &str) -> bool {
    matches!(t, "char" | "varchar" | "text")
}

/// Index of the `)` matching the `(` at `open`.
fn matching_paren(t: &[Token<'_>], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (k, x) in t.iter().enumerate().skip(open) {
        match x.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => {
                depth -= 1;
                if depth == 0 {
                    return Some(k);
                }
            }
            _ => {}
        }
    }
    None
}

/// Whether token `i` sits in a WHERE, ON or HAVING condition (not in a
/// nested SELECT list or a SET clause).
fn in_filter(t: &[Token<'_>], i: usize) -> bool {
    let mut depth = 0i32;
    for k in (0..i).rev() {
        match t[k].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            TokenKind::Semicolon => return false,
            TokenKind::Word if depth == 0 => match t[k].upper().as_str() {
                "WHERE" | "ON" | "HAVING" => return true,
                "SELECT" | "SET" | "FROM" | "JOIN" | "GROUP" | "ORDER" | "VALUES" | "INTO" | "UPDATE" | "DELETE"
                | "INSERT" | "RETURN" | "PRINT" | "DECLARE" => return false,
                _ => {}
            },
            _ => {}
        }
    }
    false
}

/// The column referenced at token `i` (`col` or `q.col`), with the index of
/// its last token. Unqualified names must match exactly one table in scope.
fn column_ref<'c>(t: &[Token<'_>], i: usize, scope: &Scope, cat: &'c Catalog) -> Option<(&'c Column, usize)> {
    let tok = t.get(i)?;
    if !tok.is_name() || (i > 0 && t[i - 1].kind == TokenKind::Dot) {
        return None;
    }
    let qualified = t.get(i + 1).is_some_and(|d| d.kind == TokenKind::Dot) && t.get(i + 2).is_some_and(|n| n.is_name());
    let name_idx = if qualified { i + 2 } else { i };
    if t.get(name_idx + 1).is_some_and(|n| matches!(n.kind, TokenKind::Dot | TokenKind::LParen)) {
        return None;
    }
    let name = t[name_idx].ident_value();
    let qualifier = qualified.then(|| tok.ident_value());
    let mut found = scope
        .tables
        .iter()
        .filter(|r| qualifier.as_deref().is_none_or(|q| r.matches_qualifier(q)))
        .filter_map(|r| resolve(cat, r))
        .filter_map(|o| o.columns.iter().find(|c| c.name.eq_ignore_ascii_case(&name)));
    let first = found.next()?;
    if qualifier.is_none() && found.next().is_some() {
        return None; // ambiguous
    }
    Some((first, name_idx))
}

/// A `q.col` reference whose qualifier is a table or alias in scope (used
/// when no catalog is loaded).
fn qualified_ref(t: &[Token<'_>], i: usize, scope: &Scope) -> bool {
    t.get(i).is_some_and(|q| q.is_name())
        && (i == 0 || t[i - 1].kind != TokenKind::Dot)
        && t.get(i + 1).is_some_and(|d| d.kind == TokenKind::Dot)
        && t.get(i + 2).is_some_and(|c| c.is_name())
        && !t.get(i + 3).is_some_and(|n| matches!(n.kind, TokenKind::Dot | TokenKind::LParen))
        && scope.tables.iter().any(|r| r.alias.is_some() && r.matches_qualifier(&t[i].ident_value()))
}

/// SQ009: `x NOT IN (SELECT col …)` returns no rows at all as soon as the
/// subquery yields a NULL.
pub(crate) fn not_in_subquery(t: &[Token<'_>], scope: &Scope, cat: Option<&Catalog>, out: &mut Vec<Diagnostic>) {
    for i in 0..t.len().saturating_sub(3) {
        if word(t.get(i)) != "NOT" || word(t.get(i + 1)) != "IN" || t[i + 2].kind != TokenKind::LParen || word(t.get(i + 3)) != "SELECT" {
            continue;
        }
        let mut k = i + 4;
        if word(t.get(k)) == "DISTINCT" {
            k += 1;
        }
        if word(t.get(k)) == "TOP" {
            k += if t.get(k + 1).is_some_and(|x| x.kind == TokenKind::LParen) { 4 } else { 2 };
        }
        let nullable = cat.and_then(|c| column_ref(t, k, scope, c)).map(|(col, _)| col.nullable);
        if nullable == Some(false) {
            continue;
        }
        let (severity, why) = match nullable {
            Some(_) => (Severity::Warning, "the subquery column allows NULL, and one NULL makes NOT IN return no rows"),
            None => (Severity::Info, "if the subquery returns a NULL, NOT IN returns no rows"),
        };
        out.push(diag(&t[i], severity, "SQ009", format!("NOT IN (SELECT …): {why}; NOT EXISTS is safer")));
    }
}

const WRAPPING_FUNCTIONS: &[&str] = &[
    "YEAR", "MONTH", "DAY", "DATEPART", "DATENAME", "DATEDIFF", "EOMONTH", "CONVERT", "CAST", "UPPER", "LOWER", "LTRIM",
    "RTRIM", "TRIM", "LEFT", "RIGHT", "SUBSTRING", "SUBSTR", "ISNULL", "COALESCE", "IFNULL", "NVL", "ABS", "LEN", "LENGTH",
    "DATE", "DATE_FORMAT", "TO_CHAR", "FORMAT", "REPLACE", "CONCAT", "FLOOR", "CEILING", "ROUND",
];

/// SQ010: `WHERE YEAR(OrderDate) = 2024` — the function hides the column
/// from its index, so every row is read.
pub(crate) fn non_sargable(t: &[Token<'_>], scope: &Scope, cat: Option<&Catalog>, out: &mut Vec<Diagnostic>) {
    for i in 0..t.len().saturating_sub(1) {
        let f = word(t.get(i));
        if !WRAPPING_FUNCTIONS.contains(&f.as_str()) || t[i + 1].kind != TokenKind::LParen || (i > 0 && t[i - 1].kind == TokenKind::Dot) {
            continue;
        }
        let Some(close) = matching_paren(t, i + 1) else { continue };
        if !(is_comparison(t.get(close + 1)) || (i > 0 && is_comparison(t.get(i - 1)))) || !in_filter(t, i) {
            continue;
        }
        let column = (i + 2..close).find_map(|j| match cat.and_then(|c| column_ref(t, j, scope, c)) {
            Some((col, _)) => Some(col.name.clone()),
            None if qualified_ref(t, j, scope) => Some(t[j + 2].ident_value()),
            None => None,
        });
        let Some(column) = column else { continue };
        let hint = match f.as_str() {
            "YEAR" | "MONTH" | "DAY" | "DATEPART" | "DATENAME" | "EOMONTH" | "DATE" | "DATE_FORMAT" | "TO_CHAR" => {
                format!("compare {column} with a range instead, e.g. {column} >= '2024-01-01' AND {column} < '2025-01-01'")
            }
            "ISNULL" | "COALESCE" | "IFNULL" | "NVL" => format!("write ({column} = … OR {column} IS NULL) instead"),
            "UPPER" | "LOWER" => "case-insensitive collations already ignore case".to_string(),
            "LTRIM" | "RTRIM" | "TRIM" => "trim the value you compare with, not the column".to_string(),
            _ => format!("compare the bare column {column} instead"),
        };
        out.push(diag(
            &t[i],
            Severity::Info,
            "SQ010",
            format!("{f}() around column {column} prevents an index seek; {hint}"),
        ));
    }
}

/// SQ011: comparisons that make the server convert the column:
/// `varchar_col = N'x'` (SQL Server) and `text_col = 123`.
pub(crate) fn implicit_conversions(
    t: &[Token<'_>],
    scope: &Scope,
    cat: &Catalog,
    variable_types: &HashMap<String, String>,
    dialect: Dialect,
    out: &mut Vec<Diagnostic>,
) {
    if dialect == Dialect::Postgres {
        return; // Postgres rejects such comparisons outright
    }
    for k in 1..t.len().saturating_sub(1) {
        let op = &t[k];
        let is_op = (op.kind == TokenKind::Operator && COMPARISON.contains(&op.text)) || word(Some(op)) == "LIKE";
        if !is_op || !in_filter(t, k) {
            continue;
        }
        // column on the left (`col` or `q.col`), value on the right — or the reverse
        let left_start = if k >= 3 && t[k - 2].kind == TokenKind::Dot { k - 3 } else { k - 1 };
        let pairs = [
            (column_ref(t, left_start, scope, cat).filter(|(_, end)| *end == k - 1), k + 1),
            (column_ref(t, k + 1, scope, cat), k - 1),
        ];
        for (col, value_idx) in pairs {
            let (Some((col, _)), Some(value)) = (col, t.get(value_idx)) else { continue };
            let ty = base_type(&col.data_type);
            let message = match value.kind {
                TokenKind::String
                    if dialect == Dialect::MsSql
                        && is_non_unicode_text(&ty)
                        && (value.text.starts_with('N') || value.text.starts_with('n'))
                        && value.text.is_ascii() =>
                {
                    format!(
                        "N'…' is nvarchar, so SQL Server converts {ty} column {} for every row and cannot seek its index; drop the N",
                        col.name
                    )
                }
                TokenKind::Variable if dialect == Dialect::MsSql && is_non_unicode_text(&ty) => {
                    let Some(vt) = variable_types.get(&value.text.to_lowercase()) else { continue };
                    if !matches!(base_type(vt).as_str(), "nvarchar" | "nchar") {
                        continue;
                    }
                    format!(
                        "{} is {}, so {ty} column {} is converted for every row and its index cannot be used; declare {} as {ty}",
                        value.text,
                        base_type(vt),
                        col.name,
                        value.text
                    )
                }
                TokenKind::Number if is_text_type(&ty) => format!(
                    "Comparing {ty} column {} with a number converts every value of it (no index seek{}); quote the value: '{}'",
                    col.name,
                    if dialect == Dialect::MsSql { ", and an error for non-numeric text" } else { "" },
                    value.text
                ),
                _ => continue,
            };
            out.push(diag(value, Severity::Warning, "SQ011", message));
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::Dialect;
    use crate::analysis::diagnostics;
    use crate::catalog::{Catalog, Column, DbObject, ObjectKind};

    fn col(name: &str, ty: &str, nullable: bool) -> Column {
        Column {
            name: name.into(),
            data_type: ty.into(),
            nullable,
            is_primary_key: false,
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
                    name: "Customers".into(),
                    kind: ObjectKind::Table,
                    columns: vec![
                        col("Id", "int", false),
                        col("Code", "varchar(10)", false),
                        col("Name", "nvarchar(100)", false),
                        col("ReferrerId", "int", true),
                        col("CreatedAt", "datetime2", false),
                    ],
                    params: vec![],
                },
                DbObject {
                    schema: "dbo".into(),
                    name: "Orders".into(),
                    kind: ObjectKind::Table,
                    columns: vec![col("Id", "int", false), col("CustomerId", "int", false)],
                    params: vec![],
                },
            ],
            foreign_keys: vec![],
        }
    }

    fn codes(src: &str, dialect: Dialect) -> Vec<String> {
        diagnostics(src, dialect, Some(&cat()))
            .into_iter()
            .map(|d| d.code)
            .filter(|c| matches!(c.as_str(), "SQ009" | "SQ010" | "SQ011"))
            .collect()
    }

    #[test]
    fn not_in_with_nullable_subquery_column() {
        let ms = Dialect::MsSql;
        assert_eq!(codes("SELECT Id FROM dbo.Customers WHERE Id NOT IN (SELECT ReferrerId FROM dbo.Customers)", ms), vec!["SQ009"]);
        assert!(codes("SELECT Id FROM dbo.Customers WHERE Id NOT IN (SELECT CustomerId FROM dbo.Orders)", ms).is_empty());
        assert!(codes("SELECT Id FROM dbo.Customers WHERE Id NOT IN (1, 2)", ms).is_empty());
        // unknown column without a catalog: still a hint
        let d = diagnostics("SELECT a FROM t WHERE a NOT IN (SELECT b FROM u)", ms, None);
        assert!(d.iter().any(|d| d.code == "SQ009"), "{d:?}");
    }

    #[test]
    fn functions_around_columns_in_filters() {
        let ms = Dialect::MsSql;
        assert_eq!(codes("SELECT Id FROM dbo.Customers WHERE YEAR(CreatedAt) = 2024", ms), vec!["SQ010"]);
        assert_eq!(codes("SELECT c.Id FROM dbo.Customers c JOIN dbo.Orders o ON UPPER(c.Code) = 'A'", ms), vec!["SQ010"]);
        assert!(codes("SELECT YEAR(CreatedAt) FROM dbo.Customers WHERE CreatedAt >= '2024-01-01'", ms).is_empty());
        assert!(codes("SELECT Id FROM dbo.Customers WHERE CreatedAt >= DATEADD(day, -1, GETDATE())", ms).is_empty());
        assert!(codes("UPDATE dbo.Customers SET Code = UPPER(Code) WHERE Id = 1", ms).is_empty());
        let msg = diagnostics("SELECT Id FROM dbo.Customers WHERE YEAR(CreatedAt) = 2024", ms, Some(&cat()))
            .into_iter()
            .find(|d| d.code == "SQ010")
            .unwrap()
            .message;
        assert!(msg.contains("CreatedAt >= '2024-01-01'"), "{msg}");
    }

    #[test]
    fn implicit_conversions_on_columns() {
        let ms = Dialect::MsSql;
        assert_eq!(codes("SELECT Id FROM dbo.Customers WHERE Code = N'A1'", ms), vec!["SQ011"]);
        assert!(codes("SELECT Id FROM dbo.Customers WHERE Code = 'A1'", ms).is_empty());
        assert!(codes("SELECT Id FROM dbo.Customers WHERE Name = N'A1'", ms).is_empty());
        assert_eq!(codes("SELECT Id FROM dbo.Customers WHERE Code = 123", ms), vec!["SQ011"]);
        assert_eq!(codes("SELECT Id FROM dbo.Customers c WHERE 123 = c.Code", ms), vec!["SQ011"]);
        assert_eq!(codes("DECLARE @c nvarchar(10) = N'x'; SELECT Id FROM dbo.Customers WHERE Code = @c", ms), vec!["SQ011"]);
        assert!(codes("DECLARE @c varchar(10) = 'x'; SELECT Id FROM dbo.Customers WHERE Code = @c", ms).is_empty());
        assert!(codes("SELECT Id FROM dbo.Customers WHERE Id = 123", ms).is_empty());
        assert_eq!(codes("SELECT Id FROM dbo.Customers WHERE Code = 123", Dialect::MySql), vec!["SQ011"]);
    }
}
