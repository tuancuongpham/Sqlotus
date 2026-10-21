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

use serde::{Deserialize, Serialize};

use crate::Dialect;
use crate::analysis::{Diagnostic, Severity, diagnostics};
use crate::catalog::Catalog;
use crate::lexer::{TokenKind, tokenize};

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

fn suppressions(src: &str, dialect: Dialect) -> Vec<Suppression> {
    let line_start = |at: usize| src[..at].rfind('\n').map_or(0, |i| i + 1);
    let line_end = |at: usize| src[at..].find('\n').map_or(src.len(), |i| at + i + 1);
    let mut out = Vec::new();
    let mut open: Vec<(usize, Vec<String>)> = Vec::new();
    for t in tokenize(src, dialect) {
        if !matches!(t.kind, TokenKind::LineComment | TokenKind::BlockComment) {
            continue;
        }
        let body = t
            .text
            .trim_start_matches(['-', '#', '/', '*'])
            .trim_end_matches(['*', '/'])
            .trim();
        let mut words = body
            .split(|c: char| c.is_whitespace() || c == ',')
            .filter(|w| !w.is_empty());
        let Some(directive) = words.next() else {
            continue;
        };
        let codes: Vec<String> = words.take_while(|w| is_code(w)).map(str::to_ascii_uppercase).collect();
        match directive.to_ascii_lowercase().as_str() {
            "sqlotus-disable-line" => out.push(Suppression {
                start: line_start(t.start),
                end: line_end(t.start),
                codes,
            }),
            "sqlotus-disable-next-line" => {
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
