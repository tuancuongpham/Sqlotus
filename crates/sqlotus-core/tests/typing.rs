//! Simulated editing sessions: what the editor asks of the engine while a
//! person types, deletes, pastes and moves around a script.
//!
//! A seeded "person" writes each corpus case into an empty editor, one
//! character at a time at a moving cursor, now and then making a typo and
//! fixing it with Backspace, typing a word and taking it back with
//! Ctrl+Backspace, jumping back to add something and removing it, or
//! pasting a fragment of another case and undoing it. Completion runs after
//! every keystroke; everything else runs whenever typing pauses (after a
//! word), as the editor's analysis does. A second person reworks finished
//! scripts: deletes ranges, duplicates and comments out lines, pastes in
//! the middle.
//!
//! Every answer must make sense for the text of that moment: ranges inside
//! it and on character boundaries, quick fixes and "Ignore on this line"
//! that apply and change only what they say, the statement to run around the
//! cursor, formatting that keeps the code. Panics are caught and reported
//! with the text that caused them, and every problem is collected, so one
//! run lists them all.

mod support;

use std::panic::{AssertUnwindSafe, catch_unwind};

use sqlotus_core::analysis::{diagnostics, hover, signature_help};
use sqlotus_core::catalog::Catalog;
use sqlotus_core::format::same_tokens;
use sqlotus_core::lint::{LintOptions, apply_fixes, ignore_edits, lint};
use sqlotus_core::safety::ScriptRun;
use sqlotus_core::statements::{Span, executable_statement_at, split_batches, split_statements, statement_at};
use sqlotus_core::{
    CompletionOptions, Dialect, FormatOptions, TokenKind, complete, format_sql, rename, scope, tokenize, variables,
};
use support::{DIALECTS, Rng, catalog, corpus};

/// The text being edited and the cursor, a byte offset on a character
/// boundary.
#[derive(Default)]
struct Editor {
    text: String,
    cursor: usize,
}

impl Editor {
    fn type_text(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    fn backspace(&mut self) {
        if let Some(c) = self.text[..self.cursor].chars().next_back() {
            self.cursor -= c.len_utf8();
            self.text.remove(self.cursor);
        }
    }

    /// Ctrl+Backspace: back to the start of the word before the cursor.
    fn delete_word_left(&mut self) -> String {
        let before = &self.text[..self.cursor];
        let kept = before.trim_end_matches(|c: char| c.is_alphanumeric() || c == '_').len();
        let removed = self.text[kept..self.cursor].to_string();
        self.text.replace_range(kept..self.cursor, "");
        self.cursor = kept;
        removed
    }

    /// Put the cursor at `at`, or on the character boundary just before it.
    fn move_to(&mut self, at: usize) {
        let mut at = at.min(self.text.len());
        while !self.text.is_char_boundary(at) {
            at -= 1;
        }
        self.cursor = at;
    }
}

/// When the checks run: completion after every keystroke, everything else
/// when typing pauses.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Moment {
    Keystroke,
    Pause,
}

const TYPOS: &[&str] = &[
    "x", "(", "'", "\"", "[", "`", "ễ", "😀", ",", ";", "@", "--", "/*", "N'", "$$",
];

