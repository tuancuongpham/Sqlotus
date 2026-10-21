//! Statement generators in SQL Prompt's layout: INSERT with column list and
//! commented VALUES, EXEC with one parameter per line, SELECT/UPDATE/DELETE
//! templates, CREATE TABLE, and "script as ALTER".
//!
//! Generators can emit Monaco snippet syntax (`${1:NULL}`) so Tab walks the
//! values after insertion.

use crate::Dialect;
use crate::catalog::{Column, DbObject, Param};
use crate::lexer::{TokenKind, tokenize};
use crate::snippet::escape_snippet;

/// `schema.name` quoted for the dialect.
pub fn object_name(o: &DbObject, dialect: Dialect, with_schema: bool) -> String {
    if with_schema && !o.schema.is_empty() {
        format!("{}.{}", dialect.quote_ident(&o.schema), dialect.quote_ident(&o.name))
    } else {
        dialect.quote_ident(&o.name)
    }
}

/// Type text for a comment: `decimal(5,2)` -> `decimal(5, 2)`.
fn type_comment(ty: &str) -> String {
    ty.replace(", ", ",").replace(',', ", ")
}

struct Values {
    snippet: bool,
    next: usize,
}

impl Values {
    fn new(snippet: bool) -> Self {
        Self { snippet, next: 1 }
    }

    /// A placeholder with `default` text (or the literal text).
    fn value(&mut self, default: &str) -> String {
        if self.snippet {
            let s = format!("${{{}:{}}}", self.next, escape_snippet(default));
            self.next += 1;
            s
        } else {
            default.to_string()
        }
    }

    fn lit(&self, text: &str) -> String {
        if self.snippet {
            escape_snippet(text)
        } else {
            text.to_string()
        }
    }
}

/// The part after `INSERT INTO `:
///
/// ```text
/// dbo.SinhVien
/// (
///     HoTen,
///     Tinh
/// )
/// VALUES
/// (   NULL, -- HoTen - nvarchar(255)
///     NULL  -- Tinh - nvarchar(100)
///     )
/// ```
pub fn insert_tail(o: &DbObject, name: &str, dialect: Dialect, indent: usize, snippet: bool) -> String {
    let mut v = Values::new(snippet);
    let pad = " ".repeat(indent);
    let cols: Vec<&Column> = o.columns.iter().filter(|c| c.is_insertable()).collect();
    if cols.is_empty() {
        return format!("{}\nDEFAULT VALUES", v.lit(name));
    }
    let mut s = v.lit(name);
    s.push_str("\n(\n");
    let names: Vec<String> = cols
        .iter()
        .map(|c| format!("{pad}{}", v.lit(&dialect.quote_ident(&c.name))))
        .collect();
    s.push_str(&names.join(",\n"));
    s.push_str("\n)\nVALUES\n");
    for (i, c) in cols.iter().enumerate() {
        let lead = if i == 0 {
            format!("({}", " ".repeat(indent.saturating_sub(1)))
        } else {
            pad.clone()
        };
        let sep = if i + 1 < cols.len() { "," } else { " " };
        let comment = v.lit(&format!("-- {} - {}", c.name, type_comment(&c.data_type)));
        let val = v.value("NULL");
        s.push_str(&format!("{lead}{val}{sep} {comment}\n"));
    }
    s.push_str(&format!("{pad})"));
    s
}

/// Literal used as a parameter value placeholder for a data type.
pub fn sample_value(data_type: &str, dialect: Dialect) -> &'static str {
    let t = data_type.to_ascii_lowercase();
    let base = t.split('(').next().unwrap_or("").trim();
    match base {
        "int" | "integer" | "bigint" | "smallint" | "tinyint" | "decimal" | "numeric" | "money" | "smallmoney"
        | "float" | "real" | "double" | "double precision" | "bit" | "serial" | "bigserial" | "mediumint" => "0",
        "nvarchar" | "nchar" | "ntext" | "sysname" if dialect == Dialect::MsSql => "N''",
        "varchar" | "char" | "text" | "nvarchar" | "nchar" | "character varying" | "character" | "longtext"
        | "mediumtext" | "tinytext" => "''",
        "boolean" | "bool" => "false",
        _ => "NULL",
    }
}

