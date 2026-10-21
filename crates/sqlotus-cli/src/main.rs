//! `sqlotus-lint`: the Sqlotus code analysis for `.sql` files, for CI and
//! pre-commit checks. Same rules, suppression comments and quick fixes as
//! the editor.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use sqlotus_core::Dialect;
use sqlotus_core::analysis::{Diagnostic, Severity};
use sqlotus_core::catalog::Catalog;
use sqlotus_core::lint::{LintOptions, RULES, apply_fixes, lint};
use sqlotus_core::schemafolder::{SNAPSHOT_FILE, Snapshot, catalog};

const HELP: &str = "\
sqlotus-lint — check SQL files with the Sqlotus code analysis

USAGE:
    sqlotus-lint [OPTIONS] <PATH>...

PATH is a .sql file or a folder (searched recursively for .sql files).

OPTIONS:
    -d, --dialect <mssql|postgres|mysql>  SQL dialect (default: mssql, or the schema folder's)
        --schema <FOLDER>      A Sqlotus schema folder: also checks table and column names
        --database <NAME>      Database name for a MySQL schema folder (default: the folder name)
        --disable <CODES>      Rules to switch off, e.g. SQ002,SQ005
        --format <FORMAT>      text (default), json, or github (annotations for GitHub Actions)
        --fail-on <LEVEL>      error, warning (default), info, or never: the exit code is 1 when a
                               problem at or above this level is found
        --fix                  Apply the safe quick fixes in place, then report what is left
        --list-rules           List the rules and exit
    -h, --help                 Show this help
    -V, --version              Show the version

In a file, `-- sqlotus-disable-line SQ002`, `-- sqlotus-disable-next-line SQ001` and
`-- sqlotus-disable SQ005` ... `-- sqlotus-enable` switch rules off (no codes: every rule).

EXIT CODES: 0 nothing at the --fail-on level, 1 problems found, 2 usage or file error.
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Text,
    Json,
    Github,
}

#[derive(Debug)]
struct Args {
    dialect: Option<Dialect>,
    schema: Option<PathBuf>,
    database: Option<String>,
    options: LintOptions,
    format: Format,
    /// Lowest severity that fails the run; None = never.
    fail_on: Option<Severity>,
    fix: bool,
    list_rules: bool,
    paths: Vec<PathBuf>,
}

fn parse_dialect(s: &str) -> Result<Dialect, String> {
    match s.to_ascii_lowercase().as_str() {
        "mssql" | "sqlserver" | "tsql" => Ok(Dialect::MsSql),
        "postgres" | "postgresql" | "pg" => Ok(Dialect::Postgres),
        "mysql" | "mariadb" => Ok(Dialect::MySql),
        _ => Err(format!("unknown dialect '{s}' (use mssql, postgres or mysql)")),
    }
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::Error => 3,
        Severity::Warning => 2,
        Severity::Info => 1,
    }
}

fn severity_word(s: Severity) -> &'static str {
    match s {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
    }
}

fn parse_args(raw: &[String]) -> Result<Option<Args>, String> {
    let mut a = Args {
        dialect: None,
        schema: None,
        database: None,
        options: LintOptions::default(),
        format: Format::Text,
        fail_on: Some(Severity::Warning),
        fix: false,
        list_rules: false,
        paths: vec![],
    };
    let mut it = raw.iter();
    while let Some(arg) = it.next() {
        let (name, inline) = match arg.split_once('=') {
            Some((n, v)) if n.starts_with("--") => (n, Some(v.to_string())),
            _ => (arg.as_str(), None),
        };
        let mut value = |what: &str| -> Result<String, String> {
            inline
                .clone()
                .or_else(|| it.next().cloned())
                .ok_or_else(|| format!("{what} needs a value"))
        };
        match name {
            "-h" | "--help" => return Ok(None),
            "-V" | "--version" => {
                println!("sqlotus-lint {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0);
            }
            "-d" | "--dialect" => a.dialect = Some(parse_dialect(&value(name)?)?),
            "--schema" => a.schema = Some(PathBuf::from(value(name)?)),
            "--database" => a.database = Some(value(name)?),
            "--disable" => a.options.disabled.extend(
                value(name)?
                    .split([',', ' '])
                    .filter(|c| !c.is_empty())
                    .map(str::to_ascii_uppercase),
            ),
            "--format" => {
                a.format = match value(name)?.as_str() {
                    "text" => Format::Text,
                    "json" => Format::Json,
                    "github" => Format::Github,
                    other => return Err(format!("unknown format '{other}' (use text, json or github)")),
                }
            }
            "--fail-on" => {
                a.fail_on = match value(name)?.as_str() {
                    "error" => Some(Severity::Error),
                    "warning" => Some(Severity::Warning),
                    "info" => Some(Severity::Info),
                    "never" => None,
                    other => return Err(format!("unknown level '{other}' (use error, warning, info or never)")),
                }
            }
            "--fix" => a.fix = true,
            "--list-rules" => a.list_rules = true,
            s if s.starts_with('-') && s.len() > 1 => return Err(format!("unknown option '{s}' (see --help)")),
            _ => a.paths.push(PathBuf::from(arg)),
        }
    }
    if a.paths.is_empty() && !a.list_rules {
        return Err("no files or folders given (see --help)".into());
    }
    Ok(Some(a))
}

/// `.sql` files under `path`, sorted; hidden folders (`.git`) are skipped.
fn collect(path: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if meta.is_file() {
        out.push(path.to_path_buf());
        return Ok(());
    }
    let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|n| n.to_str()).unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" {
            continue;
        }
        if p.is_dir() {
            collect(&p, out)?;
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("sql")) {
            out.push(p);
        }
    }
    Ok(())
}

