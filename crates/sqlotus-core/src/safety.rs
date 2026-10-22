//! What a script does when it runs, read before running it: the changes it
//! makes (for read-only connections, production confirmations and manual
//! commit), the statements worth a warning, the procedures it calls and
//! whether it manages its own transactions.
//!
//! Statements in the body of a routine, trigger or view definition are
//! stored by the server, not run, when the script runs, so they do not
//! count. MySQL runs the text of `/*!50003 ... */` comments (dumps wrap
//! CREATE TRIGGER and friends in them), so that text does.

use std::ops::Range;

use crate::catalog::{ObjectDefinition, ObjectKind};
use crate::keywords::STATEMENT_KEYWORDS;
use crate::lexer::{Token, TokenKind, matching_paren, tokenize, word};
use crate::statements::{definition_ranges, split_batches, split_statements};
use crate::{Dialect, keywords};

/// A script read once for the checks made before it runs.
pub struct ScriptRun<'a> {
    dialect: Dialect,
    /// Significant tokens in source order.
    t: Vec<Token<'a>>,
    /// Indexes into `t` where statements start, sorted.
    starts: Vec<usize>,
    /// Index ranges of routine, trigger and view definitions, sorted: of each,
    /// only the first token (CREATE / ALTER) is part of the run.
    definitions: Vec<Range<usize>>,
    /// Indexes into `t` where batches start, sorted.
    batch_starts: Vec<usize>,
}

impl<'a> ScriptRun<'a> {
    pub fn new(sql: &'a str, dialect: Dialect) -> Self {
        let t = significant_tokens(sql, dialect);
        let index = |offset: usize| t.partition_point(|x| x.start < offset);
        // statements start after each `;` too: MySQL sends `a; b` between two
        // custom delimiters to the server as two statements
        let mut starts: Vec<usize> = split_statements(sql, dialect)
            .iter()
            .map(|s| index(s.start))
            .chain(
                t.iter()
                    .enumerate()
                    .filter(|(_, x)| x.kind == TokenKind::Semicolon)
                    .map(|(i, _)| i + 1),
            )
            .filter(|&i| i < t.len())
            .collect();
        starts.sort_unstable();
        starts.dedup();
        let batches: Vec<Range<usize>> = split_batches(sql, dialect)
            .iter()
            .map(|b| index(b.span.start)..index(b.span.end))
            .collect();
        let definitions = definition_ranges(&t, &batches, &starts, dialect);
        let batch_starts = batches.iter().filter(|b| b.start < b.end).map(|b| b.start).collect();
        Self {
            dialect,
            t,
            starts,
            definitions,
            batch_starts,
        }
    }

    /// Whether token `i` is stored in a definition's body rather than run.
    fn stored(&self, i: usize) -> bool {
        let k = self.definitions.partition_point(|r| r.start < i);
        k > 0 && i < self.definitions[k - 1].end
    }

    /// Statements that would change data, schema or the server, as `KEYWORD
    /// target` strings. Changes to the session's own temp tables (`#t`,
    /// table variables, `CREATE TEMP TABLE`) are not listed.
    pub fn changes(&self) -> Vec<String> {
        const MODIFYING: &[&str] = &[
            "INSERT", "UPDATE", "DELETE", "MERGE", "TRUNCATE", "DROP", "ALTER", "CREATE", "GRANT", "REVOKE", "DENY",
            "RENAME", "REPLACE", "UPSERT",
        ];
        let (t, dialect) = (&self.t, self.dialect);
        let mut out = Vec::new();
        let mut stmt_begin = 0;
        for (i, tok) in t.iter().enumerate() {
            if self.stored(i) {
                continue;
            }
            let at_start = self.starts.binary_search(&i).is_ok();
            if at_start {
                stmt_begin = i;
            }
            let u = word(Some(tok));
            let prev = if i > 0 { word(t.get(i - 1)) } else { String::new() };
            let prev_kind = i.checked_sub(1).map(|p| t[p].kind);
            // keywords used as parts of other statements (MERGE ... THEN DELETE),
            // not statements themselves
            let part_of_other = matches!(
                prev.as_str(),
                "ON" | "OF" | "AFTER" | "BEFORE" | "FOR" | "INSTEAD" | "GRANT" | "DENY" | "REVOKE" | "KEY" | "THEN"
            ) || matches!(prev_kind, Some(TokenKind::Dot | TokenKind::Comma))
                || (prev_kind == Some(TokenKind::LParen) && !nested_change(t, i))
                || t.get(i + 1)
                    .is_some_and(|n| n.kind == TokenKind::LParen && u != "INSERT");
            if u == "UPDATE" && word(t.get(i + 1)) == "STATISTICS" && !part_of_other {
                // maintenance, not a row change
                out.push(
                    format!("UPDATE STATISTICS {}", statement_target(&t[i + 2..]))
                        .trim()
                        .to_string(),
                );
            } else if MODIFYING.contains(&u.as_str())
                && !part_of_other
                && !(u == "REPLACE" && dialect != Dialect::MySql)
                && !session_local(&u, &t[i + 1..], dialect)
            {
                out.push(format!("{u} {}", statement_target(&t[i + 1..])).trim().to_string());
            }
            // KILL / SHUTDOWN / RESTART reach other sessions; only as a statement, not a column named kill
            let statement_start = at_start || i == 0 || prev_kind == Some(TokenKind::Semicolon) || prev == "GO";
            if statement_start
                && (u == "KILL"
                    || (u == "SHUTDOWN" && dialect != Dialect::Postgres)
                    || (u == "RESTART" && dialect == Dialect::MySql))
            {
                out.push(u.clone());
            }
            // SELECT ... INTO new_table creates a table (not INTO #temp; in
            // MySQL INTO names variables, or a file)
            if u == "INTO"
                && prev != "INSERT"
                && prev != "MERGE"
                && t[stmt_begin..i].iter().any(|x| word(Some(x)) == "SELECT")
            {
                let next = t.get(i + 1);
                let next_word = word(next);
                match dialect {
                    Dialect::MySql if matches!(next_word.as_str(), "OUTFILE" | "DUMPFILE") => {
                        out.push(format!("SELECT INTO {next_word}"));
                    }
                    Dialect::MySql => {}
                    Dialect::MsSql
                        if next.is_some_and(|n| n.text.starts_with('#') || n.kind == TokenKind::Variable) => {}
                    Dialect::Postgres if matches!(next_word.as_str(), "TEMP" | "TEMPORARY") => {}
                    _ => out.push("SELECT INTO".into()),
                }
            }
            if let Some(change) = other_change(t, i, stmt_begin, at_start, dialect) {
                out.push(change);
            }
        }
        out
    }

    /// Procedure calls (`EXEC p`, `EXECUTE p`, `CALL p`, and in T-SQL a
    /// procedure named first in a batch, which runs without EXEC): they may
    /// change data without any modifying keyword in the script itself.
    pub fn calls(&self) -> Vec<String> {
        let t = &self.t;
        let mut out = Vec::new();
        for (i, tok) in t.iter().enumerate() {
            if self.dialect == Dialect::MsSql && self.batch_starts.binary_search(&i).is_ok() && bare_call(t, i) {
                out.push(format!("EXEC {}", statement_target(&t[i..])).trim().to_string());
                continue;
            }
            let u = word(Some(tok));
            if !matches!(u.as_str(), "EXEC" | "EXECUTE" | "CALL") || self.stored(i) {
                continue;
            }
            let prev = if i > 0 { word(t.get(i - 1)) } else { String::new() };
            let next = word(t.get(i + 1));
            // GRANT EXECUTE ON, WITH EXECUTE AS, EXECUTE AS USER, a column named "call"
            let not_a_call = matches!(prev.as_str(), "GRANT" | "DENY" | "REVOKE" | "WITH")
                || next == "AS"
                || next == "ON"
                || i.checked_sub(1)
                    .is_some_and(|p| matches!(t[p].kind, TokenKind::Dot | TokenKind::Comma))
                || t.get(i + 1).is_none_or(|n| n.kind == TokenKind::Semicolon);
            if !not_a_call {
                out.push(format!("{u} {}", statement_target(&t[i + 1..])).trim().to_string());
            }
        }
        out
    }

