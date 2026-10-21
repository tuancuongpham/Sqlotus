//! Index health: missing, unused, duplicate and overlapping indexes, plus
//! tables read mostly by full scans. Read-only; every fix is a script for
//! the user to review.

use serde::Serialize;
use sqlotus_core::Dialect;
use sqlotus_core::indexes::{self, FindingKind, IndexFinding, IndexInfo};
use sqlotus_db::Connection;
use tauri::State;

use crate::commands::{CmdResult, open_database};
use crate::state::AppState;

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct IndexHealth {
    findings: Vec<IndexFinding>,
    /// Context for reading the numbers (e.g. since when statistics count).
    notes: Vec<String>,
    /// Checks that could not run (usually missing permissions).
    errors: Vec<String>,
}

type Rows = Vec<Vec<Option<String>>>;

fn s(r: &[Option<String>], i: usize) -> String {
    r.get(i).cloned().flatten().unwrap_or_default()
}
fn n(r: &[Option<String>], i: usize) -> Option<i64> {
    r.get(i)
        .cloned()
        .flatten()
        .and_then(|v| v.split('.').next().and_then(|x| x.parse().ok()))
}
fn f(r: &[Option<String>], i: usize) -> f64 {
    r.get(i).cloned().flatten().and_then(|v| v.parse().ok()).unwrap_or(0.0)
}
fn list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|c| c.trim().trim_matches(|x| x == '[' || x == ']').to_string())
        .filter(|c| !c.is_empty())
        .collect()
}
fn flag(r: &[Option<String>], i: usize) -> bool {
    matches!(s(r, i).to_ascii_lowercase().as_str(), "1" | "true" | "t")
}

async fn query(conn: &mut Connection, sql: &str, what: &str, errors: &mut Vec<String>) -> Option<Rows> {
    match conn.query_strings(sql).await {
        Ok(rows) => Some(rows),
        Err(e) => {
            errors.push(format!("{what}: {e}"));
            None
        }
    }
}

const MSSQL_INDEXES: &str = "
SELECT s.name, t.name, i.name, i.is_unique, i.is_primary_key, CASE WHEN i.type = 1 THEN 1 ELSE 0 END,
  STUFF((SELECT ',' + c.name FROM sys.index_columns ic JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
         WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 0 AND ic.key_ordinal > 0
         ORDER BY ic.key_ordinal FOR XML PATH(''), TYPE).value('.', 'nvarchar(max)'), 1, 1, ''),
  STUFF((SELECT ',' + c.name FROM sys.index_columns ic JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id
         WHERE ic.object_id = i.object_id AND ic.index_id = i.index_id AND ic.is_included_column = 1
         ORDER BY c.name FOR XML PATH(''), TYPE).value('.', 'nvarchar(max)'), 1, 1, ''),
  {usage}
FROM sys.indexes i
JOIN sys.tables t ON t.object_id = i.object_id
JOIN sys.schemas s ON s.schema_id = t.schema_id
{usage_join}
WHERE i.type IN (1, 2) AND t.is_ms_shipped = 0 AND i.is_hypothetical = 0";

const MSSQL_USAGE: &str = "ISNULL(u.user_seeks + u.user_scans + u.user_lookups, 0), ISNULL(u.user_updates, 0),
  (SELECT SUM(ps.used_page_count) * 8 FROM sys.dm_db_partition_stats ps WHERE ps.object_id = i.object_id AND ps.index_id = i.index_id)";
const MSSQL_USAGE_JOIN: &str = "LEFT JOIN sys.dm_db_index_usage_stats u ON u.database_id = DB_ID() AND u.object_id = i.object_id AND u.index_id = i.index_id";

const MSSQL_MISSING: &str = "
SELECT TOP (50) OBJECT_SCHEMA_NAME(d.object_id), OBJECT_NAME(d.object_id), d.equality_columns, d.inequality_columns, d.included_columns,
  s.avg_user_impact, s.user_seeks + s.user_scans, s.avg_total_user_cost * s.avg_user_impact * (s.user_seeks + s.user_scans)