/// The text of a file: UTF-8 (BOM removed) or UTF-16 LE with BOM, as some
/// editors save .sql files. Returns the text and whether it was UTF-16.
fn read_text(path: &Path) -> Result<(String, bool), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        let units: Vec<u16> = rest.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        return Ok((String::from_utf16_lossy(&units), true));
    }
    let text = String::from_utf8_lossy(bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&bytes)).into_owned();
    Ok((text, false))
}

fn write_text(path: &Path, text: &str, utf16: bool) -> Result<(), String> {
    let bytes: Vec<u8> = if utf16 {
        [0xFF, 0xFE]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect()
    } else {
        text.as_bytes().to_vec()
    };
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// 1-based line and column (in characters) of a byte offset.
fn line_col(src: &str, offset: usize) -> (usize, usize) {
    let before = &src[..offset.min(src.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().unwrap_or("").chars().count() + 1;
    (line, col)
}

/// GitHub Actions workflow command escaping.
fn gh_escape(s: &str, property: bool) -> String {
    let s = s.replace('%', "%25").replace('\r', "%0D").replace('\n', "%0A");
    if property {
        s.replace(':', "%3A").replace(',', "%2C")
    } else {
        s
    }
}

fn load_catalog(args: &Args) -> Result<(Option<Catalog>, Option<Dialect>), String> {
    let Some(folder) = &args.schema else {
        return Ok((None, None));
    };
    let file = folder.join(SNAPSHOT_FILE);
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let snap = Snapshot::from_json(&text)?;
    let database = args.database.clone().unwrap_or_else(|| {
        std::fs::canonicalize(folder)
            .ok()
            .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_default()
    });
    Ok((Some(catalog(&snap.model, snap.dialect, &database)), Some(snap.dialect)))
}

fn run(raw: &[String], out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let args = match parse_args(raw) {
        Ok(Some(a)) => a,
        Ok(None) => {
            let _ = write!(out, "{HELP}");
            return 0;
        }
        Err(e) => {
            let _ = writeln!(err, "sqlotus-lint: {e}");
            return 2;
        }
    };
    if args.list_rules {
        for r in RULES {
            let mut notes = Vec::new();
            if r.needs_catalog {
                notes.push("needs --schema".to_string());
            }
            if let Some(d) = r.only {
                notes.push(format!("{d:?} only").replace("MsSql", "SQL Server"));
            }
            let notes = if notes.is_empty() {
                String::new()
            } else {
                format!("  ({})", notes.join(", "))
            };
            let _ = writeln!(out, "{}  {:<7}  {}{notes}", r.code, severity_word(r.severity), r.title);
        }
        return 0;
    }
    let (catalog, folder_dialect) = match load_catalog(&args) {
        Ok(x) => x,
        Err(e) => {
            let _ = writeln!(err, "sqlotus-lint: {e}");
            return 2;
        }
    };
    let dialect = args.dialect.or(folder_dialect).unwrap_or(Dialect::MsSql);
    let mut files = Vec::new();
    for p in &args.paths {
        if let Err(e) = collect(p, &mut files) {
            let _ = writeln!(err, "sqlotus-lint: {e}");
            return 2;
        }
    }
    let mut failed = false;
    let mut json = Vec::new();
    let (mut count, mut fixed) = (0usize, 0usize);
    for file in &files {
        let (mut text, utf16) = match read_text(file) {
            Ok(x) => x,
            Err(e) => {
                let _ = writeln!(err, "sqlotus-lint: {e}");
                return 2;
            }
        };
        let mut found: Vec<Diagnostic> = lint(&text, dialect, catalog.as_ref(), &args.options);
        if args.fix && found.iter().any(|d| d.fix.is_some()) {
            let (new_text, n) = apply_fixes(&text, &found);
            if let Err(e) = write_text(file, &new_text, utf16) {
                let _ = writeln!(err, "sqlotus-lint: {e}");
                return 2;
            }
            fixed += n;
            text = new_text;
            found = lint(&text, dialect, catalog.as_ref(), &args.options);
        }
        let shown = file.display().to_string().replace('\\', "/");
        for d in &found {
            count += 1;
            if args
                .fail_on
                .is_some_and(|level| severity_rank(d.severity) >= severity_rank(level))
            {
                failed = true;
            }
            let (line, col) = line_col(&text, d.start);
            let (end_line, end_col) = line_col(&text, d.end);
            match args.format {
                Format::Text => {
                    let fix = if d.fix.is_some() { " [fixable]" } else { "" };
                    let _ = writeln!(
                        out,
                        "{shown}:{line}:{col}: {} {}: {}{fix}",
                        severity_word(d.severity),
                        d.code,
                        d.message
                    );
                }
                Format::Github => {
                    let kind = match d.severity {
                        Severity::Error => "error",
                        Severity::Warning => "warning",
                        Severity::Info => "notice",
                    };
                    let _ = writeln!(
                        out,
                        "::{kind} file={},line={line},col={col},endLine={end_line},endColumn={end_col},title={}::{}",
                        gh_escape(&shown, true),
                        gh_escape(&d.code, true),
                        gh_escape(&d.message, false)
                    );
                }
                Format::Json => json.push(serde_json::json!({
                    "file": shown,
                    "line": line,
                    "column": col,
                    "endLine": end_line,
                    "endColumn": end_col,
                    "severity": severity_word(d.severity),
                    "code": d.code,
                    "message": d.message,
                    "fixable": d.fix.is_some(),
                })),
            }
        }
    }
    match args.format {
        Format::Json => {
            let _ = writeln!(out, "{}", serde_json::to_string_pretty(&json).unwrap_or_default());
        }
        Format::Text => {
            let fixed = if args.fix {
                format!(", {fixed} fixed")
            } else {
                String::new()
            };
            let _ = writeln!(out, "{} file(s), {count} problem(s){fixed}", files.len());
        }
        Format::Github => {}
    }
    u8::from(failed)
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let code = run(&raw, &mut std::io::stdout().lock(), &mut std::io::stderr().lock());
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn arguments() {
        let a = parse_args(&s(&[
            "-d",
            "postgres",
            "--disable=SQ002,sq005",
            "--fail-on",
            "never",
            "x.sql",
        ]))
        .unwrap()
        .unwrap();
        assert_eq!(a.dialect, Some(Dialect::Postgres));
        assert_eq!(a.options.disabled, vec!["SQ002", "SQ005"]);
        assert_eq!(a.fail_on, None);
        assert_eq!(a.paths, vec![PathBuf::from("x.sql")]);
        assert!(parse_args(&s(&["--help"])).unwrap().is_none());
        assert!(parse_args(&s(&[])).unwrap_err().contains("no files"));
        assert!(
            parse_args(&s(&["--format", "xml", "a"]))
                .unwrap_err()
                .contains("unknown format")
        );
        assert!(parse_args(&s(&["--nope", "a"])).unwrap_err().contains("unknown option"));
        assert!(parse_args(&s(&["--dialect"])).unwrap_err().contains("needs a value"));
        assert!(
            parse_args(&s(&["-d", "oracle", "a"]))
                .unwrap_err()
                .contains("unknown dialect")
        );
    }

    #[test]
    fn positions_and_escaping() {
        // byte 6 is the `e` after the two-byte `đ`
        assert_eq!(line_col("ab\ncđe", 6), (2, 3));
        assert_eq!(line_col("x", 0), (1, 1));
        assert_eq!(gh_escape("a:b,c%\n", true), "a%3Ab%2Cc%25%0A");
        assert_eq!(gh_escape("a:b", false), "a:b");
    }
}
