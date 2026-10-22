//! Rules, suppression comments and quick fixes on top of
//! [`diagnostics`]: what the editor's Problems list shows and what the
//! `sqlotus-lint` command reports.
//!
//! Suppression comments (line or block comments, codes optional):
//!
//! ```sql
//! SELECT * FROM t  -- sqlotus-disable-line SQ002
//! -- sqlotus-disable-next-line SQ001
//! DELETE FROM staging
//! -- sqlotus-disable SQ005 SQ008
//! ...                              -- until sqlotus-enable or the end
//! -- sqlotus-enable
//! ```
//!
//! [`ignore_edits`] writes such a comment for one problem ("Ignore on this
//! line" in the editor).

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::analysis::{Diagnostic, Fix, Severity, diagnostics};
use crate::catalog::Catalog;
use crate::lexer::{Token, TokenKind, tokenize};

const DISABLE_LINE: &str = "sqlotus-disable-line";
const DISABLE_NEXT_LINE: &str = "sqlotus-disable-next-line";

/// One analysis rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub code: &'static str,
    pub title: &'static str,
    /// Usual severity (a rule may report a milder case as Info).
    pub severity: Severity,
    /// Needs the database's objects: the editor's connection, or a schema
    /// folder for `sqlotus-lint --schema`.
    pub needs_catalog: bool,
    /// Only checked for this kind of server.
    pub only: Option<Dialect>,
}

const fn rule(code: &'static str, title: &'static str, severity: Severity) -> Rule {
    Rule {
        code,
        title,
        severity,
        needs_catalog: false,
        only: None,
    }
}

pub const RULES: &[Rule] = &[
    rule("SQ001", "UPDATE or DELETE without WHERE", Severity::Warning),
    rule("SQ002", "SELECT * in a query", Severity::Info),
    rule("SQ003", "Comparison with NULL using = or <>", Severity::Warning),
    Rule {
        only: Some(Dialect::MsSql),
        ..rule("SQ004", "Deprecated data type (text, ntext, image)", Severity::Warning)
    },
    rule("SQ005", "Variable or parameter never read", Severity::Info),
    Rule {
        needs_catalog: true,
        ..rule("SQ006", "Unknown table, view or routine", Severity::Warning)
    },
    Rule {
        needs_catalog: true,
        ..rule("SQ007", "Unknown column", Severity::Warning)
    },
    Rule {
        only: Some(Dialect::MsSql),
        ..rule("SQ008", "Unicode text without the N prefix", Severity::Warning)
    },
    rule(
        "SQ009",
        "NOT IN over a subquery that may return NULL",
        Severity::Warning,
    ),
    rule("SQ010", "Function around a column in WHERE or ON", Severity::Info),
    Rule {
        needs_catalog: true,
        ..rule("SQ011", "Comparison that converts the column", Severity::Warning)
    },
];

/// Which rules run.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LintOptions {
    /// Rule codes that are switched off.
    pub disabled: Vec<String>,
}

impl LintOptions {
    pub fn is_disabled(&self, code: &str) -> bool {
        self.disabled.iter().any(|c| c.eq_ignore_ascii_case(code))
    }
}

/// The diagnostics of `src`, without switched-off rules and suppressed lines.
pub fn lint(src: &str, dialect: Dialect, catalog: Option<&Catalog>, options: &LintOptions) -> Vec<Diagnostic> {
    let off = suppressions(src, dialect);
    diagnostics(src, dialect, catalog)
        .into_iter()
        .filter(|d| !options.is_disabled(&d.code))
        .filter(|d| !off.iter().any(|s| s.covers(d)))
        .collect()
}

/// Apply the quick fixes of `found` that do not overlap each other.
/// Returns the new text and how many fixes were applied.
pub fn apply_fixes(src: &str, found: &[Diagnostic]) -> (String, usize) {
    let mut fixes: Vec<_> = found.iter().filter_map(|d| d.fix.as_ref()).collect();
    fixes.sort_by_key(|f| (f.start, f.end));
    let mut out = String::with_capacity(src.len());
    let mut at = 0;
    let mut applied = 0;
    for f in fixes {
        if f.start < at || f.end > src.len() {
            continue;
        }
        out.push_str(&src[at..f.start]);
        out.push_str(&f.text);
        at = f.end;
        applied += 1;
    }
    out.push_str(&src[at..]);
    (out, applied)
}

