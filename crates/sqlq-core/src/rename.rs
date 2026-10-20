//! Rename a variable, table alias or CTE everywhere it is used.
//!
//! Variables are scoped to their batch (`GO` separated), aliases and CTEs to
//! their statement.

use serde::Serialize;

use crate::Dialect;
use crate::lexer::{Token, TokenKind, tokenize};
use crate::scope::analyze_statement;
use crate::statements::{split_batches, statement_at};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameTarget {
    /// What is being renamed: `variable`, `alias` or `cte`.
    pub kind: String,
    pub name: String,
    /// Byte ranges of every occurrence.
    pub ranges: Vec<(usize, usize)>,
}

fn sig<'a>(tokens: &[Token<'a>]) -> Vec<Token<'a>> {
    tokens.iter().filter(|t| !t.is_trivia()).copied().collect()
}

/// Occurrences of the renameable symbol under `offset`, if any.
pub fn rename_target(src: &str, offset: usize, dialect: Dialect) -> Option<RenameTarget> {
    let tokens = tokenize(src, dialect);
    let tok = *tokens.iter().find(|t| {
        t.start <= offset && offset <= t.end() && (t.is_name() || t.kind == TokenKind::Variable)
    })?;

    if tok.kind == TokenKind::Variable {
        if tok.text.starts_with("@@") {
            return None;
        }
        let batch = split_batches(src, dialect)
            .into_iter()
            .find(|b| b.span.start <= tok.start && tok.end() <= b.span.end)?;
        let ranges = tokens
            .iter()
            .filter(|t| t.kind == TokenKind::Variable && t.text.eq_ignore_ascii_case(tok.text))
            .filter(|t| t.start >= batch.span.start && t.end() <= batch.span.end)
            .map(|t| (t.start, t.end()))
            .collect();
        return Some(RenameTarget {
            kind: "variable".into(),
            name: tok.text.to_string(),
            ranges,
        });
    }

    let span = statement_at(src, dialect, offset)?;
    let stmt: Vec<Token<'_>> = tokens
        .iter()
        .filter(|t| t.start >= span.start && t.end() <= span.end)
        .copied()
        .collect();
    let scope = analyze_statement(&stmt, dialect);
    let t = sig(&stmt);
    let name = tok.ident_value();
    let same = |x: &Token<'_>| x.is_name() && x.ident_value().eq_ignore_ascii_case(&name);
    let idx = t.iter().position(|x| x.start == tok.start)?;
    // a name after a dot is a column / object part, never an alias
    if idx > 0 && t[idx - 1].kind == TokenKind::Dot {
        return None;
    }

    if scope.tables.iter().any(|r| {
        r.alias
            .as_ref()
            .is_some_and(|a| a.eq_ignore_ascii_case(&name))
    }) {
        let ranges = t
            .iter()
            .enumerate()
            .filter(|(i, x)| {
                same(x)
                    && !(*i > 0 && t[i - 1].kind == TokenKind::Dot)
                    && (t.get(i + 1).is_some_and(|n| n.kind == TokenKind::Dot)
                        || scope
                            .tables
                            .iter()
                            .any(|r| r.alias.is_some() && alias_token(&t, r.name_end, *i))
                        || (*i > 0 && matches!(t[i - 1].upper().as_str(), "UPDATE" | "DELETE")))
            })
            .map(|(_, x)| (x.start, x.end()))
            .collect();
        return Some(RenameTarget {
            kind: "alias".into(),
            name,
            ranges,
        });
    }

    if scope
        .ctes
        .iter()
        .any(|c| c.name.eq_ignore_ascii_case(&name))
    {
        let ranges = t
            .iter()
            .enumerate()
            .filter(|(i, x)| same(x) && !(*i > 0 && t[i - 1].kind == TokenKind::Dot))
            .map(|(_, x)| (x.start, x.end()))
            .collect();
        return Some(RenameTarget {
            kind: "cte".into(),
            name,
            ranges,
        });
    }
    None
}

/// Whether token `i` is the alias written right after a table name ending
/// at byte `name_end` (`dbo.Orders o` / `dbo.Orders AS o` / `(...) x`).
fn alias_token(t: &[Token<'_>], name_end: usize, i: usize) -> bool {
    let Some(prev) = i.checked_sub(1).map(|p| &t[p]) else {
        return false;
    };
    if prev.end() == name_end || prev.kind == TokenKind::RParen {
        return true;
    }
    prev.upper() == "AS"
        && i >= 2
        && (t[i - 2].end() == name_end || t[i - 2].kind == TokenKind::RParen)
}

/// Apply a rename to `src`; returns `None` when nothing is renameable.
pub fn rename(src: &str, offset: usize, dialect: Dialect, new_name: &str) -> Option<String> {
    let target = rename_target(src, offset, dialect)?;
    let mut out = src.to_string();
    let mut ranges = target.ranges;
    ranges.sort_by_key(|r| std::cmp::Reverse(r.0));
    for (s, e) in ranges {
        out.replace_range(s..e, new_name);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(src: &str, needle: &str) -> usize {
        src.find(needle).expect("needle") + 1
    }

    #[test]
    fn variables_within_batch() {
        let src = "DECLARE @id int = 1; SELECT @id, @ID2 FROM t WHERE x = @Id\nGO\nSELECT @id";
        let out = rename(src, at(src, "@id"), Dialect::MsSql, "@customerId").unwrap();
        assert_eq!(
            out,
            "DECLARE @customerId int = 1; SELECT @customerId, @ID2 FROM t WHERE x = @customerId\nGO\nSELECT @id"
        );
    }

    #[test]
    fn aliases_within_statement() {
        let src = "SELECT o.Id, o.Total FROM dbo.Orders o JOIN dbo.Items i ON i.OrderId = o.Id WHERE o.Total > 1; SELECT o.x FROM t o";
        let out = rename(src, at(src, "o.Total"), Dialect::MsSql, "ord").unwrap();
        assert_eq!(
            out,
            "SELECT ord.Id, ord.Total FROM dbo.Orders ord JOIN dbo.Items i ON i.OrderId = ord.Id WHERE ord.Total > 1; SELECT o.x FROM t o"
        );
        let src = "UPDATE o SET Total = 0 FROM dbo.Orders AS o WHERE o.Id = 1";
        assert_eq!(
            rename(src, 8, Dialect::MsSql, "x").unwrap(),
            "UPDATE x SET Total = 0 FROM dbo.Orders AS x WHERE x.Id = 1"
        );
    }

    #[test]
    fn ctes_and_non_renameables() {
        let src = "WITH c AS (SELECT 1 AS a) SELECT c.a FROM c";
        assert_eq!(
            rename(src, 6, Dialect::MsSql, "totals").unwrap(),
            "WITH totals AS (SELECT 1 AS a) SELECT totals.a FROM totals"
        );
        // a column after a dot and a plain table name are not renameable here
        assert!(rename_target("SELECT o.Id FROM dbo.Orders o", 10, Dialect::MsSql).is_none());
        assert!(rename_target("SELECT Id FROM Orders", 17, Dialect::MsSql).is_none());
        assert!(rename_target("SELECT @@ROWCOUNT", 9, Dialect::MsSql).is_none());
    }
}
