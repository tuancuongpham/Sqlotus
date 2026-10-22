//! SQLCMD mode, the basics: `:setvar name "value"` lines define variables
//! that `$(name)` stands for anywhere after them (in names, strings and
//! comments too, as sqlcmd does), and `:on error exit` stops a script at the
//! first batch that fails. Command lines are run by the client, never sent
//! to the server; other commands (`:r`, `:connect`, ...) are refused rather
//! than sent. SQL Server only: there a command line starts with `:`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::ops::Range;

use crate::Dialect;
use crate::lexer::{TokenKind, sqlcmd_variable_len, tokenize};

/// A script that uses SQLCMD commands, ready to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sqlcmd {
    /// The script with its command lines blanked out. It has the original's
    /// length, so offsets into it are offsets into the original.
    pub text: String,
    /// `$(name)` uses: where, and the value each stands for there.
    uses: Vec<(Range<usize>, String)>,
    /// `:on error exit`: stop at the first batch that fails.
    pub stop_on_error: bool,
}

impl Sqlcmd {
    /// The commands of a T-SQL script; `Ok(None)` when it has none (it then
    /// runs as written). An error names the line of an unknown command or of
    /// a variable used without a value.
    pub fn parse(src: &str) -> Result<Option<Sqlcmd>, String> {
        let commands: Vec<Range<usize>> = tokenize(src, Dialect::MsSql)
            .iter()
            .filter(|t| t.kind == TokenKind::LineComment && t.text.starts_with(':'))
            .map(|t| t.start..t.end())
            .collect();
        if commands.is_empty() {
            return Ok(None);
        }
        let mut vars: HashMap<String, String> = HashMap::new();
        let mut uses = Vec::new();
        let mut stop_on_error = false;
        let mut at = 0;
        for command in commands.iter().cloned().chain(std::iter::once(src.len()..src.len())) {
            // variables between the previous command line and this one
            uses.extend(variable_uses(src, at..command.start, &vars)?);
            if command.is_empty() {
                break;
            }
            let line = LineOf(src, command.start);
            let body = &src[command.start + 1..command.end];
            let (name, rest) = split_word(body);
            match name.to_ascii_lowercase().as_str() {
                "setvar" => {
                    let (var, value) = split_word(rest);
                    if var.is_empty() || var.contains(['$', '(', ')', '"', '\'']) {
                        return Err(format!(
                            "Line {line}: :setvar needs a variable name, e.g. :setvar Env \"Test\"."
                        ));
                    }
                    let value = unquote(value.trim())
                        .ok_or_else(|| format!("Line {line}: the value of {var} has no closing quote."))?;
                    if value.is_empty() && !value_given(rest) {
                        vars.remove(&var.to_lowercase());
                    } else {
                        // a value may use variables defined above it
                        let value = expand_in(&value, &vars).map_err(|name| {
                            format!("Line {line}: $({name}) has no value; add :setvar {name} … above this line.")
                        })?;
                        vars.insert(var.to_lowercase(), value);
                    }
                }
                "on" => {
                    let words: Vec<String> = rest.split_whitespace().map(str::to_ascii_lowercase).collect();
                    match words.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
                        ["error", "exit"] => stop_on_error = true,
                        ["error", "ignore"] => stop_on_error = false,
                        _ => return Err(format!("Line {line}: use :on error exit or :on error ignore.")),
                    }
                }
                other => {
                    return Err(format!(
                        "Line {line}: the SQLCMD command :{other} is not supported here (only :setvar and :on error)."
                    ));
                }
            }
            at = command.end;
        }
        // command lines become spaces: the same length, still valid UTF-8
        let mut bytes = src.as_bytes().to_vec();
        for command in &commands {
            bytes[command.clone()].fill(b' ');
        }
        let text = String::from_utf8(bytes).expect("whole characters were replaced");
        Ok(Some(Sqlcmd {
            text,
            uses,
            stop_on_error,
        }))
    }

    /// The text of `span` (offsets into [`Sqlcmd::text`]) with every variable
    /// replaced by its value.
    pub fn expand(&self, span: Range<usize>) -> String {
        let mut out = String::with_capacity(span.len());
        let mut at = span.start;
        let first = self.uses.partition_point(|(r, _)| r.start < span.start);
        for (r, value) in &self.uses[first..] {
            if r.end > span.end {
                break;
            }
            out.push_str(&self.text[at..r.start]);
            out.push_str(value);
            at = r.end;
        }
        out.push_str(&self.text[at..span.end]);
        out
    }
}

