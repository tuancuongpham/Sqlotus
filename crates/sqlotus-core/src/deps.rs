//! "Who uses this?": views, procedures, functions and triggers whose
//! definitions reference a table, a column or another module, plus the
//! foreign keys that point at a table. Works from the definitions loaded
//! for search, so it is the same for every dialect.

use std::collections::HashSet;

use serde::Serialize;

use crate::Dialect;
use crate::analysis::{sig, word};
use crate::catalog::{Catalog, ObjectDefinition, ObjectKind};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::scope::{TableRef, analyze_statement};
use crate::statements::split_statements;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reference {
    pub schema: String,
    pub name: String,
    pub kind: ObjectKind,
    /// 1-based line in the definition (0 for foreign keys).
    pub line: usize,
    /// The line itself, trimmed.
    pub text: String,
    /// `reads`, `changes`, `calls`, `trigger`, `mentions` or `foreign key`.
    pub how: String,
}

fn line_of(src: &str, offset: usize) -> (usize, String) {
    let offset = offset.min(src.len());
    let line = src[..offset].matches('\n').count() + 1;
    let start = src[..offset].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let end = src[offset..].find('\n').map(|i| offset + i).unwrap_or(src.len());
    let text: String = src[start..end].trim().chars().take(200).collect();
    (line, text)
}

