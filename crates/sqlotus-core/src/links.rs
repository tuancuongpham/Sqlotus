//! Following foreign keys from a query result: from the key columns of a
//! row to the row they reference, and from a referenced row to the rows
//! that reference it. The result must come from one SELECT whose columns
//! can be traced to table columns (`*`, `t.*`, `[t.]col [AS x]`); joins
//! are fine.

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::catalog::{Catalog, Column, DbObject, ObjectKind};
use crate::editing::{EditColumn, SelectItem, literal, select_items};
use crate::lexer::{TokenKind, significant, tokenize, word};
use crate::scope::{TableRef, analyze_statement};
use crate::statements::split_statements;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LinkDirection {
    /// From a row's foreign key to the row it references.
    References,
    /// From a row to the rows whose foreign key references it.
    ReferencedBy,
}

/// A foreign key a result row can be followed along.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowLink {
    pub direction: LinkDirection,
    /// The foreign key's name.
    pub constraint: String,
    /// Result columns holding the key's values, in key order.
    pub columns: Vec<usize>,
    /// The table to open, and its columns the values are looked up in.
    pub schema: String,
    pub table: String,
    pub target_columns: Vec<String>,
}

/// The foreign keys the rows of `sql`'s result can be followed along.
/// `result_columns`: the names of the columns the server returned. Empty
/// when the result cannot be traced to table columns.
pub fn result_links(sql: &str, dialect: Dialect, catalog: &Catalog, result_columns: &[String]) -> Vec<RowLink> {
    trace(sql, dialect, catalog, result_columns).unwrap_or_default()
}

fn trace(sql: &str, dialect: Dialect, catalog: &Catalog, result_columns: &[String]) -> Option<Vec<RowLink>> {
    let spans: Vec<_> = split_statements(sql, dialect)
        .into_iter()
        .filter(|s| {
            tokenize(s.text(sql), dialect)
                .iter()
                .any(|t| !t.is_trivia() && t.kind != TokenKind::Semicolon)
        })
        .collect();
    let [span] = spans.as_slice() else { return None };
    let tokens = tokenize(sql, dialect);
    let stmt: Vec<_> = tokens
        .iter()
        .filter(|t| t.start >= span.start && t.end() <= span.end)
        .copied()
        .collect();
    let t = significant(&stmt);
    if word(t.first()) != "SELECT" {
        return None;
    }
    // the tables of the outer query, in FROM order; rows of several queries
    // (UNION ...) cannot be traced
    let mut depth = 0i32;
    let mut outer_starts = Vec::new();
    for x in &t {
        match x.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            TokenKind::Word if depth == 0 && matches!(x.upper().as_str(), "UNION" | "INTERSECT" | "EXCEPT") => {
                return None;
            }
            _ if depth == 0 => outer_starts.push(x.start),
            _ => {}
        }
    }
    let scope = analyze_statement(&stmt, dialect);
    let tables: Vec<(&TableRef, Option<&DbObject>)> = scope
        .tables
        .iter()
        .filter(|r| outer_starts.binary_search(&r.start).is_ok())
        .map(|r| {
            let here = r
                .database
                .as_ref()
                .is_none_or(|d| d.eq_ignore_ascii_case(&catalog.database) || catalog.database.is_empty());
            let obj = (!r.local && !r.is_call && here)
                .then(|| catalog.find(r.schema.as_deref(), &r.name))
                .flatten()
                .filter(|o| o.kind == ObjectKind::Table);
            (r, obj)
        })
        .collect();
    let all_known = tables.iter().all(|(_, o)| o.is_some());
    // where each result column comes from: (table, column)
    let mut sources: Vec<Option<(usize, &str)>> = Vec::new();
    for item in select_items(&t)? {
        match item {
            SelectItem::Star(q) => {
                for (k, (r, obj)) in tables.iter().enumerate() {
                    if q.is_some_and(|q| !r.matches_qualifier(&q.ident_value())) {
                        continue;
                    }
                    // the columns of a table Sqlotus does not know cannot be counted
                    let obj = (*obj)?;
                    sources.extend(obj.columns.iter().map(|c| Some((k, c.name.as_str()))));
                }
            }
            SelectItem::Column(q, c) => {
                let name = c.ident_value();
                fn column<'c>(obj: &'c DbObject, name: &str) -> Option<&'c Column> {
                    obj.columns.iter().find(|x| x.name.eq_ignore_ascii_case(name))
                }
                let holds = |obj| column(obj, &name);
                let found = match q {
                    Some(q) => tables
                        .iter()
                        .position(|(r, _)| r.matches_qualifier(&q.ident_value()))
                        .and_then(|k| Some((k, holds(tables[k].1?)?))),
                    // unqualified: the one table that has it (unknown tables might too)
                    None if all_known => {
                        let mut holders = tables
                            .iter()
                            .enumerate()
                            .filter_map(|(k, (_, o))| Some((k, holds((*o)?)?)));
                        match (holders.next(), holders.next()) {
                            (Some(one), None) => Some(one),
                            _ => None,
                        }
                    }
                    None => None,
                };
                sources.push(found.map(|(k, c)| (k, c.name.as_str())));
            }
            SelectItem::Expression => sources.push(None),
        }
    }
    if sources.len() != result_columns.len() {
        return None;
    }
    let position = |k: usize, column: &str| {
        sources
            .iter()
            .position(|s| s.is_some_and(|(sk, c)| sk == k && c.eq_ignore_ascii_case(column)))
    };
    let mut links: Vec<RowLink> = Vec::new();
    for (k, (_, obj)) in tables.iter().enumerate() {
        let Some(obj) = obj else { continue };
        for fk in &catalog.foreign_keys {
            let mut add = |direction, own: &[String], schema: &str, table: &str, target: &[String]| {
                let columns: Option<Vec<usize>> = own.iter().map(|c| position(k, c)).collect();
                let Some(columns) = columns else { return };
                let link = RowLink {
                    direction,
                    constraint: fk.name.clone(),
                    columns,
                    schema: schema.to_string(),
                    table: table.to_string(),
                    target_columns: target.to_vec(),
                };
                if !links.contains(&link) {
                    links.push(link);
                }
            };
            if fk.is_from(obj) {
                add(
                    LinkDirection::References,
                    &fk.from_columns,
                    &fk.to_schema,
                    &fk.to_table,
                    &fk.to_columns,
                );
            }
            if fk.is_to(obj) {
                add(
                    LinkDirection::ReferencedBy,
                    &fk.to_columns,
                    &fk.from_schema,
                    &fk.from_table,
                    &fk.from_columns,
                );
            }
        }
    }
    Some(links)
}

