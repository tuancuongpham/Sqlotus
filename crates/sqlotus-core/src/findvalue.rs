//! "Find a value in every text column": a read-only script that counts, per
//! table and column, the rows holding a value, with an example of each.
//! Sqlotus writes the script and opens it for review; it runs only when the
//! user runs it.

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::catalog::{Catalog, ObjectKind};
use crate::script::object_name;
use crate::sqltype::SqlType;
use crate::text::one_line;

/// Columns per SELECT: very long UNION ALL chains strain the server.
const COLUMNS_PER_QUERY: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchMode {
    #[default]
    Contains,
    StartsWith,
    Exact,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FindValueOptions {
    pub value: String,
    pub mode: MatchMode,
    /// Only the tables of this schema; empty for every schema.
    pub schema: String,
    /// PostgreSQL compares case-sensitively unless asked (ILIKE); SQL Server
    /// and MySQL follow each column's collation.
    pub ignore_case: bool,
}

/// The search script for `catalog`'s tables.
pub fn find_value_script(catalog: &Catalog, dialect: Dialect, opts: &FindValueOptions) -> Result<String, String> {
    if opts.value.is_empty() {
        return Err("Type the value to find.".into());
    }
    let tables: Vec<_> = catalog
        .objects
        .iter()
        .filter(|o| o.kind == ObjectKind::Table)
        .filter(|o| opts.schema.is_empty() || o.schema.eq_ignore_ascii_case(&opts.schema))
        .collect();
    let mut parts = Vec::new();
    let mut table_count = 0;
    for t in &tables {
        let name = object_name(t, dialect, true);
        let label = literal(&format!("{}.{}", t.schema, t.name), dialect);
        let before = parts.len();
        for c in t.columns.iter().filter(|c| SqlType::parse(&c.data_type).is_text()) {
            let col = dialect.quote_ident(&c.name);
            let ty = SqlType::parse(&c.data_type);
            // MIN cannot read the old text / ntext types
            let readable = if dialect == Dialect::MsSql && matches!(ty.base(), "text" | "ntext") {
                format!("CAST({col} AS nvarchar(max))")
            } else {
                col.clone()
            };
            parts.push(format!(
                "SELECT {label} AS table_name, {} AS column_name, COUNT(*) AS matches, LEFT(MIN({readable}), 200) AS example\n    \
                 FROM {name} WHERE {}",
                literal(&c.name, dialect),
                condition(&readable, dialect, opts)
            ));
        }
        if parts.len() > before {
            table_count += 1;
        }
    }
    if parts.is_empty() {
        return Err(if opts.schema.is_empty() {
            "No table has a text column to search.".to_string()
        } else {
            format!("No table of schema {} has a text column to search.", opts.schema)
        });
    }
    let how = match opts.mode {
        MatchMode::Contains => "containing",
        MatchMode::StartsWith => "starting with",
        MatchMode::Exact => "equal to",
    };
    let mut out = format!(
        "-- Rows {how} {} in {} text column(s) of {table_count} table(s){}, scripted by Sqlotus.\n\
         -- It only reads: review it, then run it (F5). Each column is read in full, which takes a while on big tables.\n",
        one_line(&literal(&opts.value, dialect)),
        parts.len(),
        if catalog.database.is_empty() {
            String::new()
        } else {
            format!(" in {}", one_line(&catalog.database))
        },
    );
    for chunk in parts.chunks(COLUMNS_PER_QUERY) {
        out.push_str(&format!(
            "SELECT table_name, column_name, matches, example\nFROM (\n    {}\n) found\nWHERE matches > 0\nORDER BY table_name, column_name;\n",
            chunk.join("\n    UNION ALL\n    ")
        ));
    }
    Ok(out)
}

/// The WHERE condition matching `col` against the value.
fn condition(col: &str, dialect: Dialect, opts: &FindValueOptions) -> String {
    let fold = opts.ignore_case && dialect == Dialect::Postgres;
    if opts.mode == MatchMode::Exact {
        let value = literal(&opts.value, dialect);
        return if fold {
            format!("lower({col}) = lower({value})")
        } else {
            format!("{col} = {value}")
        };
    }
    // `!` escapes LIKE's wildcards (and SQL Server's `[`) in the value
    let mut escaped = String::new();
    for ch in opts.value.chars() {
        if matches!(ch, '!' | '%' | '_') || (ch == '[' && dialect == Dialect::MsSql) {
            escaped.push('!');
        }
        escaped.push(ch);
    }
    let pattern = match opts.mode {
        MatchMode::StartsWith => format!("{escaped}%"),
        _ => format!("%{escaped}%"),
    };
    format!(
        "{col} {} {} ESCAPE '!'",
        if fold { "ILIKE" } else { "LIKE" },
        literal(&pattern, dialect)
    )
}

/// A text literal of any character (SQL Server: `N'...'`).
fn literal(text: &str, dialect: Dialect) -> String {
    dialect.typed_string(text, &SqlType::parse("nvarchar"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Column, DbObject};

    fn table(schema: &str, name: &str, cols: &[(&str, &str)]) -> DbObject {
        DbObject {
            schema: schema.into(),
            name: name.into(),
            kind: ObjectKind::Table,
            columns: cols
                .iter()
                .map(|(n, t)| Column {
                    name: n.to_string(),
                    data_type: t.to_string(),
                    nullable: true,
                    is_primary_key: false,
                    is_identity: false,
                    is_computed: false,
                })
                .collect(),
            params: vec![],
        }
    }

    fn cat() -> Catalog {
        Catalog {
            database: "Shop".into(),
            default_schema: "dbo".into(),
            schemas: vec!["dbo".into(), "sales".into()],
            objects: vec![
                table(
                    "dbo",
                    "Customers",
                    &[("Id", "int"), ("Name", "nvarchar(100)"), ("Notes", "ntext")],
                ),
                table("dbo", "Numbers", &[("Id", "int")]),
                table("sales", "Regions", &[("Code", "char(2)")]),
                DbObject {
                    kind: ObjectKind::View,
                    ..table("dbo", "vNames", &[("Name", "nvarchar(100)")])
                },
            ],
            foreign_keys: vec![],
        }
    }

    fn opts(value: &str, mode: MatchMode) -> FindValueOptions {
        FindValueOptions {
            value: value.into(),
            mode,
            ..Default::default()
        }
    }

    #[test]
    fn every_text_column_of_every_table() {
        let s = find_value_script(&cat(), Dialect::MsSql, &opts("50% [off]_!", MatchMode::Contains)).unwrap();
        assert!(
            s.starts_with("-- Rows containing N'50% [off]_!' in 3 text column(s) of 2 table(s) in Shop"),
            "{s}"
        );
        assert!(s.contains(
            "SELECT N'dbo.Customers' AS table_name, N'Name' AS column_name, COUNT(*) AS matches, LEFT(MIN(Name), 200) AS example\n    \
             FROM dbo.Customers WHERE Name LIKE N'%50!% ![off]!_!!%' ESCAPE '!'"
        ), "{s}");
        // MIN reads ntext through nvarchar(max); views and number columns are left out
        assert!(s.contains("LEFT(MIN(CAST(Notes AS nvarchar(max))), 200)"), "{s}");
        assert!(!s.contains("vNames") && !s.contains("N'Id'"), "{s}");
        assert_eq!(s.matches("UNION ALL").count(), 2);
        assert!(
            s.trim_end()
                .ends_with("WHERE matches > 0\nORDER BY table_name, column_name;")
        );
    }

    #[test]
    fn modes_cases_and_dialects() {
        let pg = FindValueOptions {
            ignore_case: true,
            schema: "sales".into(),
            ..opts("vn", MatchMode::StartsWith)
        };
        let s = find_value_script(&cat(), Dialect::Postgres, &pg).unwrap();
        assert!(
            s.contains("FROM sales.\"Regions\" WHERE \"Code\" ILIKE 'vn%' ESCAPE '!'"),
            "{s}"
        );
        assert!(!s.contains("Customers"));
        let exact = FindValueOptions {
            ignore_case: true,
            ..opts("O'Hara", MatchMode::Exact)
        };
        let s = find_value_script(&cat(), Dialect::Postgres, &exact).unwrap();
        assert!(s.contains("WHERE lower(\"Name\") = lower('O''Hara')"), "{s}");
        // MySQL doubles backslashes in strings; the collation decides the case
        let s = find_value_script(
            &cat(),
            Dialect::MySql,
            &FindValueOptions {
                ignore_case: true,
                ..opts("a\\b", MatchMode::Contains)
            },
        )
        .unwrap();
        assert!(s.contains("WHERE Name LIKE '%a\\\\b%' ESCAPE '!'"), "{s}");
        assert!(s.contains("FROM (\n    SELECT 'dbo.Customers' AS table_name"), "{s}");
    }

    #[test]
    fn nothing_to_search() {
        assert_eq!(
            find_value_script(&cat(), Dialect::MsSql, &opts("", MatchMode::Contains)).unwrap_err(),
            "Type the value to find."
        );
        let numbers_only = FindValueOptions {
            schema: "nope".into(),
            ..opts("x", MatchMode::Contains)
        };
        assert!(
            find_value_script(&cat(), Dialect::MsSql, &numbers_only)
                .unwrap_err()
                .contains("schema nope")
        );
    }

    #[test]
    fn long_searches_are_split() {
        let cols: Vec<(String, String)> = (0..250).map(|i| (format!("c{i}"), "varchar(10)".to_string())).collect();
        let refs: Vec<(&str, &str)> = cols.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
        let c = Catalog {
            objects: vec![table("dbo", "Wide", &refs)],
            ..cat()
        };
        let s = find_value_script(&c, Dialect::MsSql, &opts("x", MatchMode::Contains)).unwrap();
        assert_eq!(s.matches(") found").count(), 3);
        // narrow text columns compare with a narrow literal too
        assert!(s.contains("c0 LIKE N'%x%'"), "{s}");
    }
}