fn same(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Whether a reference in a definition points at `schema.name`.
fn is_target(r: &TableRef, catalog: &Catalog, schema: &str, name: &str) -> bool {
    if r.local || !same(&r.name, name) {
        return false;
    }
    match &r.schema {
        Some(s) => same(s, schema),
        None => catalog.find(None, name).is_none_or(|o| same(&o.schema, schema)),
    }
}

/// References to `schema.name` (or to its `column`) in `definitions`.
pub fn find_references(
    definitions: &[ObjectDefinition],
    catalog: &Catalog,
    dialect: Dialect,
    schema: &str,
    name: &str,
    column: Option<&str>,
) -> Vec<Reference> {
    let mut out: Vec<Reference> = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |d: &ObjectDefinition, offset: usize, how: &str, out: &mut Vec<Reference>| {
        let (line, text) = line_of(&d.definition, offset);
        if seen.insert((d.schema.to_lowercase(), d.name.to_lowercase(), line, how.to_string())) {
            out.push(Reference {
                schema: d.schema.clone(),
                name: d.name.clone(),
                kind: d.kind,
                line,
                text,
                how: how.into(),
            });
        }
    };
    for d in definitions {
        if same(&d.name, name) && same(&d.schema, schema) {
            continue; // the object itself
        }
        let src = d.definition.as_str();
        let tokens = tokenize(src, dialect);
        for span in split_statements(src, dialect) {
            let stmt: Vec<Token<'_>> = tokens
                .iter()
                .filter(|t| t.start >= span.start && t.end() <= span.end)
                .copied()
                .collect();
            let scope = analyze_statement(&stmt, dialect);
            let refs: Vec<&TableRef> = scope
                .tables
                .iter()
                .filter(|r| is_target(r, catalog, schema, name))
                .collect();
            let t = sig(&stmt);
            match column {
                None => {
                    for r in &refs {
                        let how = match r.via.as_str() {
                            "UPDATE" | "INTO" | "DELETE" | "MERGE" | "INSERT" => "changes",
                            _ => "reads",
                        };
                        push(d, r.name_start, how, &mut out);
                    }
                    // calls and other mentions the scope does not see: EXEC p, dbo.fn(…), TRIGGER … ON t
                    for (i, tok) in t.iter().enumerate() {
                        if !tok.is_name()
                            || !same(&tok.ident_value(), name)
                            || refs.iter().any(|r| r.name_start == tok.start)
                        {
                            continue;
                        }
                        let qualified_by = (i >= 2 && t[i - 1].kind == TokenKind::Dot).then(|| t[i - 2].ident_value());
                        if qualified_by.as_deref().is_some_and(|q| !same(q, schema)) {
                            continue; // another schema's object, or a column of an alias
                        }
                        if t.get(i + 1).is_some_and(|n| n.kind == TokenKind::Dot) {
                            continue; // a qualifier, not the object
                        }
                        let before = if qualified_by.is_some() {
                            i.checked_sub(3)
                        } else {
                            i.checked_sub(1)
                        };
                        let prev = word(before.and_then(|p| t.get(p)));
                        let how = if matches!(prev.as_str(), "EXEC" | "EXECUTE" | "CALL")
                            || t.get(i + 1).is_some_and(|n| n.kind == TokenKind::LParen)
                        {
                            "calls"
                        } else if d.kind == ObjectKind::Trigger && prev == "ON" {
                            "trigger"
                        } else if qualified_by.is_some() {
                            "mentions"
                        } else {
                            continue; // a bare word with the same name is too likely a column or alias
                        };
                        push(d, tok.start, how, &mut out);
                    }
                }
                Some(col) => {
                    if refs.is_empty() {
                        continue;
                    }
                    // other tables in the statement that also have this column make bare names ambiguous
                    let ambiguous = scope.tables.iter().any(|r| {
                        !is_target(r, catalog, schema, name)
                            && catalog
                                .find(r.schema.as_deref(), &r.name)
                                .is_some_and(|o| o.columns.iter().any(|c| same(&c.name, col)))
                    });
                    for (i, tok) in t.iter().enumerate() {
                        if !tok.is_name() || !same(&tok.ident_value(), col) {
                            continue;
                        }
                        if t.get(i + 1)
                            .is_some_and(|n| matches!(n.kind, TokenKind::Dot | TokenKind::LParen))
                        {
                            continue;
                        }
                        let qualified = i >= 2 && t[i - 1].kind == TokenKind::Dot;
                        let hit = if qualified {
                            let q = t[i - 2].ident_value();
                            refs.iter().any(|r| r.matches_qualifier(&q))
                        } else {
                            !ambiguous
                        };
                        if hit {
                            let changes = refs
                                .iter()
                                .any(|r| matches!(r.via.as_str(), "UPDATE" | "INTO" | "INSERT" | "MERGE"));
                            push(d, tok.start, if changes { "changes" } else { "reads" }, &mut out);
                        }
                    }
                }
            }
        }
    }
    for f in &catalog.foreign_keys {
        let to_target = same(&f.to_schema, schema) && same(&f.to_table, name);
        let from_target = same(&f.from_schema, schema) && same(&f.from_table, name);
        let hit = match column {
            None => to_target && !from_target,
            Some(c) => {
                (to_target && f.to_columns.iter().any(|x| same(x, c)))
                    || (from_target && f.from_columns.iter().any(|x| same(x, c)))
            }
        };
        if hit {
            let (s, n) = if to_target && !(column.is_some() && from_target) {
                (&f.from_schema, &f.from_table)
            } else {
                (&f.to_schema, &f.to_table)
            };
            out.push(Reference {
                schema: s.clone(),
                name: n.clone(),
                kind: ObjectKind::Table,
                line: 0,
                text: format!(
                    "{}: {}.{}({}) → {}.{}({})",
                    f.name,
                    f.from_schema,
                    f.from_table,
                    f.from_columns.join(", "),
                    f.to_schema,
                    f.to_table,
                    f.to_columns.join(", ")
                ),
                how: "foreign key".into(),
            });
        }
    }
    out.sort_by(|a, b| {
        (a.how == "foreign key", &a.schema, &a.name, a.line).cmp(&(b.how == "foreign key", &b.schema, &b.name, b.line))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Column, DbObject, ForeignKey};

    fn col(n: &str) -> Column {
        Column {
            name: n.into(),
            data_type: "int".into(),
            nullable: true,
            is_primary_key: n == "Id",
            is_identity: false,
            is_computed: false,
        }
    }

    fn table(name: &str, cols: &[&str]) -> DbObject {
        DbObject {
            schema: "dbo".into(),
            name: name.into(),
            kind: ObjectKind::Table,
            columns: cols.iter().map(|c| col(c)).collect(),
            params: vec![],
        }
    }

    fn def(name: &str, kind: ObjectKind, text: &str) -> ObjectDefinition {
        ObjectDefinition {
            schema: "dbo".into(),
            name: name.into(),
            kind,
            definition: text.into(),
        }
    }

    fn catalog() -> Catalog {
        Catalog {
            database: "Shop".into(),
            default_schema: "dbo".into(),
            schemas: vec!["dbo".into(), "sales".into()],
            objects: vec![
                table("Customers", &["Id", "Name", "Country"]),
                table("Orders", &["Id", "CustomerId", "Total", "Country"]),
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

    fn defs() -> Vec<ObjectDefinition> {
        vec![
            def(
                "vTotals",
                ObjectKind::View,
                "CREATE VIEW dbo.vTotals AS\nSELECT c.Name, SUM(o.Total) AS Total\nFROM dbo.Customers c\nJOIN dbo.Orders o ON o.CustomerId = c.Id\nGROUP BY c.Name",
            ),
            def(
                "pRename",
                ObjectKind::Procedure,
                "CREATE PROCEDURE dbo.pRename @Id int, @Name nvarchar(50) AS\nUPDATE Customers SET Name = @Name WHERE Id = @Id",
            ),
            def(
                "pRun",
                ObjectKind::Procedure,
                "CREATE PROCEDURE dbo.pRun AS\nEXEC dbo.pRename 1, N'x'",
            ),
            def(
                "pOther",
                ObjectKind::Procedure,
                "CREATE PROCEDURE dbo.pOther AS\nSELECT Name FROM sales.Customers\n-- dbo.Customers is mentioned only in a comment",
            ),
            def(
                "tAudit",
                ObjectKind::Trigger,
                "CREATE TRIGGER dbo.tAudit ON dbo.Orders AFTER INSERT AS\nSELECT 1",
            ),
            def(
                "pCountry",
                ObjectKind::Procedure,
                "CREATE PROCEDURE dbo.pCountry AS\nSELECT Country FROM dbo.Customers c JOIN dbo.Orders o ON o.CustomerId = c.Id\nSELECT c.Country FROM dbo.Customers c",
            ),
        ]
    }

    fn names(r: &[Reference]) -> Vec<String> {
        r.iter().map(|x| format!("{}:{}:{}", x.name, x.line, x.how)).collect()
    }

    #[test]
    fn table_references() {
        let r = find_references(&defs(), &catalog(), Dialect::MsSql, "dbo", "Customers", None);
        assert_eq!(
            names(&r),
            vec![
                "pCountry:2:reads",
                "pCountry:3:reads",
                "pRename:2:changes",
                "vTotals:3:reads",
                "Orders:0:foreign key"
            ]
        );
        assert!(r[2].text.starts_with("UPDATE Customers SET"));
    }

    #[test]
    fn calls_and_triggers() {
        let r = find_references(&defs(), &catalog(), Dialect::MsSql, "dbo", "pRename", None);
        assert_eq!(names(&r), vec!["pRun:2:calls"]);
        let r = find_references(&defs(), &catalog(), Dialect::MsSql, "dbo", "Orders", None);
        assert!(names(&r).contains(&"tAudit:1:trigger".to_string()), "{:?}", names(&r));
    }

    #[test]
    fn column_references() {
        let r = find_references(&defs(), &catalog(), Dialect::MsSql, "dbo", "Customers", Some("Name"));
        assert_eq!(
            names(&r),
            vec!["pRename:2:changes", "vTotals:2:reads", "vTotals:5:reads"]
        );
        // bare Country is ambiguous between Customers and Orders; c.Country is not
        let r = find_references(&defs(), &catalog(), Dialect::MsSql, "dbo", "Customers", Some("Country"));
        assert_eq!(names(&r), vec!["pCountry:3:reads"]);
        let r = find_references(&defs(), &catalog(), Dialect::MsSql, "dbo", "Customers", Some("Id"));
        assert!(names(&r).contains(&"Orders:0:foreign key".to_string()));
    }
}