    /// Whether the script manages transactions itself (BEGIN TRAN, COMMIT,
    /// ROLLBACK, SAVEPOINT, autocommit settings, ...).
    pub fn controls_transactions(&self) -> bool {
        let (t, dialect) = (&self.t, self.dialect);
        t.iter().enumerate().any(|(i, tok)| {
            let next = word(t.get(i + 1));
            let controls = match word(Some(tok)).as_str() {
                "COMMIT" | "ROLLBACK" | "SAVEPOINT" => true,
                "BEGIN" => {
                    matches!(next.as_str(), "TRAN" | "TRANSACTION" | "DISTRIBUTED" | "WORK")
                        || (dialect != Dialect::MsSql && t.get(i + 1).is_none_or(|n| n.kind == TokenKind::Semicolon))
                }
                "START" => next == "TRANSACTION",
                "SAVE" => matches!(next.as_str(), "TRAN" | "TRANSACTION"),
                "SET" => matches!(next.as_str(), "IMPLICIT_TRANSACTIONS" | "AUTOCOMMIT" | "XACT_ABORT"),
                _ => false,
            };
            controls && !self.stored(i)
        })
    }

    /// Whether the script can run inside a transaction that Sqlotus commits
    /// or rolls back: every change it makes is a row change (INSERT, UPDATE,
    /// DELETE, MERGE) — no DDL, which some servers commit implicitly — and it
    /// does not manage transactions itself.
    pub fn fits_in_transaction(&self) -> bool {
        self.changes().iter().all(|c| {
            matches!(
                c.split(' ').next().unwrap_or(""),
                "INSERT" | "UPDATE" | "DELETE" | "MERGE" | "REPLACE" | "UPSERT"
            ) && !c.starts_with("UPDATE STATISTICS")
        }) && !self.controls_transactions()
    }

    /// Whether rows change in a statement nested in another, whose count
    /// the server does not report: a PostgreSQL CTE that deletes (`WITH d AS
    /// (DELETE ... RETURNING *) SELECT ...`), or the OUTPUT of a T-SQL change
    /// read by an INSERT.
    pub fn has_nested_changes(&self) -> bool {
        (0..self.t.len()).any(|i| nested_change(&self.t, i) && !self.stored(i))
    }

    /// Statements the server refuses inside a transaction (BACKUP, RESTORE,
    /// CREATE / ALTER / DROP DATABASE, PostgreSQL's VACUUM and CONCURRENTLY
    /// index builds, ...): a script with them must not be wrapped in one.
    pub fn refuses_transaction(&self) -> Vec<String> {
        let dialect = self.dialect;
        self.changes()
            .into_iter()
            .filter(|c| {
                let words: Vec<&str> = c.split(' ').collect();
                let second = words.get(1).copied().unwrap_or("");
                match words[0] {
                    "BACKUP" | "RESTORE" | "RECONFIGURE" => dialect == Dialect::MsSql,
                    "CREATE" | "ALTER" | "DROP" if second == "DATABASE" => dialect != Dialect::MySql,
                    "CREATE" | "ALTER" | "DROP" if second == "FULLTEXT" => dialect == Dialect::MsSql,
                    "ALTER" if second == "SYSTEM" => dialect == Dialect::Postgres,
                    "CREATE" | "DROP" if second == "TABLESPACE" => dialect == Dialect::Postgres,
                    // maintenance: VACUUM never runs in a transaction block, CLUSTER and
                    // REINDEX not in all forms, and none needs one
                    "VACUUM" | "CLUSTER" | "REINDEX" => dialect == Dialect::Postgres,
                    _ => dialect == Dialect::Postgres && words.contains(&"CONCURRENTLY"),
                }
            })
            .collect()
    }

    /// PostgreSQL DO blocks of the script, and procedures it calls, that
    /// commit or roll back by themselves: PostgreSQL refuses to run them
    /// inside a transaction block ("invalid transaction termination"), so
    /// Sqlotus must not open one around the script. `definitions`: the
    /// database's stored routines, to look into the procedures called.
    pub fn commits_by_itself(&self, definitions: &[ObjectDefinition]) -> Vec<String> {
        if self.dialect != Dialect::Postgres {
            return Vec::new();
        }
        let t = &self.t;
        let mut out = Vec::new();
        for (i, tok) in t.iter().enumerate() {
            if self.stored(i) || self.starts.binary_search(&i).is_err() {
                continue;
            }
            match word(Some(tok)).as_str() {
                // DO [LANGUAGE x] 'body' (or DO 'body' LANGUAGE x)
                "DO" => {
                    let body = t[i + 1..].iter().take(3).find(|x| x.kind == TokenKind::String);
                    if body.is_some_and(|b| body_controls_transactions(b.text)) {
                        out.push("a DO block".to_string());
                    }
                }
                // CALL [schema.]name(...)
                "CALL" => {
                    let parts: Vec<String> = t[i + 1..]
                        .iter()
                        .take_while(|x| x.is_name() || x.kind == TokenKind::Dot)
                        .filter(|x| x.is_name())
                        .map(|x| x.ident_value())
                        .collect();
                    let Some(name) = parts.last() else { continue };
                    let schema = (parts.len() > 1).then(|| &parts[parts.len() - 2]);
                    let called = definitions.iter().find(|d| {
                        d.kind == ObjectKind::Procedure
                            && d.name.eq_ignore_ascii_case(name)
                            && schema.is_none_or(|s| d.schema.eq_ignore_ascii_case(s))
                    });
                    if let Some(d) = called.filter(|d| definition_controls_transactions(&d.definition)) {
                        out.push(format!("{}.{}", d.schema, d.name));
                    }
                }
                _ => {}
            }
        }
        out
    }

    /// Whether the script changes rows and nothing else (see
    /// [`ScriptRun::fits_in_transaction`]).
    pub fn row_changes_only(&self) -> bool {
        !self.changes().is_empty() && self.fits_in_transaction()
    }

    /// Whether running the script changes the database's objects (CREATE,
    /// ALTER, DROP, RENAME, SELECT INTO a new table, `sp_rename`), so objects
    /// loaded for completion and analysis are out of date afterwards.
    pub fn changes_schema(&self) -> bool {
        let ddl = self.changes().iter().any(|c| {
            c == "SELECT INTO"
                || matches!(
                    c.split(' ').next().unwrap_or(""),
                    "CREATE" | "ALTER" | "DROP" | "RENAME"
                )
        });
        ddl || self.calls().iter().any(|c| {
            let target = c.split(' ').nth(1).unwrap_or("");
            let name = target.rsplit('.').next().unwrap_or("");
            name.trim_matches(['[', ']', '"']).eq_ignore_ascii_case("sp_rename")
        })
    }

