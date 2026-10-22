//! Properties over a corpus of valid SQL for every dialect (`tests/corpus`),
//! each case with LF and with CRLF line ends: the lexer loses nothing,
//! formatting succeeds, is stable and keeps batches and statements,
//! refactorings change only what they say, statements cover the script, and
//! quick fixes, "Ignore on this line" and renames do what they promise.
//! Every violation is collected, so one run lists them all.

mod support;

use sqlotus_core::format::{CommaStyle, KeywordCase, ListLayout, same_tokens};
use sqlotus_core::lint::{LintOptions, ignore_edits, lint};
use sqlotus_core::statements::{split_batches, split_statements, statement_at};
use sqlotus_core::{Dialect, FormatOptions, Token, TokenKind, format_sql, refactor, rename, tokenize};
use support::{DIALECTS, corpus as cases};

fn significant<'a>(tokens: &[Token<'a>]) -> Vec<Token<'a>> {
    tokens.iter().filter(|t| !t.is_trivia()).copied().collect()
}

/// Code tokens only (no whitespace or comments), as text.
fn code(src: &str, dialect: Dialect) -> Vec<String> {
    significant(&tokenize(src, dialect))
        .iter()
        .map(|t| t.text.to_string())
        .collect()
}

fn batches(src: &str, dialect: Dialect) -> Vec<(String, u32)> {
    split_batches(src, dialect)
        .into_iter()
        .map(|b| (b.span.text(src).to_string(), b.repeat))
        .collect()
}

fn same_batches(a: &str, b: &str, dialect: Dialect) -> bool {
    let (x, y) = (batches(a, dialect), batches(b, dialect));
    x.len() == y.len()
        && x.iter()
            .zip(&y)
            .all(|(p, q)| p.1 == q.1 && same_tokens(&p.0, &q.0, dialect))
}

/// The same code (comments aside, words in any case).
fn same_code(a: &str, b: &str, dialect: Dialect) -> bool {
    let norm = |s: &str| -> Vec<String> {
        significant(&tokenize(s, dialect))
            .iter()
            .map(|t| {
                if t.kind == TokenKind::Word {
                    t.text.to_ascii_uppercase()
                } else {
                    t.text.to_string()
                }
            })
            .collect()
    };
    norm(a) == norm(b)
}

/// The same statements; a comment may move to a neighbour.
fn same_statements(a: &str, b: &str, dialect: Dialect) -> bool {
    let (x, y) = (split_statements(a, dialect), split_statements(b, dialect));
    x.len() == y.len() && x.iter().zip(&y).all(|(p, q)| same_code(p.text(a), q.text(b), dialect))
}

fn format_options() -> Vec<(&'static str, FormatOptions)> {
    let base = FormatOptions::default();
    vec![
        ("default", base.clone()),
        (
            "lower, leading commas, stacked",
            FormatOptions {
                keyword_case: KeywordCase::Lower,
                comma_style: CommaStyle::Leading,
                list_layout: ListLayout::Stacked,
                ..base.clone()
            },
        ),
        (
            "compact",
            FormatOptions {
                indent_size: 2,
                indent_joins: false,
                on_new_line: false,
                and_or_new_line: false,
                blank_line_between_statements: false,
                ..base
            },
        ),
    ]
}

fn report(problems: Vec<String>) {
    if !problems.is_empty() {
        panic!("{} problem(s):\n\n{}", problems.len(), problems.join("\n\n"));
    }
}

#[test]
fn lexer_is_lossless() {
    let mut problems = Vec::new();
    for d in DIALECTS {
        for (name, src) in cases(d) {
            let tokens = tokenize(&src, d);
            let mut at = 0;
            for t in &tokens {
                if t.start != at || t.text.is_empty() {
                    problems.push(format!("{name}: token {:?} at {} (expected {at})", t.text, t.start));
                    break;
                }
                at = t.end();
            }
            if at != src.len() {
                problems.push(format!("{name}: tokens end at {at} of {}", src.len()));
            }
        }
    }
    report(problems);
}