/// Type `target` into an empty editor as a person would, calling `step`
/// with the editor at every moment worth checking. The editor ends with
/// exactly `target`.
fn write(target: &str, rng: &mut Rng, fragments: &[&str], mut step: impl FnMut(&Editor, Moment)) -> String {
    let mut ed = Editor::default();
    for c in target.chars() {
        // a typo, seen, then fixed
        if rng.chance(3) {
            let typo = *rng.pick(TYPOS);
            ed.type_text(typo);
            step(&ed, Moment::Keystroke);
            for _ in typo.chars() {
                ed.backspace();
            }
        }
        // a word typed and taken back
        if rng.chance(1) && c.is_whitespace() {
            ed.type_text(" FROM");
            step(&ed, Moment::Pause);
            ed.delete_word_left();
            ed.backspace();
        }
        // back up the text to add something, then remove it and come back
        if rng.chance(1) && ed.cursor > 0 {
            let end = ed.cursor;
            ed.move_to(rng.below(end + 1));
            let at = ed.cursor;
            let fragment = *rng.pick(fragments);
            ed.type_text(fragment);
            step(&ed, Moment::Pause);
            ed.text.replace_range(at..at + fragment.len(), "");
            ed.cursor = end;
        }
        ed.type_text(c.encode_utf8(&mut [0; 4]));
        step(&ed, Moment::Keystroke);
        if c.is_whitespace() || matches!(c, ';' | ',' | ')') {
            step(&ed, Moment::Pause);
        }
    }
    step(&ed, Moment::Pause);
    ed.text
}

fn valid(text: &str, start: usize, end: usize) -> bool {
    start <= end && end <= text.len() && text.is_char_boundary(start) && text.is_char_boundary(end)
}

