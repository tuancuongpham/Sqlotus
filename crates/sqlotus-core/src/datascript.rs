//! A table's rows as INSERT statements (Object Explorer → Data as INSERT):
//! the SELECT that reads them, and the script that writes them again.
//! Identity values are kept (SQL Server's IDENTITY_INSERT, PostgreSQL's
//! OVERRIDING SYSTEM VALUE and sequence reset); computed and row-version
//! columns are left out, since the server sets them.

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::catalog::{Column, DbObject};
use crate::editing::{EditColumn, literal};
use crate::safety::ScriptRun;
use crate::script::object_name;
use crate::sqltype::SqlType;
use crate::statements::split_statements;
use crate::text::one_line;

/// Rows per INSERT: SQL Server takes at most 1,000 rows in one VALUES list.
const ROWS_PER_INSERT: usize = 1000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DataScriptOptions {
    /// At most this many rows.
    pub max_rows: usize,
    /// Only rows matching this condition (what follows WHERE); empty for all.
    pub filter: String,
}

impl Default for DataScriptOptions {
    fn default() -> Self {
        Self {
            max_rows: 1000,
            filter: String::new(),
        }
    }
}

/// The columns the script writes: not computed, not a row version.
pub fn scripted_columns(table: &DbObject, dialect: Dialect) -> Vec<&Column> {
    table
        .columns
        .iter()
        .filter(|c| {
            let base = SqlType::parse(&c.data_type).base().to_string();
            !c.is_computed && base != "rowversion" && !(dialect == Dialect::MsSql && base == "timestamp")
        })
        .collect()
}

/// The SELECT that reads the rows of `table` to script, in key order. It
/// asks for one row more than `max_rows`, so a cut can be told.
pub fn data_query(table: &DbObject, dialect: Dialect, opts: &DataScriptOptions) -> Result<String, String> {
    let cols = scripted_columns(table, dialect);
    if cols.is_empty() {
        return Err(format!("{}.{} has no columns to script.", table.schema, table.name));
    }
    let q = |n: &str| dialect.quote_ident(n);
    let list = cols.iter().map(|c| q(&c.name)).collect::<Vec<_>>().join(", ");
    let limit = opts.max_rows.max(1) + 1;
    let top = if dialect == Dialect::MsSql {
        format!("TOP ({limit}) ")
    } else {
        String::new()
    };
    let mut sql = format!("SELECT {top}{list}\nFROM {}", object_name(table, dialect, true));
    let filter = opts.filter.trim();
    if !filter.is_empty() {
        // on its own lines: a `--` comment at its end cannot hide what follows
        sql.push_str(&format!("\nWHERE (\n{filter}\n)"));
    }
    let keys: Vec<String> = table
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| q(&c.name))
        .collect();
    if !keys.is_empty() {
        sql.push_str(&format!("\nORDER BY {}", keys.join(", ")));
    }
    if dialect != Dialect::MsSql {
        sql.push_str(&format!("\nLIMIT {limit}"));
    }
    // the condition reads rows; it must not change anything
    let run = ScriptRun::new(&sql, dialect);
    if split_statements(&sql, dialect).len() != 1 || !run.changes().is_empty() || !run.calls().is_empty() {
        return Err("The condition must be part of one SELECT (what follows WHERE), and change nothing.".into());
    }
    Ok(sql)
}

/// The script, and values it could not write as they are.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DataScript {
    pub script: String,
    /// Rows in the script.
    pub rows: usize,
    /// The table has more rows than were scripted.
    pub truncated: bool,
    pub problems: Vec<String>,
}

