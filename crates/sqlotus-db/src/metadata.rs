//! Loading the catalog (objects, columns, parameters, foreign keys) and the
//! source of programmable objects for SQL Search.

use serde::{Deserialize, Serialize};
use sqlotus_core::Dialect;
use sqlotus_core::catalog::{Catalog, Column, DbObject, ForeignKey, ObjectDefinition, ObjectKind, Param};

use crate::{Connection, Result, kind_from_mssql};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSnapshot {
    pub catalog: Catalog,
    pub definitions: Vec<ObjectDefinition>,
}

type Rows = Vec<Vec<Option<String>>>;

fn s(row: &[Option<String>], i: usize) -> String {
    row.get(i).cloned().flatten().unwrap_or_default()
}

fn b(row: &[Option<String>], i: usize) -> bool {
    matches!(s(row, i).to_ascii_lowercase().as_str(), "1" | "t" | "true" | "yes")
}

fn n(row: &[Option<String>], i: usize) -> i64 {
    s(row, i).parse().unwrap_or(0)
}

pub async fn load_catalog(conn: &mut Connection) -> Result<CatalogSnapshot> {
    match conn.dialect() {
        Dialect::MsSql => load_mssql(conn).await,
        Dialect::Postgres => load_pg(conn).await,
        Dialect::MySql => load_mysql(conn).await,
    }
}

/// Add rows `(schema, name, kind, column...)` to `objects`, grouping
/// consecutive rows of the same object.
fn push_object_row(
    objects: &mut Vec<DbObject>,
    schema: String,
    name: String,
    kind: ObjectKind,
    column: Option<Column>,
) {
    let same = objects.last().is_some_and(|o| o.schema == schema && o.name == name);
    if !same {
        objects.push(DbObject {
            schema,
            name,
            kind,
            columns: Vec::new(),
            params: Vec::new(),
        });
    }
    if let Some(c) = column {
        objects.last_mut().expect("just pushed").columns.push(c);
    }
}

fn attach_params(objects: &mut [DbObject], schema: &str, name: &str, p: Param) {
    if let Some(o) = objects.iter_mut().find(|o| o.schema == schema && o.name == name) {
        o.params.push(p);
    }
}

fn foreign_keys(rows: Rows, split: Option<char>) -> Vec<ForeignKey> {
    let mut fks: Vec<ForeignKey> = Vec::new();
    for r in rows {
        let name = s(&r, 0);
        let cols = |i: usize| -> Vec<String> {
            match split {
                Some(c) => s(&r, i).split(c).map(str::to_string).collect(),
                None => vec![s(&r, i)],
            }
        };
        match fks.last_mut() {
            // one row per column pair, consecutive for the same constraint
            Some(fk) if split.is_none() && fk.name == name && fk.from_table == s(&r, 2) => {
                fk.from_columns.push(s(&r, 3));
                fk.to_columns.push(s(&r, 6));
            }
            _ => fks.push(ForeignKey {
                name,
                from_schema: s(&r, 1),
                from_table: s(&r, 2),
                from_columns: cols(3),
                to_schema: s(&r, 4),
                to_table: s(&r, 5),
                to_columns: cols(6),
            }),
        }
    }
    fks
}

// ---------------------------------------------------------------------------
// SQL Server
// ---------------------------------------------------------------------------

fn mssql_type(name: &str, max_length: i64, precision: i64, scale: i64) -> String {
    match name {
        "varchar" | "char" | "varbinary" | "binary" => {
            if max_length < 0 {
                format!("{name}(max)")
            } else {
                format!("{name}({max_length})")
            }
        }
        "nvarchar" | "nchar" => {
            if max_length < 0 {
                format!("{name}(max)")
            } else {
                format!("{name}({})", max_length / 2)
            }
        }
        "decimal" | "numeric" => format!("{name}({precision}, {scale})"),
        "datetime2" | "time" | "datetimeoffset" if scale != 7 => format!("{name}({scale})"),
        _ => name.to_string(),
    }
}