/// The query that follows `link` from one row: `values` are the row's
/// values of `link.columns`.
pub fn link_query(
    link: &RowLink,
    values: &[Option<String>],
    catalog: &Catalog,
    dialect: Dialect,
) -> Result<String, String> {
    if values.len() != link.target_columns.len() {
        return Err("The row does not hold every column of the key.".into());
    }
    let target = catalog.find(Some(&link.schema), &link.table);
    let mut problems = Vec::new();
    let mut conditions = Vec::new();
    for (column, value) in link.target_columns.iter().zip(values) {
        if value.is_none() {
            return Err(match link.direction {
                LinkDirection::References => "The key is NULL in this row: it references no row.".to_string(),
                LinkDirection::ReferencedBy => format!("{column} is NULL in this row: no row can reference it."),
            });
        }
        let data_type = target
            .and_then(|t| t.columns.iter().find(|c| c.name.eq_ignore_ascii_case(column)))
            .map(|c| c.data_type.clone())
            .unwrap_or_default();
        let col = EditColumn {
            column: Some(column.clone()),
            data_type,
            nullable: true,
            is_key: false,
            read_only: false,
        };
        let v = literal(value, &col, dialect, &mut problems);
        conditions.push(format!("{} = {v}", dialect.quote_ident(column)));
    }
    if !problems.is_empty() {
        return Err(problems.join(" "));
    }
    Ok(format!(
        "SELECT *\nFROM {}.{}\nWHERE {}",
        dialect.quote_ident(&link.schema),
        dialect.quote_ident(&link.table),
        conditions.join("\n  AND ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ForeignKey;

    fn table(name: &str, cols: &[(&str, &str, bool)]) -> DbObject {
        DbObject {
            schema: "dbo".into(),
            name: name.into(),
            kind: ObjectKind::Table,
            columns: cols
                .iter()
                .map(|(n, ty, pk)| Column {
                    name: n.to_string(),
                    data_type: ty.to_string(),
                    nullable: !pk,
                    is_primary_key: *pk,
                    is_identity: false,
                    is_computed: false,
                })
                .collect(),
            params: vec![],
        }
    }

    fn fk(name: &str, from: (&str, &[&str]), to: (&str, &[&str])) -> ForeignKey {
        let v = |c: &[&str]| c.iter().map(|s| s.to_string()).collect();
        ForeignKey {
            name: name.into(),
            from_schema: "dbo".into(),
            from_table: from.0.into(),
            from_columns: v(from.1),
            to_schema: "dbo".into(),
            to_table: to.0.into(),
            to_columns: v(to.1),
        }
    }

    fn cat() -> Catalog {
        Catalog {
            database: "Shop".into(),
            default_schema: "dbo".into(),
            schemas: vec!["dbo".into()],
            objects: vec![
                table("Customers", &[("Id", "int", true), ("Name", "nvarchar(100)", false)]),
                table(
                    "Orders",
                    &[
                        ("Id", "int", true),
                        ("CustomerId", "int", false),
                        ("Total", "decimal(10,2)", false),
                    ],
                ),
                table(
                    "OrderLines",
                    &[("OrderId", "int", true), ("LineNo", "int", true), ("Qty", "int", false)],
                ),
                table(
                    "Shipments",
                    &[
                        ("Id", "uniqueidentifier", true),
                        ("OrderId", "int", false),
                        ("LineNo", "int", false),
                    ],
                ),
            ],
            foreign_keys: vec![
                fk(
                    "FK_Orders_Customers",
                    ("Orders", &["CustomerId"]),
                    ("Customers", &["Id"]),
                ),
                fk(
                    "FK_OrderLines_Orders",
                    ("OrderLines", &["OrderId"]),
                    ("Orders", &["Id"]),
                ),
                fk(
                    "FK_Shipments_OrderLines",
                    ("Shipments", &["OrderId", "LineNo"]),
                    ("OrderLines", &["OrderId", "LineNo"]),
                ),
            ],
        }
    }

    /// `(direction, constraint, result columns)` of every link.
    fn links(sql: &str, columns: &[&str]) -> Vec<(LinkDirection, String, Vec<usize>)> {
        let cols: Vec<String> = columns.iter().map(|s| s.to_string()).collect();
        result_links(sql, Dialect::MsSql, &cat(), &cols)
            .into_iter()
            .map(|l| (l.direction, l.constraint, l.columns))
            .collect()
    }

    use LinkDirection::{ReferencedBy as By, References as To};

    #[test]
    fn keys_of_one_table() {
        assert_eq!(
            links(
                "SELECT * FROM dbo.Orders WHERE Total > 0",
                &["Id", "CustomerId", "Total"]
            ),
            vec![
                (To, "FK_Orders_Customers".into(), vec![1]),
                (By, "FK_OrderLines_Orders".into(), vec![0]),
            ]
        );
        // only the key columns in the result count
        assert_eq!(
            links(
                "SELECT TOP (10) Total, CustomerId AS Cust FROM Orders",
                &["Total", "Cust"]
            ),
            vec![(To, "FK_Orders_Customers".into(), vec![1])]
        );
        // a composite key needs all its columns
        assert_eq!(
            links("SELECT OrderId, Qty FROM dbo.OrderLines", &["OrderId", "Qty"]),
            vec![(To, "FK_OrderLines_Orders".into(), vec![0])]
        );
        assert_eq!(
            links("SELECT * FROM dbo.Shipments", &["Id", "OrderId", "LineNo"]),
            vec![(To, "FK_Shipments_OrderLines".into(), vec![1, 2])]
        );
    }

    #[test]
    fn joins_trace_columns_to_their_tables() {
        let sql = "SELECT o.Id, c.Name, o.CustomerId, c.Id AS CustId FROM dbo.Orders o JOIN dbo.Customers c ON c.Id = o.CustomerId";
        assert_eq!(
            links(sql, &["Id", "Name", "CustomerId", "CustId"]),
            vec![
                (To, "FK_Orders_Customers".into(), vec![2]),
                (By, "FK_OrderLines_Orders".into(), vec![0]),
                (By, "FK_Orders_Customers".into(), vec![3]),
            ]
        );
        // `*` lists the tables in FROM order
        let star = "SELECT * FROM dbo.Orders o INNER JOIN dbo.Customers c ON c.Id = o.CustomerId";
        assert_eq!(
            links(star, &["Id", "CustomerId", "Total", "Id", "Name"]),
            vec![
                (To, "FK_Orders_Customers".into(), vec![1]),
                (By, "FK_OrderLines_Orders".into(), vec![0]),
                (By, "FK_Orders_Customers".into(), vec![3]),
            ]
        );
        // unqualified names belong to the one table that has them; `Id` is in both
        assert_eq!(
            links(
                "SELECT Id, CustomerId, Name FROM Orders o JOIN Customers c ON c.Id = o.CustomerId",
                &["Id", "CustomerId", "Name"]
            ),
            vec![(To, "FK_Orders_Customers".into(), vec![1])]
        );
        // tables of a subquery are not part of the result
        assert_eq!(
            links(
                "SELECT * FROM dbo.Orders WHERE CustomerId IN (SELECT Id FROM dbo.Customers)",
                &["Id", "CustomerId", "Total"]
            )
            .len(),
            2
        );
    }

    #[test]
    fn results_that_cannot_be_traced() {
        for (sql, cols) in [
            ("SELECT CustomerId + 0 AS c FROM Orders", vec!["c"]),
            (
                "SELECT * FROM Orders UNION ALL SELECT * FROM Orders",
                vec!["Id", "CustomerId", "Total"],
            ),
            ("SELECT * FROM Orders; SELECT 1", vec!["Id", "CustomerId", "Total"]),
            // the result does not match the loaded columns (reload objects)
            ("SELECT * FROM Orders", vec!["Id", "CustomerId"]),
            // a derived table's columns are not known
            (
                "SELECT * FROM (SELECT Id FROM Orders) x JOIN Customers c ON c.Id = x.Id",
                vec!["Id", "Id", "Name"],
            ),
            ("UPDATE Orders SET Total = 0", vec![]),
            ("SELECT * FROM #work", vec!["Id"]),
        ] {
            let cols: Vec<&str> = cols;
            assert!(links(sql, &cols).is_empty(), "{sql}");
        }
    }

    #[test]
    fn queries_that_open_a_link() {
        let c = cat();
        let all = |sql: &str, cols: &[&str]| {
            let cols: Vec<String> = cols.iter().map(|s| s.to_string()).collect();
            result_links(sql, Dialect::MsSql, &c, &cols)
        };
        let order = all("SELECT * FROM dbo.Orders", &["Id", "CustomerId", "Total"]);
        assert_eq!(
            link_query(&order[0], &[Some("42".into())], &c, Dialect::MsSql).unwrap(),
            "SELECT *\nFROM dbo.Customers\nWHERE Id = 42"
        );
        assert_eq!(
            link_query(&order[1], &[Some("7".into())], &c, Dialect::MsSql).unwrap(),
            "SELECT *\nFROM dbo.OrderLines\nWHERE OrderId = 7"
        );
        assert!(
            link_query(&order[0], &[None], &c, Dialect::MsSql)
                .unwrap_err()
                .contains("NULL")
        );
        assert!(link_query(&order[0], &[Some("x'; DROP TABLE t --".into())], &c, Dialect::MsSql).is_err());
        let ship = all("SELECT * FROM dbo.Shipments", &["Id", "OrderId", "LineNo"]);
        assert_eq!(
            link_query(&ship[0], &[Some("7".into()), Some("2".into())], &c, Dialect::MsSql).unwrap(),
            "SELECT *\nFROM dbo.OrderLines\nWHERE OrderId = 7\n  AND [LineNo] = 2"
        );
        // text keys are quoted for the column's type
        let mut named = order[0].clone();
        named.target_columns = vec!["Name".into()];
        assert_eq!(
            link_query(&named, &[Some("O'Hara".into())], &c, Dialect::Postgres).unwrap(),
            "SELECT *\nFROM dbo.\"Customers\"\nWHERE \"Name\" = 'O''Hara'"
        );
    }
}
