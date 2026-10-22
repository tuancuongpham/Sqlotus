//! Editor intelligence commands: diagnostics, hover, parameter hints,
//! object lookup, refactorings, object scripts and the object explorer.
//! Offsets are UTF-16 code units on the way in and out.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sqlotus_core::analysis::{self, Fix, ObjectAt, Severity, SignatureInfo};
use sqlotus_core::catalog::DbObject;
use sqlotus_core::format::KeywordCase;
use sqlotus_core::lint::{self, RULES, Rule};
use sqlotus_core::safety::ScriptRun;
use sqlotus_core::{Dialect, byte_to_utf16, refactor, script, utf16_to_byte};
use sqlotus_db::CatalogSnapshot;
use tauri::State;

use crate::commands::{CmdResult, fetch_catalog, no_panic};
use crate::state::AppState;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorArgs {
    connection_id: Option<String>,
    database: Option<String>,
    dialect: Dialect,
    text: String,
    #[serde(default)]
    offset: usize,
}

impl EditorArgs {
    fn catalog(&self, state: &AppState) -> Option<Arc<CatalogSnapshot>> {
        match (&self.connection_id, &self.database) {
            (Some(c), Some(d)) => state.cached_catalog(c, d),
            _ => None,
        }
    }

    fn byte_offset(&self) -> usize {
        utf16_to_byte(&self.text, self.offset)
    }

