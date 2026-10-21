//! Loading the full schema of a database for schema compare: columns with
//! types and defaults, keys, indexes, foreign keys, checks, and the
//! executable definitions of views, routines and triggers.

use sqlotus_core::Dialect;
use sqlotus_core::catalog::ObjectKind;
use sqlotus_core::schemadiff::{
    CheckDef, ColumnDef, ForeignKeyDef, IndexDef, KeyDef, ModuleDef, SchemaModel, TableDef,
};

use crate::{Connection, DbError, Result};

type Row = Vec<Option<String>>;

fn s(r: &Row, i: usize) -> String {
    r.get(i).cloned().flatten().unwrap_or_default()
}
fn opt(r: &Row, i: usize) -> Option<String> {
    r.get(i).cloned().flatten().filter(|v| !v.is_empty())
}
fn n(r: &Row, i: usize) -> i64 {
    s(r, i).split('.').next().and_then(|v| v.parse().ok()).unwrap_or(0)
}
fn b(r: &Row, i: usize) -> bool {
    matches!(s(r, i).to_ascii_lowercase().as_str(), "1" | "true" | "t" | "yes")
}
fn list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

fn table_mut<'a>(tables: &'a mut Vec<TableDef>, schema: &str, name: &str) -> &'a mut TableDef {
    if let Some(i) = tables.iter().position(|t| t.schema == schema && t.name == name) {
        return &mut tables[i];
    }
    tables.push(TableDef {
        schema: schema.to_string(),
        name: name.to_string(),
        ..Default::default()
    });
    tables.last_mut().expect("just pushed")
}

/// The schema of the connection's current database.
pub async fn load_schema(conn: &mut Connection) -> Result<SchemaModel> {
    match conn.dialect() {
        Dialect::MsSql => mssql(conn).await,
        Dialect::Postgres => postgres(conn).await,
        Dialect::MySql => mysql(conn).await,
    }
}

// ---------------------------------------------------------------------------
// SQL Server
// ---------------------------------------------------------------------------

fn mssql_type(t: &str, max_length: i64, precision: i64, scale: i64) -> String {
    match t {
        "varchar" | "char" | "varbinary" | "binary" => {
            if max_length < 0 {
                format!("{t}(max)")
            } else {
                format!("{t}({max_length})")
            }
        }
        "nvarchar" | "nchar" => {
            if max_length < 0 {
                format!("{t}(max)")
            } else {
                format!("{t}({})", max_length / 2)
            }
        }
        "decimal" | "numeric" => format!("{t}({precision}, {scale})"),
        "datetime2" | "time" | "datetimeoffset" => format!("{t}({scale})"),
        _ => t.to_string(),
    }
}

fn mssql_action(a: &str) -> Option<String> {
    match a {
        "" | "NO_ACTION" => None,
        other => Some(other.replace('_', " ")),
    }
}