/// The text the server receives for `sql`: with SQLCMD command lines
/// blanked and `$(name)` replaced by its value (T-SQL scripts that use
/// commands); `sql` itself otherwise, and when a command cannot be honoured
/// (then nothing runs). The checks made before a run read this text, so a
/// `$(Cmd)` cannot hide a DROP from a read-only or production connection.
pub fn as_run(sql: &str, dialect: Dialect) -> Cow<'_, str> {
    if dialect != Dialect::MsSql {
        return Cow::Borrowed(sql);
    }
    match Sqlcmd::parse(sql) {
        Ok(Some(cmd)) => Cow::Owned(cmd.expand(0..cmd.text.len())),
        _ => Cow::Borrowed(sql),
    }
}

/// `$(name)` uses in `src[range]`, with the value each has.
fn variable_uses(
    src: &str,
    range: Range<usize>,
    vars: &HashMap<String, String>,
) -> Result<Vec<(Range<usize>, String)>, String> {
    let mut out = Vec::new();
    let mut i = range.start;
    while let Some(found) = src[i..range.end].find("$(") {
        let start = i + found;
        match sqlcmd_variable_len(&src[start..range.end]) {
            Some(len) => {
                let name = &src[start + 2..start + len - 1];
                let value = vars.get(&name.to_lowercase()).ok_or_else(|| {
                    format!(
                        "Line {}: $({name}) has no value; add :setvar {name} … above this line.",
                        line_of(src, start)
                    )
                })?;
                out.push((start..start + len, value.clone()));
                i = start + len;
            }
            None => i = start + 2,
        }
    }
    Ok(out)
}

/// `text` with its `$(name)` variables replaced; `Err(name)` for one
/// without a value.
fn expand_in(text: &str, vars: &HashMap<String, String>) -> Result<String, String> {
    let mut out = String::new();
    let mut at = 0;
    while let Some(found) = text[at..].find("$(") {
        let start = at + found;
        match sqlcmd_variable_len(&text[start..]) {
            Some(len) => {
                let name = &text[start + 2..start + len - 1];
                let value = vars.get(&name.to_lowercase()).ok_or_else(|| name.to_string())?;
                out.push_str(&text[at..start]);
                out.push_str(value);
                at = start + len;
            }
            None => {
                out.push_str(&text[at..start + 2]);
                at = start + 2;
            }
        }
    }
    out.push_str(&text[at..]);
    Ok(out)
}

/// The first whitespace-separated word of `s` and the rest after it.
fn split_word(s: &str) -> (&str, &str) {
    let s = s.trim_start();
    let end = s.find(char::is_whitespace).unwrap_or(s.len());
    (&s[..end], &s[end..])
}

/// A value as written after the variable name: `"quoted"` (`""` is a quote
/// inside) or the rest of the line. `None` when a quote is not closed.
fn unquote(v: &str) -> Option<String> {
    let Some(body) = v.strip_prefix('"') else {
        return Some(v.to_string());
    };
    let mut out = String::new();
    let mut chars = body.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '"' {
            if chars.peek() == Some(&'"') {
                chars.next();
                out.push('"');
            } else {
                return Some(out);
            }
        } else {
            out.push(c);
        }
    }
    None
}

/// Whether a value follows the variable name (`:setvar x ""` sets an empty
/// value; `:setvar x` alone removes the variable).
fn value_given(rest: &str) -> bool {
    !split_word(rest).1.trim().is_empty()
}

fn line_of(src: &str, offset: usize) -> usize {
    src[..offset].matches('\n').count() + 1
}

/// The line of an offset, counted only when an error names it (counting it
/// for every command made long scripts quadratic).
struct LineOf<'a>(&'a str, usize);

