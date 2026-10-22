//! Variables a script uses without declaring them, and the declarations
//! that bind values to them before it runs.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::analysis::in_declare;
use crate::keywords::STATEMENT_KEYWORDS;
use crate::lexer::{Token, TokenKind, significant, tokenize, word};
use crate::sqltype::SqlType;
use crate::statements::{defines_module, split_batches, tokens_in};

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

/// `EXEC dbo.p @Param = 1`, or `dbo.p @Param = 1` first in a batch (run
/// without EXEC, `bare_call`): the parameter name of the called procedure,
/// not a variable of the script.
fn is_named_argument(t: &[Token<'_>], i: usize, bare_call: bool) -> bool {
    let keyword = statement_keyword(t, i);
    t.get(i + 1).is_some_and(|n| n.text == "=")
        && !matches!(
            word(i.checked_sub(1).and_then(|p| t.get(p))).as_str(),
            "EXEC" | "EXECUTE"
        )
        && (matches!(keyword.as_str(), "EXEC" | "EXECUTE")
            // still in the call that starts the batch
            || (bare_call
                && !STATEMENT_KEYWORDS.contains(&keyword.as_str())
                && !t[..i].iter().any(|x| x.kind == TokenKind::Semicolon)))
}

fn mssql_undeclared(t: &[Token<'_>], out: &mut Vec<String>, seen: &mut HashSet<String>) {
    // the server reports undeclared variables in a module body when it is
    // created, and CREATE PROCEDURE must start its batch: nothing goes in front
    if defines_module(t) {
        return;
    }
    let bare_call = !t.is_empty() && crate::safety::bare_call(t, 0);
    let mut declared = HashSet::new();
    for (i, x) in t.iter().enumerate() {
        if x.kind != TokenKind::Variable || x.text.starts_with("@@") {
            continue;
        }
        let key = x.text.to_lowercase();
        let prev = word(i.checked_sub(1).and_then(|p| t.get(p)));
        let declares = prev == "DECLARE" || (i > 0 && t[i - 1].kind == TokenKind::Comma && in_declare(t, i));
        // a table variable (FROM @t, INSERT INTO @t): no value can stand in for
        // it, so the server's "must declare the table variable" is the answer
        let table = matches!(
            prev.as_str(),
            "FROM" | "JOIN" | "INTO" | "UPDATE" | "TABLE" | "MERGE" | "USING" | "INSERT" | "DELETE" | "APPLY"
        );
        if declares {
            declared.insert(key);
        } else if !declared.contains(&key) && !table && !is_named_argument(t, i, bare_call) && seen.insert(key) {
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
        Dialect::MySql => mysql_undeclared(&significant(&tokens)),
        Dialect::MsSql => {
            let all = significant(&tokens);
            let mut out = Vec::new();
            let mut seen = HashSet::new();
            for batch in split_batches(sql, dialect) {
                // a variable missing in two batches is asked for once
                let mut batch_seen = HashSet::new();
                let before = out.len();
                mssql_undeclared(tokens_in(&all, &batch.span), &mut out, &mut batch_seen);
                let mut kept = out.split_off(before);
                kept.retain(|v| seen.insert(v.to_lowercase()));
                out.extend(kept);
            }
            out
        }
    }
}

/// The value as a SQL literal for a variable of `data_type`.
pub fn literal(value: Option<&str>, data_type: &str, dialect: Dialect) -> String {
    let Some(v) = value else { return "NULL".into() };
    let ty = SqlType::parse(data_type);
    let numeric_value = {
        let s = v.trim();
        !s.is_empty()
            && s.parse::<f64>().is_ok()
            && !s.contains(['e', 'E', 'x', 'X'])
            && s.chars().all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
    };
    let numeric_type = ty.is_number() || ty.base() == "bit" || (dialect == Dialect::MySql && data_type.is_empty());
    if numeric_type && numeric_value {
        return v.trim().to_string();
    }
    dialect.typed_string(v, &ty)
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
            let mut declarations: String = vars
                .iter()
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
                .collect();
            // a procedure named first runs without EXEC only while it is first
            if !declarations.is_empty() && crate::safety::bare_call_start(batch).is_some() {
                declarations.push_str("EXEC ");
            }
            declarations
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A procedure named first in a batch runs without EXEC: its parameter
    /// names are not variables, and declarations in front keep it a call.
    #[test]
    fn procedure_named_first_in_a_batch() {
        let ms = Dialect::MsSql;
        let sql = "dbo.GetOrders @CustomerId = @x, @Top = 5";
        assert_eq!(undeclared_variables(sql, ms), vec!["@x"]);
        let x = VariableValue {
            name: "@x".into(),
            data_type: "int".into(),
            value: Some("7".into()),
        };
        assert_eq!(prelude(sql, ms, std::slice::from_ref(&x)), "DECLARE @x int = 7; EXEC ");
        // a statement after the call names variables again
        assert_eq!(undeclared_variables("dbo.p @a = 1; SELECT @b = 2", ms), vec!["@b"]);
        assert_eq!(prelude("SELECT @x", ms, &[x]), "DECLARE @x int = 7; ");
    }

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
        // ... but a batch that switches a trigger off is not a module
        assert_eq!(
            undeclared_variables("ALTER TABLE dbo.t DISABLE TRIGGER ALL; UPDATE dbo.t SET a = @x", ms),
            vec!["@x"]
        );
        // declared in another batch does not count, and each name is asked once
        assert_eq!(
            undeclared_variables("DECLARE @x int = 1\nGO\nSELECT @x\nGO\nSELECT @x", ms),
            vec!["@x"]
        );
        // a table variable has no value to ask for: the server says it must be declared
        assert!(undeclared_variables("INSERT INTO @t (a) VALUES (1); SELECT * FROM @t", ms).is_empty());
        assert_eq!(
            undeclared_variables("SELECT * FROM t JOIN @ids i ON i.Id = t.Id WHERE t.x = @x", ms),
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
        // ISO 8601, which no DATEFORMAT misreads
        assert_eq!(
            literal(Some("2024-01-12 10:20:30.123"), "datetime", Dialect::MsSql),
            "'2024-01-12T10:20:30.123'"
        );
        assert_eq!(
            literal(Some("2024-01-12 10:20"), "smalldatetime", Dialect::MsSql),
            "'2024-01-12T10:20'"
        );
        assert_eq!(literal(Some("12/01/2024"), "datetime", Dialect::MsSql), "'12/01/2024'");
        assert_eq!(literal(Some("abc"), "int", Dialect::MsSql), "N'abc'");
        assert!(prelude("SELECT @Id", Dialect::MySql, &vars[..1]).starts_with("SET @Id = 5; "));
        // a path typed for MySQL keeps its backslashes
        assert_eq!(literal(Some("C:\\temp"), "", Dialect::MySql), "'C:\\\\temp'");
        assert_eq!(literal(Some("C:\\temp"), "nvarchar(50)", Dialect::MsSql), "N'C:\\temp'");
    }
}
