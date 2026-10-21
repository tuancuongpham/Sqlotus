//! Source control of a database schema: the database is linked to a folder
//! (usually a git repository) holding one `.sql` file per object and the
//! snapshot Sqlotus compares with. Commit writes database changes into the
//! folder and runs `git commit`; Get latest builds a script that brings the
//! folder's changes into the database, for review — it is never run here.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use sqlotus_core::Dialect;
use sqlotus_core::schemadiff::{self, CompareOptions, SchemaModel, SyncScript};
use sqlotus_core::schemafolder::{self, OBJECT_FOLDERS, POST_SCRIPT, PRE_SCRIPT, SNAPSHOT_FILE, Snapshot, SyncItem};
use tauri::State;

use crate::commands::{CmdResult, err, no_panic};
use crate::compare::load_model;
use crate::state::AppState;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Link {
    connection_id: String,
    database: String,
    folder: String,
}

const LINKS: &str = "sc.links";

fn base_key(connection_id: &str, database: &str) -> String {
    format!("sc.base:{connection_id}/{database}")
}

fn links(state: &AppState) -> CmdResult<Vec<Link>> {
    Ok(state
        .store
        .get_setting::<Vec<Link>>(LINKS)
        .map_err(err)?
        .unwrap_or_default())
}

fn find_link(state: &AppState, connection_id: &str, database: &str) -> CmdResult<Option<Link>> {
    Ok(links(state)?
        .into_iter()
        .find(|l| l.connection_id == connection_id && l.database == database))
}

fn require_link(state: &AppState, connection_id: &str, database: &str) -> CmdResult<Link> {
    find_link(state, connection_id, database)?.ok_or_else(|| format!("{database} is not linked to a folder yet."))
}

// ---------------------------------------------------------------------------
// git
// ---------------------------------------------------------------------------

/// Run git in `folder`; stdout on success, the error text otherwise.
fn git(folder: &Path, args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.current_dir(folder).args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "Git is not installed or not on PATH (https://git-scm.com).".to_string()
        } else {
            format!("Could not run git: {e}")
        }
    })?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    if out.status.success() {
        Ok(if stdout.is_empty() { stderr } else { stdout })
    } else {
        // git puts "nothing to commit" on stdout and warnings on stderr: keep both
        Err(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or_default(),
            format!("{stderr}\n{stdout}").trim()
        ))
    }
}

fn is_git(folder: &Path) -> bool {
    git(folder, &["rev-parse", "--is-inside-work-tree"]).is_ok_and(|o| o == "true")
}

fn branch(folder: &Path) -> Option<String> {
    // symbolic-ref also works before the first commit
    git(folder, &["symbolic-ref", "--short", "HEAD"]).ok()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> CmdResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(err)
}

// ---------------------------------------------------------------------------
// folder files
// ---------------------------------------------------------------------------

const PRE_TEMPLATE: &str = "-- Runs before the statements of \"Get latest\" (for example to move data before a column is\n-- dropped, or to rename a column instead of dropping and adding it). Comments only = nothing to run.\n";
const POST_TEMPLATE: &str = "-- Runs after the statements of \"Get latest\" (for example to fill a new column).\n-- Comments only = nothing to run.\n";

fn read_snapshot(folder: &Path, dialect: Dialect) -> CmdResult<Option<SchemaModel>> {
    let path = folder.join(SNAPSHOT_FILE);
    if !path.exists() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let snap = Snapshot::from_json(&text)?;
    if snap.dialect != dialect {
        return Err(format!(
            "The folder holds a {:?} schema, but this database is {:?}.",
            snap.dialect, dialect
        ));
    }
    Ok(Some(snap.model))
}

/// `.sql` files under the object folders, as `Tables/dbo.X.sql`.
fn object_files_on_disk(folder: &Path) -> Vec<String> {
    let mut out = Vec::new();
    for dir in OBJECT_FOLDERS {
        let Ok(entries) = std::fs::read_dir(folder.join(dir)) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("sql")) {
                out.push(format!("{dir}/{}", e.file_name().to_string_lossy()));
            }
        }
    }
    out.sort();
    out
}

/// Files that do not match the snapshot: edited, missing, or added by hand.
/// Sqlotus compares the snapshot, so such edits are not picked up.
fn hand_edited(folder: &Path, model: &SchemaModel, dialect: Dialect) -> Vec<String> {
    let expected = schemafolder::object_files(model, dialect);
    let names: HashSet<String> = expected.iter().map(|(p, _)| p.to_lowercase()).collect();
    let mut out = Vec::new();
    for (path, text) in &expected {
        match std::fs::read_to_string(folder.join(path)) {
            // git may check files out with CRLF line ends
            Ok(on_disk) if on_disk.replace("\r\n", "\n") == *text => {}
            Ok(_) => out.push(format!("{path} (edited)")),
            Err(_) => out.push(format!("{path} (missing)")),
        }
    }
    for path in object_files_on_disk(folder) {
        if !names.contains(&path.to_lowercase()) {
            out.push(format!("{path} (not written by Sqlotus)"));
        }
    }
    out
}