/// The same code (comments aside, words in any case).
fn same_code(a: &str, b: &str, d: Dialect) -> bool {
    let norm = |s: &str| -> Vec<String> {
        tokenize(s, d)
            .iter()
            .filter(|t| !t.is_trivia())
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

/// Problems found at one moment, each with the case, the text and where
/// the cursor was.
struct Checker<'a> {
    dialect: Dialect,
    catalog: &'a Catalog,
    problems: Vec<String>,
}

impl Checker<'_> {
    fn report(&mut self, case: &str, ed: &Editor, what: &str) {
        let (before, after) = ed.text.split_at(ed.cursor);
        let show = |s: &str, n: usize| s.chars().take(n).collect::<String>();
        let shown = format!(
            "{}‸{}",
            show(&before.chars().rev().collect::<String>(), 120)
                .chars()
                .rev()
                .collect::<String>(),
            show(after, 120)
        );
        // the first few of a kind say enough; TYPING_FULL=1 shows the whole text
        if self.problems.iter().filter(|p| p.contains(what)).count() < 3 {
            let full = if std::env::var_os("TYPING_FULL").is_some() {
                format!("\n    text: {:?}\n    cursor: {}", ed.text, ed.cursor)
            } else {
                String::new()
            };
            self.problems.push(format!("{case}: {what}\n    at: {shown:?}{full}"));
        }
    }

    /// Run `f`, reporting a panic as a problem.
    fn guard<T>(&mut self, case: &str, ed: &Editor, feature: &str, f: impl FnOnce() -> T) -> Option<T> {
        match catch_unwind(AssertUnwindSafe(f)) {
            Ok(v) => Some(v),
            Err(_) => {
                self.report(case, ed, &format!("{feature} panicked"));
                None
            }
        }
    }

    fn check(&mut self, case: &str, ed: &Editor, moment: Moment) {
        let (d, text, at) = (self.dialect, ed.text.as_str(), ed.cursor);
        let cat = self.catalog;
        // completion, after every keystroke
        if let Some(r) = self.guard(case, ed, "completion", || {
            complete(text, at, d, Some(cat), &[], &CompletionOptions::default())
        }) {
            let items_ok = r
                .items
                .iter()
                .all(|i| i.replace_start.is_none_or(|s| valid(text, s, r.replace_end)));
            if !valid(text, r.replace_start, r.replace_end) || r.replace_start > at || !items_ok {
                self.report(
                    case,
                    ed,
                    "completion replaces a range outside the text or after the cursor",
                );
            }
        }
        if moment == Moment::Keystroke {
            return;
        }
        // the lexer loses nothing
        let joined: String = tokenize(text, d).iter().map(|t| t.text).collect();
        if joined != text {
            self.report(case, ed, "the tokens do not add up to the text");
        }
        // batches and statements: in order, inside the text
        let ordered = |spans: &[Span]| {
            spans.iter().all(|s| valid(text, s.start, s.end)) && spans.windows(2).all(|w| w[0].end <= w[1].start)
        };
        if let Some(spans) = self.guard(case, ed, "statement split", || split_statements(text, d))
            && !ordered(&spans)
        {
            self.report(case, ed, "statements overlap or leave the text");
        }
        if let Some(batches) = self.guard(case, ed, "batch split", || split_batches(text, d)) {
            let spans: Vec<Span> = batches.iter().map(|b| b.span).collect();
            if !ordered(&spans) {
                self.report(case, ed, "batches overlap or leave the text");
            }
        }
        // what Shift+F5 runs holds the statement at the cursor
        let here = self.guard(case, ed, "statement at the cursor", || statement_at(text, d, at));
        let run = self.guard(case, ed, "execute current statement", || {
            executable_statement_at(text, d, at)
        });
        if let (Some(Some(here)), Some(Some(run))) = (here, run)
            && (!valid(text, run.start, run.end) || run.start > here.start || run.end < here.end)
        {
            self.report(
                case,
                ed,
                "execute current statement does not cover the statement at the cursor",
            );
        }
        // analysis: ranges inside the text, quick fixes that apply
        if let Some(found) = self.guard(case, ed, "analysis", || diagnostics(text, d, Some(cat))) {
            let ranges_ok = found
                .iter()
                .all(|x| valid(text, x.start, x.end) && x.fix.as_ref().is_none_or(|f| valid(text, f.start, f.end)));
            if !ranges_ok {
                self.report(case, ed, "a problem or its quick fix points outside the text");
            } else if let Some((fixed, _)) = self.guard(case, ed, "apply quick fixes", || apply_fixes(text, &found)) {
                let joined: String = tokenize(&fixed, d).iter().map(|t| t.text).collect();
                if joined != fixed {
                    self.report(case, ed, "the text after the quick fixes does not tokenize");
                }
            }
        }
        // "Ignore on this line" adds a comment and nothing else
        let options = LintOptions::default();
        if let Some(found) = self.guard(case, ed, "lint", || lint(text, d, Some(cat), &options))
            && let Some(edits) = self.guard(case, ed, "ignore on this line", || ignore_edits(text, d, &found))
        {
            for e in edits.into_iter().flatten() {
                if !valid(text, e.start, e.end) || e.start != e.end {
                    self.report(case, ed, "Ignore on this line is not an insertion inside the text");
                    continue;
                }
                let mut after = text.to_string();
                after.insert_str(e.start, &e.text);
                if !same_code(text, &after, d) {
                    self.report(case, ed, "Ignore on this line changes code, not just comments");
                }
            }
        }
        // hover, parameter hints, rename
        if let Some(Some(h)) = self.guard(case, ed, "hover", || hover(text, at, d, Some(cat)))
            && !valid(text, h.start, h.end)
        {
            self.report(case, ed, "hover range outside the text");
        }
        self.guard(case, ed, "parameter hints", || signature_help(text, at, d, Some(cat)));
        if let Some(Some(target)) = self.guard(case, ed, "rename target", || rename::rename_target(text, at, d)) {
            let mut ranges = target.ranges.clone();
            ranges.sort_unstable();
            if !ranges.iter().all(|r| valid(text, r.0, r.1)) || ranges.windows(2).any(|w| w[0].1 > w[1].0) {
                self.report(case, ed, "rename ranges overlap or leave the text");
            }
            self.guard(case, ed, "rename", || rename::rename(text, at, d, "renamed_1"));
        }
        // the checks made before running, variables and symbols
        self.guard(case, ed, "checks before running", || {
            let run = ScriptRun::new(text, d);
            (run.changes(), run.warnings(), run.calls(), run.row_changes_only())
        });
        self.guard(case, ed, "undeclared variables", || {
            variables::undeclared_variables(text, d)
        });
        self.guard(case, ed, "document symbols", || scope::document_symbols(text, d));
        // formatting keeps the code (checked at the ends of lines: it is the slowest)
        if text.ends_with(['\n', ';'])
            && let Some(out) = self.guard(case, ed, "format", || format_sql(text, d, &FormatOptions::default()))
            && !same_tokens(text, &out, d)
        {
            self.report(case, ed, "formatting changed the code");
        }
    }
}