/// The part after `EXEC ` for a procedure, one parameter per line aligned
/// under the first, with a type comment:
///
/// ```text
/// dbo.GetCustomerOrders @CustomerId = 0,   -- int
///                       @From = NULL       -- date
/// ```
///
/// `start_col` is the column where the inserted text begins relative to the
/// line's indentation (Monaco re-indents following lines itself).
pub fn exec_tail(p: &DbObject, name: &str, dialect: Dialect, start_col: usize, snippet: bool) -> String {
    let mut v = Values::new(snippet);
    if p.params.is_empty() {
        return v.lit(name);
    }
    if dialect != Dialect::MsSql {
        let args: Vec<String> = p
            .params
            .iter()
            .map(|x| v.value(sample_value(&x.data_type, dialect)))
            .collect();
        return format!("{}({})", v.lit(name), args.join(", "));
    }
    let align = " ".repeat(start_col + name.chars().count() + 1);
    let width = p.params.iter().map(param_width).max().unwrap_or(0);
    let mut s = v.lit(name);
    s.push(' ');
    for (i, x) in p.params.iter().enumerate() {
        if i > 0 {
            s.push('\n');
            s.push_str(&align);
        }
        let value = if x.is_output {
            x.name.clone()
        } else {
            sample_value(&x.data_type, dialect).to_string()
        };
        let out = if x.is_output { " OUTPUT" } else { "" };
        let sep = if i + 1 < p.params.len() { "," } else { "" };
        let shown = format!("{} = {}{}{}", x.name, value, out, sep);
        let gap = " ".repeat(width.saturating_sub(shown.chars().count()) + 1);
        let default = if x.has_default { " (has default)" } else { "" };
        s.push_str(&v.lit(&format!("{} = ", x.name)));
        s.push_str(&v.value(&value));
        s.push_str(&v.lit(&format!("{out}{sep}{gap}-- {}{default}", type_comment(&x.data_type))));
    }
    s
}

fn param_width(x: &Param) -> usize {
    // name + " = " + a typical value + " OUTPUT," leaves comments roughly aligned
    x.name.chars().count() + 3 + 6 + if x.is_output { 7 } else { 0 } + 1
}

pub fn select_script(o: &DbObject, dialect: Dialect, top: usize, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let cols = if o.columns.is_empty() {
        format!("{pad}*")
    } else {
        o.columns
            .iter()
            .map(|c| format!("{pad}{}", dialect.quote_ident(&c.name)))
            .collect::<Vec<_>>()
            .join(",\n")
    };
    let name = object_name(o, dialect, true);
    match dialect {
        Dialect::MsSql => format!("SELECT TOP ({top})\n{cols}\nFROM {name};"),
        _ => format!("SELECT\n{cols}\nFROM {name}\nLIMIT {top};"),
    }
}

pub fn update_script(o: &DbObject, dialect: Dialect, indent: usize) -> String {
    let pad = " ".repeat(indent);
    let cols: Vec<&Column> = o
        .columns
        .iter()
        .filter(|c| c.is_insertable() && !c.is_primary_key)
        .collect();
    let mut s = format!("UPDATE {}\nSET ", object_name(o, dialect, true));
    for (i, c) in cols.iter().enumerate() {
        if i > 0 {
            s.push_str(&format!("\n{pad}"));
        }
        let sep = if i + 1 < cols.len() { "," } else { " " };
        s.push_str(&format!(
            "{} = NULL{sep} -- {}",
            dialect.quote_ident(&c.name),
            type_comment(&c.data_type)
        ));
    }
    s.push_str(&format!("\nWHERE {};", key_predicate(o, dialect)));
    s
}

pub fn delete_script(o: &DbObject, dialect: Dialect) -> String {
    format!(
        "DELETE FROM {}\nWHERE {};",
        object_name(o, dialect, true),
        key_predicate(o, dialect)
    )
}

fn key_predicate(o: &DbObject, dialect: Dialect) -> String {
    let keys: Vec<String> = o
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| format!("{} = NULL", dialect.quote_ident(&c.name)))
        .collect();
    if keys.is_empty() {
        "1 = 0 /* add a condition */".into()
    } else {
        keys.join(" AND ")
    }
}

pub fn create_table_script(o: &DbObject, dialect: Dialect) -> String {
    let q = |s: &str| match dialect {
        Dialect::MsSql => format!("[{}]", s.replace(']', "]]")),
        _ => dialect.quote_ident(s),
    };
    let mut lines: Vec<String> = o
        .columns
        .iter()
        .map(|c| {
            let mut l = format!("    {} {}", q(&c.name), c.data_type);
            if c.is_identity && dialect == Dialect::MsSql {
                l.push_str(" IDENTITY");
            }
            l.push_str(if c.nullable { " NULL" } else { " NOT NULL" });
            l
        })
        .collect();
    let pk: Vec<String> = o
        .columns
        .iter()
        .filter(|c| c.is_primary_key)
        .map(|c| q(&c.name))
        .collect();
    if !pk.is_empty() {
        lines.push(format!("    PRIMARY KEY ({})", pk.join(", ")));
    }
    format!(
        "CREATE TABLE {}.{} (\n{}\n);",
        q(&o.schema),
        q(&o.name),
        lines.join(",\n")
    )
}