#[test]
fn formatting_succeeds_is_stable_and_keeps_batches() {
    let mut problems = Vec::new();
    for d in DIALECTS {
        for (name, src) in cases(d) {
            for (label, opts) in format_options() {
                let out = format_sql(&src, d, &opts);
                // the corpus is written in lower case: a formatter that gave up returns it as is
                if opts.keyword_case == KeywordCase::Upper && out == src {
                    problems.push(format!("{name} [{label}]: the formatter gave up on valid SQL"));
                    continue;
                }
                let again = format_sql(&out, d, &opts);
                if again != out {
                    problems.push(format!(
                        "{name} [{label}]: formatting is not stable\n--- once\n{out}\n--- twice\n{again}"
                    ));
                }
                if !same_batches(&src, &out, d) {
                    problems.push(format!(
                        "{name} [{label}]: batches changed\n{:?}\n{:?}",
                        batches(&src, d),
                        batches(&out, d)
                    ));
                }
                if !same_statements(&src, &out, d) {
                    let show = |s: &str| {
                        split_statements(s, d)
                            .iter()
                            .map(|x| x.text(s).replace(['\r', '\n'], " "))
                            .collect::<Vec<_>>()
                    };
                    problems.push(format!(
                        "{name} [{label}]: statements changed\n{:#?}\n{:#?}",
                        show(&src),
                        show(&out)
                    ));
                }
                if out.contains("\r\n") != src.contains("\r\n") {
                    problems.push(format!("{name} [{label}]: line ends changed"));
                }
            }
            // keyword case "preserve" keeps every word exactly
            let keep = FormatOptions {
                keyword_case: KeywordCase::Preserve,
                ..FormatOptions::default()
            };
            let out = format_sql(&src, d, &keep);
            let words = |s: &str| -> Vec<String> {
                tokenize(s, d)
                    .into_iter()
                    .filter(|t| t.kind == TokenKind::Word)
                    .map(|t| t.text.to_string())
                    .collect()
            };
            if words(&out) != words(&src) {
                problems.push(format!("{name}: keyword case \"preserve\" changed words"));
            }
        }
    }
    report(problems);
}

#[test]
fn refactorings_change_only_what_they_say() {
    let mut problems = Vec::new();
    for d in DIALECTS {
        for (name, src) in cases(d) {
            for case in [KeywordCase::Upper, KeywordCase::Lower] {
                let out = refactor::apply_keyword_case(&src, d, case);
                let (a, b) = (tokenize(&src, d), tokenize(&out, d));
                let ok = a.len() == b.len()
                    && a.iter().zip(&b).all(|(x, y)| {
                        x.kind == y.kind
                            && if x.kind == TokenKind::Word {
                                x.text.eq_ignore_ascii_case(y.text)
                            } else {
                                x.text == y.text
                            }
                    });
                if !ok {
                    problems.push(format!("{name}: keyword case {case:?} changed more than case"));
                }
            }

            let semi = refactor::insert_semicolons(&src, d);
            let without = |s: &str| -> Vec<String> {
                significant(&tokenize(s, d))
                    .iter()
                    .filter(|t| t.kind != TokenKind::Semicolon)
                    .map(|t| t.text.to_string())
                    .collect()
            };
            if without(&semi) != without(&src) {
                problems.push(format!(
                    "{name}: insert semicolons changed more than semicolons\n{semi}"
                ));
            }
            let twice = refactor::insert_semicolons(&semi, d);
            if twice != semi {
                problems.push(format!(
                    "{name}: insert semicolons is not stable\n--- once\n{semi}\n--- twice\n{twice}"
                ));
            }
            if batches(&semi, d).len() != batches(&src, d).len() {
                problems.push(format!("{name}: insert semicolons changed the batches\n{semi}"));
            }

            let plain = refactor::remove_brackets(&src, d);
            let (a, b) = (significant(&tokenize(&src, d)), significant(&tokenize(&plain, d)));
            let ok = a.len() == b.len()
                && a.iter().zip(&b).all(|(x, y)| {
                    x.text == y.text
                        || (x.kind == TokenKind::QuotedIdent
                            && y.kind == TokenKind::Word
                            && x.text.trim_start_matches('[').trim_end_matches(']') == y.text)
                });
            if !ok {
                problems.push(format!("{name}: remove brackets changed more than brackets\n{plain}"));
            }
        }
    }
    report(problems);
}

#[test]
fn statements_cover_the_script() {
    let mut problems = Vec::new();
    for d in DIALECTS {
        for (name, src) in cases(d) {
            let spans = split_statements(&src, d);
            let mut at = 0;
            for s in &spans {
                if s.start < at || s.end > src.len() || s.start >= s.end {
                    problems.push(format!("{name}: bad statement span {}..{} after {at}", s.start, s.end));
                }
                at = s.end;
                let mid = s.start + (s.end - s.start) / 2;
                let mid = (mid..=s.end).find(|&i| src.is_char_boundary(i)).unwrap_or(s.end);
                if statement_at(&src, d, mid).is_none_or(|x| x.start != s.start || x.end != s.end) {
                    problems.push(format!("{name}: statement_at({mid}) does not find {:?}", s.text(&src)));
                }
            }
            // code outside every statement: only separators
            let tokens = tokenize(&src, d);
            let line_of = |t: &Token<'_>| {
                let start = src[..t.start].rfind('\n').map_or(0, |i| i + 1);
                let end = src[t.start..].find('\n').map_or(src.len(), |i| t.start + i);
                src[start..end].trim().to_string()
            };
            for t in significant(&tokens) {
                if spans.iter().any(|s| s.start <= t.start && t.end() <= s.end) || t.kind == TokenKind::Semicolon {
                    continue;
                }
                // MySQL cuts at a custom terminator even inside a word (`end$$`)
                if d == Dialect::MySql && spans.iter().any(|s| s.start < t.end() && t.start < s.end) {
                    continue;
                }
                let line = line_of(&t);
                let upper = line.to_ascii_uppercase();
                let separator = (d == Dialect::MsSql && (upper == "GO" || upper.starts_with("GO ")))
                    || (d == Dialect::MySql && upper.starts_with("DELIMITER "))
                    || (d == Dialect::MySql && t.text.chars().all(|c| c == '/' || c == '$'));
                if !separator {
                    problems.push(format!("{name}: {:?} (line {line:?}) is in no statement", t.text));
                }
            }
        }
    }
    report(problems);
}