FROM sys.dm_db_missing_index_details d
JOIN sys.dm_db_missing_index_groups g ON g.index_handle = d.index_handle
JOIN sys.dm_db_missing_index_group_stats s ON s.group_handle = g.index_group_handle
WHERE d.database_id = DB_ID()
ORDER BY 8 DESC";

const MSSQL_HEAPS: &str = "
SELECT s.name, t.name, SUM(p.rows)
FROM sys.tables t JOIN sys.schemas s ON s.schema_id = t.schema_id
JOIN sys.partitions p ON p.object_id = t.object_id AND p.index_id = 0
WHERE t.is_ms_shipped = 0
GROUP BY s.name, t.name HAVING SUM(p.rows) > 10000";

const PG_INDEXES: &str = "
SELECT n.nspname, t.relname, i.relname, ix.indisunique, ix.indisprimary, ix.indisclustered,
  (SELECT string_agg(a.attname, ',' ORDER BY k.ord) FROM unnest(ix.indkey) WITH ORDINALITY k(attnum, ord)
     JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum WHERE k.ord <= ix.indnkeyatts),
  (SELECT string_agg(a.attname, ',' ORDER BY k.ord) FROM unnest(ix.indkey) WITH ORDINALITY k(attnum, ord)
     JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum WHERE k.ord > ix.indnkeyatts),
  s.idx_scan, st.n_tup_ins + st.n_tup_upd + st.n_tup_del, pg_relation_size(ix.indexrelid) / 1024
FROM pg_index ix
JOIN pg_class i ON i.oid = ix.indexrelid
JOIN pg_class t ON t.oid = ix.indrelid
JOIN pg_namespace n ON n.oid = t.relnamespace
LEFT JOIN pg_stat_user_indexes s ON s.indexrelid = ix.indexrelid
LEFT JOIN pg_stat_user_tables st ON st.relid = t.oid
WHERE n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_toast%'
  AND ix.indexprs IS NULL AND ix.indpred IS NULL";

const PG_SCANS: &str = "
SELECT schemaname, relname, seq_scan, COALESCE(idx_scan, 0), n_live_tup
FROM pg_stat_user_tables
WHERE n_live_tup > 10000 AND seq_scan > COALESCE(idx_scan, 0) * 10
ORDER BY seq_scan * n_live_tup DESC LIMIT 50";

const MYSQL_INDEXES: &str = "
SELECT TABLE_NAME, INDEX_NAME, NON_UNIQUE, COLUMN_NAME
FROM information_schema.STATISTICS
WHERE TABLE_SCHEMA = DATABASE()
ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX";

const MYSQL_USAGE: &str = "
SELECT OBJECT_NAME, INDEX_NAME, COUNT_READ, COUNT_WRITE
FROM performance_schema.table_io_waits_summary_by_index_usage
WHERE OBJECT_SCHEMA = DATABASE() AND INDEX_NAME IS NOT NULL";

const MYSQL_SCANS: &str = "
SELECT object_name, rows_full_scanned
FROM sys.schema_tables_with_full_table_scans
WHERE object_schema = DATABASE() AND rows_full_scanned > 100000
ORDER BY rows_full_scanned DESC LIMIT 50";