async fn load_mssql(conn: &mut Connection) -> Result<CatalogSnapshot> {
    let mut cat = Catalog::default();
    let head = conn.query_strings("SELECT DB_NAME(), SCHEMA_NAME()").await?;
    if let Some(r) = head.first() {
        cat.database = s(r, 0);
        cat.default_schema = s(r, 1);
    }
    cat.schemas = conn
        .query_strings(
            "SELECT name FROM sys.schemas WHERE name NOT IN ('sys','INFORMATION_SCHEMA','guest') \
             AND name NOT LIKE 'db[_]%' ORDER BY name",
        )
        .await?
        .iter()
        .map(|r| s(r, 0))
        .collect();

    let rows = conn
        .query_strings(
            "SELECT s.name, o.name, RTRIM(o.type), c.name, TYPE_NAME(c.user_type_id), c.max_length, c.precision, c.scale, \
                    c.is_nullable, c.is_identity, CASE WHEN pk.column_id IS NULL THEN 0 ELSE 1 END, c.is_computed \
             FROM sys.objects o \
             JOIN sys.schemas s ON s.schema_id = o.schema_id \
             LEFT JOIN sys.columns c ON c.object_id = o.object_id \
             LEFT JOIN (SELECT ic.object_id, ic.column_id FROM sys.indexes i \
                        JOIN sys.index_columns ic ON ic.object_id = i.object_id AND ic.index_id = i.index_id \
                        WHERE i.is_primary_key = 1) pk ON pk.object_id = c.object_id AND pk.column_id = c.column_id \
             WHERE o.type IN ('U','V','P','PC','FN','FS','IF','TF','FT','SN','TR','SO') AND o.is_ms_shipped = 0 \
             ORDER BY s.name, o.name, c.column_id",
        )
        .await?;
    for r in rows {
        let Some(kind) = kind_from_mssql(&s(&r, 2)) else {
            continue;
        };
        let column = r.get(3).cloned().flatten().map(|name| Column {
            name,
            data_type: mssql_type(&s(&r, 4), n(&r, 5), n(&r, 6), n(&r, 7)),
            nullable: b(&r, 8),
            is_identity: b(&r, 9),
            is_primary_key: b(&r, 10),
            is_computed: b(&r, 11),
        });
        push_object_row(&mut cat.objects, s(&r, 0), s(&r, 1), kind, column);
    }

    let params = conn
        .query_strings(
            "SELECT s.name, o.name, p.name, TYPE_NAME(p.user_type_id), p.max_length, p.precision, p.scale, \
                    p.is_output, p.has_default_value \
             FROM sys.parameters p JOIN sys.objects o ON o.object_id = p.object_id \
             JOIN sys.schemas s ON s.schema_id = o.schema_id \
             WHERE p.parameter_id > 0 AND o.is_ms_shipped = 0 ORDER BY s.name, o.name, p.parameter_id",
        )
        .await?;
    for r in params {
        let p = Param {
            name: s(&r, 2),
            data_type: mssql_type(&s(&r, 3), n(&r, 4), n(&r, 5), n(&r, 6)),
            is_output: b(&r, 7),
            has_default: b(&r, 8),
        };
        attach_params(&mut cat.objects, &s(&r, 0), &s(&r, 1), p);
    }

    let fk_rows = conn
        .query_strings(
            "SELECT fk.name, ss.name, so.name, sc.name, ts.name, tobj.name, tc.name \
             FROM sys.foreign_keys fk \
             JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
             JOIN sys.objects so ON so.object_id = fk.parent_object_id JOIN sys.schemas ss ON ss.schema_id = so.schema_id \
             JOIN sys.columns sc ON sc.object_id = fkc.parent_object_id AND sc.column_id = fkc.parent_column_id \
             JOIN sys.objects tobj ON tobj.object_id = fk.referenced_object_id JOIN sys.schemas ts ON ts.schema_id = tobj.schema_id \
             JOIN sys.columns tc ON tc.object_id = fkc.referenced_object_id AND tc.column_id = fkc.referenced_column_id \
             ORDER BY fk.object_id, fkc.constraint_column_id",
        )
        .await?;
    cat.foreign_keys = foreign_keys(fk_rows, None);

    let defs = conn
        .query_strings(
            "SELECT s.name, o.name, RTRIM(o.type), m.definition FROM sys.sql_modules m \
             JOIN sys.objects o ON o.object_id = m.object_id JOIN sys.schemas s ON s.schema_id = o.schema_id \
             WHERE o.is_ms_shipped = 0",
        )
        .await?;
    let definitions: Vec<ObjectDefinition> = defs
        .into_iter()
        .filter_map(|r| {
            Some(ObjectDefinition {
                schema: s(&r, 0),
                name: s(&r, 1),
                kind: kind_from_mssql(&s(&r, 2))?,
                definition: s(&r, 3),
            })
        })
        .collect();
    // sys.parameters.has_default_value is only set for CLR modules; read
    // T-SQL defaults (`@p int = 0`) from the module header instead.
    for d in &definitions {
        let defaults = tsql_parameter_defaults(&d.definition);
        if defaults.is_empty() {
            continue;
        }
        if let Some(o) = cat
            .objects
            .iter_mut()
            .find(|o| o.schema == d.schema && o.name == d.name)
        {
            for p in &mut o.params {
                if defaults.iter().any(|n| n.eq_ignore_ascii_case(&p.name)) {
                    p.has_default = true;
                }
            }
        }
    }
    Ok(CatalogSnapshot {
        catalog: cat,
        definitions,
    })
}