#[test]
fn quick_fixes_and_ignore_comments_do_what_they_say() {
    let mut problems = Vec::new();
    let none = LintOptions::default();
    let count = |src: &str, d: Dialect, code: &str| lint(src, d, None, &none).iter().filter(|x| x.code == code).count();
    let without_comments = |src: &str, d: Dialect| code(src, d);
    for d in DIALECTS {
        for (name, src) in cases(d) {
            let found = lint(&src, d, None, &none);
            for x in &found {
                if let Some(f) = &x.fix {
                    let mut t = src.clone();
                    t.replace_range(f.start..f.end, &f.text);
                    if count(&t, d, &x.code) >= count(&src, d, &x.code) {
                        problems.push(format!(
                            "{name}: fix {:?} of {} leaves the problem\n{t}",
                            f.title, x.code
                        ));
                    }
                    if batches(&t, d).len() != batches(&src, d).len() {
                        problems.push(format!("{name}: fix {:?} changed the batches", f.title));
                    }
                }
            }
            for (x, edit) in found.iter().zip(ignore_edits(&src, d, &found)) {
                let Some(f) = edit else {
                    problems.push(format!(
                        "{name}: no ignore comment for {} at {:?}",
                        x.code,
                        &src[x.start..x.end]
                    ));
                    continue;
                };
                let mut t = src.clone();
                t.replace_range(f.start..f.end, &f.text);
                if count(&t, d, &x.code) >= count(&src, d, &x.code) {
                    problems.push(format!("{name}: ignoring {} does not hide it\n{t}", x.code));
                }
                if without_comments(&t, d) != without_comments(&src, d) {
                    problems.push(format!("{name}: ignoring {} changed the code\n{t}", x.code));
                }
                let others = |s: &str| {
                    let mut v: Vec<String> = lint(s, d, None, &none)
                        .into_iter()
                        .map(|y| y.code)
                        .filter(|c| *c != x.code)
                        .collect();
                    v.sort();
                    v
                };
                if others(&t) != others(&src) {
                    problems.push(format!("{name}: ignoring {} hid other problems too\n{t}", x.code));
                }
            }
        }
    }
    report(problems);
}

#[test]
fn renames_can_be_undone() {
    let mut problems = Vec::new();
    for d in DIALECTS {
        for (name, src) in cases(d) {
            let tokens = tokenize(&src, d);
            let mut tried = std::collections::HashSet::new();
            for t in significant(&tokens) {
                if !(t.kind == TokenKind::Variable || t.is_name()) {
                    continue;
                }
                let Some(target) = rename::rename_target(&src, t.start, d) else {
                    continue;
                };
                if !tried.insert((target.kind.clone(), target.name.to_lowercase(), target.ranges.clone())) {
                    continue;
                }
                // every occurrence spells the name
                for &(s, e) in &target.ranges {
                    let at = &src[s..e];
                    let bare = |x: &str| x.trim_matches(|c| matches!(c, '[' | ']' | '"' | '`')).to_lowercase();
                    if bare(at) != bare(&target.name) {
                        problems.push(format!("{name}: rename range {at:?} is not {:?}", target.name));
                    }
                }
                // same length, so offsets stay valid for the way back
                let len = target.name.chars().count();
                let new = if target.kind == "variable" {
                    format!("@{}", "q".repeat(len.saturating_sub(1).max(1)))
                } else {
                    "q".repeat(len.max(1))
                };
                let Some(renamed) = rename::rename(&src, t.start, d, &new) else {
                    problems.push(format!("{name}: rename of {:?} failed", target.name));
                    continue;
                };
                if renamed.len() != src.len() {
                    continue;
                }
                match rename::rename(&renamed, t.start, d, &target.name) {
                    Some(back) if back == src => {}
                    other => problems.push(format!(
                        "{name}: renaming {:?} to {new:?} and back does not restore the text\n{}",
                        target.name,
                        other.unwrap_or_default()
                    )),
                }
            }
        }
    }
    report(problems);
}