    /// Warnings to confirm before running: UPDATE / DELETE without WHERE,
    /// TRUNCATE, DROP TABLE / DATABASE / SCHEMA / COLUMN, and statements that
    /// reach other sessions (ROLLBACK IMMEDIATE, SET OFFLINE, KILL,
    /// SHUTDOWN).
    pub fn warnings(&self) -> Vec<String> {
        let (t, dialect) = (&self.t, self.dialect);
        let mut out = Vec::new();
        for i in unfiltered_modifications(t) {
            if self.stored(i) {
                continue;
            }
            let target = changed_table(&t[i + 1..]);
            out.push(format!(
                "{} {}has no WHERE clause and will affect every row.",
                t[i].upper(),
                if target.is_empty() {
                    String::new()
                } else {
                    format!("{target} ")
                }
            ));
        }
        for (i, tok) in t.iter().enumerate() {
            if self.stored(i) {
                continue;
            }
            let u = word(Some(tok));
            let next = word(t.get(i + 1));
            let prev = if i > 0 { word(t.get(i - 1)) } else { String::new() };
            // `TRUNCATE t` is valid in PostgreSQL and MySQL; `TRUNCATE(x, 2)` is a MySQL function
            let next_is_name = t.get(i + 1).is_some_and(|n| n.is_name());
            let call = t.get(i + 1).is_some_and(|n| n.kind == TokenKind::LParen);
            if u == "TRUNCATE" && !call && (next == "TABLE" || next_is_name) {
                out.push("TRUNCATE TABLE removes every row.".into());
            }
            if u == "DROP"
                && matches!(next.as_str(), "TABLE" | "DATABASE" | "SCHEMA")
                && prev != "IF"
                // a temp table of the session (`DROP TABLE #t`) is the script's own
                && !session_local(&u, &t[i + 1..], dialect)
            {
                out.push(format!("DROP {next} permanently removes the object and its data."));
            }
            // ALTER TABLE ... DROP COLUMN c, or MySQL's ALTER TABLE ... DROP c
            let mysql_short = dialect == Dialect::MySql
                && next_is_name
                && !matches!(
                    next.as_str(),
                    "CONSTRAINT"
                        | "INDEX"
                        | "KEY"
                        | "PRIMARY"
                        | "FOREIGN"
                        | "CHECK"
                        | "PARTITION"
                        | "DEFAULT"
                        | "TRIGGER"
                )
                && in_alter_table(&t[..i]);
            if u == "DROP" && (next == "COLUMN" || mysql_short) {
                out.push("DROP COLUMN permanently removes the column and its data.".into());
            }
            // statements that reach other people's sessions or the whole server
            let statement_start = i == 0 || t[i - 1].kind == TokenKind::Semicolon || prev == "GO";
            if u == "ROLLBACK" && next == "IMMEDIATE" && prev == "WITH" {
                out.push(
                    "WITH ROLLBACK IMMEDIATE disconnects other users and rolls back their open transactions.".into(),
                );
            }
            if u == "OFFLINE" && prev == "SET" {
                out.push("SET OFFLINE takes the database offline for everyone.".into());
            }
            if u == "KILL"
                && statement_start
                && t.get(i + 1).is_some_and(|n| n.kind == TokenKind::Number || n.is_name())
            {
                out.push("KILL ends another session and rolls back its open transaction.".into());
            }
            if u == "SHUTDOWN" && statement_start && dialect == Dialect::MsSql {
                out.push("SHUTDOWN stops the SQL Server instance for everyone.".into());
            }
        }
        out
    }
}

/// Whether a PostgreSQL routine body (a quoted or dollar-quoted string)
/// commits or rolls back.
fn body_controls_transactions(quoted: &str) -> bool {
    let inner = if let Some(after) = quoted.strip_prefix('$') {
        // $tag$ body $tag$
        let Some(close) = after.find('$') else { return false };
        let tag = &quoted[..close + 2];
        match quoted.strip_suffix(tag) {
            Some(body) if body.len() >= tag.len() => body[tag.len()..].to_string(),
            _ => return false,
        }
    } else if let Some(body) = quoted.strip_prefix('\'').and_then(|q| q.strip_suffix('\'')) {
        body.replace("''", "'")
    } else {
        return false;
    };
    tokenize(&inner, Dialect::Postgres)
        .iter()
        .any(|t| t.kind == TokenKind::Word && matches!(t.upper().as_str(), "COMMIT" | "ROLLBACK"))
}

/// Whether the stored definition of a PostgreSQL procedure commits or rolls
/// back in its body.
fn definition_controls_transactions(definition: &str) -> bool {
    tokenize(definition, Dialect::Postgres)
        .iter()
        .any(|t| t.kind == TokenKind::String && body_controls_transactions(t.text))
}

/// Significant tokens of `sql`; for MySQL also the code inside `/*!50003
/// ... */` comments, which the server runs.
fn significant_tokens(sql: &str, dialect: Dialect) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    for t in tokenize(sql, dialect) {
        let runs = dialect == Dialect::MySql
            && t.kind == TokenKind::BlockComment
            && t.text.starts_with("/*!")
            && t.text.ends_with("*/")
            && t.text.len() >= 5;
        if runs {
            let digits = t.text[3..].chars().take_while(char::is_ascii_digit).count();
            let inner = (t.start + 3 + digits).min(t.end() - 2)..t.end() - 2;
            out.extend(
                tokenize(&sql[inner.clone()], dialect)
                    .into_iter()
                    .filter(|x| !x.is_trivia())
                    .map(|x| Token {
                        start: x.start + inner.start,
                        ..x
                    }),
            );
        } else if !t.is_trivia() {
            out.push(t);
        }
    }
    out
}

/// UPDATE / DELETE statements (index of the keyword) with no WHERE clause.
pub(crate) fn unfiltered_modifications(t: &[Token<'_>]) -> Vec<usize> {
    let mut found = Vec::new();
    for (i, tok) in t.iter().enumerate() {
        let u = word(Some(tok));
        if u != "UPDATE" && u != "DELETE" {
            continue;
        }
        let prev = if i > 0 { word(t.get(i - 1)) } else { String::new() };
        let prev_kind = i.checked_sub(1).map(|p| t[p].kind);
        // ON DELETE CASCADE, BEFORE / AFTER INSERT OR UPDATE (triggers), GRANT DELETE,
        // UPDATE(col), MERGE ... THEN DELETE
        if matches!(
            prev.as_str(),
            "ON" | "OF" | "AFTER" | "BEFORE" | "OR" | "FOR" | "INSTEAD" | "GRANT" | "DENY" | "REVOKE" | "THEN" | "KEY"
        ) || prev_kind == Some(TokenKind::Comma)
            || (prev_kind == Some(TokenKind::LParen) && !nested_change(t, i))
            || t.get(i + 1).is_some_and(|n| n.kind == TokenKind::LParen)
            || word(t.get(i + 1)) == "STATISTICS"
        {
            continue;
        }
        let mut depth = 0i32;
        let mut has_where = false;
        for (k, n) in t.iter().enumerate().skip(i + 1) {
            match n.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                TokenKind::Semicolon if depth <= 0 => break,
                TokenKind::Word if depth == 0 => {
                    let w = n.upper();
                    if w == "WHERE" || w == "CURRENT" {
                        has_where = true;
                        break;
                    }
                    // UPDATE ... FROM t JOIN inserted ON ... / UPDATE a JOIN b ON ... SET:
                    // an inner join limits the rows (an outer or cross join does not)
                    if w == "JOIN"
                        && !matches!(
                            word(t.get(k - 1)).as_str(),
                            "LEFT" | "RIGHT" | "FULL" | "OUTER" | "CROSS"
                        )
                    {
                        has_where = true;
                        break;
                    }
                    // a nested SELECT in UPDATE ... FROM (SELECT ...) stays inside parens;
                    // at depth 0 a new statement keyword ends this one (SET belongs to UPDATE)
                    let ends = STATEMENT_KEYWORDS.contains(&w.as_str())
                        && !(w == "SET" && u == "UPDATE")
                        && !(w == "SELECT" && k > 0 && t[k - 1].kind == TokenKind::LParen);
                    if ends {
                        break;
                    }
                }
                _ => {}
            }
            if depth < 0 {
                break;
            }
        }
        if !has_where {
            found.push(i);
        }
    }
    found
}