    fn u16(&self, byte: usize) -> usize {
        byte_to_utf16(&self.text, byte)
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticDto {
    start: usize,
    end: usize,
    severity: Severity,
    code: String,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fix: Option<FixDto>,
    /// "Ignore on this line": the comment that switches the rule off there.
    #[serde(skip_serializing_if = "Option::is_none")]
    ignore: Option<FixDto>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FixDto {
    title: String,
    start: usize,
    end: usize,
    text: String,
}

/// Code analysis of the editor text, without switched-off rules and lines
/// suppressed by `-- sqlotus-disable…` comments.
#[tauri::command]
pub async fn diagnostics(state: State<'_, AppState>, args: EditorArgs) -> CmdResult<Vec<DiagnosticDto>> {
    let snap = args.catalog(&state);
    let options = state.settings().analysis;
    let (found, ignore) = no_panic("code analysis", || {
        let found = lint::lint(&args.text, args.dialect, snap.as_ref().map(|s| &s.catalog), &options);
        let ignore = lint::ignore_edits(&args.text, args.dialect, &found);
        (found, ignore)
    })?;
    let dto = |f: Fix| FixDto {
        title: f.title,
        start: args.u16(f.start),
        end: args.u16(f.end),
        text: f.text,
    };
    Ok(found
        .into_iter()
        .zip(ignore)
        .map(|(d, ignore)| DiagnosticDto {
            start: args.u16(d.start),
            end: args.u16(d.end),
            severity: d.severity,
            code: d.code,
            message: d.message,
            fix: d.fix.map(dto),
            ignore: ignore.map(dto),
        })
        .collect())
}

/// The analysis rules (for Options → Analysis).
#[tauri::command]
pub fn analysis_rules() -> Vec<Rule> {
    RULES.to_vec()
}

#[tauri::command]
pub async fn execution_warnings(text: String, dialect: Dialect) -> CmdResult<Vec<String>> {
    no_panic("execution warnings", || {
        ScriptRun::new(&sqlotus_core::sqlcmd::as_run(&text, dialect), dialect).warnings()
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HoverDto {
    start: usize,
    end: usize,
    contents: String,
}

#[tauri::command]
pub async fn hover(state: State<'_, AppState>, args: EditorArgs) -> CmdResult<Option<HoverDto>> {
    let snap = args.catalog(&state);
    let h = no_panic("hover", || {
        analysis::hover(
            &args.text,
            args.byte_offset(),
            args.dialect,
            snap.as_ref().map(|s| &s.catalog),
        )
    })?;
    Ok(h.map(|h| HoverDto {
        start: args.u16(h.start),
        end: args.u16(h.end),
        contents: h.contents,
    }))
}

#[tauri::command]
pub async fn signature_help(state: State<'_, AppState>, args: EditorArgs) -> CmdResult<Option<SignatureInfo>> {
    let snap = args.catalog(&state);
    no_panic("parameter hints", || {
        analysis::signature_help(
            &args.text,
            args.byte_offset(),
            args.dialect,
            snap.as_ref().map(|s| &s.catalog),
        )
    })
}

#[tauri::command]
pub async fn object_at(state: State<'_, AppState>, args: EditorArgs) -> CmdResult<Option<ObjectAt>> {
    let Some(snap) = args.catalog(&state) else {
        return Ok(None);
    };
    no_panic("object lookup", || {
        analysis::object_at(&args.text, args.byte_offset(), args.dialect, &snap.catalog)
    })
}

/// Run a whole-document refactoring and return the new text.
#[tauri::command]
pub async fn refactor_text(state: State<'_, AppState>, args: EditorArgs, action: String) -> CmdResult<String> {
    let snap = args.catalog(&state);
    let cat = snap.as_ref().map(|s| &s.catalog);
    let (text, d) = (&args.text, args.dialect);
    no_panic("refactoring", || match action.as_str() {
        "expandWildcards" => Ok(refactor::expand_wildcards(text, d, cat)),
        "qualifyNames" => cat
            .map(|c| refactor::qualify_object_names(text, d, c))
            .ok_or_else(|| "objects not loaded".to_string()),
        "insertSemicolons" => Ok(refactor::insert_semicolons(text, d)),
        "removeBrackets" => Ok(refactor::remove_brackets(text, d)),
        "upperKeywords" => Ok(refactor::apply_keyword_case(text, d, KeywordCase::Upper)),
        "lowerKeywords" => Ok(refactor::apply_keyword_case(text, d, KeywordCase::Lower)),
        other => Err(format!("unknown action {other}")),
    })?
}

/// All objects of a database with columns and parameters (object explorer).
#[tauri::command]
pub async fn catalog_objects(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
) -> CmdResult<Vec<DbObject>> {
    Ok(state
        .cached_catalog(&connection_id, &database)
        .map(|s| s.catalog.objects.clone())
        .unwrap_or_default())
}

/// Generate a script for an object: select | insert | update | delete |
/// create | alter.
#[tauri::command]
pub async fn script_object(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    schema: String,
    name: String,
    dialect: Dialect,
    kind: String,
) -> CmdResult<String> {
    let snap = state
        .cached_catalog(&connection_id, &database)
        .ok_or("objects not loaded")?;
    let obj = snap.catalog.find(Some(&schema), &name).ok_or("object not found")?;
    let definition = snap
        .definitions
        .iter()
        .find(|d| d.schema.eq_ignore_ascii_case(&schema) && d.name.eq_ignore_ascii_case(&name))
        .map(|d| d.definition.trim().to_string());
    let full = script::object_name(obj, dialect, true);
    Ok(match kind.as_str() {
        "select" => script::select_script(obj, dialect, 100, 4),
        "insert" => format!("INSERT INTO {}", script::insert_tail(obj, &full, dialect, 4, false)),
        "update" => script::update_script(obj, dialect, 4),
        "delete" => script::delete_script(obj, dialect),
        "exec" => format!(
            "{} {}",
            if dialect == Dialect::MsSql { "EXEC" } else { "CALL" },
            script::exec_tail(obj, &full, dialect, 5, false)
        ),
        "alter" => match definition {
            Some(d) => script::script_as_alter(&d, dialect),
            None => {
                return Err("only views, procedures, functions and triggers can be scripted as ALTER".into());
            }
        },
        _ => definition.unwrap_or_else(|| script::create_table_script(obj, dialect)),
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameDto {
    kind: String,
    name: String,
    /// UTF-16 ranges of every occurrence.
    ranges: Vec<(usize, usize)>,
}

/// The variable / alias / CTE under the cursor and all its occurrences.
#[tauri::command]
pub async fn rename_target(args: EditorArgs) -> CmdResult<Option<RenameDto>> {
    let t = no_panic("rename", || {
        sqlotus_core::rename::rename_target(&args.text, args.byte_offset(), args.dialect)
    })?;
    Ok(t.map(|t| RenameDto {
        kind: t.kind,
        name: t.name,
        ranges: t.ranges.into_iter().map(|(s, e)| (args.u16(s), args.u16(e))).collect(),
    }))
}

/// Views, procedures, functions and triggers that reference missing tables
/// or columns. Reloads the catalog first so the answer is current.
#[tauri::command]
pub async fn find_invalid_objects(
    state: State<'_, AppState>,
    connection_id: String,
    database: String,
    dialect: Dialect,
) -> CmdResult<Vec<analysis::ObjectProblem>> {
    let snap = fetch_catalog(&state, &connection_id, &database).await?;
    no_panic("find invalid objects", || {
        analysis::invalid_objects_in_definitions(&snap.definitions, dialect, &snap.catalog)
    })
}

/// Write text to a file chosen by the user (CSV export).
#[tauri::command]
pub async fn save_text_file(path: String, contents: String) -> CmdResult<()> {
    // Excel reads UTF-8 CSV correctly only with a BOM
    let data = if path.to_ascii_lowercase().ends_with(".csv") {
        format!("\u{feff}{contents}")
    } else {
        contents
    };
    std::fs::write(&path, data).map_err(|e| e.to_string())
}
