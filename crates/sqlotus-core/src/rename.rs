//! Rename a variable, table alias or CTE everywhere it is used.
//!
//! Variables are scoped to their batch (`GO` separated), aliases and CTEs to
//! their statement.

use serde::Serialize;

use crate::Dialect;
use crate::lexer::{Token, TokenKind, significant, tokenize};
use crate::scope::analyze_statement;
use crate::statements::{split_batches, statement_at};
use crate::text::apply_edits;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameTarget {
    /// What is being renamed: `variable`, `alias` or `cte`.
    pub kind: String,
    pub name: String,
    /// Byte ranges of every occurrence.
    pub ranges: Vec<(usize, usize)>,
}

/// Occurrences of the renameable symbol under `offset`, if any.
pub fn rename_target(src: &str, offset: usize, dialect: Dialect) -> Option<RenameTarget> {
    let tokens = tokenize(src, dialect);
    let tok = *tokens
        .iter()
        .find(|t| t.start <= offset && offset <= t.end() && (t.is_name() || t.kind == TokenKind::Variable))?;

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
    let t = significant(&stmt);
    let name = tok.ident_value();
    let same = |x: &Token<'_>| x.is_name() && x.ident_value().eq_ignore_ascii_case(&name);
    let idx = t.iter().position(|x| x.start == tok.start)?;
    // a name after a dot is a column / object part, never an alias
    if idx > 0 && t[idx - 1].kind == TokenKind::Dot {
        return None;
    }

    let after_dot = |i: usize| i > 0 && t[i - 1].kind == TokenKind::Dot;
    let before_dot = |i: usize| t.get(i + 1).is_some_and(|n| n.kind == TokenKind::Dot);
    if let Some(table) = scope
        .tables
        .iter()
        .find(|r| r.alias.as_ref().is_some_and(|a| a.eq_ignore_ascii_case(&name)))
    {
        let ranges: Vec<(usize, usize)> = t
            .iter()
            .enumerate()
            .filter(|(i, x)| {
                same(x)
                    && !after_dot(*i)
                    && (before_dot(*i)
                        || scope
                            .tables
                            .iter()
                            .any(|r| r.alias.is_some() && alias_token(&t, r.name_end, *i))
                        || (*i > 0 && matches!(t[i - 1].upper().as_str(), "UPDATE" | "DELETE")))
            })
            .map(|(_, x)| (x.start, x.end()))
            .collect();
        // a bare name under the cursor is a column, not the alias
        if !ranges.iter().any(|&(s, _)| s == tok.start) {
            return None;
        }
        // `generate_series(1, 9) AS g`: a bare g is the function's column,
        // which the alias names too, so renaming one would break the other
        let bare = t
            .iter()
            .enumerate()
            .any(|(i, x)| same(x) && !after_dot(i) && !ranges.contains(&(x.start, x.end())));
        if table.is_call && bare {
            return None;
        }
        return Some(RenameTarget {
            kind: "alias".into(),
            name,
            ranges,
        });
    }

    if scope.ctes.iter().any(|c| c.name.eq_ignore_ascii_case(&name)) {
        // the CTE list: from WITH to the statement it serves, outside parentheses
        let with = t.iter().position(|x| x.upper() == "WITH");
        let mut depth = 0i32;
        let mut body = t.len();
        for (i, x) in t.iter().enumerate().skip(with.map_or(t.len(), |w| w + 1)) {
            match x.kind {
                TokenKind::LParen => depth += 1,
                TokenKind::RParen => depth -= 1,
                _ if depth == 0
                    && matches!(x.upper().as_str(), "SELECT" | "INSERT" | "UPDATE" | "DELETE" | "MERGE") =>
                {
                    body = i;
                    break;
                }
                _ => {}
            }
        }
        let definition = |i: usize| {
            with.is_some_and(|w| w < i && i < body)
                && i > 0
                && (matches!(t[i - 1].upper().as_str(), "WITH" | "RECURSIVE") || t[i - 1].kind == TokenKind::Comma)
                && t.get(i + 1)
                    .is_some_and(|n| n.upper() == "AS" || n.kind == TokenKind::LParen)
        };
        // the definition, table references and `cte.column` qualifiers; a
        // column or column alias of the same name stays as it is
        let ranges: Vec<(usize, usize)> = t
            .iter()
            .enumerate()
            .filter(|(i, x)| {
                same(x)
                    && !after_dot(*i)
                    && (definition(*i)
                        || before_dot(*i)
                        || scope
                            .tables
                            .iter()
                            .any(|r| r.schema.is_none() && r.name_start == x.start))
            })
            .map(|(_, x)| (x.start, x.end()))
            .collect();
        if !ranges.iter().any(|&(s, _)| s == tok.start) {
            return None;
        }
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
    prev.upper() == "AS" && i >= 2 && (t[i - 2].end() == name_end || t[i - 2].kind == TokenKind::RParen)
}

/// Apply a rename to `src`; returns `None` when nothing is renameable.
pub fn rename(src: &str, offset: usize, dialect: Dialect, new_name: &str) -> Option<String> {
    let target = rename_target(src, offset, dialect)?;
    Some(apply_edits(src, target.ranges.into_iter().map(|(s, e)| (s, e, new_name))).0)
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

    #[test]
    fn columns_named_like_a_cte_or_alias_stay() {
        // a column and a column alias named like the CTE are not the CTE
        let src = "WITH total AS (SELECT sum(amount) AS total FROM t) SELECT total FROM total";
        assert_eq!(
            rename(src, src.rfind("total").unwrap(), Dialect::Postgres, "sums").unwrap(),
            "WITH sums AS (SELECT sum(amount) AS total FROM t) SELECT total FROM sums"
        );
        assert!(rename_target(src, src.find("SELECT total").unwrap() + 8, Dialect::Postgres).is_none());
        // a recursive CTE is renamed inside its own body too
        let src = "WITH RECURSIVE tree AS (SELECT id FROM nodes UNION ALL SELECT n.id FROM nodes n JOIN tree t ON t.id = n.parent) SELECT tree.id FROM tree";
        assert_eq!(
            rename(src, src.rfind("tree").unwrap(), Dialect::Postgres, "walk").unwrap(),
            "WITH RECURSIVE walk AS (SELECT id FROM nodes UNION ALL SELECT n.id FROM nodes n JOIN walk t ON t.id = n.parent) SELECT walk.id FROM walk"
        );
        // a bare column named like a table alias is the column
        let src = "SELECT c, c.Id FROM t c";
        assert!(rename_target(src, 7, Dialect::MsSql).is_none());
        assert_eq!(rename(src, 10, Dialect::MsSql, "x").unwrap(), "SELECT c, x.Id FROM t x");
        // generate_series(...) AS g: g is also the function's column
        let src = "SELECT g FROM generate_series(1, 3) AS g WHERE g > 1";
        assert!(rename_target(src, src.find("AS g").unwrap() + 3, Dialect::Postgres).is_none());
    }
}