/// Whether the INSERT, UPDATE, DELETE or MERGE at `i`, right after a `(`, is
/// a statement of its own nested in another: a PostgreSQL CTE that changes
/// rows (`WITH d AS [MATERIALIZED] (DELETE ...)`) or a T-SQL change whose
/// OUTPUT an INSERT reads (`INSERT ... SELECT ... FROM (DELETE ... OUTPUT ...)`).
fn nested_change(t: &[Token<'_>], i: usize) -> bool {
    i >= 2
        && t[i - 1].kind == TokenKind::LParen
        && matches!(word(t.get(i - 2)).as_str(), "AS" | "MATERIALIZED" | "FROM")
        && matches!(word(t.get(i)).as_str(), "INSERT" | "UPDATE" | "DELETE" | "MERGE")
}

/// Where the T-SQL batch `batch` names a procedure first, which then runs
/// without EXEC: the byte offset of the name. Text put in front of such a
/// batch must add an EXEC, or the server no longer reads a call.
pub fn bare_call_start(batch: &str) -> Option<usize> {
    let t = significant_tokens(batch, Dialect::MsSql);
    (!t.is_empty() && bare_call(&t, 0)).then(|| t[0].start)
}

/// Whether the batch starting at `i` names a procedure first, which T-SQL
/// runs without EXEC (`dbo.ArchiveOrders @Days = 30`, `sp_rename 'a', 'b'`).
pub(crate) fn bare_call(t: &[Token<'_>], i: usize) -> bool {
    // statements that start with a word the keyword list does not reserve
    const STATEMENTS: &[&str] = &[
        "THROW",
        "ENABLE",
        "DISABLE",
        "SEND",
        "RECEIVE",
        "GET",
        "MOVE",
        "REVERT",
        "GO",
        "MERGE",
        "WITH",
        "CHECKPOINT",
        "DBCC",
        "SHUTDOWN",
        "RECONFIGURE",
        "SAVE",
        "LOAD",
        "SETUSER",
        "READTEXT",
        "UPDATETEXT",
        "WRITETEXT",
        "RENAME",
        "COPY",
    ];
    let first = &t[i];
    let named = match first.kind {
        TokenKind::QuotedIdent => true,
        // a temporary procedure (`#p 1`) too
        TokenKind::Word => {
            let u = first.upper();
            !keywords::is_reserved(&u, Dialect::MsSql)
                && !STATEMENT_KEYWORDS.contains(&u.as_str())
                && !STATEMENTS.contains(&u.as_str())
        }
        _ => false,
    };
    // `name:` is a label, `name(` or `name =` no call; `p -1` passes a number
    named
        && !t.get(i + 1).is_some_and(|n| {
            n.kind == TokenKind::LParen || (n.kind == TokenKind::Operator && !matches!(n.text, "-" | "+"))
        })
}

/// Whether a change only touches the session's own objects: T-SQL temp
/// tables (`#t`, `##t`) and table variables (`@t`), or a temporary table
/// created or dropped as such.
fn session_local(keyword: &str, rest: &[Token<'_>], dialect: Dialect) -> bool {
    const LEAD: &[&str] = &["FROM", "INTO", "TABLE", "IF", "EXISTS", "NOT", "TOP"];
    match dialect {
        Dialect::MsSql => {
            // every name the statement targets, e.g. DROP TABLE #a, #b
            let mut names = rest
                .iter()
                .take_while(|x| !matches!(x.kind, TokenKind::Semicolon | TokenKind::LParen))
                .skip_while(|x| x.kind == TokenKind::Word && LEAD.contains(&x.upper().as_str()))
                .take_while(|x| {
                    x.kind != TokenKind::Word || x.text.starts_with('#') || !keywords::is_reserved(&x.upper(), dialect)
                })
                .filter(|x| x.kind != TokenKind::Comma)
                .peekable();
            if names.peek().is_none() {
                return false;
            }
            let local = |x: &Token<'_>| x.kind == TokenKind::Variable || x.text.starts_with('#');
            if keyword == "DROP" {
                names.all(local)
            } else {
                names.next().is_some_and(local)
            }
        }
        Dialect::Postgres | Dialect::MySql => {
            matches!(keyword, "CREATE" | "DROP")
                && rest
                    .iter()
                    .take(4)
                    .any(|x| matches!(x.upper().as_str(), "TEMP" | "TEMPORARY"))
        }
    }
}

/// Changes without the usual keywords: PostgreSQL DO blocks, COPY ... FROM,
/// REFRESH MATERIALIZED VIEW, VACUUM / CLUSTER / REINDEX and functions that
/// end sessions or reset sequences; T-SQL RESTORE, BACKUP, DBCC (all but the
/// read-only checks), RECONFIGURE, ENABLE / DISABLE TRIGGER, UPDATETEXT /
/// WRITETEXT; MySQL LOAD DATA, SET GLOBAL / PERSIST, INSTALL / UNINSTALL,
/// PURGE, RESET, FLUSH, OPTIMIZE / REPAIR TABLE.
fn other_change(t: &[Token<'_>], i: usize, stmt_begin: usize, at_start: bool, dialect: Dialect) -> Option<String> {
    const DBCC_READ_ONLY: &[&str] = &[
        "SHOW_STATISTICS",
        "INPUTBUFFER",
        "OUTPUTBUFFER",
        "OPENTRAN",
        "SQLPERF",
        "USEROPTIONS",
        "TRACESTATUS",
        "PROCCACHE",
        "SHOWCONTIG",
        "HELP",
        "CHECKDB",
        "CHECKTABLE",
        "CHECKALLOC",
        "CHECKCATALOG",
        "CHECKFILEGROUP",
        "CHECKCONSTRAINTS",
        "MEMORYSTATUS",
        "LOGINFO",
    ];
    let tok = &t[i];
    if tok.kind != TokenKind::Word || (i > 0 && t[i - 1].kind == TokenKind::Dot) {
        return None;
    }
    let u = tok.upper();
    let next = word(t.get(i + 1));
    let target = || statement_target(&t[i + 1..]);
    // the rest of the statement
    let stmt_end = t[i..]
        .iter()
        .position(|x| x.kind == TokenKind::Semicolon)
        .map_or(t.len(), |p| i + p);
    // a word of this statement outside parentheses
    let in_stmt = |w: &str| {
        let mut depth = 0i32;
        t[stmt_begin..stmt_end].iter().any(|x| {
            match x.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                _ => {}
            }
            depth == 0 && x.kind == TokenKind::Word && x.upper() == w
        })
    };
    match dialect {
        Dialect::MsSql => match u.as_str() {
            "RESTORE" | "BACKUP" if matches!(next.as_str(), "DATABASE" | "LOG") => Some(
                format!("{u} {next} {}", statement_target(&t[(i + 2).min(t.len())..]))
                    .trim()
                    .to_string(),
            ),
            "DBCC"
                if !DBCC_READ_ONLY.contains(&next.as_str())
                    || t[i..stmt_end].iter().any(|x| x.upper().starts_with("REPAIR_")) =>
            {
                Some(format!("DBCC {next}"))
            }
            "RECONFIGURE" | "UPDATETEXT" | "WRITETEXT" => Some(u),
            "ENABLE" | "DISABLE" if next == "TRIGGER" => Some(format!("{u} {}", target())),
            _ => None,
        },
        Dialect::Postgres => match u.as_str() {
            "DO" if at_start => Some("DO block".into()),
            "COPY" if at_start && in_stmt("FROM") => Some(format!("COPY {}", target())),
            // the server runs a program, or writes a file of its own
            "COPY" if at_start && in_stmt("TO") && !in_stmt("STDOUT") => Some(if in_stmt("PROGRAM") {
                "COPY TO PROGRAM".into()
            } else {
                "COPY TO a server file".into()
            }),
            "REFRESH" if at_start && next == "MATERIALIZED" => Some(format!(
                "REFRESH MATERIALIZED {}",
                statement_target(&t[(i + 2).min(t.len())..])
            )),
            "VACUUM" | "CLUSTER" | "REINDEX" if at_start => Some(u),
            "PG_TERMINATE_BACKEND" | "PG_CANCEL_BACKEND" | "SETVAL"
                if t.get(i + 1).is_some_and(|n| n.kind == TokenKind::LParen) =>
            {
                Some(tok.text.to_ascii_lowercase())
            }
            _ => None,
        },
        Dialect::MySql => match u.as_str() {
            "LOAD" if at_start && matches!(next.as_str(), "DATA" | "XML") => Some(format!("LOAD {next}")),
            "SET"
                if at_start
                    && t[i + 1..stmt_end].iter().any(|x| {
                        let w = x.upper();
                        (x.kind == TokenKind::Word && matches!(w.as_str(), "GLOBAL" | "PERSIST" | "PERSIST_ONLY"))
                            || (x.kind == TokenKind::Variable
                                && matches!(w.as_str(), "@@GLOBAL" | "@@PERSIST" | "@@PERSIST_ONLY"))
                    }) =>
            {
                Some("SET GLOBAL".into())
            }
            "INSTALL" | "UNINSTALL" if at_start && matches!(next.as_str(), "PLUGIN" | "COMPONENT") => {
                Some(format!("{u} {next}"))
            }
            "PURGE" | "RESET" | "FLUSH" if at_start => Some(format!("{u} {next}").trim().to_string()),
            "OPTIMIZE" | "REPAIR" if at_start && matches!(next.as_str(), "TABLE" | "NO_WRITE_TO_BINLOG" | "LOCAL") => {
                Some(format!("{u} TABLE"))
            }
            _ => None,
        },
    }
}

/// The words naming what a statement changes, e.g. `FROM dbo.Orders` or
/// `TABLE [Sales].[Archive]`: leading keywords plus one qualified name.
fn statement_target(rest: &[Token<'_>]) -> String {
    const LEAD: &[&str] = &[
        "FROM",
        "INTO",
        "TOP",
        "TABLE",
        "VIEW",
        "PROCEDURE",
        "PROC",
        "FUNCTION",
        "TRIGGER",
        "INDEX",
        "DATABASE",
        "SCHEMA",
        "LOGIN",
        "USER",
        "ROLE",
        "SEQUENCE",
        "TYPE",
        "SYNONYM",
        "OR",
        "UNIQUE",
        "CLUSTERED",
        "NONCLUSTERED",
        "IF",
        "EXISTS",
        "NOT",
        "TEMPORARY",
        "TEMP",
    ];
    let mut words: Vec<String> = Vec::new();
    let mut name = String::new();
    for x in rest {
        if x.kind == TokenKind::Dot && !name.is_empty() {
            name.push('.');
        } else if x.is_name() && (name.is_empty() || name.ends_with('.')) {
            if name.is_empty() && x.kind == TokenKind::Word && LEAD.contains(&x.upper().as_str()) {
                words.push(x.upper());
            } else {
                name.push_str(x.text);
            }
        } else {
            break;
        }
    }
    words.push(name);
    words.join(" ").trim().to_string()
}

/// The table an UPDATE or DELETE changes, from the tokens after the keyword:
/// `dbo.Orders` of `TOP (10) FROM dbo.Orders`; empty when none is named.
fn changed_table(rest: &[Token<'_>]) -> String {
    let mut k = 0;
    while let Some(x) = rest.get(k) {
        match word(Some(x)).as_str() {
            "FROM" | "ONLY" => k += 1,
            "TOP" if rest.get(k + 1).is_some_and(|n| n.kind == TokenKind::LParen) => {
                k = matching_paren(rest, k + 1).map_or(rest.len(), |close| close + 1);
            }
            _ => break,
        }
    }
    let mut name = String::new();
    for x in &rest[k.min(rest.len())..] {
        if x.kind == TokenKind::Dot && !name.is_empty() && !name.ends_with('.') {
            name.push('.');
        } else if x.is_name() && (name.is_empty() || name.ends_with('.')) {
            name.push_str(x.text);
        } else {
            break;
        }
    }
    name
}

/// Whether the statement these tokens belong to is an ALTER TABLE.
fn in_alter_table(before: &[Token<'_>]) -> bool {
    let start = before
        .iter()
        .rposition(|x| x.kind == TokenKind::Semicolon)
        .map_or(0, |p| p + 1);
    let stmt = &before[start..];
    stmt.windows(2)
        .any(|w| word(Some(&w[0])) == "ALTER" && word(Some(&w[1])) == "TABLE")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warnings(sql: &str, d: Dialect) -> Vec<String> {
        ScriptRun::new(sql, d).warnings()
    }

    fn changes(sql: &str, d: Dialect) -> Vec<String> {
        ScriptRun::new(sql, d).changes()
    }

    fn calls(sql: &str, d: Dialect) -> Vec<String> {
        ScriptRun::new(sql, d).calls()
    }

    fn rows_only(sql: &str, d: Dialect) -> bool {
        ScriptRun::new(sql, d).row_changes_only()
    }

    #[test]
    fn warnings_to_confirm() {
        assert_eq!(warnings("delete from dbo.Orders", Dialect::MsSql).len(), 1);
        assert!(warnings("delete from dbo.Orders where Id = 1", Dialect::MsSql).is_empty());
        // trigger events and joined updates are not unfiltered changes
        for ok in [
            "CREATE TRIGGER t_touch BEFORE UPDATE ON orders FOR EACH ROW EXECUTE FUNCTION f()",
            "CREATE TRIGGER t AFTER INSERT OR UPDATE OR DELETE ON orders FOR EACH ROW EXECUTE FUNCTION f()",
            "UPDATE o SET Changed = 1 FROM dbo.Orders o JOIN inserted i ON i.Id = o.Id",
            "UPDATE o SET Changed = 1 FROM dbo.Orders o INNER JOIN inserted i ON i.Id = o.Id",
            "UPDATE orders o JOIN customers c ON c.id = o.customer_id SET o.status = 'late'",
            "DELETE o FROM dbo.Orders o JOIN deleted d ON d.Id = o.Id",
        ] {
            assert!(warnings(ok, Dialect::MsSql).is_empty(), "{ok}");
        }
        // ... but an outer join keeps every row of the table
        assert_eq!(
            warnings(
                "UPDATE o SET x = 1 FROM dbo.Orders o LEFT JOIN c ON c.Id = o.CId",
                Dialect::MsSql
            )
            .len(),
            1
        );
        assert_eq!(warnings("TRUNCATE TABLE t; DROP TABLE t", Dialect::MsSql).len(), 2);
        // PostgreSQL / MySQL allow TRUNCATE without TABLE; MySQL's TRUNCATE() is a function
        assert_eq!(warnings("TRUNCATE orders", Dialect::Postgres).len(), 1);
        assert_eq!(warnings("truncate orders;", Dialect::MySql).len(), 1);
        assert!(warnings("SELECT TRUNCATE(total, 0) FROM orders", Dialect::MySql).is_empty());
        // dropping a column loses its data
        let w = warnings("ALTER TABLE dbo.Orders DROP COLUMN Total", Dialect::MsSql);
        assert_eq!(w, vec!["DROP COLUMN permanently removes the column and its data."]);
        assert_eq!(warnings("ALTER TABLE orders DROP total", Dialect::MySql).len(), 1);
        assert_eq!(
            warnings("ALTER TABLE orders ADD note text, DROP total", Dialect::MySql).len(),
            1
        );
        assert!(warnings("ALTER TABLE orders DROP CONSTRAINT fk", Dialect::MsSql).is_empty());
        assert!(warnings("ALTER TABLE orders DROP INDEX ix", Dialect::MySql).is_empty());
        // statements that reach other sessions or the server
        let ms = |s: &str| warnings(s, Dialect::MsSql);
        assert_eq!(
            ms("ALTER DATABASE Shop SET SINGLE_USER WITH ROLLBACK IMMEDIATE;").len(),
            1
        );
        assert_eq!(ms("ALTER DATABASE Shop SET OFFLINE").len(), 1);
        assert_eq!(ms("KILL 53").len(), 1);
        assert_eq!(warnings("KILL QUERY 12", Dialect::MySql).len(), 1);
        assert_eq!(
            ms("SELECT 1;
SHUTDOWN WITH NOWAIT")
            .len(),
            1
        );
        assert!(ms("ALTER DATABASE Shop SET MULTI_USER").is_empty());
        assert!(ms("SELECT kill, shutdown FROM t").is_empty());
        assert!(ms("ROLLBACK TRANSACTION").is_empty());
        assert!(warnings("SELECT 1 AS shutdown", Dialect::Postgres).is_empty());
    }

    #[test]
    fn procedure_calls() {
        let c = |s: &str, d: Dialect| calls(s, d);
        assert_eq!(
            c("EXEC dbo.ArchiveOrders @Days = 30", Dialect::MsSql),
            vec!["EXEC dbo.ArchiveOrders"]
        );
        assert_eq!(
            c("execute sp_rename 'a', 'b'", Dialect::MsSql),
            vec!["EXECUTE sp_rename"]
        );
        assert_eq!(
            c("CALL archive_orders(30)", Dialect::MySql),
            vec!["CALL archive_orders"]
        );
        assert!(c("GRANT EXECUTE ON dbo.p TO u", Dialect::MsSql).is_empty());
        assert!(c("EXECUTE AS USER = 'u'; SELECT 1", Dialect::MsSql).is_empty());
        assert!(c("SELECT 1 -- exec dbo.p", Dialect::MsSql).is_empty());
        assert!(c("SELECT t.call FROM t", Dialect::Postgres).is_empty());
    }

    #[test]
    fn procedures_named_first_in_a_batch_run_without_exec() {
        let ms = Dialect::MsSql;
        let c = |s: &str| calls(s, ms);
        assert_eq!(c("dbo.ArchiveOrders @Days = 30"), vec!["EXEC dbo.ArchiveOrders"]);
        assert_eq!(
            c("/* first */ -- the call\n[dbo].[Archive] 30, 'x'"),
            vec!["EXEC [dbo].[Archive]"]
        );
        assert_eq!(c("SELECT 1\nGO 2\nxp_cmdshell 'dir'"), vec!["EXEC xp_cmdshell"]);
        assert_eq!(c("#plus_one -1; SELECT 1"), vec!["EXEC #plus_one"]);
        assert_eq!(
            c("CREATE PROCEDURE p AS SELECT 1\nGO\np"),
            vec!["EXEC p"],
            "the call after the definition"
        );
        // renaming changes the loaded objects
        assert!(ScriptRun::new("sp_rename 'dbo.A', 'B'", ms).changes_schema());
        // only first in a batch, and only a name
        for not_a_call in [
            "SELECT 1; dbo.p",
            "THROW 50000, 'x', 1",
            "DBCC CHECKIDENT ('t', RESEED, 0)",
            "CHECKPOINT",
            "lbl: SELECT 1",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "ENABLE TRIGGER ALL ON DATABASE",
            "MERGE t USING s ON 1 = 0 WHEN NOT MATCHED THEN INSERT VALUES (1);",
            "WAITFOR DELAY '00:00:01'",
            "(SELECT 1)",
        ] {
            assert!(c(not_a_call).is_empty(), "{not_a_call}: {:?}", c(not_a_call));
        }
        // other servers need CALL
        assert!(calls("archive_orders(30)", Dialect::MySql).is_empty());
        // what Sqlotus puts in front of such a batch must add an EXEC
        assert_eq!(bare_call_start("\n -- x\n dbo.p 1"), Some(8));
        assert_eq!(bare_call_start("EXEC dbo.p 1"), None);
    }

    #[test]
    fn changes_nested_in_other_statements() {
        let (ms, pg) = (Dialect::MsSql, Dialect::Postgres);
        // PostgreSQL CTEs that change rows: the outer SELECT reports no count
        let cte = "WITH gone AS (DELETE FROM orders RETURNING *) SELECT count(*) FROM gone";
        assert_eq!(changes(cte, pg), vec!["DELETE FROM orders"]);
        assert_eq!(
            warnings(cte, pg),
            vec!["DELETE orders has no WHERE clause and will affect every row."]
        );
        assert!(ScriptRun::new(cte, pg).has_nested_changes());
        assert!(rows_only(cte, pg));
        let filtered = "WITH a AS (SELECT 1), u AS MATERIALIZED (UPDATE orders SET total = 0 WHERE id = 1 RETURNING id) \
                        SELECT * FROM u";
        assert_eq!(changes(filtered, pg), vec!["UPDATE orders"]);
        assert!(warnings(filtered, pg).is_empty());
        assert_eq!(
            changes(
                "WITH i AS (INSERT INTO log VALUES (1) RETURNING id) SELECT id FROM i",
                pg
            ),
            vec!["INSERT INTO log"]
        );
        // T-SQL: an INSERT reading the OUTPUT of a DELETE
        let moved = "INSERT INTO dbo.Archive SELECT * FROM (DELETE FROM dbo.Orders OUTPUT deleted.*) AS d;";
        assert_eq!(
            changes(moved, ms),
            vec!["INSERT INTO dbo.Archive", "DELETE FROM dbo.Orders"]
        );
        // statements in parentheses that are no changes of their own
        for plain in [
            "UPDATE orders SET total = (SELECT 1) WHERE id = 1",
            "CREATE VIEW v AS (SELECT 1)",
            "SELECT * FROM (SELECT 1) AS x",
        ] {
            assert!(!ScriptRun::new(plain, pg).has_nested_changes(), "{plain}");
        }
        assert!(!ScriptRun::new("DELETE FROM orders WHERE id = 1", pg).has_nested_changes());
    }

    #[test]
    fn warnings_name_the_table() {
        let ms = Dialect::MsSql;
        let w = |s: &str| warnings(s, ms);
        assert_eq!(
            w("UPDATE dbo.t SET a = 1"),
            vec!["UPDATE dbo.t has no WHERE clause and will affect every row."]
        );
        assert_eq!(
            w("UPDATE [x].[t] SET a = 1"),
            vec!["UPDATE [x].[t] has no WHERE clause and will affect every row."]
        );
        assert_eq!(
            w("DELETE TOP (10) FROM dbo.t"),
            vec!["DELETE dbo.t has no WHERE clause and will affect every row."]
        );
        assert_eq!(
            warnings("UPDATE ONLY orders SET a = 1", Dialect::Postgres),
            vec!["UPDATE orders has no WHERE clause and will affect every row."]
        );
        // statistics are no rows; temp tables are the script's own
        assert!(w("UPDATE STATISTICS dbo.t").is_empty());
        assert!(!ScriptRun::new("UPDATE STATISTICS dbo.t", ms).fits_in_transaction());
        assert!(w("IF OBJECT_ID('tempdb..#t') IS NOT NULL DROP TABLE #t").is_empty());
        assert!(w("DROP TABLE IF EXISTS #a, #b").is_empty());
        assert_eq!(w("DROP TABLE #a, dbo.b").len(), 1);
    }

    #[test]
    fn changes_without_the_usual_keywords() {
        let m = |s: &str, d: Dialect| changes(s, d);
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        // PostgreSQL: a DO block can change anything; COPY FROM loads rows
        assert_eq!(
            m("DO $$ BEGIN UPDATE orders SET total = 0; END $$;", pg),
            vec!["DO block"]
        );
        assert_eq!(m("SELECT 1;\nDO $$ BEGIN PERFORM 1; END $$", pg), vec!["DO block"]);
        assert!(m("INSERT INTO t VALUES (1) ON CONFLICT DO NOTHING", pg).len() == 1);
        assert_eq!(m("COPY orders FROM STDIN", pg), vec!["COPY orders"]);
        assert!(m("COPY (SELECT * FROM orders) TO STDOUT", pg).is_empty());
        // ... COPY TO runs a program, or writes a file, on the server
        assert_eq!(
            m("COPY orders TO PROGRAM 'gzip > /tmp/o.gz'", pg),
            vec!["COPY TO PROGRAM"]
        );
        assert_eq!(
            m("COPY (SELECT * FROM orders) TO '/tmp/o.csv' WITH (FORMAT csv)", pg),
            vec!["COPY TO a server file"]
        );
        assert_eq!(
            m("REFRESH MATERIALIZED VIEW mv_totals", pg),
            vec!["REFRESH MATERIALIZED VIEW mv_totals"]
        );
        assert_eq!(m("VACUUM FULL orders", pg), vec!["VACUUM"]);
        assert_eq!(
            m(
                "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE usename = 'x'",
                pg
            ),
            vec!["pg_terminate_backend"]
        );
        assert_eq!(m("SELECT setval('orders_id_seq', 1)", pg), vec!["setval"]);
        assert!(m("SELECT nextval_x, setval FROM t", pg).is_empty());
        // SQL Server: restores, DBCC commands that change things, triggers switched off
        assert_eq!(
            m("RESTORE DATABASE Shop FROM DISK = 'c:\\b\\shop.bak' WITH REPLACE", ms),
            vec!["RESTORE DATABASE Shop"]
        );
        assert!(m("RESTORE FILELISTONLY FROM DISK = 'x.bak'", ms).is_empty());
        assert_eq!(m("BACKUP LOG Shop TO DISK = 'x.trn'", ms), vec!["BACKUP LOG Shop"]);
        assert_eq!(
            m("DBCC CHECKIDENT ('dbo.Orders', RESEED, 0)", ms),
            vec!["DBCC CHECKIDENT"]
        );
        assert_eq!(m("SELECT 1\nDBCC FREEPROCCACHE", ms), vec!["DBCC FREEPROCCACHE"]);
        assert!(
            m(
                "DBCC SHOW_STATISTICS ('dbo.Orders', IX_1); DBCC CHECKDB WITH NO_INFOMSGS",
                ms
            )
            .is_empty()
        );
        assert_eq!(
            m("DBCC CHECKDB (Shop, REPAIR_ALLOW_DATA_LOSS)", ms),
            vec!["DBCC CHECKDB"]
        );
        assert_eq!(m("EXEC sp_configure 'x', 1; RECONFIGURE", ms), vec!["RECONFIGURE"]);
        assert_eq!(
            m("DISABLE TRIGGER dbo.tOrders ON dbo.Orders", ms),
            vec!["DISABLE TRIGGER dbo.tOrders"]
        );
        assert!(m("SELECT enable, disable FROM t", ms).is_empty());
        // MySQL: loads, server settings, dumps' /*!50003 ... */ statements
        assert_eq!(
            m("LOAD DATA INFILE '/tmp/x.csv' INTO TABLE orders", my),
            vec!["LOAD DATA"]
        );
        assert_eq!(m("SET GLOBAL max_connections = 10", my), vec!["SET GLOBAL"]);
        assert_eq!(m("SET @@global.sql_mode = ''", my), vec!["SET GLOBAL"]);
        assert_eq!(m("SET PERSIST max_connections = 10", my), vec!["SET GLOBAL"]);
        assert!(m("SET @a = 1, @@session.sql_mode = ''; SET NAMES utf8mb4", my).is_empty());
        assert_eq!(m("SHUTDOWN", my), vec!["SHUTDOWN"]);
        assert_eq!(m("RESTART", my), vec!["RESTART"]);
        assert_eq!(m("PURGE BINARY LOGS BEFORE NOW()", my), vec!["PURGE BINARY"]);
        assert_eq!(m("FLUSH TABLES WITH READ LOCK", my), vec!["FLUSH TABLES"]);
        assert_eq!(m("INSTALL PLUGIN x SONAME 'x.so'", my), vec!["INSTALL PLUGIN"]);
        assert_eq!(m("OPTIMIZE TABLE orders", my), vec!["OPTIMIZE TABLE"]);
        let dump = m(
            "/*!50003 CREATE*/ /*!50017 DEFINER=`root`@`%`*/ /*!50003 TRIGGER t1 BEFORE INSERT ON t FOR EACH ROW SET NEW.a = 1 */",
            my,
        );
        assert!(dump.len() == 1 && dump[0].starts_with("CREATE"), "{dump:?}");
        assert_eq!(calls("/*!50003 CALL archive() */", my), vec!["CALL archive"]);
        assert!(m("/*!40101 SET NAMES utf8mb4 */; /*+ BKA(t) */ SELECT 1", my).is_empty());
    }

    #[test]
    fn session_objects_are_not_changes() {
        let m = |s: &str, d: Dialect| changes(s, d);
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        // temp tables and table variables belong to the session
        for s in [
            "SELECT a INTO #x FROM t",
            "CREATE TABLE #work (Id int); INSERT INTO #work (Id) SELECT Id FROM dbo.Orders",
            "DECLARE @t table (Id int); INSERT @t (Id) VALUES (1); UPDATE @t SET Id = 2; DELETE FROM @t",
            "DROP TABLE IF EXISTS #work, ##shared",
            "TRUNCATE TABLE #work; ALTER TABLE #work ADD b int; MERGE #work AS w USING t ON 1 = 0 WHEN MATCHED THEN DELETE;",
        ] {
            assert!(m(s, ms).is_empty(), "{s}: {:?}", m(s, ms));
        }
        // ... but not when a real table is named as well
        assert_eq!(m("DROP TABLE #work, dbo.Orders", ms).len(), 1);
        assert_eq!(
            m("SELECT * INTO #t FROM a; DELETE FROM dbo.Orders WHERE 1 = 0", ms),
            vec!["DELETE FROM dbo.Orders"]
        );
        // OUTPUT ... INTO a table variable inside a real change still counts once
        assert_eq!(
            m(
                "DELETE o OUTPUT deleted.Id INTO @gone FROM dbo.Orders o WHERE NOT EXISTS (SELECT 1 FROM c)",
                ms
            ),
            vec!["DELETE o"]
        );
        assert!(m("CREATE TEMP TABLE w AS SELECT 1; SELECT * INTO TEMP x FROM t", pg).is_empty());
        assert_eq!(m("SELECT * INTO archive FROM t", pg), vec!["SELECT INTO"]);
        assert!(m("CREATE TEMPORARY TABLE w (a int); DROP TEMPORARY TABLE w", my).is_empty());
        // MySQL SELECT ... INTO names variables (or writes a file)
        assert!(m("SELECT id INTO @last FROM orders ORDER BY id DESC LIMIT 1", my).is_empty());
        assert_eq!(
            m("SELECT * INTO OUTFILE '/tmp/o.csv' FROM orders", my),
            vec!["SELECT INTO OUTFILE"]
        );
    }

    #[test]
    fn read_only_classification() {
        let m = |s: &str| changes(s, Dialect::MsSql);
        assert!(m("SELECT * FROM t WHERE x IN (SELECT y FROM u)").is_empty());
        assert!(m("SELECT REPLACE(a, 'x', 'y'), LEFT(b, 1) FROM t").is_empty());
        assert_eq!(m("UPDATE dbo.t SET a = 1"), vec!["UPDATE dbo.t"]);
        assert_eq!(
            m("DELETE FROM [Sales].Orders WHERE 1 = 0"),
            vec!["DELETE FROM [Sales].Orders"]
        );
        assert_eq!(m("DROP TABLE IF EXISTS dbo.Old"), vec!["DROP TABLE IF EXISTS dbo.Old"]);
        assert_eq!(m("DELETE FROM t; INSERT INTO t VALUES (1)").len(), 2);
        assert_eq!(m("SELECT a INTO dbo.x FROM t"), vec!["SELECT INTO"]);
        assert_eq!(m("CREATE TABLE t (a int REFERENCES u(a) ON DELETE CASCADE)").len(), 1);
        assert!(changes("SELECT 1; -- delete from t", Dialect::MsSql).is_empty());
        assert_eq!(m("KILL 53"), vec!["KILL"]);
        assert_eq!(m("SELECT 1; SHUTDOWN"), vec!["SHUTDOWN"]);
        assert!(m("SELECT kill, shutdown FROM t").is_empty());
        let ms = Dialect::MsSql;
        assert!(rows_only(
            "UPDATE t SET a = 1 WHERE b = 2; DELETE FROM u WHERE 1 = 0",
            ms
        ));
        assert!(!rows_only("BEGIN TRAN; UPDATE t SET a = 1", ms));
        assert!(!rows_only("UPDATE t SET a = 1; DROP TABLE u", ms));
        assert!(!rows_only("SELECT 1", ms));
        assert!(rows_only("IF 1 = 1 BEGIN UPDATE t SET a = 1 END", ms));
        assert!(!rows_only("BEGIN; UPDATE t SET a = 1; COMMIT;", Dialect::Postgres));
    }

    #[test]
    fn transactions_sqlotus_may_wrap() {
        let fits = |s: &str, d: Dialect| ScriptRun::new(s, d).fits_in_transaction();
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        // procedure calls and plain reads fit; the procedures' own work is theirs
        assert!(fits("EXEC dbo.ArchiveOrders @Days = 30", ms));
        assert!(fits("UPDATE t SET a = 1; EXEC dbo.Recalc", ms));
        assert!(fits("CALL archive_orders(30)", my));
        assert!(fits("SELECT 1", pg));
        // DDL is committed implicitly by some servers; own transactions stay theirs
        assert!(!fits("EXEC dbo.p; DROP TABLE dbo.Old", ms));
        assert!(!fits("BEGIN TRAN; EXEC dbo.p; COMMIT", ms));
        assert!(!fits("CREATE TABLE t (a int); CALL p()", my));
        assert!(!fits("CALL p(); COMMIT;", pg));
    }

    #[test]
    fn postgres_procedures_that_commit_by_themselves() {
        let def = |name: &str, body: &str| ObjectDefinition {
            schema: "public".into(),
            name: name.into(),
            kind: ObjectKind::Procedure,
            definition: format!(
                "CREATE OR REPLACE PROCEDURE public.{name}()\n LANGUAGE plpgsql\nAS $procedure${body}$procedure$\n"
            ),
        };
        let defs = vec![
            def("batch_load", "BEGIN INSERT INTO t VALUES (1); COMMIT; END"),
            def(
                "plain",
                "BEGIN INSERT INTO t VALUES (1); -- commit later\n RAISE NOTICE 'commit'; END",
            ),
            def(
                "undo",
                "BEGIN UPDATE t SET a = 1; EXCEPTION WHEN others THEN ROLLBACK; END",
            ),
        ];
        let c = |s: &str| ScriptRun::new(s, Dialect::Postgres).commits_by_itself(&defs);
        assert_eq!(c("CALL batch_load()"), vec!["public.batch_load"]);
        assert_eq!(c("call public.BATCH_LOAD(); CALL plain();"), vec!["public.batch_load"]);
        assert!(c("CALL plain()").is_empty());
        assert_eq!(c("CALL undo()"), vec!["public.undo"]);
        // the same name in another schema is another procedure
        assert!(c("CALL other.batch_load()").is_empty());
        assert_eq!(c("DO $$ BEGIN UPDATE t SET a = 1; COMMIT; END $$;"), vec!["a DO block"]);
        assert_eq!(c("DO LANGUAGE plpgsql 'BEGIN ROLLBACK; END'"), vec!["a DO block"]);
        assert!(c("DO $$ BEGIN RAISE NOTICE 'commit'; END $$").is_empty());
        // a procedure the script creates is stored, not run
        assert!(c("CREATE PROCEDURE x() LANGUAGE plpgsql AS $$ BEGIN COMMIT; END $$").is_empty());
        // other servers nest a procedure's transaction in the caller's
        assert!(
            ScriptRun::new("CALL batch_load()", Dialect::MySql)
                .commits_by_itself(&defs)
                .is_empty()
        );
    }

    #[test]
    fn statements_that_cannot_run_in_a_transaction() {
        let refuses = |s: &str, d: Dialect| ScriptRun::new(s, d).refuses_transaction();
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        assert_eq!(
            refuses(
                "BACKUP DATABASE [Shop] TO DISK = N'x.bak'\nGO\nRESTORE VERIFYONLY FROM DISK = N'x.bak'",
                ms
            ),
            vec!["BACKUP DATABASE [Shop]"]
        );
        assert_eq!(
            refuses(
                "USE master;\nRESTORE DATABASE Shop FROM DISK = N'x.bak' WITH REPLACE",
                ms
            ),
            vec!["RESTORE DATABASE Shop"]
        );
        assert_eq!(refuses("ALTER DATABASE Shop SET RECOVERY FULL", ms).len(), 1);
        assert_eq!(refuses("CREATE DATABASE Scratch", pg).len(), 1);
        assert_eq!(refuses("VACUUM orders", pg), vec!["VACUUM"]);
        assert_eq!(
            refuses("CLUSTER; REINDEX DATABASE shop", pg),
            vec!["CLUSTER", "REINDEX"]
        );
        assert_eq!(
            refuses("ALTER FULLTEXT INDEX ON dbo.Docs START FULL POPULATION", ms).len(),
            1
        );
        assert_eq!(refuses("CREATE INDEX CONCURRENTLY ix ON orders (total)", pg).len(), 1);
        // ordinary changes run in a transaction; MySQL commits DDL itself
        assert!(refuses("UPDATE t SET a = 1; CREATE TABLE u (a int)", ms).is_empty());
        assert!(refuses("CREATE DATABASE scratch", my).is_empty());
        assert!(refuses("CREATE INDEX ix ON orders (total)", pg).is_empty());
    }

    #[test]
    fn schema_changes_make_the_loaded_objects_stale() {
        let schema = |s: &str, d: Dialect| ScriptRun::new(s, d).changes_schema();
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        for s in [
            "CREATE TABLE dbo.Audit (Id int)",
            "ALTER TABLE dbo.Orders ADD Note nvarchar(100)",
            "DROP VIEW IF EXISTS dbo.vTotals",
            "SELECT * INTO dbo.Archive FROM dbo.Orders",
            "EXEC sp_rename 'dbo.Orders.Note', 'Remark', 'COLUMN'",
            "EXECUTE sys.sp_rename N'dbo.A', N'B'",
            "CREATE OR ALTER PROCEDURE dbo.p AS SELECT 1",
            "SELECT 1\nGO\nCREATE INDEX IX_Orders_Total ON dbo.Orders (Total)",
        ] {
            assert!(schema(s, ms), "{s}");
        }
        assert!(schema("ALTER TABLE orders RENAME COLUMN note TO remark", pg));
        assert!(schema("CREATE OR REPLACE VIEW v AS SELECT 1", pg));
        assert!(schema("RENAME TABLE orders TO orders_old", my));
        for s in [
            "SELECT * FROM dbo.Orders",
            "UPDATE dbo.Orders SET Total = 0 WHERE Id = 1",
            "CREATE TABLE #work (Id int); DROP TABLE #work",
            "SELECT * INTO #copy FROM dbo.Orders",
            "EXEC dbo.Recalc",
            "TRUNCATE TABLE dbo.Staging",
            // stored in the body, not run
            "CREATE PROCEDURE dbo.p AS\nBEGIN\n  DROP TABLE dbo.Old;\nEND",
        ] {
            assert_eq!(schema(s, ms), s.starts_with("CREATE PROCEDURE"), "{s}");
        }
        assert!(!schema("SELECT * INTO OUTFILE '/tmp/o.csv' FROM orders", my));
        assert!(!schema("CREATE TEMPORARY TABLE w (a int)", my));
    }

    #[test]
    fn definition_bodies_are_stored_not_run() {
        let (ms, pg, my) = (Dialect::MsSql, Dialect::Postgres, Dialect::MySql);
        // T-SQL: the rest of the batch is the body
        let proc = "CREATE PROCEDURE dbo.ClearStaging AS\nBEGIN\n  DELETE FROM dbo.Staging;\n  TRUNCATE TABLE dbo.Log;\n  \
                    DROP TABLE dbo.Old;\n  EXEC dbo.Other;\n  COMMIT;\nEND";
        assert!(warnings(proc, ms).is_empty(), "{:?}", warnings(proc, ms));
        assert_eq!(changes(proc, ms), vec!["CREATE PROCEDURE dbo.ClearStaging"]);
        assert!(calls(proc, ms).is_empty());
        assert!(!ScriptRun::new(proc, ms).controls_transactions());
        // ... and the next batch runs
        let script = format!("{proc}\nGO\nDELETE FROM dbo.Staging");
        assert_eq!(warnings(&script, ms).len(), 1);
        assert_eq!(
            changes(&script, ms),
            vec!["CREATE PROCEDURE dbo.ClearStaging", "DELETE FROM dbo.Staging"]
        );
        assert!(
            warnings(
                "CREATE TRIGGER trg ON dbo.t AFTER DELETE AS UPDATE dbo.Counts SET n = n - 1",
                ms
            )
            .is_empty()
        );
        // switching a trigger off defines nothing
        assert_eq!(
            warnings("ALTER TABLE dbo.t DISABLE TRIGGER ALL; DELETE FROM dbo.t", ms).len(),
            1
        );
        // MySQL: the BEGIN ... END body (END IF / END CASE inside), with or without DELIMITER
        let body = "CREATE PROCEDURE clear_staging() BEGIN IF 1 THEN DELETE FROM staging; END IF; \
                    CASE WHEN 1 THEN TRUNCATE log; END CASE; END;\nDELETE FROM orders;";
        assert_eq!(
            warnings(body, my),
            vec!["DELETE orders has no WHERE clause and will affect every row."]
        );
        let delimited = "DELIMITER //\nCREATE TRIGGER t BEFORE DELETE ON orders FOR EACH ROW BEGIN DELETE FROM audit; END //\n\
                         DELIMITER ;\nTRUNCATE orders;";
        assert_eq!(warnings(delimited, my), vec!["TRUNCATE TABLE removes every row."]);
        assert!(warnings("CREATE EVENT purge ON SCHEDULE EVERY 1 DAY DO DELETE FROM log;", my).is_empty());
        let dump = "/*!50003 CREATE*/ /*!50017 DEFINER=`root`@`%`*/ /*!50003 TRIGGER t BEFORE INSERT ON x FOR EACH ROW DELETE FROM y */";
        assert!(warnings(dump, my).is_empty());
        // PostgreSQL: BEGIN ATOMIC bodies
        let atomic =
            "CREATE PROCEDURE clear() LANGUAGE sql BEGIN ATOMIC DELETE FROM staging; END;\nDELETE FROM orders;";
        assert_eq!(warnings(atomic, pg).len(), 1);
        assert_eq!(
            changes(atomic, pg),
            vec!["CREATE PROCEDURE clear", "DELETE FROM orders"]
        );
    }
}