/// Write the snapshot and the per-object files of `model`; remove object
/// files of objects that are gone. Returns (files written, files removed).
fn write_folder(folder: &Path, dialect: Dialect, model: &SchemaModel) -> Result<(usize, usize), String> {
    let io = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
    let snapshot = Snapshot::new(dialect, model.clone());
    let snap_path = folder.join(SNAPSHOT_FILE);
    std::fs::write(&snap_path, snapshot.to_json()).map_err(|e| io(&snap_path, e))?;
    let files = schemafolder::object_files(&snapshot.model, dialect);
    let keep: HashSet<String> = files.iter().map(|(p, _)| p.to_lowercase()).collect();
    let mut written = 0;
    for (rel, text) in &files {
        let path = folder.join(rel);
        let same = std::fs::read_to_string(&path).is_ok_and(|t| t.replace("\r\n", "\n") == *text);
        if same {
            continue;
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io(dir, e))?;
        }
        std::fs::write(&path, text).map_err(|e| io(&path, e))?;
        written += 1;
    }
    let mut removed = 0;
    for rel in object_files_on_disk(folder) {
        if !keep.contains(&rel.to_lowercase()) {
            let path = folder.join(&rel);
            std::fs::remove_file(&path).map_err(|e| io(&path, e))?;
            removed += 1;
        }
    }
    for dir in OBJECT_FOLDERS {
        let _ = std::fs::remove_dir(folder.join(dir)); // only when empty
    }
    Ok((written, removed))
}