async fn mssql(conn: &mut Connection, out: &mut IndexHealth) -> Vec<IndexInfo> {
    let full = MSSQL_INDEXES
        .replace("{usage}", MSSQL_USAGE)
        .replace("{usage_join}", MSSQL_USAGE_JOIN);
    let mut usage_known = true;
    let rows = match conn.query_strings(&full).await {
        Ok(r) => r,
        Err(e) => {
            // usage statistics need VIEW SERVER STATE: still find duplicates
            out.errors.push(format!(
                "Index usage statistics are not available ({e}). Duplicates are still checked."
            ));
            usage_known = false;
            let plain = MSSQL_INDEXES
                .replace("{usage}", "NULL, NULL, NULL")
                .replace("{usage_join}", "");
            query(conn, &plain, "Indexes", &mut out.errors)
                .await
                .unwrap_or_default()
        }
    };
    if usage_known
        && let Ok(r) = conn
            .query_strings("SELECT CONVERT(varchar(16), sqlserver_start_time, 120) FROM sys.dm_os_sys_info")
            .await
        && let Some(since) = r.first().map(|x| s(x, 0))
    {
        out.notes.push(format!("Usage counts start at the last SQL Server restart ({since}); an index used only by monthly jobs may look unused."));
    }
    if let Some(rows) = query(conn, MSSQL_MISSING, "Missing index suggestions", &mut out.errors).await {
        for r in rows {
            let mut keys = list(&s(&r, 2));
            keys.extend(list(&s(&r, 3)));
            let include = list(&s(&r, 4));
            let (schema, table) = (s(&r, 0), s(&r, 1));
            out.findings.push(IndexFinding {
                kind: FindingKind::Missing,
                detail: format!(
                    "Queries on {table} would be about {:.0}% cheaper with an index on ({}){} — used {} time(s)",
                    f(&r, 5),
                    keys.join(", "),
                    if include.is_empty() {
                        String::new()
                    } else {
                        format!(" including {}", include.join(", "))
                    },
                    n(&r, 6).unwrap_or(0)
                ),
                score: 100.0 + f(&r, 7).min(1e9).log10().max(0.0) * 10.0,
                script: Some(indexes::create_index_sql(
                    Dialect::MsSql,
                    &schema,
                    &table,
                    &keys,
                    &include,
                )),
                schema,
                table,
                index: None,
            });
        }
    }
    if let Some(rows) = query(conn, MSSQL_HEAPS, "Heaps", &mut out.errors).await {
        for r in rows {
            out.findings.push(IndexFinding {
                kind: FindingKind::Heap,
                schema: s(&r, 0),
                table: s(&r, 1),
                index: None,
                detail: format!(
                    "{} rows without a clustered index: lookups and updates are slower and space is not reclaimed",
                    n(&r, 2).unwrap_or(0)
                ),
                score: 20.0,
                script: None,
            });
        }
    }
    rows.iter()
        .map(|r| IndexInfo {
            schema: s(r, 0),
            table: s(r, 1),
            name: s(r, 2),
            unique: flag(r, 3),
            primary: flag(r, 4),
            clustered: flag(r, 5),
            key_columns: list(&s(r, 6)),
            include_columns: list(&s(r, 7)),
            reads: n(r, 8),
            writes: n(r, 9),
            size_kb: n(r, 10),
        })
        .collect()
}

async fn postgres(conn: &mut Connection, out: &mut IndexHealth) -> Vec<IndexInfo> {
    if let Ok(r) = conn.query_strings("SELECT to_char(stats_reset, 'YYYY-MM-DD HH24:MI') FROM pg_stat_database WHERE datname = current_database()").await {
        let since = r.first().map(|x| s(x, 0)).filter(|v| !v.is_empty()).unwrap_or_else(|| "when the database was created".into());
        out.notes.push(format!("Usage counts start at the last statistics reset ({since})."));
    }
    if let Some(rows) = query(conn, PG_SCANS, "Tables read by full scans", &mut out.errors).await {
        for r in rows {
            out.findings.push(IndexFinding {
                kind: FindingKind::Scans,
                schema: s(&r, 0),
                table: s(&r, 1),
                index: None,
                detail: format!(
                    "{} sequential scans vs {} index scans on {} rows: queries filter it on columns without an index (run a query with Ctrl+L to see which)",
                    n(&r, 2).unwrap_or(0),
                    n(&r, 3).unwrap_or(0),
                    n(&r, 4).unwrap_or(0)
                ),
                score: 90.0,
                script: None,
            });
        }
    }
    query(conn, PG_INDEXES, "Indexes", &mut out.errors)
        .await
        .unwrap_or_default()
        .iter()
        .map(|r| IndexInfo {
            schema: s(r, 0),
            table: s(r, 1),
            name: s(r, 2),
            unique: flag(r, 3),
            primary: flag(r, 4),
            clustered: flag(r, 5),
            key_columns: list(&s(r, 6)),
            include_columns: list(&s(r, 7)),
            reads: n(r, 8),
            writes: n(r, 9),
            size_kb: n(r, 10),
        })
        .collect()
}

