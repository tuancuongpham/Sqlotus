//! Runs the `sqlotus-lint` binary on files in a temporary folder.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let p = std::env::temp_dir().join(format!("sqlotus-lint-{name}-{nanos}"));
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }

    fn file(&self, rel: &str, text: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        p
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn lint(args: &[&str], dir: &Path) -> (Output, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sqlotus-lint"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    (out, text)
}

#[test]
fn reports_and_exit_codes() {
    let d = Dir::new("report");
    d.file("q/orders.sql", "SELECT * FROM t\nDELETE FROM t\n");
    d.file("q/ok.sql", "SELECT a FROM t WHERE b = 1\n");
    d.file(".git/ignored.sql", "DELETE FROM x\n");
    d.file("notes.txt", "DELETE FROM x\n");
    let (out, text) = lint(&["q", "."], &d.0);
    // a folder given twice is fine; .git and non-.sql files are skipped
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(
        text.contains("q/orders.sql:2:1: warning SQ001: DELETE without a WHERE clause"),
        "{text}"
    );
    assert!(text.contains("q/orders.sql:1:8: info SQ002"), "{text}");
    assert!(!text.contains("ignored.sql"), "{text}");
    // only an info problem: passes at the default level, fails with --fail-on info
    let d = Dir::new("info");
    d.file("a.sql", "SELECT * FROM t\n");
    assert_eq!(lint(&["a.sql"], &d.0).0.status.code(), Some(0));
    assert_eq!(lint(&["--fail-on", "info", "a.sql"], &d.0).0.status.code(), Some(1));
    // switched off and suppressed
    d.file(
        "b.sql",
        "DELETE FROM t -- sqlotus-disable-line SQ001\nSELECT * FROM t\n",
    );
    let (out, text) = lint(&["--disable", "SQ002", "--fail-on", "info", "b.sql"], &d.0);
    assert_eq!(out.status.code(), Some(0), "{text}");
    assert!(text.contains("1 file(s), 0 problem(s)"), "{text}");
    // usage errors
    assert_eq!(lint(&["missing.sql"], &d.0).0.status.code(), Some(2));
    assert_eq!(lint(&["--format", "xml", "a.sql"], &d.0).0.status.code(), Some(2));
}

#[test]
fn json_and_github_output() {
    let d = Dir::new("formats");
    d.file("a.sql", "UPDATE t SET a = 1\n");
    let (_, text) = lint(&["--format", "json", "a.sql"], &d.0);
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v[0]["code"], "SQ001");
    assert_eq!(v[0]["line"], 1);
    assert_eq!(v[0]["severity"], "warning");
    let (_, text) = lint(&["--format", "github", "a.sql"], &d.0);
    assert!(
        text.starts_with("::warning file=a.sql,line=1,col=1,endLine=1,endColumn=7,title=SQ001::UPDATE without"),
        "{text}"
    );
}

#[test]
fn fixes_in_place_keeping_the_encoding() {
    let d = Dir::new("fix");
    let utf8 = d.file("a.sql", "SELECT a FROM t WHERE b = NULL AND c = 'Ý'\n");
    // a UTF-16 file with a BOM, as some editors save .sql files
    let utf16 = d.0.join("b.sql");
    let text = "DECLARE @unused int = 1;\nSELECT 1;\n";
    let bytes: Vec<u8> = [0xFF, 0xFE]
        .into_iter()
        .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    std::fs::write(&utf16, bytes).unwrap();
    let (out, report) = lint(&["--fix", "--fail-on", "info", "."], &d.0);
    assert!(report.contains("3 fixed"), "{report}");
    assert_eq!(out.status.code(), Some(0), "{report}");
    assert_eq!(
        std::fs::read_to_string(&utf8).unwrap(),
        "SELECT a FROM t WHERE b IS NULL AND c = N'Ý'\n"
    );
    let raw = std::fs::read(&utf16).unwrap();
    assert_eq!(&raw[..2], &[0xFF, 0xFE], "still UTF-16");
    let units: Vec<u16> = raw[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_le_bytes(*c))
        .collect();
    assert_eq!(String::from_utf16(&units).unwrap(), "SELECT 1;\n");
}