impl std::fmt::Display for LineOf<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", line_of(self.0, self.1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> Vec<String> {
        let cmd = Sqlcmd::parse(src).unwrap().unwrap();
        crate::statements::split_batches(&cmd.text, Dialect::MsSql)
            .iter()
            .map(|b| cmd.expand(b.span.start..b.span.end))
            .collect()
    }

    #[test]
    fn plain_scripts_run_as_written() {
        assert_eq!(Sqlcmd::parse("SELECT '$(not a variable)', 1 -- :setvar x 1"), Ok(None));
        assert_eq!(Sqlcmd::parse("SELECT 1\nGO\nSELECT 2"), Ok(None));
    }

    #[test]
    fn variables_are_replaced_everywhere_after_their_definition() {
        let src = ":setvar Schema dbo\n:setvar Suffix \"_2024\"\nSELECT '$(Schema)' AS s, * FROM $(Schema).Orders$(Suffix) -- $(schema)\nGO\n\
                   :setvar Suffix \"_2025\"\nSELECT * FROM [$(Schema)].[Orders$(Suffix)]";
        assert_eq!(
            run(src),
            vec![
                "SELECT 'dbo' AS s, * FROM dbo.Orders_2024 -- dbo",
                "SELECT * FROM [dbo].[Orders_2025]",
            ]
        );
        // command lines are blanked: same length, so offsets still match
        let cmd = Sqlcmd::parse(src).unwrap().unwrap();
        assert_eq!(cmd.text.len(), src.len());
        assert!(!cmd.text.contains(":setvar"));
        assert!(!cmd.stop_on_error);
    }

    #[test]
    fn values_quotes_and_removal() {
        let src = ":setvar Name \"O\"\"Brien says \"\"hi\"\"\"\n:setvar Empty \"\"\n:setvar Path C:\\Data\\x.bak\n\
                   :setvar Full \"$(Path).old\"\nSELECT N'$(Name)|$(Empty)|$(Path)|$(Full)'";
        assert_eq!(
            run(src),
            vec!["SELECT N'O\"Brien says \"hi\"||C:\\Data\\x.bak|C:\\Data\\x.bak.old'"]
        );
        let removed = Sqlcmd::parse(":setvar x 1\nSELECT $(x)\nGO\n:setvar x\nSELECT $(x)").unwrap_err();
        assert!(removed.starts_with("Line 5: $(x) has no value"), "{removed}");
    }

    #[test]
    fn the_checks_read_what_the_server_runs() {
        let src = ":setvar Cmd \"DROP TABLE dbo.Orders\"\nSELECT 1;\n$(Cmd)";
        assert_eq!(
            as_run(src, Dialect::MsSql).trim_start(),
            "SELECT 1;\nDROP TABLE dbo.Orders"
        );
        let run = crate::safety::ScriptRun::new(&as_run(src, Dialect::MsSql), Dialect::MsSql).changes();
        assert_eq!(run, vec!["DROP TABLE dbo.Orders"]);
        // without commands, on other servers, and when a command cannot run: as written
        assert_eq!(as_run("SELECT '$(x)'", Dialect::MsSql), "SELECT '$(x)'");
        assert_eq!(as_run(":setvar x 1", Dialect::Postgres), ":setvar x 1");
        assert_eq!(
            as_run(":r x.sql\nDELETE FROM t", Dialect::MsSql),
            ":r x.sql\nDELETE FROM t"
        );
    }

    #[test]
    fn on_error_exit() {
        let cmd = Sqlcmd::parse(":on error exit\nSELECT 1").unwrap().unwrap();
        assert!(cmd.stop_on_error);
        let cmd = Sqlcmd::parse(":On Error Exit\n:on error ignore\nSELECT 1")
            .unwrap()
            .unwrap();
        assert!(!cmd.stop_on_error);
        assert!(Sqlcmd::parse(":on error stop").unwrap_err().contains(":on error exit"));
    }

    #[test]
    fn mistakes_are_reported_with_their_line() {
        assert_eq!(
            Sqlcmd::parse("SELECT 1\n:r .\\setup.sql").unwrap_err(),
            "Line 2: the SQLCMD command :r is not supported here (only :setvar and :on error)."
        );
        assert!(
            Sqlcmd::parse(":setvar x 1\nSELECT $(y)")
                .unwrap_err()
                .starts_with("Line 2: $(y) has no value")
        );
        // used before it is defined
        assert!(
            Sqlcmd::parse("SELECT $(x)\n:setvar x 1")
                .unwrap_err()
                .starts_with("Line 1:")
        );
        assert!(
            Sqlcmd::parse(":setvar x \"open")
                .unwrap_err()
                .contains("no closing quote")
        );
        assert!(
            Sqlcmd::parse(":setvar\nSELECT 1")
                .unwrap_err()
                .contains("needs a variable name")
        );
        assert!(
            Sqlcmd::parse(":setvar $(a) 1")
                .unwrap_err()
                .contains("needs a variable name")
        );
    }

    #[test]
    fn names_are_case_insensitive_and_unicode_values_keep_offsets() {
        let src = ":setvar Tên \"Phạm Tuấn\"\nSELECT N'$(TÊN)' -- ẫ\nGO 2";
        let cmd = Sqlcmd::parse(src).unwrap().unwrap();
        assert_eq!(cmd.text.len(), src.len());
        let batches = crate::statements::split_batches(&cmd.text, Dialect::MsSql);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].repeat, 2);
        assert_eq!(
            cmd.expand(batches[0].span.start..batches[0].span.end),
            "SELECT N'Phạm Tuấn' -- ẫ"
        );
    }
}