async fn mysql(conn: &mut Connection, out: &mut IndexHealth) -> Vec<IndexInfo> {
    let rows = query(conn, MYSQL_INDEXES, "Indexes", &mut out.errors)
        .await
        .unwrap_or_default();
    let mut list: Vec<IndexInfo> = Vec::new();
    let mut functional: Vec<(String, String)> = Vec::new();
    for r in &rows {
        let (table, name) = (s(r, 0), s(r, 1));
        let Some(col) = r.get(3).cloned().flatten() else {
            functional.push((table, name)); // expression index: keys unknown
            continue;
        };
        match list.iter_mut().find(|i| i.table == table && i.name == name) {
            Some(i) => i.key_columns.push(col),
            None => list.push(IndexInfo {
                schema: String::new(),
                table,
                primary: name == "PRIMARY",
                clustered: name == "PRIMARY",
                unique: s(r, 2) == "0",
                name,
                key_columns: vec![col],
                ..Default::default()
            }),
        }
    }
    list.retain(|i| !functional.contains(&(i.table.clone(), i.name.clone())));
    match conn.query_strings(MYSQL_USAGE).await {
        Ok(usage) => {
            for i in &mut list {
                let u = usage.iter().find(|u| s(u, 0) == i.table && s(u, 1) == i.name);
                i.reads = Some(u.and_then(|u| n(u, 2)).unwrap_or(0));
                i.writes = Some(u.and_then(|u| n(u, 3)).unwrap_or(0));
            }
            out.notes.push("Usage counts start at the last MySQL restart.".into());
        }
        Err(e) => out.errors.push(format!(
            "Index usage statistics are not available (performance_schema: {e}). Duplicates are still checked."
        )),
    }
    if let Ok(scans) = conn.query_strings(MYSQL_SCANS).await {
        for r in scans {
            out.findings.push(IndexFinding {
                kind: FindingKind::Scans,
                schema: String::new(),
                table: s(&r, 0),
                index: None,
                detail: format!(
                    "{} rows read by full table scans: queries filter it on columns without an index",
                    n(&r, 1).unwrap_or(0)
                ),
                score: 90.0,
                script: None,
            });
        }
    }
    list
}

/// Index findings for one database, most important first.
#[tauri::command]
pub async fn index_health(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    dialect: Dialect,
) -> CmdResult<IndexHealth> {
    let mut conn = open_database(&state, &connection_id, &database).await?;
    let mut out = IndexHealth::default();
    let list = match dialect {
        Dialect::MsSql => mssql(&mut conn, &mut out).await,
        Dialect::Postgres => postgres(&mut conn, &mut out).await,
        Dialect::MySql => mysql(&mut conn, &mut out).await,
    };
    conn.close().await;
    let redundant = indexes::redundant_indexes(&list, dialect);
    // an index already reported as redundant is not listed again as unused
    let unused: Vec<IndexFinding> = indexes::unused_indexes(&list, dialect)
        .into_iter()
        .filter(|u| {
            !redundant
                .iter()
                .any(|r| r.index == u.index && r.table == u.table && r.schema == u.schema)
        })
        .collect();
    out.findings.extend(redundant);
    out.findings.extend(unused);
    out.findings
        .sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
    out.notes.push(format!("{} index(es) checked.", list.len()));
    Ok(out)
}