/// Turn a module definition into an ALTER script (SQL Server): the first
/// `CREATE` keyword outside comments becomes `ALTER`. Other dialects
/// re-create objects with `CREATE OR REPLACE`, so the text is returned as is.
pub fn script_as_alter(definition: &str, dialect: Dialect) -> String {
    if dialect != Dialect::MsSql {
        return definition.to_string();
    }
    for t in tokenize(definition, dialect) {
        match t.kind {
            TokenKind::Whitespace | TokenKind::LineComment | TokenKind::BlockComment => continue,
            TokenKind::Word if t.text.eq_ignore_ascii_case("CREATE") => {
                // CREATE OR ALTER stays valid; plain CREATE becomes ALTER
                let rest = &definition[t.end()..];
                if rest.trim_start().to_ascii_uppercase().starts_with("OR ALTER") {
                    return definition.to_string();
                }
                return format!("{}ALTER{}", &definition[..t.start], rest);
            }
            _ => break,
        }
    }
    definition.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::ObjectKind;

    fn col(n: &str, t: &str, identity: bool) -> Column {
        Column {
            name: n.into(),
            data_type: t.into(),
            nullable: true,
            is_primary_key: identity,
            is_identity: identity,
            is_computed: false,
        }
    }

    fn sinh_vien() -> DbObject {
        DbObject {
            schema: "dbo".into(),
            name: "SinhVien".into(),
            kind: ObjectKind::Table,
            columns: vec![
                col("MaSV", "int", true),
                col("HoTen", "nvarchar(255)", false),
                col("GioiTinh", "nvarchar(10)", false),
                col("NgaySinh", "date", false),
                col("MaLop", "int", false),
                col("HocBong", "decimal(5,2)", false),
                col("Tinh", "nvarchar(100)", false),
            ],
            params: vec![],
        }
    }

    /// Exactly SQL Prompt's output for the user's table.
    #[test]
    fn insert_matches_sql_prompt() {
        let got = format!(
            "INSERT INTO {}",
            insert_tail(&sinh_vien(), "dbo.SinhVien", Dialect::MsSql, 4, false)
        );
        let want = "INSERT INTO dbo.SinhVien
(
    HoTen,
    GioiTinh,
    NgaySinh,
    MaLop,
    HocBong,
    Tinh
)
VALUES
(   NULL, -- HoTen - nvarchar(255)
    NULL, -- GioiTinh - nvarchar(10)
    NULL, -- NgaySinh - date
    NULL, -- MaLop - int
    NULL, -- HocBong - decimal(5, 2)
    NULL  -- Tinh - nvarchar(100)
    )";
        assert_eq!(got, want);
    }

    #[test]
    fn insert_snippet_has_tab_stops() {
        let s = insert_tail(&sinh_vien(), "dbo.SinhVien", Dialect::MsSql, 4, true);
        assert!(s.contains("(   ${1:NULL}, -- HoTen - nvarchar(255)"));
        assert!(s.contains("    ${6:NULL}  -- Tinh - nvarchar(100)"));
    }

    #[test]
    fn exec_lines_up_parameters() {
        let p = DbObject {
            schema: "dbo".into(),
            name: "GetOrders".into(),
            kind: ObjectKind::Procedure,
            columns: vec![],
            params: vec![
                Param {
                    name: "@CustomerId".into(),
                    data_type: "int".into(),
                    is_output: false,
                    has_default: false,
                },
                Param {
                    name: "@Name".into(),
                    data_type: "nvarchar(50)".into(),
                    is_output: false,
                    has_default: true,
                },
                Param {
                    name: "@Total".into(),
                    data_type: "money".into(),
                    is_output: true,
                    has_default: false,
                },
            ],
        };
        let s = exec_tail(&p, "dbo.GetOrders", Dialect::MsSql, 5, false);
        let lines: Vec<&str> = s.lines().collect();
        assert!(lines[0].starts_with("dbo.GetOrders @CustomerId = 0,"), "{s}");
        assert!(lines[0].ends_with("-- int"), "{s}");
        assert!(
            lines[1].starts_with(&format!("{}@Name = N'',", " ".repeat(5 + 14))),
            "{s}"
        );
        assert!(lines[1].ends_with("-- nvarchar(50) (has default)"), "{s}");
        assert!(lines[2].contains("@Total = @Total OUTPUT"), "{s}");
        assert_eq!(
            exec_tail(&p, "get_orders", Dialect::Postgres, 0, false),
            "get_orders(0, '', 0)"
        );
    }

    #[test]
    fn templates() {
        let o = sinh_vien();
        assert!(select_script(&o, Dialect::MsSql, 100, 4).starts_with("SELECT TOP (100)\n    MaSV,"));
        assert!(select_script(&o, Dialect::Postgres, 100, 4).ends_with("FROM dbo.\"SinhVien\"\nLIMIT 100;"));
        let u = update_script(&o, Dialect::MsSql, 4);
        assert!(
            u.starts_with("UPDATE dbo.SinhVien\nSET HoTen = NULL, -- nvarchar(255)\n    GioiTinh"),
            "{u}"
        );
        assert!(u.ends_with("WHERE MaSV = NULL;"), "{u}");
        assert_eq!(
            delete_script(&o, Dialect::MsSql),
            "DELETE FROM dbo.SinhVien\nWHERE MaSV = NULL;"
        );
    }

    #[test]
    fn alter_script() {
        assert_eq!(
            script_as_alter("-- c\nCREATE   PROCEDURE p AS SELECT 'CREATE'", Dialect::MsSql),
            "-- c\nALTER   PROCEDURE p AS SELECT 'CREATE'"
        );
        assert_eq!(
            script_as_alter("CREATE OR ALTER VIEW v AS SELECT 1", Dialect::MsSql),
            "CREATE OR ALTER VIEW v AS SELECT 1"
        );
    }
}