/// Lines (byte range) where some or all codes are off.
struct Suppression {
    start: usize,
    end: usize,
    /// Empty: every rule.
    codes: Vec<String>,
}

impl Suppression {
    fn covers(&self, d: &Diagnostic) -> bool {
        self.start <= d.start
            && d.start < self.end
            && (self.codes.is_empty() || self.codes.iter().any(|c| c.eq_ignore_ascii_case(&d.code)))
    }
}

fn is_code(w: &str) -> bool {
    let letters = w.chars().take_while(char::is_ascii_alphabetic).count();
    letters > 0 && letters < w.len() && w[letters..].chars().all(|c| c.is_ascii_digit())
}

/// A comment's first word and the rule codes after it.
struct Directive {
    /// Lowercase, e.g. `sqlotus-disable-line`.
    name: String,
    /// Uppercase; empty means every rule.
    codes: Vec<String>,
    /// Byte offset after the last code (or the directive): where another
    /// code goes.
    end: usize,
}

fn directive(t: &Token<'_>) -> Option<Directive> {
    if !t.is_comment() {
        return None;
    }
    let rest = t.text.trim_start_matches(['-', '#', '/', '*']);
    let base = t.start + t.text.len() - rest.len();
    let body = rest.trim_end_matches(['*', '/']);
    let mut words = comment_words(body).into_iter();
    let (at, first) = words.next()?;
    let mut end = base + at + first.len();
    let mut codes = Vec::new();
    for (at, w) in words {
        if !is_code(w) {
            break;
        }
        codes.push(w.to_ascii_uppercase());
        end = base + at + w.len();
    }
    Some(Directive {
        name: first.to_ascii_lowercase(),
        codes,
        end,
    })
}

/// The words of a comment (split at spaces and commas) with their byte offsets.
fn comment_words(body: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut from = None;
    for (i, c) in body.char_indices().chain([(body.len(), ' ')]) {
        let sep = c.is_whitespace() || c == ',';
        match from {
            Some(s) if sep => {
                out.push((s, &body[s..i]));
                from = None;
            }
            None if !sep => from = Some(i),
            _ => {}
        }
    }
    out
}

/// Where the line holding `at` starts (after the `\n`).
fn line_start(src: &str, at: usize) -> usize {
    src[..at].rfind('\n').map_or(0, |i| i + 1)
}

/// Where the next line after `at` starts (after the `\n`), or the end.
fn next_line(src: &str, at: usize) -> usize {
    src[at..].find('\n').map_or(src.len(), |i| at + i + 1)
}

fn suppressions(src: &str, dialect: Dialect) -> Vec<Suppression> {
    let line_start = |at: usize| line_start(src, at);
    let line_end = |at: usize| next_line(src, at);
    let mut out = Vec::new();
    let mut open: Vec<(usize, Vec<String>)> = Vec::new();
    for t in tokenize(src, dialect) {
        let Some(Directive { name, codes, .. }) = directive(&t) else {
            continue;
        };
        match name.as_str() {
            DISABLE_LINE => out.push(Suppression {
                start: line_start(t.start),
                end: line_end(t.start),
                codes,
            }),
            DISABLE_NEXT_LINE => {
                let next = line_end(t.end());
                out.push(Suppression {
                    start: next,
                    end: line_end(next),
                    codes,
                });
            }
            "sqlotus-disable" => open.push((t.start, codes)),
            "sqlotus-enable" => {
                let (closing, still): (Vec<_>, Vec<_>) =
                    open.into_iter().partition(|(_, c)| codes.is_empty() || *c == codes);
                open = still;
                out.extend(closing.into_iter().map(|(start, codes)| Suppression {
                    start,
                    end: t.start,
                    codes,
                }));
            }
            _ => {}
        }
    }
    out.extend(open.into_iter().map(|(start, codes)| Suppression {
        start,
        end: src.len() + 1,
        codes,
    }));
    out
}