/// A pre/post-deploy script with something to run (not only comments).
fn deploy_script(folder: &Path, rel: &str) -> Option<String> {
    let text = std::fs::read_to_string(folder.join(rel)).ok()?;
    text.lines()
        .any(|l| {
            let l = l.trim();
            !l.is_empty() && !l.starts_with("--")
        })
        .then(|| text.trim().to_string())
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkInfo {
    folder: String,
    git: bool,
    branch: Option<String>,
    has_snapshot: bool,
}

fn link_info(folder: &Path) -> LinkInfo {
    let git_repo = is_git(folder);
    LinkInfo {
        folder: folder.display().to_string(),
        git: git_repo,
        branch: if git_repo { branch(folder) } else { None },
        has_snapshot: folder.join(SNAPSHOT_FILE).exists(),
    }
}

#[tauri::command]
pub async fn sc_link_info(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
) -> CmdResult<Option<LinkInfo>> {
    let Some(link) = find_link(&state, &connection_id, &database)? else {
        return Ok(None);
    };
    Ok(Some(blocking(move || link_info(Path::new(&link.folder))).await?))
}

/// Link the database to `folder`. The folder must be empty (dot files and a
/// README aside) or already hold a schema snapshot, so a commit never takes
/// unrelated files with it.
#[tauri::command]
pub async fn sc_link(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    folder: String,
    init_git: bool,
) -> CmdResult<LinkInfo> {
    let cfg = state
        .store
        .get_connection(&connection_id)
        .map_err(err)?
        .ok_or("The connection no longer exists.")?;
    let path = PathBuf::from(folder.trim());
    if !path.is_absolute() {
        return Err("Choose a folder (a full path).".into());
    }
    std::fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    if read_snapshot(&path, cfg.dialect)?.is_none() {
        let others: Vec<String> = std::fs::read_dir(&path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.starts_with('.') && !n.to_lowercase().starts_with("readme"))
            .collect();
        if !others.is_empty() {
            return Err(format!(
                "{} already holds other files ({}). Choose an empty folder, or one that already has a Sqlotus schema ({SNAPSHOT_FILE}).",
                path.display(),
                others.into_iter().take(3).collect::<Vec<_>>().join(", ")
            ));
        }
    }
    let p = path.clone();
    let info = blocking(move || -> CmdResult<LinkInfo> {
        if init_git && !is_git(&p) {
            git(&p, &["init"])?;
        }
        for (rel, text) in [(PRE_SCRIPT, PRE_TEMPLATE), (POST_SCRIPT, POST_TEMPLATE)] {
            let f = p.join(rel);
            if !f.exists() {
                if let Some(dir) = f.parent() {
                    std::fs::create_dir_all(dir).map_err(err)?;
                }
                std::fs::write(&f, text).map_err(err)?;
            }
        }
        Ok(link_info(&p))
    })
    .await??;
    let mut all = links(&state)?;
    all.retain(|l| !(l.connection_id == connection_id && l.database == database));
    all.push(Link {
        connection_id: connection_id.clone(),
        database: database.clone(),
        folder: path.display().to_string(),
    });
    state.store.set_setting(LINKS, &all).map_err(err)?;
    // a new link starts without a common base
    state
        .store
        .set_setting::<Option<SchemaModel>>(&base_key(&connection_id, &database), &None)
        .map_err(err)?;
    Ok(info)
}

#[tauri::command]
pub fn sc_unlink(state: State<'_, AppState>, connection_id: String, database: String) -> CmdResult<()> {
    let mut all = links(&state)?;
    all.retain(|l| !(l.connection_id == connection_id && l.database == database));
    state.store.set_setting(LINKS, &all).map_err(err)?;
    state
        .store
        .set_setting::<Option<SchemaModel>>(&base_key(&connection_id, &database), &None)
        .map_err(err)
}

/// A status kept for Commit and Get latest (the latest few).
struct Pending {
    connection_id: String,
    database: String,
    folder: PathBuf,
    dialect: Dialect,
    options: CompareOptions,
    db: SchemaModel,
    folder_model: SchemaModel,
    base: SchemaModel,
}

#[derive(Default)]
pub struct SourceControlStore(Mutex<HashMap<String, Pending>>);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusDto {
    id: String,
    folder: String,
    dialect: Dialect,
    items: Vec<SyncItem>,
    in_sync: usize,
    skipped: Vec<String>,
    hand_edited: Vec<String>,
    git: bool,
    branch: Option<String>,
    /// Nothing committed to the folder yet.
    first_commit: bool,
    /// A common version is known; without one, differences cannot be told
    /// apart and are all conflicts.
    has_base: bool,
}

/// Compare the database with the linked folder.
#[tauri::command]
pub async fn sc_status(
    state: State<'_, AppState>,
    store: State<'_, SourceControlStore>,
    connection_id: String,
    database: String,
) -> CmdResult<StatusDto> {
    let link = require_link(&state, &connection_id, &database)?;
    let cfg = state
        .store
        .get_connection(&connection_id)
        .map_err(err)?
        .ok_or("The connection no longer exists.")?;
    let dialect = cfg.dialect;
    let folder = PathBuf::from(&link.folder);
    let db = load_model(&state, &connection_id, &database, "database").await?;
    let snapshot = read_snapshot(&folder, dialect)?;
    let first_commit = snapshot.is_none();
    let folder_model = snapshot.unwrap_or_default();
    let base: Option<SchemaModel> = state
        .store
        .get_setting::<Option<SchemaModel>>(&base_key(&connection_id, &database))
        .map_err(err)?
        .flatten();
    let options = CompareOptions {
        ignore_whitespace: true,
        ignore_case: false,
        case_insensitive_names: dialect != Dialect::Postgres,
    };
    let has_base = base.is_some();
    let status = no_panic("source control", || {
        schemafolder::sync_status(&db, &folder_model, base.as_ref(), dialect, &options)
    })?;
    state
        .store
        .set_setting(&base_key(&connection_id, &database), &Some(&status.base))
        .map_err(err)?;
    let (f, m) = (folder.clone(), folder_model.clone());
    let (edited, info) = blocking(move || {
        let edited = if first_commit {
            vec![]
        } else {
            hand_edited(&f, &m, dialect)
        };
        (edited, link_info(&f))
    })
    .await?;
    let id = format!(
        "sc-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default()
    );
    {
        let mut map = store.0.lock().map_err(err)?;
        if map.len() >= 4 {
            map.clear();
        }
        map.insert(
            id.clone(),
            Pending {
                connection_id,
                database,
                folder: folder.clone(),
                dialect,
                options,
                db,
                folder_model,
                base: status.base.clone(),
            },
        );
    }
    Ok(StatusDto {
        id,
        folder: folder.display().to_string(),
        dialect,
        items: status.items,
        in_sync: status.in_sync,
        skipped: status.skipped,
        hand_edited: edited,
        git: info.git,
        branch: info.branch,
        first_commit,
        has_base,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitResult {
    written: usize,
    removed: usize,
    /// Short hash of the new commit.
    commit: Option<String>,
    note: String,
}

/// Write the chosen database objects into the folder and commit them.
#[tauri::command]
pub async fn sc_commit(
    state: State<'_, AppState>,
    store: State<'_, SourceControlStore>,
    id: String,
    keys: Vec<String>,
    message: String,
) -> CmdResult<CommitResult> {
    if keys.is_empty() {
        return Err("Choose at least one change to commit.".into());
    }
    let p = store
        .0
        .lock()
        .map_err(err)?
        .remove(&id)
        .ok_or("This status is out of date; refresh and try again.")?;
    let (new_folder, new_base) = schemafolder::commit(&p.db, &p.folder_model, &p.base, &keys);
    let (folder, dialect) = (p.folder.clone(), p.dialect);
    let message = message.trim().to_string();
    let result = blocking(move || -> CmdResult<CommitResult> {
        let git_repo = is_git(&folder);
        if git_repo && message.is_empty() {
            return Err("Write a commit message first.".into());
        }
        let (written, removed) = write_folder(&folder, dialect, &new_folder)?;
        if !git_repo {
            return Ok(CommitResult {
                written,
                removed,
                commit: None,
                note: "Files written. The folder is not a git repository, so nothing was committed.".into(),
            });
        }
        git(&folder, &["add", "-A", "--", "."])?;
        match git(&folder, &["commit", "-m", &message, "--", "."]) {
            Ok(_) => Ok(CommitResult {
                written,
                removed,
                commit: git(&folder, &["rev-parse", "--short", "HEAD"]).ok(),
                note: String::new(),
            }),
            Err(e) if e.contains("nothing to commit") || e.contains("nothing added to commit") => Ok(CommitResult {
                written,
                removed,
                commit: None,
                note: "The folder already had these changes; nothing new to commit.".into(),
            }),
            Err(e) => Err(format!("The files were written, but {e}")),
        }
    })
    .await??;
    state
        .store
        .set_setting(&base_key(&p.connection_id, &p.database), &Some(&new_base))
        .map_err(err)?;
    Ok(result)
}

/// The script that brings the chosen folder changes into the database,
/// between the pre- and post-deploy scripts. It opens in a tab for review.
#[tauri::command]
pub fn sc_get_latest_script(
    store: State<'_, SourceControlStore>,
    id: String,
    keys: Vec<String>,
) -> CmdResult<SyncScript> {
    if keys.is_empty() {
        return Err("Choose at least one change to get.".into());
    }
    let map = store.0.lock().map_err(err)?;
    let p = map
        .get(&id)
        .ok_or("This status is out of date; refresh and try again.")?;
    let mut script = no_panic("source control", || {
        let diff = schemadiff::compare(&p.folder_model, &p.db, p.dialect, &p.options);
        schemadiff::sync_script(&p.folder_model, &p.db, &diff, &keys, p.dialect, &p.options)
    })?;
    let go = if p.dialect == Dialect::MsSql { "\nGO\n" } else { "\n" };
    if let Some(pre) = deploy_script(&p.folder, PRE_SCRIPT) {
        script.script = format!("-- pre-deploy ({PRE_SCRIPT})\n{pre}{go}\n{}", script.script);
    }
    if let Some(post) = deploy_script(&p.folder, POST_SCRIPT) {
        script.script = format!(
            "{}\n{go}-- post-deploy ({POST_SCRIPT})\n{post}\n",
            script.script.trim_end()
        );
    }
    Ok(script)
}

/// `pull`, `push` or `log` in the linked folder.
#[tauri::command]
pub async fn sc_git(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    action: String,
) -> CmdResult<String> {
    let link = require_link(&state, &connection_id, &database)?;
    let folder = PathBuf::from(link.folder);
    let args: Vec<&'static str> = match action.as_str() {
        "pull" => vec!["pull", "--ff-only"],
        "push" => vec!["push"],
        "log" => vec!["log", "-n", "15", "--format=%h%x09%an%x09%ar%x09%s", "--", "."],
        _ => return Err(format!("unknown action {action}")),
    };
    blocking(move || {
        if !is_git(&folder) {
            return Err("The folder is not a git repository.".to_string());
        }
        git(&folder, &args)
    })
    .await?
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlotus_core::catalog::ObjectKind;
    use sqlotus_core::schemadiff::{ColumnDef, ModuleDef, TableDef};

    struct Temp(PathBuf);
    impl Temp {
        fn new(name: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p = std::env::temp_dir().join(format!("sqlotus-sc-{name}-{nanos}"));
            std::fs::create_dir_all(&p).unwrap();
            Temp(p)
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn model(tables: &[&str], views: &[&str]) -> SchemaModel {
        SchemaModel {
            tables: tables
                .iter()
                .map(|n| TableDef {
                    schema: "dbo".into(),
                    name: (*n).into(),
                    columns: vec![ColumnDef {
                        name: "Id".into(),
                        data_type: "int".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                })
                .collect(),
            modules: views
                .iter()
                .map(|n| ModuleDef {
                    schema: "dbo".into(),
                    name: (*n).into(),
                    kind: ObjectKind::View,
                    definition: format!("CREATE VIEW dbo.{n} AS SELECT 1 AS One"),
                    table: None,
                })
                .collect(),
            unreadable: vec![],
        }
    }

    #[test]
    fn writes_files_and_finds_hand_edits() {
        let t = Temp::new("files");
        let ms = Dialect::MsSql;
        let (written, removed) = write_folder(&t.0, ms, &model(&["A", "B"], &["v"])).unwrap();
        assert_eq!((written, removed), (3, 0));
        assert!(t.0.join("Tables/dbo.A.sql").exists() && t.0.join("Views/dbo.v.sql").exists());
        assert_eq!(read_snapshot(&t.0, ms).unwrap().unwrap().tables.len(), 2);
        assert!(
            read_snapshot(&t.0, Dialect::Postgres)
                .unwrap_err()
                .contains("this database is")
        );
        // writing the same model again touches nothing
        assert_eq!(write_folder(&t.0, ms, &model(&["A", "B"], &["v"])).unwrap(), (0, 0));
        // CRLF from a git checkout is not an edit
        let a = t.0.join("Tables/dbo.A.sql");
        let crlf = std::fs::read_to_string(&a).unwrap().replace('\n', "\r\n");
        std::fs::write(&a, crlf).unwrap();
        assert!(hand_edited(&t.0, &model(&["A", "B"], &["v"]), ms).is_empty());
        // a real edit, a deleted file and a file someone added
        std::fs::write(&a, "CREATE TABLE dbo.A (Id bigint);\n").unwrap();
        std::fs::remove_file(t.0.join("Views/dbo.v.sql")).unwrap();
        std::fs::write(t.0.join("Tables/dbo.Extra.sql"), "-- mine\n").unwrap();
        let mut edits = hand_edited(&t.0, &model(&["A", "B"], &["v"]), ms);
        edits.sort();
        assert_eq!(
            edits,
            vec![
                "Tables/dbo.A.sql (edited)",
                "Tables/dbo.Extra.sql (not written by Sqlotus)",
                "Views/dbo.v.sql (missing)"
            ]
        );
        // a dropped object's file goes, and an empty folder with it
        let (_, removed) = write_folder(&t.0, ms, &model(&["A"], &[])).unwrap();
        assert_eq!(removed, 2, "dbo.B and dbo.Extra");
        assert!(!t.0.join("Views").exists());
    }

    #[test]
    fn deploy_scripts_need_a_statement() {
        let t = Temp::new("deploy");
        std::fs::create_dir_all(t.0.join("Scripts")).unwrap();
        std::fs::write(t.0.join(PRE_SCRIPT), PRE_TEMPLATE).unwrap();
        assert_eq!(deploy_script(&t.0, PRE_SCRIPT), None);
        std::fs::write(t.0.join(POST_SCRIPT), "-- fill it\nUPDATE dbo.A SET Id = Id;\n").unwrap();
        assert_eq!(
            deploy_script(&t.0, POST_SCRIPT).as_deref(),
            Some("-- fill it\nUPDATE dbo.A SET Id = Id;")
        );
        assert_eq!(deploy_script(&t.0, "Scripts/none.sql"), None);
    }

    #[test]
    fn git_commit_of_the_folder_only() {
        let t = Temp::new("git");
        if git(&t.0, &["init"]).is_err() {
            eprintln!("git not available, skipping");
            return;
        }
        git(&t.0, &["config", "user.email", "test@example.com"]).unwrap();
        git(&t.0, &["config", "user.name", "Test"]).unwrap();
        assert!(is_git(&t.0));
        assert!(branch(&t.0).is_some(), "the branch is known before the first commit");
        write_folder(&t.0, Dialect::MsSql, &model(&["A"], &[])).unwrap();
        git(&t.0, &["add", "-A", "--", "."]).unwrap();
        git(&t.0, &["commit", "-m", "first", "--", "."]).unwrap();
        let log = git(&t.0, &["log", "--format=%s"]).unwrap();
        assert_eq!(log, "first");
        // nothing changed: git says so, and the caller treats it as a note
        let again = git(&t.0, &["commit", "-m", "again", "--", "."]).unwrap_err();
        assert!(again.contains("nothing") || again.contains("did not match"), "{again}");
        // not a repository
        let plain = Temp::new("plain");
        assert!(!is_git(&plain.0));
    }
}