fn report(problems: &[String]) {
    assert!(
        problems.is_empty(),
        "{} problem(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

#[test]
fn typing_every_corpus_case() {
    let cat = catalog();
    let mut problems = Vec::new();
    for d in DIALECTS {
        let cases = corpus(d);
        let fragments: Vec<&str> = cases.iter().map(|(_, t)| t.lines().next().unwrap_or("")).collect();
        let mut checker = Checker {
            dialect: d,
            catalog: &cat,
            problems: Vec::new(),
        };
        // the LF copy of each case; CRLF copies are reworked below
        for (seed, (name, text)) in cases.iter().enumerate().filter(|(_, (n, _))| !n.ends_with("(CRLF)")) {
            let mut rng = Rng::new(seed as u64 + 1);
            let written = write(text, &mut rng, &fragments, |ed, moment| checker.check(name, ed, moment));
            assert_eq!(&written, text, "{name}: the session must end with the case");
        }
        problems.extend(checker.problems);
    }
    report(&problems);
}

#[test]
fn reworking_finished_scripts() {
    let cat = catalog();
    let mut problems = Vec::new();
    for d in DIALECTS {
        let cases = corpus(d);
        let fragments: Vec<String> = cases.iter().map(|(_, t)| t.clone()).collect();
        let mut checker = Checker {
            dialect: d,
            catalog: &cat,
            problems: Vec::new(),
        };
        for (seed, (name, text)) in cases.iter().enumerate() {
            let mut rng = Rng::new(1_000 + seed as u64);
            let mut ed = Editor {
                text: text.clone(),
                cursor: 0,
            };
            for _ in 0..12 {
                let len = ed.text.len();
                ed.move_to(rng.below(len + 1));
                match rng.below(6) {
                    // select and delete a stretch
                    0 => {
                        let from = ed.cursor;
                        ed.move_to(from + rng.below(40));
                        let to = ed.cursor;
                        ed.text.replace_range(from..to, "");
                        ed.cursor = from;
                    }
                    // duplicate the line (Shift+Alt+Down)
                    1 => {
                        let start = ed.text[..ed.cursor].rfind('\n').map_or(0, |i| i + 1);
                        let end = ed.text[ed.cursor..].find('\n').map_or(ed.text.len(), |i| ed.cursor + i);
                        let line = format!("{}\n", &ed.text[start..end]);
                        ed.text.insert_str(start, &line);
                    }
                    // comment the line out (Ctrl+/)
                    2 => {
                        let start = ed.text[..ed.cursor].rfind('\n').map_or(0, |i| i + 1);
                        ed.text.insert_str(start, "-- ");
                        ed.cursor += 3;
                    }
                    // paste part of another script
                    3 => {
                        let other = rng.pick(&fragments);
                        let mut cut = rng.below(other.len().min(120) + 1);
                        while !other.is_char_boundary(cut) {
                            cut -= 1;
                        }
                        ed.type_text(&other[..cut]);
                    }
                    // lose a closing quote, bracket or parenthesis
                    4 => {
                        if let Some(i) = ed.text[ed.cursor..].find([')', '\'', ']', '"', '`']) {
                            ed.text.remove(ed.cursor + i);
                        }
                    }
                    // type over: a word, a newline and a keyword
                    _ => ed.type_text(rng.pick(&[" x", "\n", " SELECT ", " END", "(", " GO\n", ";"])),
                }
                ed.move_to(ed.cursor);
                checker.check(name, &ed, Moment::Pause);
            }
        }
        problems.extend(checker.problems);
    }
    report(&problems);
}