/// For each of `found`, the comment that switches its rule off on its line
/// ("Ignore on this line"): the code added to a `sqlotus-disable-line`
/// comment already there, a trailing `-- sqlotus-disable-line CODE` (a
/// `/* … */` one before a comment that ends the line), or, when the line
/// ends inside a string or comment, `-- sqlotus-disable-next-line CODE` on
/// a new line above. `None` when no comment can go there without changing
/// a string, or the rule is already off.
pub fn ignore_edits(src: &str, dialect: Dialect, found: &[Diagnostic]) -> Vec<Option<Fix>> {
    let tokens = tokenize(src, dialect);
    found.iter().map(|d| ignore_edit(src, &tokens, d)).collect()
}

fn ignore_edit(src: &str, tokens: &[Token<'_>], d: &Diagnostic) -> Option<Fix> {
    let at = d.start.min(src.len());
    let start = line_start(src, at);
    let mut end = src[at..].find('\n').map_or(src.len(), |i| at + i);
    if end > at && src.as_bytes()[end - 1] == b'\r' {
        end -= 1;
    }
    let code = d.code.as_str();
    let insert = |at: usize, text: String| {
        Some(Fix {
            title: format!("Ignore {code} on this line"),
            start: at,
            end: at,
            text,
        })
    };
    // the code joins a directive that already covers the line
    let add_code = |dir: Directive| {
        let off = dir.codes.is_empty() || dir.codes.iter().any(|c| c.eq_ignore_ascii_case(code));
        if off { None } else { insert(dir.end, format!(" {code}")) }
    };
    let on_line = |t: &&Token<'_>| (start..end).contains(&t.start);
    if let Some(dir) = tokens
        .iter()
        .filter(on_line)
        .filter_map(|t| directive(t))
        .find(|x| x.name == DISABLE_LINE)
    {
        return add_code(dir);
    }
    // inside a string, quoted name or block comment that goes on past `at`
    let inside = |at: usize| {
        tokens
            .iter()
            .any(|t| t.kind != TokenKind::Whitespace && t.start < at && at < t.end())
    };
    if !inside(end) {
        if let Some(c) = tokens.iter().filter(on_line).find(|t| t.kind == TokenKind::LineComment) {
            // a comment ends the line: ours goes before it, which stays as it is
            return insert(c.start, format!("/* {DISABLE_LINE} {code} */ "));
        }
        let code_end = start + src[start..end].trim_end().len();
        return insert(code_end, format!(" -- {DISABLE_LINE} {code}"));
    }
    if inside(start) {
        return None;
    }
    // the line ends in a string or comment: a directive on the line above
    if let Some(dir) = tokens
        .iter()
        .find_map(|t| directive(t).filter(|x| x.name == DISABLE_NEXT_LINE && next_line(src, t.end()) == start))
    {
        return add_code(dir);
    }
    let indent: String = src[start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
    let eol = if src.contains("\r\n") { "\r\n" } else { "\n" };
    insert(start, format!("{indent}-- {DISABLE_NEXT_LINE} {code}{eol}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codes(src: &str, options: &LintOptions) -> Vec<String> {
        lint(src, Dialect::MsSql, None, options)
            .into_iter()
            .map(|d| d.code)
            .collect()
    }

    #[test]
    fn rules_are_listed_once() {
        let mut seen: Vec<&str> = RULES.iter().map(|r| r.code).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), RULES.len());
        assert_eq!(RULES.len(), 11);
    }

    #[test]
    fn switched_off_rules() {
        let src = "SELECT * FROM t; DELETE FROM t";
        assert_eq!(codes(src, &LintOptions::default()), vec!["SQ002", "SQ001"]);
        let off = LintOptions {
            disabled: vec!["sq002".into()],
        };
        assert_eq!(codes(src, &off), vec!["SQ001"]);
    }

    #[test]
    fn suppression_comments() {
        let none = LintOptions::default();
        // same line, one code or all
        assert!(codes("SELECT * FROM t -- sqlotus-disable-line SQ002", &none).is_empty());
        assert!(codes("DELETE FROM t /* sqlotus-disable-line */", &none).is_empty());
        assert_eq!(
            codes("DELETE FROM t -- sqlotus-disable-line SQ002", &none),
            vec!["SQ001"]
        );
        // next line only
        let src = "-- sqlotus-disable-next-line SQ001 staging table, emptied on purpose\nDELETE FROM a\nDELETE FROM b";
        let found = lint(src, Dialect::MsSql, None, &none);
        assert_eq!(found.len(), 1);
        assert_eq!(&src[found[0].start..found[0].start + 6], "DELETE");
        assert!(found[0].start > src.find("FROM a").unwrap());
        // a region, closed by sqlotus-enable
        let src =
            "-- sqlotus-disable SQ002\nSELECT * FROM a\nSELECT * FROM b\n-- sqlotus-enable SQ002\nSELECT * FROM c";
        assert_eq!(codes(src, &none), vec!["SQ002"]);
        // a region left open runs to the end; other codes still report
        let src = "-- sqlotus-disable SQ002, SQ005\nSELECT * FROM a; DELETE FROM b";
        assert_eq!(codes(src, &none), vec!["SQ001"]);
        // not a directive: a normal comment mentioning the word
        assert_eq!(
            codes("-- we do not use sqlotus-disable here\nDELETE FROM t", &none),
            vec!["SQ001"]
        );
        // MySQL # comments work too
        let my = lint("DELETE FROM t # sqlotus-disable-line", Dialect::MySql, None, &none);
        assert!(my.is_empty());
    }

    /// `src` after the "Ignore on this line" edit of its `code` problem
    /// (checking that only that problem goes away), or None without one.
    fn ignored(dialect: Dialect, src: &str, code: &str) -> Option<String> {
        let none = LintOptions::default();
        let others = |s: &str| -> Vec<String> {
            lint(s, dialect, None, &none)
                .into_iter()
                .map(|d| d.code)
                .filter(|c| c != code)
                .collect()
        };
        let found = lint(src, dialect, None, &none);
        let i = found.iter().position(|d| d.code == code).expect(code);
        let f = ignore_edits(src, dialect, &found)[i].clone()?;
        assert_eq!(f.start, f.end, "an insertion");
        assert_eq!(f.title, format!("Ignore {code} on this line"));
        let mut out = src.to_string();
        out.replace_range(f.start..f.end, &f.text);
        let after = lint(&out, dialect, None, &none);
        assert!(after.iter().all(|d| d.code != code), "{out}\n{after:?}");
        assert_eq!(others(&out), others(src), "only {code} goes: {out}");
        Some(out)
    }

    #[test]
    fn ignore_on_this_line() {
        let ms = Dialect::MsSql;
        let some = |s: &str| Some(s.to_string());
        assert_eq!(
            ignored(ms, "SELECT * FROM t", "SQ002"),
            some("SELECT * FROM t -- sqlotus-disable-line SQ002")
        );
        // trailing spaces and CRLF line ends
        assert_eq!(
            ignored(ms, "SELECT * FROM t  \r\nDELETE FROM u", "SQ002"),
            some("SELECT * FROM t -- sqlotus-disable-line SQ002  \r\nDELETE FROM u")
        );
        // a directive already on the line gets the code
        assert_eq!(
            ignored(ms, "DELETE FROM t -- sqlotus-disable-line SQ002", "SQ001"),
            some("DELETE FROM t -- sqlotus-disable-line SQ002 SQ001")
        );
        assert_eq!(
            ignored(
                ms,
                "SELECT * FROM t WHERE a = NULL /* sqlotus-disable-line SQ003, note */",
                "SQ002"
            ),
            some("SELECT * FROM t WHERE a = NULL /* sqlotus-disable-line SQ003 SQ002, note */")
        );
        // a comment ending the line stays a comment of its own
        assert_eq!(
            ignored(ms, "SELECT * FROM t -- the main list", "SQ002"),
            some("SELECT * FROM t /* sqlotus-disable-line SQ002 */ -- the main list")
        );
        assert_eq!(
            ignored(
                ms,
                "SELECT * FROM t -- sqlotus-disable-next-line SQ001\nDELETE FROM u",
                "SQ002"
            ),
            some("SELECT * FROM t /* sqlotus-disable-line SQ002 */ -- sqlotus-disable-next-line SQ001\nDELETE FROM u")
        );
        assert_eq!(
            ignored(Dialect::MySql, "SELECT * FROM t # note", "SQ002"),
            some("SELECT * FROM t /* sqlotus-disable-line SQ002 */ # note")
        );
        // the line ends inside a string or comment: never written into it
        assert_eq!(
            ignored(ms, "  SELECT * FROM t WHERE a = 'two\nlines'", "SQ002"),
            some("  -- sqlotus-disable-next-line SQ002\n  SELECT * FROM t WHERE a = 'two\nlines'")
        );
        assert_eq!(
            ignored(ms, "DELETE FROM t /* why:\nstaging */", "SQ001"),
            some("-- sqlotus-disable-next-line SQ001\nDELETE FROM t /* why:\nstaging */")
        );
        assert_eq!(
            ignored(ms, "SELECT 1\r\nDELETE FROM [odd\r\nname]", "SQ001"),
            some("SELECT 1\r\n-- sqlotus-disable-next-line SQ001\r\nDELETE FROM [odd\r\nname]")
        );
        // ... joining a next-line directive already above it
        assert_eq!(
            ignored(
                ms,
                "-- sqlotus-disable-next-line SQ003\nSELECT * FROM t WHERE a = NULL AND b = 'x\ny'",
                "SQ002"
            ),
            some("-- sqlotus-disable-next-line SQ003 SQ002\nSELECT * FROM t WHERE a = NULL AND b = 'x\ny'")
        );
        // starts and ends inside strings: no safe place
        assert_eq!(
            ignored(
                ms,
                "SELECT a FROM t WHERE c = 'one\ntwo' AND d = NULL AND e = 'three\nfour'",
                "SQ003"
            ),
            None
        );
        // the rule is already off there
        let d = |code: &str| Diagnostic {
            start: 0,
            end: 6,
            severity: Severity::Warning,
            code: code.into(),
            message: String::new(),
            fix: None,
        };
        let src = "DELETE FROM t -- sqlotus-disable-line sq001";
        assert_eq!(ignore_edits(src, ms, &[d("SQ001")]), vec![None]);
        assert_eq!(
            ignore_edits("DELETE FROM t -- sqlotus-disable-line", ms, &[d("SQ001")]),
            vec![None]
        );
        // a directive word inside a string is not a comment
        let src = "DELETE FROM t WHERE a = '-- sqlotus-disable-line SQ002'";
        let edit = ignore_edits(src, ms, &[d("SQ001")]).remove(0).unwrap();
        assert_eq!(edit.start, src.len());
    }

    #[test]
    fn quick_fixes() {
        let none = LintOptions::default();
        let fix = |src: &str| {
            let found = lint(src, Dialect::MsSql, None, &none);
            apply_fixes(src, &found)
        };
        assert_eq!(
            fix("SELECT a FROM t WHERE b = NULL OR c <> NULL").0,
            "SELECT a FROM t WHERE b IS NULL OR c IS NOT NULL"
        );
        assert_eq!(
            fix("UPDATE t SET a = 1 WHERE b = 'Nguyễn'").0,
            "UPDATE t SET a = 1 WHERE b = N'Nguyễn'"
        );
        // an unused declaration alone on its line goes with the line
        assert_eq!(fix("DECLARE @unused int = 5;\nSELECT 1;").0, "SELECT 1;");
        // not when the variable is assigned (SET would break), or shares the DECLARE
        assert_eq!(fix("DECLARE @a int; SET @a = 1").1, 0);
        assert_eq!(fix("DECLARE @a int, @b int; SELECT @a").1, 0);
        // NULL = col needs reordering: no automatic fix
        assert_eq!(fix("SELECT a FROM t WHERE NULL = b").1, 0);
        // fixes never overlap
        let (text, n) = fix("SELECT a FROM t WHERE b = NULL AND c = 'Ý'");
        assert_eq!(n, 2);
        assert_eq!(text, "SELECT a FROM t WHERE b IS NULL AND c = N'Ý'");
    }
}