#[test]
fn encodings_are_kept_or_left_alone() {
    let d = Dir::new("enc");
    let src = "SELECT a FROM t WHERE b = NULL\n";
    let fixed = "SELECT a FROM t WHERE b IS NULL\n";
    // UTF-16 big-endian and UTF-8 with a BOM are written back as they were
    let be: Vec<u8> = [0xFE, 0xFF]
        .into_iter()
        .chain(src.encode_utf16().flat_map(u16::to_be_bytes))
        .collect();
    std::fs::write(d.0.join("be.sql"), be).unwrap();
    std::fs::write(d.0.join("bom.sql"), [&[0xEF, 0xBB, 0xBF], src.as_bytes()].concat()).unwrap();
    // Windows-1252 (é = 0xE9): checked, but never rewritten
    let latin = d.0.join("latin.sql");
    let latin_bytes = b"SELECT a FROM t WHERE b = NULL -- caf\xE9\n".to_vec();
    std::fs::write(&latin, &latin_bytes).unwrap();
    let (out, report) = lint(&["--fix", "."], &d.0);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(report.contains("2 fixed"), "{report}");
    assert!(
        report.contains("latin.sql:1:"),
        "the problem is still reported: {report}"
    );
    assert!(
        stderr.contains("latin.sql: not UTF-8 or UTF-16 text, fixes not written"),
        "{stderr}"
    );
    assert_eq!(out.status.code(), Some(1), "{report}");
    let raw = std::fs::read(d.0.join("be.sql")).unwrap();
    assert_eq!(&raw[..2], &[0xFE, 0xFF], "still UTF-16 BE");
    let units: Vec<u16> = raw[2..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| u16::from_be_bytes(*c))
        .collect();
    assert_eq!(String::from_utf16(&units).unwrap(), fixed);
    assert_eq!(
        std::fs::read(d.0.join("bom.sql")).unwrap(),
        [&[0xEF, 0xBB, 0xBF], fixed.as_bytes()].concat()
    );
    assert_eq!(std::fs::read(&latin).unwrap(), latin_bytes, "left as it was");
}

#[test]
fn links_to_folders_are_not_followed() {
    let d = Dir::new("links");
    d.file("q/a.sql", "DELETE FROM t\n");
    // a link back to the parent folder would loop forever if followed
    let link = d.0.join("q").join("loop");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&d.0, &link).is_ok()
        || Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&d.0)
            .output()
            .is_ok_and(|o| o.status.success());
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&d.0, &link).is_ok();
    if !made {
        eprintln!("cannot make a folder link here, skipping");
        return;
    }
    let (out, report) = lint(&["."], &d.0);
    assert_eq!(out.status.code(), Some(1), "{report}");
    assert!(report.contains("1 file(s), 1 problem(s)"), "{report}");
    // remove the link first: removing the folder must not follow it
    #[cfg(windows)]
    let _ = std::fs::remove_dir(&link);
    #[cfg(unix)]
    let _ = std::fs::remove_file(&link);
}

#[test]
fn schema_folder_checks_names() {
    let d = Dir::new("schema");
    let snapshot = serde_json::json!({
        "format": 1,
        "dialect": "postgres",
        "model": {
            "tables": [{
                "schema": "public", "name": "orders",
                "columns": [{"name": "id", "dataType": "integer"}, {"name": "total", "dataType": "numeric(12,2)"}]
            }],
            "modules": []
        }
    });
    d.file("db/sqlotus-schema.json", &snapshot.to_string());
    d.file("q.sql", "SELECT o.totl FROM orders o;\nSELECT * FROM ordrs;\n");
    let (out, text) = lint(&["--schema", "db", "q.sql"], &d.0);
    assert_eq!(out.status.code(), Some(1), "{text}");
    assert!(text.contains("SQ007"), "unknown column: {text}");
    assert!(text.contains("SQ006"), "unknown table: {text}");
    // without the schema folder, names are not checked
    let (_, text) = lint(&["-d", "postgres", "q.sql"], &d.0);
    assert!(!text.contains("SQ006") && !text.contains("SQ007"), "{text}");
    // a broken snapshot is a usage error
    d.file("bad/sqlotus-schema.json", "{");
    assert_eq!(lint(&["--schema", "bad", "q.sql"], &d.0).0.status.code(), Some(2));
}

#[test]
fn lists_rules() {
    let d = Dir::new("rules");
    let (out, text) = lint(&["--list-rules"], &d.0);
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(text.lines().count(), 11);
    assert!(
        text.contains("SQ006  warning  Unknown table, view or routine  (needs --schema)"),
        "{text}"
    );
    assert!(text.contains("(SQL Server only)"), "{text}");
}
