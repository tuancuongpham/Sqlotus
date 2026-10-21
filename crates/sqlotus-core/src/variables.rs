//! Variables a script uses without declaring them, and the declarations
//! that bind values to them before it runs.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::analysis::{STATEMENT_KEYWORDS, in_declare, sig, word};
use crate::lexer::{Token, TokenKind, tokenize};
use crate::statements::split_batches;

/// A value for an undeclared variable, entered by the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableValue {
    /// With the `@`.
    pub name: String,
    /// SQL Server type for the DECLARE, e.g. `int`, `nvarchar(4000)`.
    pub data_type: String,
    /// `None` binds NULL.
    pub value: Option<String>,
}

/// First word of the statement containing token `i`.
fn statement_keyword(t: &[Token<'_>], i: usize) -> String {
    let mut depth = 0i32;
    let mut first = String::new();
    for k in (0..i).rev() {
        match t[k].kind {
            TokenKind::RParen => depth += 1,
            TokenKind::LParen => depth -= 1,
            TokenKind::Semicolon if depth <= 0 => break,
            TokenKind::Word if depth <= 0 => {
                let w = t[k].upper();
                if STATEMENT_KEYWORDS.contains(&w.as_str()) {
                    return w;
                }
                first = w;
            }
            _ => {}
        }
    }
    first
}

/// `EXEC dbo.p @Param = 1`: the parameter name of the called procedure, not
/// a variable of the script.
fn is_named_argument(t: &[Token<'_>], i: usize) -> bool {
    t.get(i + 1).is_some_and(|n| n.text == "=")
        && !matches!(
            word(i.checked_sub(1).and_then(|p| t.get(p))).as_str(),
            "EXEC" | "EXECUTE"
        )
        && matches!(statement_keyword(t, i).as_str(), "EXEC" | "EXECUTE")
}

/// `CREATE/ALTER PROCEDURE|FUNCTION|TRIGGER|VIEW` must be the first
/// statement of its batch, so nothing may be put in front of it.
fn is_module_batch(t: &[Token<'_>]) -> bool {
    matches!(word(t.first()).as_str(), "CREATE" | "ALTER")
        && t.iter().take(6).any(|x| {
            matches!(
                x.upper().as_str(),
                "PROCEDURE" | "PROC" | "FUNCTION" | "TRIGGER" | "VIEW"
            )
        })
}

fn mssql_undeclared(t: &[Token<'_>], out: &mut Vec<String>, seen: &mut HashSet<String>) {
    // the server reports undeclared variables in a module body when it is created
    if is_module_batch(t) {
        return;
    }
    let mut declared = HashSet::new();
    for (i, x) in t.iter().enumerate() {
        if x.kind != TokenKind::Variable || x.text.starts_with("@@") {
            continue;
        }
        let key = x.text.to_lowercase();
        let declares = word(i.checked_sub(1).and_then(|p| t.get(p))) == "DECLARE"
            || (i > 0 && t[i - 1].kind == TokenKind::Comma && in_declare(t, i));
        if declares {
            declared.insert(key);
        } else if !declared.contains(&key) && !is_named_argument(t, i) && seen.insert(key) {
            out.push(x.text.to_string());
        }
    }
}

fn mysql_undeclared(t: &[Token<'_>]) -> Vec<String> {
    if t.iter()
        .any(|x| matches!(x.upper().as_str(), "PROCEDURE" | "FUNCTION" | "TRIGGER"))
    {
        return Vec::new(); // routine bodies declare their own variables
    }
    let assigned: HashSet<String> = t
        .iter()
        .enumerate()
        .filter(|(i, x)| {
            x.kind == TokenKind::Variable
                && (t.get(i + 1).is_some_and(|n| n.text == ":=")
                    || matches!(word(i.checked_sub(1).and_then(|p| t.get(p))).as_str(), "SET" | "INTO")
                    || (*i > 0 && t[i - 1].kind == TokenKind::Comma && {
                        // SET @a = 1, @b = 2  /  INTO @a, @b
                        let start = t[..*i]
                            .iter()
                            .rposition(|y| matches!(y.upper().as_str(), "SET" | "INTO" | "SELECT"));
                        start.is_some_and(|s| t[s].upper() != "SELECT")
                    }))
        })
        .map(|(_, x)| x.text.to_lowercase())
        .collect();
    let mut seen = HashSet::new();
    t.iter()
        .filter(|x| x.kind == TokenKind::Variable && !x.text.starts_with("@@"))
        .filter(|x| !assigned.contains(&x.text.to_lowercase()) && seen.insert(x.text.to_lowercase()))
        .map(|x| x.text.to_string())
        .collect()
}

/// Variables used in `sql` but never declared (SQL Server: per batch) or
/// assigned (MySQL user variables). Postgres has no `@variables`.
pub fn undeclared_variables(sql: &str, dialect: Dialect) -> Vec<String> {
    let tokens = tokenize(sql, dialect);
    match dialect {
        Dialect::Postgres => Vec::new(),
        Dialect::MySql => mysql_undeclared(&sig(&tokens)),
        Dialect::MsSql => {
            let all = sig(&tokens);
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for batch in split_batches(sql, dialect) {
                let t: Vec<Token<'_>> = all
                    .iter()
                    .filter(|x| x.start >= batch.span.start && x.end() <= batch.span.end)
                    .copied()
                    .collect();
                // a variable missing in two batches is asked for once
                let mut batch_seen = HashSet::new();
                let before = out.len();
                mssql_undeclared(&t, &mut out, &mut batch_seen);
                let mut kept = out.split_off(before);
                kept.retain(|v| seen.insert(v.to_lowercase()));
                out.extend(kept);
            }
            out
        }
    }
}

const NUMERIC_TYPES: &[&str] = &[
    "int",
    "bigint",
    "smallint",
    "tinyint",
    "bit",
    "decimal",
    "numeric",
    "money",
    "smallmoney",
    "float",
    "real",
];

/// The value as a SQL literal for a variable of `data_type`.
pub fn literal(value: Option<&str>, data_type: &str, dialect: Dialect) -> String {
    let Some(v) = value else { return "NULL".into() };
    let base = data_type.split('(').next().unwrap_or("").trim().to_ascii_lowercase();
    let numeric_value = {
        let s = v.trim();
        !s.is_empty()
            && s.parse::<f64>().is_ok()
            && !s.contains(['e', 'E', 'x', 'X'])
            && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
    };
    if (NUMERIC_TYPES.contains(&base.as_str()) || (dialect == Dialect::MySql && data_type.is_empty())) && numeric_value
    {
        return v.trim().to_string();
    }
    let quoted = format!("'{}'", v.replace('\'', "''"));
    let unicode = dialect == Dialect::MsSql
        && !matches!(
            base.as_str(),
            "varchar"
                | "char"
                | "date"
                | "datetime"
                | "datetime2"
                | "smalldatetime"
                | "time"
                | "datetimeoffset"
                | "uniqueidentifier"
        );
    if unicode { format!("N{quoted}") } else { quoted }
}

/// Text to put in front of `batch` so the given variables exist in it. It
/// has no line breaks, so error line numbers stay the same.
pub fn prelude(batch: &str, dialect: Dialect, vars: &[VariableValue]) -> String {
    if vars.is_empty() {
        return String::new();
    }
    match dialect {
        Dialect::Postgres => String::new(),
        Dialect::MySql => vars
            .iter()
            .map(|v| {
                format!(
                    "SET {} = {}; ",
                    v.name,
                    literal(v.value.as_deref(), &v.data_type, dialect)
                )
            })
            .collect(),
        Dialect::MsSql => {
            let used: HashSet<String> = tokenize(batch, dialect)
                .iter()
                .filter(|t| t.kind == TokenKind::Variable)
                .map(|t| t.text.to_lowercase())
                .collect();
            let missing: HashSet<String> = undeclared_variables(batch, dialect)
                .iter()
                .map(|v| v.to_lowercase())
                .collect();
            vars.iter()
                .filter(|v| used.contains(&v.name.to_lowercase()) && missing.contains(&v.name.to_lowercase()))
                .map(|v| {
                    let ty = if v.data_type.trim().is_empty() {
                        "nvarchar(4000)"
                    } else {
                        v.data_type.trim()
                    };
                    format!(
                        "DECLARE {} {ty} = {}; ",
                        v.name,
                        literal(v.value.as_deref(), ty, dialect)
                    )
                })
                .collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_undeclared_tsql_variables() {
        let ms = Dialect::MsSql;
        assert_eq!(
            undeclared_variables("SELECT * FROM t WHERE a = @Id AND b = @Name", ms),
            vec!["@Id", "@Name"]
        );
        assert!(undeclared_variables("DECLARE @Id int = 1; SELECT @Id, @@ROWCOUNT", ms).is_empty());
        assert!(undeclared_variables("DECLARE @a int, @b int; SELECT @a + @b", ms).is_empty());
        // named procedure arguments are not variables; the return code variable is
        assert!(undeclared_variables("EXEC dbo.GetOrders @CustomerId = 1, @From = '2024-01-01'", ms).is_empty());
        assert_eq!(undeclared_variables("EXEC @rc = dbo.p @x = 1", ms), vec!["@rc"]);
        // a module body is checked by the server when it is created; nothing goes in front of CREATE
        assert!(undeclared_variables("CREATE PROCEDURE dbo.p @Id int AS SELECT @Id, @Other", ms).is_empty());
        let v = [VariableValue {
            name: "@Other".into(),
            data_type: "int".into(),
            value: Some("1".into()),
        }];
        assert_eq!(prelude("CREATE PROCEDURE dbo.p AS SELECT @Other", ms, &v), "");
        // declared in another batch does not count, and each name is asked once
        assert_eq!(
            undeclared_variables("DECLARE @x int = 1\nGO\nSELECT @x\nGO\nSELECT @x", ms),
            vec!["@x"]
        );
    }

    #[test]
    fn finds_unassigned_mysql_variables() {
        let my = Dialect::MySql;
        assert_eq!(undeclared_variables("SELECT * FROM t WHERE id = @id", my), vec!["@id"]);
        assert!(undeclared_variables("SET @id = 1; SELECT * FROM t WHERE id = @id", my).is_empty());
        assert!(undeclared_variables("SELECT COUNT(*) INTO @n FROM t; SELECT @n", my).is_empty());
        assert!(undeclared_variables("SELECT @@version", my).is_empty());
        assert!(undeclared_variables("SELECT 1 WHERE $1 = 2", Dialect::Postgres).is_empty());
    }

    #[test]
    fn builds_declarations() {
        let vars = vec![
            VariableValue {
                name: "@Id".into(),
                data_type: "int".into(),
                value: Some("5".into()),
            },
            VariableValue {
                name: "@Name".into(),
                data_type: "nvarchar(50)".into(),
                value: Some("O'Brien".into()),
            },
            VariableValue {
                name: "@From".into(),
                data_type: "date".into(),
                value: None,
            },
        ];
        assert_eq!(
            prelude("SELECT @Id, @Name", Dialect::MsSql, &vars),
            "DECLARE @Id int = 5; DECLARE @Name nvarchar(50) = N'O''Brien'; "
        );
        assert_eq!(prelude("DECLARE @Id int = 1; SELECT @Id", Dialect::MsSql, &vars), "");
        assert_eq!(literal(Some("2024-01-31"), "date", Dialect::MsSql), "'2024-01-31'");
        assert_eq!(literal(Some("abc"), "int", Dialect::MsSql), "N'abc'");
        assert!(prelude("SELECT @Id", Dialect::MySql, &vars[..1]).starts_with("SET @Id = 5; "));
    }
}