/// INSERT statements for `rows`, read by [`data_query`] (one value per
/// [`scripted_columns`] column; a row past `max_rows` means there were more).
pub fn data_script(table: &DbObject, dialect: Dialect, rows: &[Vec<Option<String>>], max_rows: usize) -> DataScript {
    let cols = scripted_columns(table, dialect);
    let max_rows = max_rows.max(1);
    let truncated = rows.len() > max_rows;
    let rows = &rows[..rows.len().min(max_rows)];
    let q = |n: &str| dialect.quote_ident(n);
    let target = object_name(table, dialect, true);
    let mut problems = Vec::new();
    let mut out = format!(
        "-- Data of {}: {} row(s){}, scripted by Sqlotus.\n",
        one_line(&target),
        rows.len(),
        if truncated {
            format!(" (the first {max_rows}; the table has more)")
        } else {
            String::new()
        }
    );
    let left_out: Vec<&str> = table
        .columns
        .iter()
        .filter(|c| !cols.iter().any(|x| x.name == c.name))
        .map(|c| c.name.as_str())
        .collect();
    if !left_out.is_empty() {
        out.push_str(&format!(
            "-- Not written (the server sets them): {}\n",
            one_line(&left_out.join(", "))
        ));
    }
    if rows.is_empty() {
        out.push_str("-- The table has no rows to script.\n");
        return DataScript {
            script: out,
            rows: 0,
            truncated,
            problems,
        };
    }
    let identity: Vec<&Column> = cols.iter().copied().filter(|c| c.is_identity).collect();
    let header = cols.iter().map(|c| q(&c.name)).collect::<Vec<_>>().join(", ");
    let overriding = if dialect == Dialect::Postgres && !identity.is_empty() {
        " OVERRIDING SYSTEM VALUE"
    } else {
        ""
    };
    let edit_cols: Vec<EditColumn> = cols
        .iter()
        .map(|c| EditColumn {
            column: Some(c.name.clone()),
            data_type: c.data_type.clone(),
            nullable: true,
            is_key: false,
            read_only: false,
        })
        .collect();
    let lines: Vec<String> = rows
        .iter()
        .map(|row| {
            let values: Vec<String> = edit_cols
                .iter()
                .enumerate()
                .map(|(j, col)| {
                    let mut found = Vec::new();
                    let v = literal(&row.get(j).cloned().flatten(), col, dialect, &mut found);
                    for p in found {
                        if problems.len() < 20 && !problems.contains(&p) {
                            problems.push(p);
                        }
                    }
                    v
                })
                .collect();
            format!("({})", values.join(", "))
        })
        .collect();
    let identity_insert = dialect == Dialect::MsSql && !identity.is_empty();
    if identity_insert {
        out.push_str(&format!("SET IDENTITY_INSERT {target} ON;\n"));
    }
    for chunk in lines.chunks(ROWS_PER_INSERT) {
        out.push_str(&format!(
            "INSERT INTO {target} ({header}){overriding}\nVALUES {};\n",
            chunk.join(",\n       ")
        ));
    }
    if identity_insert {
        out.push_str(&format!("SET IDENTITY_INSERT {target} OFF;\n"));
    }
    if dialect == Dialect::Postgres {
        // the next generated value follows the scripted ones
        for c in &identity {
            out.push_str(&format!(
                "SELECT setval(pg_get_serial_sequence({}, {}), (SELECT MAX({}) FROM {target}));\n",
                dialect.string_literal(&target),
                dialect.string_literal(&c.name),
                q(&c.name)
            ));
        }
    }
    if !problems.is_empty() {
        let notes: String = problems.iter().map(|p| format!("-- ⚠ {}\n", one_line(p))).collect();
        out.insert_str(out.find('\n').map_or(0, |i| i + 1), &notes);
    }
    DataScript {
        script: out,
        rows: rows.len(),
        truncated,
        problems,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ObjectKind;

    fn col(name: &str, ty: &str, pk: bool, identity: bool, computed: bool) -> Column {
        Column {
            name: name.into(),
            data_type: ty.into(),
            nullable: !pk,
            is_primary_key: pk,
            is_identity: identity,
            is_computed: computed,
        }
    }

    fn customers() -> DbObject {
        DbObject {
            schema: "dbo".into(),
            name: "Customers".into(),
            kind: ObjectKind::Table,
            columns: vec![
                col("Id", "int", true, true, false),
                col("Name", "nvarchar(100)", false, false, false),
                col("Joined", "datetime", false, false, false),
                col("Photo", "varbinary(max)", false, false, false),
                col("NameLength", "int", false, false, true),
                col("Version", "rowversion", false, false, false),
            ],
            params: vec![],
        }
    }

    fn row(values: &[Option<&str>]) -> Vec<Option<String>> {
        values.iter().map(|v| v.map(str::to_string)).collect()
    }

    #[test]
    fn reads_the_rows_in_key_order() {
        let t = customers();
        let opts = DataScriptOptions {
            max_rows: 100,
            filter: String::new(),
        };
        assert_eq!(
            data_query(&t, Dialect::MsSql, &opts).unwrap(),
            "SELECT TOP (101) Id, Name, Joined, Photo\nFROM dbo.Customers\nORDER BY Id"
        );
        let filtered = DataScriptOptions {
            max_rows: 10,
            filter: "Name LIKE N'A%' -- only A".into(),
        };
        assert_eq!(
            data_query(&t, Dialect::Postgres, &filtered).unwrap(),
            "SELECT \"Id\", \"Name\", \"Joined\", \"Photo\"\nFROM dbo.\"Customers\"\nWHERE (\nName LIKE N'A%' -- only A\n)\nORDER BY \"Id\"\nLIMIT 11"
        );
        // the condition only reads
        for bad in [
            "1 = 1; DELETE FROM dbo.Customers",
            "1 = 1) UPDATE dbo.Customers SET Name = '' WHERE (1 = 1",
            "1 = 1; EXEC dbo.p",
        ] {
            let o = DataScriptOptions {
                max_rows: 10,
                filter: bad.into(),
            };
            assert!(data_query(&t, Dialect::MsSql, &o).is_err(), "{bad}");
        }
    }

    #[test]
    fn identity_values_are_kept() {
        let t = customers();
        let rows = vec![
            row(&[Some("1"), Some("O'Hara"), Some("2024-01-12 10:20:30"), Some("0x0A0B")]),
            row(&[Some("2"), Some("Phạm"), None, None]),
        ];
        let s = data_script(&t, Dialect::MsSql, &rows, 100);
        assert_eq!(
            s.script,
            "-- Data of dbo.Customers: 2 row(s), scripted by Sqlotus.\n\
             -- Not written (the server sets them): NameLength, Version\n\
             SET IDENTITY_INSERT dbo.Customers ON;\n\
             INSERT INTO dbo.Customers (Id, Name, Joined, Photo)\n\
             VALUES (1, N'O''Hara', '2024-01-12T10:20:30', 0x0A0B),\n       (2, N'Phạm', NULL, NULL);\n\
             SET IDENTITY_INSERT dbo.Customers OFF;\n"
        );
        assert!(!s.truncated && s.problems.is_empty() && s.rows == 2);
        let pg = data_script(&t, Dialect::Postgres, &rows[..1], 100).script;
        assert!(pg.contains("INSERT INTO dbo.\"Customers\" (\"Id\", \"Name\", \"Joined\", \"Photo\") OVERRIDING SYSTEM VALUE\nVALUES (1, 'O''Hara',"), "{pg}");
        assert!(
            pg.ends_with("SELECT setval(pg_get_serial_sequence('dbo.\"Customers\"', 'Id'), (SELECT MAX(\"Id\") FROM dbo.\"Customers\"));\n"),
            "{pg}"
        );
        let my = data_script(&t, Dialect::MySql, &rows[..1], 100).script;
        assert!(!my.contains("IDENTITY_INSERT") && !my.contains("OVERRIDING"), "{my}");
    }

    #[test]
    fn many_rows_are_split_and_cut() {
        let t = DbObject {
            columns: vec![col("Id", "int", true, false, false)],
            ..customers()
        };
        let rows: Vec<_> = (0..2501).map(|i| vec![Some(i.to_string())]).collect();
        let s = data_script(&t, Dialect::MsSql, &rows, 2500);
        assert_eq!(s.script.matches("INSERT INTO").count(), 3);
        assert!(s.truncated && s.rows == 2500);
        assert!(
            s.script
                .starts_with("-- Data of dbo.Customers: 2500 row(s) (the first 2500; the table has more)")
        );
        assert!(!s.script.contains("2500)"), "the extra row is not written");
        let empty = data_script(&t, Dialect::MsSql, &[], 10);
        assert!(empty.script.ends_with("-- The table has no rows to script.\n") && empty.rows == 0);
    }

    #[test]
    fn values_that_cannot_be_written_are_reported() {
        let t = DbObject {
            columns: vec![
                col("Id", "int", true, false, false),
                col("Ratio", "float", false, false, false),
            ],
            ..customers()
        };
        let s = data_script(&t, Dialect::Postgres, &[row(&[Some("1"), Some("NaN")])], 10);
        assert_eq!(s.problems, vec!["Ratio: 'NaN' is not a number."]);
        assert!(
            s.script.lines().nth(1).unwrap().starts_with("-- ⚠ Ratio"),
            "{}",
            s.script
        );
        // MySQL timestamp is a date and time, not a row version
        let my = DbObject {
            columns: vec![
                col("Id", "int", true, false, false),
                col("At", "timestamp", false, false, false),
            ],
            ..customers()
        };
        assert_eq!(scripted_columns(&my, Dialect::MySql).len(), 2);
        assert_eq!(scripted_columns(&my, Dialect::MsSql).len(), 1);
    }
}
