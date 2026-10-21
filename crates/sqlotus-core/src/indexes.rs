//! Index health from facts the server reports: duplicate and overlapping
//! indexes, indexes nobody reads, and scripts to create or drop them.

use serde::{Deserialize, Serialize};

use crate::Dialect;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct IndexInfo {
    pub schema: String,
    pub table: String,
    pub name: String,
    pub key_columns: Vec<String>,
    pub include_columns: Vec<String>,
    pub unique: bool,
    pub primary: bool,
    pub clustered: bool,
    /// Seeks + scans + lookups since statistics were reset; None if unknown.
    pub reads: Option<i64>,
    /// Updates since statistics were reset; None if unknown.
    pub writes: Option<i64>,
    pub size_kb: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FindingKind {
    /// The server suggests an index for queries it ran.
    Missing,
    /// Same key columns as another index.
    Duplicate,
    /// Key columns are the leading columns of another index.
    Overlapping,
    /// Never read since statistics were reset, but maintained on every write.
    Unused,
    /// A large table read mostly by full scans.
    Scans,
    /// A table without a clustered index (SQL Server).
    Heap,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexFinding {
    pub kind: FindingKind,
    pub schema: String,
    pub table: String,
    pub index: Option<String>,
    pub detail: String,
    /// For ordering: higher is more important.
    pub score: f64,
    /// CREATE or DROP statement to review (never run automatically).
    pub script: Option<String>,
}

fn q(dialect: Dialect, s: &str) -> String {
    dialect.quote_ident(s)
}

fn table_name(dialect: Dialect, schema: &str, table: &str) -> String {
    if schema.is_empty() {
        q(dialect, table)
    } else {
        format!("{}.{}", q(dialect, schema), q(dialect, table))
    }
}

pub fn drop_index_sql(dialect: Dialect, schema: &str, table: &str, name: &str) -> String {
    match dialect {
        Dialect::MsSql => format!(
            "DROP INDEX {} ON {};",
            q(dialect, name),
            table_name(dialect, schema, table)
        ),
        Dialect::Postgres => format!("DROP INDEX {};", table_name(dialect, schema, name)),
        Dialect::MySql => format!(
            "DROP INDEX {} ON {};",
            q(dialect, name),
            table_name(dialect, schema, table)
        ),
    }
}

pub fn create_index_sql(dialect: Dialect, schema: &str, table: &str, keys: &[String], include: &[String]) -> String {
    let clean = |c: &String| {
        c.trim_matches(|x| x == '[' || x == ']' || x == '"' || x == '`')
            .to_string()
    };
    let keys: Vec<String> = keys.iter().map(clean).collect();
    let include: Vec<String> = include.iter().map(clean).filter(|c| !keys.contains(c)).collect();
    let name = format!("IX_{}_{}", table, keys.join("_"))
        .chars()
        .take(120)
        .collect::<String>();
    let cols = keys.iter().map(|c| q(dialect, c)).collect::<Vec<_>>().join(", ");
    let mut s = format!(
        "CREATE {}INDEX {} ON {} ({cols})",
        if dialect == Dialect::MsSql { "NONCLUSTERED " } else { "" },
        q(dialect, &name),
        table_name(dialect, schema, table)
    );
    if !include.is_empty() && dialect != Dialect::MySql {
        s.push_str(&format!(
            " INCLUDE ({})",
            include.iter().map(|c| q(dialect, c)).collect::<Vec<_>>().join(", ")
        ));
    }
    s.push(';');
    s
}

fn lower(v: &[String]) -> Vec<String> {
    v.iter().map(|s| s.to_lowercase()).collect()
}

/// Indexes made redundant by another index on the same table.
pub fn redundant_indexes(indexes: &[IndexInfo], dialect: Dialect) -> Vec<IndexFinding> {
    let mut out = Vec::new();
    let protected = |i: &IndexInfo| i.unique || i.primary || i.clustered;
    for (a_i, a) in indexes.iter().enumerate() {
        if protected(a) || a.key_columns.is_empty() {
            continue;
        }
        let ak = lower(&a.key_columns);
        let ai = lower(&a.include_columns);
        for (b_i, b) in indexes.iter().enumerate() {
            if a_i == b_i || !a.table.eq_ignore_ascii_case(&b.table) || !a.schema.eq_ignore_ascii_case(&b.schema) {
                continue;
            }
            let bk = lower(&b.key_columns);
            let bi = lower(&b.include_columns);
            let covers_includes = ai.iter().all(|c| bk.contains(c) || bi.contains(c));
            let duplicate = ak == bk && covers_includes;
            // of two identical plain indexes, report only the later one
            if duplicate && !protected(b) && ai.len() == bi.len() && b_i > a_i {
                continue;
            }
            let overlapping = !duplicate && bk.len() > ak.len() && bk[..ak.len()] == ak[..] && covers_includes;
            if duplicate || overlapping {
                let kind = if duplicate {
                    FindingKind::Duplicate
                } else {
                    FindingKind::Overlapping
                };
                let detail = format!(
                    "{} ({}) {} {} ({}){}",
                    a.name,
                    a.key_columns.join(", "),
                    if duplicate {
                        "has the same keys as"
                    } else {
                        "is the leading part of"
                    },
                    b.name,
                    b.key_columns.join(", "),
                    match a.size_kb {
                        Some(k) if k > 0 => format!(" — dropping it frees {} and speeds up writes", size(k)),
                        _ => " — dropping it speeds up writes".to_string(),
                    }
                );
                out.push(IndexFinding {
                    kind,
                    schema: a.schema.clone(),
                    table: a.table.clone(),
                    index: Some(a.name.clone()),
                    detail,
                    score: 50.0 + a.size_kb.unwrap_or(0) as f64 / 1024.0,
                    script: Some(drop_index_sql(dialect, &a.schema, &a.table, &a.name)),
                });
                break;
            }
        }
    }
    out
}

fn size(kb: i64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GB", kb as f64 / 1024.0 / 1024.0)
    } else if kb >= 1024 {
        format!("{:.1} MB", kb as f64 / 1024.0)
    } else {
        format!("{kb} KB")
    }
}

/// Indexes with no reads since statistics were reset.
pub fn unused_indexes(indexes: &[IndexInfo], dialect: Dialect) -> Vec<IndexFinding> {
    indexes
        .iter()
        .filter(|i| !i.unique && !i.primary && !i.clustered && i.reads == Some(0))
        .map(|i| IndexFinding {
            kind: FindingKind::Unused,
            schema: i.schema.clone(),
            table: i.table.clone(),
            index: Some(i.name.clone()),
            detail: format!(
                "{} ({}) was not read, but was updated {} time(s){}",
                i.name,
                i.key_columns.join(", "),
                i.writes.unwrap_or(0),
                i.size_kb
                    .filter(|k| *k > 0)
                    .map(|k| format!("; size {}", size(k)))
                    .unwrap_or_default()
            ),
            score: 10.0 + i.writes.unwrap_or(0) as f64 / 1000.0,
            script: Some(drop_index_sql(dialect, &i.schema, &i.table, &i.name)),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ix(name: &str, keys: &[&str], include: &[&str]) -> IndexInfo {
        IndexInfo {
            schema: "dbo".into(),
            table: "Orders".into(),
            name: name.into(),
            key_columns: keys.iter().map(|s| s.to_string()).collect(),
            include_columns: include.iter().map(|s| s.to_string()).collect(),
            reads: Some(5),
            writes: Some(100),
            size_kb: Some(2048),
            ..Default::default()
        }
    }

    #[test]
    fn finds_duplicates_and_overlaps() {
        let pk = IndexInfo {
            primary: true,
            clustered: true,
            ..ix("PK_Orders", &["Id"], &[])
        };
        let list = vec![
            pk,
            ix("IX_a", &["CustomerId"], &[]),
            ix("IX_b", &["CustomerId", "OrderDate"], &[]),
            ix("IX_c", &["Total"], &[]),
            ix("IX_c2", &["total"], &[]),
            ix("IX_d", &["Status"], &["Total"]),
            ix("IX_e", &["Status", "OrderDate"], &[]),
            ix("IX_id", &["Id"], &[]),
        ];
        let found: Vec<(String, FindingKind)> = redundant_indexes(&list, Dialect::MsSql)
            .into_iter()
            .map(|f| (f.index.unwrap(), f.kind))
            .collect();
        assert_eq!(
            found,
            vec![
                ("IX_a".into(), FindingKind::Overlapping),
                ("IX_c2".into(), FindingKind::Duplicate),
                ("IX_id".into(), FindingKind::Duplicate),
            ],
            "IX_d is kept: IX_e does not include Total"
        );
        let f = &redundant_indexes(&list, Dialect::MsSql)[0];
        assert_eq!(f.script.as_deref(), Some("DROP INDEX IX_a ON dbo.Orders;"));
        assert!(f.detail.contains("frees 2.0 MB"));
    }

    #[test]
    fn unused_and_scripts() {
        let mut a = ix("IX_unused", &["Note"], &[]);
        a.reads = Some(0);
        let mut b = ix("IX_unknown", &["Note2"], &[]);
        b.reads = None;
        let pk = IndexInfo {
            primary: true,
            reads: Some(0),
            ..ix("PK", &["Id"], &[])
        };
        let found = unused_indexes(&[a, b, pk], Dialect::Postgres);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].script.as_deref(), Some("DROP INDEX dbo.\"IX_unused\";"));
        assert_eq!(
            create_index_sql(
                Dialect::MsSql,
                "dbo",
                "Orders",
                &["[CustomerId]".into()],
                &["[Total]".into(), "[CustomerId]".into()]
            ),
            "CREATE NONCLUSTERED INDEX IX_Orders_CustomerId ON dbo.Orders (CustomerId) INCLUDE (Total);"
        );
        assert_eq!(
            create_index_sql(Dialect::MySql, "", "orders", &["status".into()], &["total".into()]),
            "CREATE INDEX IX_orders_status ON orders (status);"
        );
    }
}
