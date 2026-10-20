//! Result grid → Excel workbook (.xlsx). Numbers stay numbers, so Excel can
//! sum and chart them, and the file opens the same way in every locale
//! (a CSV opens as one column where the list separator is ';').

use std::path::PathBuf;

use rust_xlsxwriter::{Format, Workbook};
use serde::Deserialize;
use tauri_plugin_opener::OpenerExt;

use crate::commands::{CmdResult, err};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetColumn {
    name: String,
    type_name: String,
}

const NUMERIC: &[&str] = &[
    "tinyint", "smallint", "int", "bigint", "decimal", "numeric", "money", "smallmoney", "float", "real", "integer",
    "double", "int2", "int4", "int8", "float4", "float8", "serial", "bigserial", "LONGLONG", "LONG", "SHORT", "TINY",
    "NEWDECIMAL", "DOUBLE", "FLOAT", "INT24",
];

fn is_numeric(type_name: &str) -> bool {
    let base = type_name.split('(').next().unwrap_or("").trim();
    NUMERIC.iter().any(|n| n.eq_ignore_ascii_case(base))
}

fn write_workbook(path: &PathBuf, columns: &[SheetColumn], rows: &[Vec<Option<String>>]) -> Result<(), rust_xlsxwriter::XlsxError> {
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Results")?;
    let header = Format::new().set_bold().set_background_color("#DDE6F0");
    let numeric: Vec<bool> = columns.iter().map(|c| is_numeric(&c.type_name)).collect();
    for (j, c) in columns.iter().enumerate() {
        ws.write_string_with_format(0, j as u16, if c.name.is_empty() { "(No column name)" } else { &c.name }, &header)?;
    }
    for (i, row) in rows.iter().enumerate() {
        let r = (i + 1) as u32;
        for (j, v) in row.iter().enumerate() {
            let Some(v) = v else { continue };
            let col = j as u16;
            match v.parse::<f64>() {
                Ok(n) if numeric.get(j).copied().unwrap_or(false) && n.is_finite() => ws.write_number(r, col, n)?,
                _ => ws.write_string(r, col, v)?,
            };
        }
    }
    if !columns.is_empty() {
        ws.autofilter(0, 0, rows.len() as u32, (columns.len() - 1) as u16)?;
    }
    ws.set_freeze_panes(1, 0)?;
    ws.autofit();
    wb.save(path)
}

/// Write the rows to `path` (or a new file in the temp folder) and, when
/// `open` is set, open it with the default app for .xlsx (Excel).
#[tauri::command]
pub async fn export_xlsx(
    app: tauri::AppHandle,
    path: Option<String>,
    columns: Vec<SheetColumn>,
    rows: Vec<Vec<Option<String>>>,
    open: bool,
) -> CmdResult<String> {
    if rows.len() >= 1_048_576 {
        return Err("Excel sheets hold at most 1,048,575 rows; filter the results or save them as CSV.".into());
    }
    let path = match path {
        Some(p) => PathBuf::from(p),
        None => {
            let dir = std::env::temp_dir().join("Sqlotus");
            std::fs::create_dir_all(&dir).map_err(err)?;
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or_default();
            dir.join(format!("results-{stamp}.xlsx"))
        }
    };
    let target = path.clone();
    tauri::async_runtime::spawn_blocking(move || write_workbook(&target, &columns, &rows))
        .await
        .map_err(err)?
        .map_err(|e| format!("Could not write the workbook: {e}"))?;
    let shown = path.to_string_lossy().to_string();
    if open {
        app.opener()
            .open_path(&shown, None::<&str>)
            .map_err(|e| format!("The workbook was saved to {shown}, but no app opened it: {e}"))?;
    }
    Ok(shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_workbook() {
        let path = std::env::temp_dir().join(format!("sqlotus-test-{}.xlsx", std::process::id()));
        let columns = vec![
            SheetColumn { name: "Id".into(), type_name: "int".into() },
            SheetColumn { name: "Name".into(), type_name: "nvarchar".into() },
        ];
        let rows = vec![vec![Some("1".into()), Some("Nguyễn".into())], vec![Some("2".into()), None]];
        write_workbook(&path, &columns, &rows).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"PK"); // a zip container
        std::fs::remove_file(&path).ok();
        assert!(is_numeric("decimal(18, 2)") && !is_numeric("varchar"));
    }
}