async fn mssql(conn: &mut Connection) -> Result<SchemaModel> {
    // without VIEW DEFINITION, SQL Server hides routines and blanks defaults
    // and checks: objects would look missing and a sync script would drop them
    let allowed = conn
        .query_strings("SELECT HAS_PERMS_BY_NAME(DB_NAME(), 'DATABASE', 'VIEW DEFINITION')")
        .await?;
    if allowed.first().map(|r| s(r, 0)) != Some("1".to_string()) {
        let db = conn.current_database().await.unwrap_or_default();
        return Err(DbError::Query(format!(
            "Schema compare needs the VIEW DEFINITION permission on {db}. Without it SQL Server hides procedures, functions, \
             defaults and checks, so they would look missing and the script would drop them. \
             Ask an administrator for: GRANT VIEW DEFINITION TO [your login];"
        )));
    }
    let mut tables: Vec<TableDef> = Vec::new();
    let cols = conn
        .query_strings(
            "SELECT s.name, t.name, c.name, TYPE_NAME(c.user_type_id), c.max_length, c.precision, c.scale, c.is_nullable, \
                    c.is_identity, cc.definition, dc.name, dc.definition \
             FROM sys.tables t JOIN sys.schemas s ON s.schema_id = t.schema_id \
             JOIN sys.columns c ON c.object_id = t.object_id \
             LEFT JOIN sys.computed_columns cc ON cc.object_id = c.object_id AND cc.column_id = c.column_id \
             LEFT JOIN sys.default_constraints dc ON dc.parent_object_id = c.object_id AND dc.parent_column_id = c.column_id \
             WHERE t.is_ms_shipped = 0 ORDER BY s.name, t.name, c.column_id",
        )
        .await?;
    for r in cols {
        let t = table_mut(&mut tables, &s(&r, 0), &s(&r, 1));
        t.columns.push(ColumnDef {
            name: s(&r, 2),
            data_type: mssql_type(&s(&r, 3), n(&r, 4), n(&r, 5), n(&r, 6)),
            nullable: b(&r, 7),
            identity: b(&r, 8),
            computed: opt(&r, 9),
            default_name: opt(&r, 10),
            default: opt(&r, 11),
        });
    }
    let idx = conn
        .query_strings(
            "SELECT s.name, t.name, i.name, i.is_primary_key, i.is_unique, i.type, i.filter_definition, i.is_unique_constraint, \
                    c.name, ic.is_descending_key, ic.is_included_column \
             FROM sys.indexes i JOIN sys.tables t ON t.object_id = i.object_id JOIN sys.schemas s ON s.schema_id = t.schema_id \
             JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
             JOIN sys.columns c ON c.object_id = ic.object_id AND c.column_id = ic.column_id \
             WHERE t.is_ms_shipped = 0 AND i.type IN (1, 2) AND i.is_hypothetical = 0 \
             ORDER BY s.name, t.name, i.name, ic.is_included_column, ic.key_ordinal, ic.index_column_id",
        )
        .await?;
    for r in idx {
        let t = table_mut(&mut tables, &s(&r, 0), &s(&r, 1));
        let (name, col, desc, included) = (s(&r, 2), s(&r, 8), b(&r, 9), b(&r, 10));
        if b(&r, 3) {
            let pk = t.primary_key.get_or_insert_with(|| KeyDef {
                name: name.clone(),
                columns: vec![],
            });
            pk.columns.push(col);
            continue;
        }
        let i = match t.indexes.iter().position(|i| i.name == name) {
            Some(i) => &mut t.indexes[i],
            None => {
                t.indexes.push(IndexDef {
                    name: name.clone(),
                    unique: b(&r, 4),
                    clustered: s(&r, 5) == "1",
                    filter: opt(&r, 6),
                    constraint: b(&r, 7),
                    ..Default::default()
                });
                t.indexes.last_mut().expect("just pushed")
            }
        };
        if included {
            i.include.push(col);
        } else {
            i.columns.push(if desc { format!("{col} DESC") } else { col });
        }
    }
    let fks = conn
        .query_strings(
            "SELECT s.name, t.name, fk.name, pc.name, rs.name, rt.name, rc.name, fk.delete_referential_action_desc, \
                    fk.update_referential_action_desc \
             FROM sys.foreign_keys fk JOIN sys.tables t ON t.object_id = fk.parent_object_id \
             JOIN sys.schemas s ON s.schema_id = t.schema_id \
             JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
             JOIN sys.columns pc ON pc.object_id = fkc.parent_object_id AND pc.column_id = fkc.parent_column_id \
             JOIN sys.tables rt ON rt.object_id = fk.referenced_object_id JOIN sys.schemas rs ON rs.schema_id = rt.schema_id \
             JOIN sys.columns rc ON rc.object_id = fkc.referenced_object_id AND rc.column_id = fkc.referenced_column_id \
             WHERE t.is_ms_shipped = 0 ORDER BY s.name, t.name, fk.name, fkc.constraint_column_id",
        )
        .await?;
    for r in fks {
        let t = table_mut(&mut tables, &s(&r, 0), &s(&r, 1));
        let name = s(&r, 2);
        match t.foreign_keys.iter_mut().find(|f| f.name == name) {
            Some(f) => {
                f.columns.push(s(&r, 3));
                f.ref_columns.push(s(&r, 6));
            }
            None => t.foreign_keys.push(ForeignKeyDef {
                name,
                columns: vec![s(&r, 3)],
                ref_schema: s(&r, 4),
                ref_table: s(&r, 5),
                ref_columns: vec![s(&r, 6)],
                on_delete: mssql_action(&s(&r, 7)),
                on_update: mssql_action(&s(&r, 8)),
            }),
        }
    }
    let checks = conn
        .query_strings(
            "SELECT s.name, t.name, cc.name, cc.definition FROM sys.check_constraints cc \
             JOIN sys.tables t ON t.object_id = cc.parent_object_id JOIN sys.schemas s ON s.schema_id = t.schema_id \
             WHERE t.is_ms_shipped = 0 ORDER BY cc.name",
        )
        .await?;
    for r in checks {
        table_mut(&mut tables, &s(&r, 0), &s(&r, 1)).checks.push(CheckDef {
            name: s(&r, 2),
            definition: s(&r, 3),
        });
    }
    let mods = conn
        .query_strings(
            "SELECT s.name, o.name, RTRIM(o.type), m.definition, OBJECT_NAME(o.parent_object_id) \
             FROM sys.sql_modules m JOIN sys.objects o ON o.object_id = m.object_id JOIN sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.is_ms_shipped = 0 AND o.type IN ('V', 'P', 'FN', 'IF', 'TF', 'TR') ORDER BY s.name, o.name",
        )
        .await?;
    let mut modules = Vec::new();
    let mut unreadable = Vec::new();
    for r in mods {
        let Some(kind) = crate::kind_from_mssql(&s(&r, 2)) else {
            continue;
        };
        // without VIEW DEFINITION the text is NULL
        match opt(&r, 3) {
            Some(definition) => modules.push(ModuleDef {
                schema: s(&r, 0),
                name: s(&r, 1),
                kind,
                definition,
                table: opt(&r, 4),
            }),
            None => unreadable.push(format!("{}.{}", s(&r, 0), s(&r, 1))),
        }
    }
    Ok(SchemaModel {
        tables,
        modules,
        unreadable,
    })
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

const PG_USER: &str = "n.nspname NOT IN ('pg_catalog', 'information_schema') AND n.nspname NOT LIKE 'pg_toast%' AND n.nspname NOT LIKE 'pg_temp%'";

fn pg_action(code: &str) -> Option<String> {
    match code {
        "r" => Some("RESTRICT".into()),
        "c" => Some("CASCADE".into()),
        "n" => Some("SET NULL".into()),
        "d" => Some("SET DEFAULT".into()),
        _ => None,
    }
}

async fn postgres(conn: &mut Connection) -> Result<SchemaModel> {
    let mut tables: Vec<TableDef> = Vec::new();
    let cols = conn
        .query_strings(&format!(
            "SELECT n.nspname, c.relname, a.attname, format_type(a.atttypid, a.atttypmod), NOT a.attnotnull, \
                    a.attidentity::text, pg_get_expr(d.adbin, d.adrelid), a.attgenerated::text \
             FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE c.relkind IN ('r', 'p') AND a.attnum > 0 AND NOT a.attisdropped AND NOT c.relispartition AND {PG_USER} \
             ORDER BY n.nspname, c.relname, a.attnum"
        ))
        .await?;
    for r in cols {
        let t = table_mut(&mut tables, &s(&r, 0), &s(&r, 1));
        let default = opt(&r, 6);
        let generated = s(&r, 7) == "s";
        // serial columns: a sequence default works like identity
        let serial = default.as_deref().is_some_and(|d| d.starts_with("nextval("));
        t.columns.push(ColumnDef {
            name: s(&r, 2),
            data_type: s(&r, 3),
            nullable: b(&r, 4),
            identity: !s(&r, 5).is_empty() || serial,
            computed: if generated { default.clone() } else { None },
            default: if generated || serial { None } else { default },
            default_name: None,
        });
    }
    let cons = conn
        .query_strings(&format!(
            "SELECT n.nspname, c.relname, con.conname, con.contype::text, \
               (SELECT string_agg(a.attname, ',' ORDER BY k.ord) FROM unnest(con.conkey) WITH ORDINALITY k(attnum, ord) \
                  JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum), \
               rn.nspname, rc.relname, \
               (SELECT string_agg(a.attname, ',' ORDER BY k.ord) FROM unnest(con.confkey) WITH ORDINALITY k(attnum, ord) \
                  JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum), \
               con.confdeltype::text, con.confupdtype::text, pg_get_constraintdef(con.oid) \
             FROM pg_constraint con JOIN pg_class c ON c.oid = con.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_class rc ON rc.oid = con.confrelid LEFT JOIN pg_namespace rn ON rn.oid = rc.relnamespace \
             WHERE con.contype IN ('p', 'u', 'f', 'c') AND c.relkind IN ('r', 'p') AND {PG_USER} \
             ORDER BY n.nspname, c.relname, con.conname"
        ))
        .await?;
    for r in cons {
        let t = table_mut(&mut tables, &s(&r, 0), &s(&r, 1));
        let (name, kind, cols) = (s(&r, 2), s(&r, 3), list(&s(&r, 4)));
        match kind.as_str() {
            "p" => t.primary_key = Some(KeyDef { name, columns: cols }),
            "u" => t.indexes.push(IndexDef {
                name,
                columns: cols,
                unique: true,
                constraint: true,
                ..Default::default()
            }),
            "f" => t.foreign_keys.push(ForeignKeyDef {
                name,
                columns: cols,
                ref_schema: s(&r, 5),
                ref_table: s(&r, 6),
                ref_columns: list(&s(&r, 7)),
                on_delete: pg_action(&s(&r, 8)),
                on_update: pg_action(&s(&r, 9)),
            }),
            _ => t.checks.push(CheckDef {
                name,
                definition: s(&r, 10),
            }),
        }
    }
    let idx = conn
        .query_strings(&format!(
            "SELECT n.nspname, t.relname, i.relname, ix.indisunique, pg_get_indexdef(ix.indexrelid), \
               (SELECT string_agg(a.attname, ',' ORDER BY k.ord) FROM unnest(ix.indkey) WITH ORDINALITY k(attnum, ord) \
                  JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum WHERE k.ord <= ix.indnkeyatts), \
               (SELECT string_agg(a.attname, ',' ORDER BY k.ord) FROM unnest(ix.indkey) WITH ORDINALITY k(attnum, ord) \
                  JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.attnum WHERE k.ord > ix.indnkeyatts), \
               pg_get_expr(ix.indpred, ix.indrelid) \
             FROM pg_index ix JOIN pg_class i ON i.oid = ix.indexrelid JOIN pg_class t ON t.oid = ix.indrelid \
             JOIN pg_namespace n ON n.oid = t.relnamespace \
             WHERE t.relkind IN ('r', 'p') AND {PG_USER} \
               AND NOT EXISTS (SELECT 1 FROM pg_constraint con WHERE con.conindid = ix.indexrelid) \
             ORDER BY n.nspname, t.relname, i.relname"
        ))
        .await?;
    for r in idx {
        table_mut(&mut tables, &s(&r, 0), &s(&r, 1)).indexes.push(IndexDef {
            name: s(&r, 2),
            unique: b(&r, 3),
            definition: opt(&r, 4),
            columns: list(&s(&r, 5)),
            include: list(&s(&r, 6)),
            filter: opt(&r, 7),
            ..Default::default()
        });
    }
    let mut modules = Vec::new();
    let views = conn
        .query_strings(&format!(
            "SELECT n.nspname, c.relname, pg_get_viewdef(c.oid, true) FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind = 'v' AND {PG_USER} \
               AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = c.oid AND d.deptype = 'e') ORDER BY 1, 2"
        ))
        .await?;
    let dialect = Dialect::Postgres;
    for r in views {
        let (schema, name) = (s(&r, 0), s(&r, 1));
        modules.push(ModuleDef {
            definition: format!(
                "CREATE OR REPLACE VIEW {}.{} AS\n{}",
                dialect.quote_ident(&schema),
                dialect.quote_ident(&name),
                s(&r, 2).trim_end()
            ),
            schema,
            name,
            kind: ObjectKind::View,
            table: None,
        });
    }
    let funcs = conn
        .query_strings(&format!(
            "SELECT n.nspname, p.proname, p.prokind::text, p.proretset::text, pg_get_functiondef(p.oid) \
             FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE p.prokind IN ('f', 'p') AND {PG_USER} \
               AND NOT EXISTS (SELECT 1 FROM pg_depend d WHERE d.objid = p.oid AND d.deptype = 'e') ORDER BY 1, 2"
        ))
        .await?;
    for r in funcs {
        let kind = match (s(&r, 2).as_str(), b(&r, 3)) {
            ("p", _) => ObjectKind::Procedure,
            (_, true) => ObjectKind::TableFunction,
            _ => ObjectKind::ScalarFunction,
        };
        if let Some(def) = opt(&r, 4) {
            modules.push(ModuleDef {
                schema: s(&r, 0),
                name: s(&r, 1),
                kind,
                definition: def,
                table: None,
            });
        }
    }
    let triggers = conn
        .query_strings(&format!(
            "SELECT n.nspname, t.tgname, pg_get_triggerdef(t.oid), c.relname FROM pg_trigger t \
             JOIN pg_class c ON c.oid = t.tgrelid JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE NOT t.tgisinternal AND {PG_USER} ORDER BY 1, 2"
        ))
        .await?;
    for r in triggers {
        modules.push(ModuleDef {
            schema: s(&r, 0),
            name: s(&r, 1),
            kind: ObjectKind::Trigger,
            definition: s(&r, 2),
            table: opt(&r, 3),
        });
    }
    Ok(SchemaModel {
        tables,
        modules,
        unreadable: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------

/// `CREATE DEFINER=`root`@`%` PROCEDURE …` → `CREATE PROCEDURE …`, and
/// names qualified with the current database lose the qualifier, so two
/// databases compare equal and the script runs in the target.
fn mysql_portable(def: &str, db: &str) -> String {
    let mut out = def.to_string();
    if let Some(start) = out.find("DEFINER=") {
        let rest = &out[start..];
        let end = rest.find(' ').map(|i| start + i + 1).unwrap_or(out.len());
        out.replace_range(start..end, "");
    }
    out = out
        .replace("ALGORITHM=UNDEFINED ", "")
        .replace("SQL SECURITY DEFINER ", "");
    if !db.is_empty() {
        out = out.replace(&format!("`{db}`."), "");
    }
    out
}

fn mysql_default(r: &Row, type_col: usize, default_col: usize, extra_col: usize) -> Option<String> {
    let raw = r.get(default_col).cloned().flatten()?;
    let extra = s(r, extra_col).to_ascii_uppercase();
    let ty = s(r, type_col).to_ascii_lowercase();
    if extra.contains("DEFAULT_GENERATED") {
        let up = raw.to_ascii_uppercase();
        return Some(if up.starts_with("CURRENT_TIMESTAMP") || raw.starts_with('(') {
            raw
        } else {
            format!("({raw})")
        });
    }
    let numeric = ["int", "decimal", "numeric", "float", "double", "bit", "real"]
        .iter()
        .any(|t| ty.starts_with(t) || ty.contains(&format!(" {t}")));
    Some(if numeric {
        raw
    } else {
        // information_schema shows the plain value: escape it the way MySQL reads it
        Dialect::MySql.string_literal(&raw)
    })
}

fn mysql_action(a: &str) -> Option<String> {
    match a {
        "" | "NO ACTION" => None,
        other => Some(other.to_string()),
    }
}

async fn mysql(conn: &mut Connection) -> Result<SchemaModel> {
    let db = conn.current_database().await.unwrap_or_default();
    let mut tables: Vec<TableDef> = Vec::new();
    let cols = conn
        .query_strings(
            "SELECT c.TABLE_NAME, c.COLUMN_NAME, c.COLUMN_TYPE, c.IS_NULLABLE, c.COLUMN_DEFAULT, c.EXTRA, c.GENERATION_EXPRESSION \
             FROM information_schema.COLUMNS c JOIN information_schema.TABLES t \
               ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
             WHERE c.TABLE_SCHEMA = DATABASE() AND t.TABLE_TYPE = 'BASE TABLE' ORDER BY c.TABLE_NAME, c.ORDINAL_POSITION",
        )
        .await?;
    for r in cols {
        let t = table_mut(&mut tables, "", &s(&r, 0));
        let extra = s(&r, 5).to_ascii_uppercase();
        let generated = extra.contains("GENERATED") && !extra.contains("DEFAULT_GENERATED");
        t.columns.push(ColumnDef {
            name: s(&r, 1),
            data_type: s(&r, 2),
            nullable: s(&r, 3) == "YES",
            default: if generated { None } else { mysql_default(&r, 2, 4, 5) },
            default_name: None,
            identity: extra.contains("AUTO_INCREMENT"),
            computed: if generated { opt(&r, 6) } else { None },
        });
    }
    let idx = conn
        .query_strings(
            "SELECT TABLE_NAME, INDEX_NAME, NON_UNIQUE, COLUMN_NAME, COLLATION, SUB_PART \
             FROM information_schema.STATISTICS WHERE TABLE_SCHEMA = DATABASE() ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX",
        )
        .await?;
    for r in idx {
        let t = table_mut(&mut tables, "", &s(&r, 0));
        let name = s(&r, 1);
        let mut col = s(&r, 3);
        if col.is_empty() {
            continue; // functional index part: not compared
        }
        if let Some(len) = opt(&r, 5) {
            col = format!("{col}({len})");
        }
        if s(&r, 4) == "D" {
            col.push_str(" DESC");
        }
        if name == "PRIMARY" {
            t.primary_key
                .get_or_insert_with(|| KeyDef {
                    name: "PRIMARY".into(),
                    columns: vec![],
                })
                .columns
                .push(col);
            continue;
        }
        match t.indexes.iter_mut().find(|i| i.name == name) {
            Some(i) => i.columns.push(col),
            None => t.indexes.push(IndexDef {
                name,
                columns: vec![col],
                unique: s(&r, 2) == "0",
                ..Default::default()
            }),
        }
    }
    let fks = conn
        .query_strings(
            "SELECT k.TABLE_NAME, k.CONSTRAINT_NAME, k.COLUMN_NAME, k.REFERENCED_TABLE_SCHEMA, k.REFERENCED_TABLE_NAME, \
                    k.REFERENCED_COLUMN_NAME, r.DELETE_RULE, r.UPDATE_RULE \
             FROM information_schema.KEY_COLUMN_USAGE k JOIN information_schema.REFERENTIAL_CONSTRAINTS r \
               ON r.CONSTRAINT_SCHEMA = k.CONSTRAINT_SCHEMA AND r.CONSTRAINT_NAME = k.CONSTRAINT_NAME AND r.TABLE_NAME = k.TABLE_NAME \
             WHERE k.TABLE_SCHEMA = DATABASE() AND k.REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY k.TABLE_NAME, k.CONSTRAINT_NAME, k.ORDINAL_POSITION",
        )
        .await?;
    for r in fks {
        let t = table_mut(&mut tables, "", &s(&r, 0));
        let name = s(&r, 1);
        let ref_schema = if s(&r, 3) == db { String::new() } else { s(&r, 3) };
        match t.foreign_keys.iter_mut().find(|f| f.name == name) {
            Some(f) => {
                f.columns.push(s(&r, 2));
                f.ref_columns.push(s(&r, 5));
            }
            None => t.foreign_keys.push(ForeignKeyDef {
                name,
                columns: vec![s(&r, 2)],
                ref_schema,
                ref_table: s(&r, 4),
                ref_columns: vec![s(&r, 5)],
                on_delete: mysql_action(&s(&r, 6)),
                on_update: mysql_action(&s(&r, 7)),
            }),
        }
    }
    // CHECK constraints exist from MySQL 8.0.16 on
    if let Ok(checks) = conn
        .query_strings(
            "SELECT tc.TABLE_NAME, cc.CONSTRAINT_NAME, cc.CHECK_CLAUSE FROM information_schema.CHECK_CONSTRAINTS cc \
             JOIN information_schema.TABLE_CONSTRAINTS tc ON tc.CONSTRAINT_SCHEMA = cc.CONSTRAINT_SCHEMA AND tc.CONSTRAINT_NAME = cc.CONSTRAINT_NAME \
             WHERE cc.CONSTRAINT_SCHEMA = DATABASE() AND tc.CONSTRAINT_TYPE = 'CHECK'",
        )
        .await
    {
        for r in checks {
            table_mut(&mut tables, "", &s(&r, 0)).checks.push(CheckDef { name: s(&r, 1), definition: format!("CHECK ({})", s(&r, 2)) });
        }
    }
    let quote = |name: &str| format!("`{}`", name.replace('`', "``"));
    let mut modules = Vec::new();
    let mut unreadable = Vec::new();
    let mut objects: Vec<(ObjectKind, String, &str, usize, Option<String>)> = Vec::new();
    for r in conn
        .query_strings("SELECT TABLE_NAME FROM information_schema.VIEWS WHERE TABLE_SCHEMA = DATABASE()")
        .await?
    {
        objects.push((ObjectKind::View, s(&r, 0), "VIEW", 1, None));
    }
    for r in conn
        .query_strings(
            "SELECT ROUTINE_NAME, ROUTINE_TYPE FROM information_schema.ROUTINES WHERE ROUTINE_SCHEMA = DATABASE()",
        )
        .await?
    {
        let proc = s(&r, 1) == "PROCEDURE";
        objects.push((
            if proc {
                ObjectKind::Procedure
            } else {
                ObjectKind::ScalarFunction
            },
            s(&r, 0),
            if proc { "PROCEDURE" } else { "FUNCTION" },
            2,
            None,
        ));
    }
    for r in conn
        .query_strings("SELECT TRIGGER_NAME, EVENT_OBJECT_TABLE FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = DATABASE()")
        .await?
    {
        objects.push((ObjectKind::Trigger, s(&r, 0), "TRIGGER", 2, opt(&r, 1)));
    }
    for (kind, name, what, col, table) in objects {
        // without the privilege the definition is empty or SHOW CREATE fails: never guess
        let def = conn
            .query_strings(&format!("SHOW CREATE {what} {}", quote(&name)))
            .await
            .ok()
            .and_then(|rows| rows.first().and_then(|r| opt(r, col)));
        match def {
            Some(def) => modules.push(ModuleDef {
                schema: String::new(),
                name,
                kind,
                definition: mysql_portable(&def, &db),
                table,
            }),
            None => unreadable.push(format!(".{name}")),
        }
    }
    Ok(SchemaModel {
        tables,
        modules,
        unreadable,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mysql_definitions_are_portable() {
        assert_eq!(
            mysql_portable(
                "CREATE ALGORITHM=UNDEFINED DEFINER=`root`@`%` SQL SECURITY DEFINER VIEW `v` AS select `shop`.`t`.`a` AS `a` from `shop`.`t`",
                "shop"
            ),
            "CREATE VIEW `v` AS select `t`.`a` AS `a` from `t`"
        );
        assert_eq!(
            mysql_portable(
                "CREATE DEFINER=`app`@`localhost` PROCEDURE `p`() BEGIN SELECT 1; END",
                "x"
            ),
            "CREATE PROCEDURE `p`() BEGIN SELECT 1; END"
        );
    }

    #[test]
    fn mysql_defaults() {
        let row = |ty: &str, default: Option<&str>, extra: &str| -> Row {
            vec![Some(ty.into()), default.map(String::from), Some(extra.into())]
        };
        let d = |r: Row| mysql_default(&r, 0, 1, 2);
        assert_eq!(d(row("int", Some("0"), "")).as_deref(), Some("0"));
        assert_eq!(d(row("decimal(12,2)", Some("1.50"), "")).as_deref(), Some("1.50"));
        assert_eq!(d(row("varchar(10)", Some("it's"), "")).as_deref(), Some("'it''s'"));
        // information_schema shows the plain value; the script must escape the backslash
        assert_eq!(
            d(row("varchar(20)", Some("C:\\tmp"), "")).as_deref(),
            Some("'C:\\\\tmp'")
        );
        assert_eq!(
            d(row("datetime", Some("CURRENT_TIMESTAMP"), "DEFAULT_GENERATED")).as_deref(),
            Some("CURRENT_TIMESTAMP")
        );
        assert_eq!(d(row("varchar(5)", None, "")), None);
    }

    #[test]
    fn sql_server_types() {
        assert_eq!(mssql_type("nvarchar", 200, 0, 0), "nvarchar(100)");
        assert_eq!(mssql_type("varchar", -1, 0, 0), "varchar(max)");
        assert_eq!(mssql_type("decimal", 9, 12, 2), "decimal(12, 2)");
        assert_eq!(mssql_type("datetime2", 8, 27, 7), "datetime2(7)");
        assert_eq!(mssql_type("int", 4, 10, 0), "int");
    }
}