/// Parameters with a default value in a T-SQL CREATE PROCEDURE/FUNCTION
/// header: `@name type = value` before the body's `AS`.
fn tsql_parameter_defaults(definition: &str) -> Vec<String> {
    use sqlotus_core::lexer::{TokenKind, tokenize};
    let tokens: Vec<_> = tokenize(definition, sqlotus_core::Dialect::MsSql)
        .into_iter()
        .filter(|t| !t.is_trivia())
        .collect();
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut current: Option<String> = None;
    let mut seen_param = false;
    for t in &tokens {
        match t.kind {
            TokenKind::LParen => depth += 1,
            TokenKind::RParen => depth -= 1,
            TokenKind::Word if t.text.eq_ignore_ascii_case("AS") && depth <= 0 && seen_param => {
                break;
            }
            TokenKind::Word if t.text.eq_ignore_ascii_case("RETURNS") => break,
            TokenKind::Variable if depth <= 1 => {
                seen_param = true;
                current = Some(t.text.to_string());
            }
            TokenKind::Comma if depth <= 1 => current = None,
            TokenKind::Operator if t.text == "=" && depth <= 1 => {
                if let Some(c) = current.take() {
                    out.push(c);
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// PostgreSQL
// ---------------------------------------------------------------------------

const PG_USER_NS: &str = "n.nspname NOT LIKE 'pg\\_%' AND n.nspname <> 'information_schema'";

/// Parse `pg_get_function_arguments` output: `a integer, OUT b text DEFAULT 1`.
fn pg_params(args: &str) -> Vec<Param> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in args.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            ',' if depth == 0 => {
                parts.push(&args[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&args[start..]);
    parts
        .into_iter()
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (p, has_default) = match p.find(" DEFAULT ").or_else(|| p.find(" = ")) {
                Some(i) => (&p[..i], true),
                None => (p, false),
            };
            let mut words: Vec<&str> = p.split_whitespace().collect();
            let mut is_output = false;
            if let Some(m) = words.first().map(|w| w.to_ascii_uppercase())
                && matches!(m.as_str(), "IN" | "OUT" | "INOUT" | "VARIADIC")
            {
                is_output = m != "IN" && m != "VARIADIC";
                words.remove(0);
            }
            let (name, data_type) = if words.len() >= 2 {
                (words[0].to_string(), words[1..].join(" "))
            } else {
                (String::new(), words.join(" "))
            };
            Param {
                name,
                data_type,
                is_output,
                has_default,
            }
        })
        .collect()
}

async fn load_pg(conn: &mut Connection) -> Result<CatalogSnapshot> {
    let mut cat = Catalog::default();
    let head = conn
        .query_strings("SELECT current_database(), current_schema()")
        .await?;
    if let Some(r) = head.first() {
        cat.database = s(r, 0);
        cat.default_schema = s(r, 1);
    }
    cat.schemas = conn
        .query_strings(&format!(
            "SELECT n.nspname FROM pg_namespace n WHERE {PG_USER_NS} ORDER BY 1"
        ))
        .await?
        .iter()
        .map(|r| s(r, 0))
        .collect();

    let rows = conn
        .query_strings(&format!(
            "SELECT n.nspname, c.relname, c.relkind::text, a.attname, format_type(a.atttypid, a.atttypmod), \
                    (NOT a.attnotnull)::text, \
                    (a.attidentity <> '' OR COALESCE(pg_get_expr(d.adbin, d.adrelid), '') LIKE 'nextval(%')::text, \
                    EXISTS (SELECT 1 FROM pg_index i WHERE i.indrelid = c.oid AND i.indisprimary AND a.attnum = ANY(i.indkey))::text, \
                    (COALESCE(to_jsonb(a) ->> 'attgenerated', '') <> '')::text \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             LEFT JOIN pg_attribute a ON a.attrelid = c.oid AND a.attnum > 0 AND NOT a.attisdropped \
             LEFT JOIN pg_attrdef d ON d.adrelid = c.oid AND d.adnum = a.attnum \
             WHERE c.relkind IN ('r','v','m','p','f') AND {PG_USER_NS} \
             ORDER BY 1, 2, a.attnum"
        ))
        .await?;
    for r in rows {
        let kind = if matches!(s(&r, 2).as_str(), "v" | "m") {
            ObjectKind::View
        } else {
            ObjectKind::Table
        };
        let column = r.get(3).cloned().flatten().map(|name| Column {
            name,
            data_type: s(&r, 4),
            nullable: b(&r, 5),
            is_identity: b(&r, 6),
            is_primary_key: b(&r, 7),
            is_computed: b(&r, 8),
        });
        push_object_row(&mut cat.objects, s(&r, 0), s(&r, 1), kind, column);
    }

    let not_extension = "NOT EXISTS (SELECT 1 FROM pg_depend dep WHERE dep.objid = p.oid AND dep.deptype = 'e')";
    let routines = match conn
        .query_strings(&format!(
            "SELECT n.nspname, p.proname, p.prokind::text, p.proretset::text, pg_get_function_arguments(p.oid), \
                    CASE WHEN p.prokind IN ('f','p') THEN pg_get_functiondef(p.oid) END \
             FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
             WHERE {PG_USER_NS} AND {not_extension} ORDER BY 1, 2"
        ))
        .await
    {
        Ok(r) => r,
        // PostgreSQL < 11 has no prokind
        Err(_) => {
            conn.query_strings(&format!(
                "SELECT n.nspname, p.proname, CASE WHEN p.proisagg THEN 'a' ELSE 'f' END, p.proretset::text, \
                        pg_get_function_arguments(p.oid), CASE WHEN NOT p.proisagg THEN pg_get_functiondef(p.oid) END \
                 FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace \
                 WHERE {PG_USER_NS} AND {not_extension} ORDER BY 1, 2"
            ))
            .await?
        }
    };
    let mut definitions = Vec::new();
    for r in routines {
        let kind = match (s(&r, 2).as_str(), b(&r, 3)) {
            ("p", _) => ObjectKind::Procedure,
            ("f", true) => ObjectKind::TableFunction,
            ("f", false) => ObjectKind::ScalarFunction,
            _ => continue,
        };
        let (schema, name) = (s(&r, 0), s(&r, 1));
        cat.objects.push(DbObject {
            schema: schema.clone(),
            name: name.clone(),
            kind,
            columns: Vec::new(),
            params: pg_params(&s(&r, 4)),
        });
        if let Some(def) = r.get(5).cloned().flatten() {
            definitions.push(ObjectDefinition {
                schema,
                name,
                kind,
                definition: def,
            });
        }
    }

    let views = conn
        .query_strings(&format!(
            "SELECT n.nspname, c.relname, c.relkind::text, pg_get_viewdef(c.oid, true) \
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.relkind IN ('v','m') AND {PG_USER_NS}"
        ))
        .await?;
    for r in views {
        let what = if s(&r, 2) == "m" { "MATERIALIZED VIEW" } else { "VIEW" };
        definitions.push(ObjectDefinition {
            schema: s(&r, 0),
            name: s(&r, 1),
            kind: ObjectKind::View,
            definition: format!("CREATE OR REPLACE {what} {}.{} AS\n{}", s(&r, 0), s(&r, 1), s(&r, 3)),
        });
    }

    let fk_rows = conn
        .query_strings(
            "SELECT con.conname, sn.nspname, sc.relname, \
               (SELECT string_agg(a.attname, chr(31) ORDER BY k.ord) FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord) \
                  JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum), \
               tn.nspname, tc.relname, \
               (SELECT string_agg(a.attname, chr(31) ORDER BY k.ord) FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord) \
                  JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum) \
             FROM pg_constraint con \
             JOIN pg_class sc ON sc.oid = con.conrelid JOIN pg_namespace sn ON sn.oid = sc.relnamespace \
             JOIN pg_class tc ON tc.oid = con.confrelid JOIN pg_namespace tn ON tn.oid = tc.relnamespace \
             WHERE con.contype = 'f'",
        )
        .await?;
    cat.foreign_keys = foreign_keys(fk_rows, Some('\u{1f}'));
    Ok(CatalogSnapshot {
        catalog: cat,
        definitions,
    })
}

// ---------------------------------------------------------------------------
// MySQL
// ---------------------------------------------------------------------------

async fn load_mysql(conn: &mut Connection) -> Result<CatalogSnapshot> {
    let mut cat = Catalog::default();
    let db = conn.current_database().await?;
    if db.is_empty() {
        return Ok(CatalogSnapshot::default());
    }
    cat.database = db.clone();
    cat.default_schema = db.clone();
    cat.schemas = vec![db];

    let rows = conn
        .query_strings(
            "SELECT c.TABLE_SCHEMA, c.TABLE_NAME, t.TABLE_TYPE, c.COLUMN_NAME, c.COLUMN_TYPE, c.IS_NULLABLE, c.EXTRA, c.COLUMN_KEY \
             FROM information_schema.COLUMNS c JOIN information_schema.TABLES t \
               ON t.TABLE_SCHEMA = c.TABLE_SCHEMA AND t.TABLE_NAME = c.TABLE_NAME \
             WHERE c.TABLE_SCHEMA = DATABASE() ORDER BY c.TABLE_NAME, c.ORDINAL_POSITION",
        )
        .await?;
    for r in rows {
        let kind = if s(&r, 2).contains("VIEW") {
            ObjectKind::View
        } else {
            ObjectKind::Table
        };
        let column = Column {
            name: s(&r, 3),
            data_type: s(&r, 4),
            nullable: b(&r, 5),
            is_identity: s(&r, 6).contains("auto_increment"),
            is_computed: s(&r, 6).contains("GENERATED"),
            is_primary_key: s(&r, 7) == "PRI",
        };
        push_object_row(&mut cat.objects, s(&r, 0), s(&r, 1), kind, Some(column));
    }

    let mut definitions = Vec::new();
    let routines = conn
        .query_strings(
            "SELECT ROUTINE_SCHEMA, ROUTINE_NAME, ROUTINE_TYPE, ROUTINE_DEFINITION FROM information_schema.ROUTINES \
             WHERE ROUTINE_SCHEMA = DATABASE()",
        )
        .await?;
    for r in routines {
        let kind = if s(&r, 2) == "PROCEDURE" {
            ObjectKind::Procedure
        } else {
            ObjectKind::ScalarFunction
        };
        cat.objects.push(DbObject {
            schema: s(&r, 0),
            name: s(&r, 1),
            kind,
            columns: Vec::new(),
            params: Vec::new(),
        });
        definitions.push(ObjectDefinition {
            schema: s(&r, 0),
            name: s(&r, 1),
            kind,
            definition: format!("-- {} {}\n{}", s(&r, 2), s(&r, 1), s(&r, 3)),
        });
    }
    let params = conn
        .query_strings(
            "SELECT SPECIFIC_SCHEMA, SPECIFIC_NAME, PARAMETER_NAME, DTD_IDENTIFIER, PARAMETER_MODE \
             FROM information_schema.PARAMETERS WHERE SPECIFIC_SCHEMA = DATABASE() AND PARAMETER_NAME IS NOT NULL \
             ORDER BY SPECIFIC_NAME, ORDINAL_POSITION",
        )
        .await?;
    for r in params {
        let mode = s(&r, 4);
        let p = Param {
            name: s(&r, 2),
            data_type: s(&r, 3),
            is_output: mode.contains("OUT"),
            has_default: false,
        };
        attach_params(&mut cat.objects, &s(&r, 0), &s(&r, 1), p);
    }
    let views = conn
        .query_strings("SELECT TABLE_SCHEMA, TABLE_NAME, VIEW_DEFINITION FROM information_schema.VIEWS WHERE TABLE_SCHEMA = DATABASE()")
        .await?;
    for r in views {
        definitions.push(ObjectDefinition {
            schema: s(&r, 0),
            name: s(&r, 1),
            kind: ObjectKind::View,
            definition: format!("CREATE VIEW `{}` AS\n{}", s(&r, 1), s(&r, 2)),
        });
    }
    let triggers = conn
        .query_strings("SELECT TRIGGER_SCHEMA, TRIGGER_NAME, ACTION_STATEMENT FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA = DATABASE()")
        .await?;
    for r in triggers {
        definitions.push(ObjectDefinition {
            schema: s(&r, 0),
            name: s(&r, 1),
            kind: ObjectKind::Trigger,
            definition: s(&r, 2),
        });
    }
    let fk_rows = conn
        .query_strings(
            "SELECT CONSTRAINT_NAME, TABLE_SCHEMA, TABLE_NAME, COLUMN_NAME, REFERENCED_TABLE_SCHEMA, REFERENCED_TABLE_NAME, \
                    REFERENCED_COLUMN_NAME \
             FROM information_schema.KEY_COLUMN_USAGE \
             WHERE TABLE_SCHEMA = DATABASE() AND REFERENCED_TABLE_NAME IS NOT NULL \
             ORDER BY TABLE_NAME, CONSTRAINT_NAME, ORDINAL_POSITION",
        )
        .await?;
    cat.foreign_keys = foreign_keys(fk_rows, None);
    Ok(CatalogSnapshot {
        catalog: cat,
        definitions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mssql_types() {
        assert_eq!(mssql_type("nvarchar", 100, 0, 0), "nvarchar(50)");
        assert_eq!(mssql_type("varchar", -1, 0, 0), "varchar(max)");
        assert_eq!(mssql_type("decimal", 9, 18, 2), "decimal(18, 2)");
        assert_eq!(mssql_type("datetime2", 8, 27, 7), "datetime2");
        assert_eq!(mssql_type("int", 4, 10, 0), "int");
    }

    #[test]
    fn tsql_defaults() {
        let d = "CREATE PROCEDURE dbo.p @a INT, @b DATE = NULL, @c NVARCHAR(10) = N'x' OUTPUT AS SELECT @a = 1";
        assert_eq!(tsql_parameter_defaults(d), vec!["@b", "@c"]);
        let f = "CREATE FUNCTION dbo.f (@x INT = 0, @y INT) RETURNS INT AS BEGIN RETURN @x END";
        assert_eq!(tsql_parameter_defaults(f), vec!["@x"]);
    }

    #[test]
    fn pg_param_parsing() {
        let p = pg_params("a integer, OUT b text, c numeric(10,2) DEFAULT 0");
        let got: Vec<_> = p
            .iter()
            .map(|p| (p.name.as_str(), p.data_type.as_str(), p.is_output, p.has_default))
            .collect();
        assert_eq!(
            got,
            vec![
                ("a", "integer", false, false),
                ("b", "text", true, false),
                ("c", "numeric(10,2)", false, true)
            ]
        );
    }

    #[test]
    fn fk_grouping() {
        let row = |n: &str, fc: &str, tc: &str| {
            vec![
                Some(n.into()),
                Some("dbo".into()),
                Some("A".into()),
                Some(fc.into()),
                Some("dbo".into()),
                Some("B".into()),
                Some(tc.into()),
            ]
        };
        let fks = foreign_keys(
            vec![row("fk1", "x", "id1"), row("fk1", "y", "id2"), row("fk2", "z", "id")],
            None,
        );
        assert_eq!(fks.len(), 2);
        assert_eq!(fks[0].from_columns, vec!["x", "y"]);
        assert_eq!(fks[0].to_columns, vec!["id1", "id2"]);
    }
}
