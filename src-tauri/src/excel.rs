//! Result grid → Excel workbook (.xlsx). Numbers stay numbers, so Excel can
//! sum and chart them, and the file opens the same way in every locale
//! (a CSV opens as one column where the list separator is ';').

use std::path::PathBuf;

use rust_xlsxwriter::{Format, Workbook};
use serde::{Deserialize, Serialize};
use sqlotus_core::sqltype::SqlType;
use tauri_plugin_opener::OpenerExt;

use crate::commands::{CmdResult, err};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SheetColumn {
    name: String,
    type_name: String,
}

fn is_numeric(type_name: &str) -> bool {
    SqlType::parse(type_name).is_number()
}

/// Excel stores numbers as doubles: only values that survive the round
/// trip (at most 15 significant digits) are written as numbers, so bigint
/// keys and decimal(38, x) amounts are not silently rounded.
fn exact_number(v: &str) -> Option<f64> {
    let n: f64 = v.trim().parse().ok()?;
    let digits = v
        .trim()
        .trim_start_matches(['-', '+'])
        .chars()
        .filter(char::is_ascii_digit)
        .collect::<String>();
    let significant = digits.trim_start_matches('0').trim_end_matches('0').len();
    (n.is_finite() && significant <= 15 && !v.contains(['e', 'E'])).then_some(n)
}

/// The most characters an Excel cell holds.
const CELL_LIMIT: usize = 32_767;

/// Write the sheet; returns how many values were cut to [`CELL_LIMIT`].
fn write_workbook(
    path: &PathBuf,
    columns: &[SheetColumn],
    rows: &[Vec<Option<String>>],
) -> Result<usize, rust_xlsxwriter::XlsxError> {
    let mut cut = 0;
    let mut wb = Workbook::new();
    let ws = wb.add_worksheet();
    ws.set_name("Results")?;
    let header = Format::new().set_bold().set_background_color("#DDE6F0");
    let numeric: Vec<bool> = columns.iter().map(|c| is_numeric(&c.type_name)).collect();
    for (j, c) in columns.iter().enumerate() {
        ws.write_string_with_format(
            0,
            j as u16,
            if c.name.is_empty() { "(No column name)" } else { &c.name },
            &header,
        )?;
    }
    for (i, row) in rows.iter().enumerate() {
        let r = (i + 1) as u32;
        for (j, v) in row.iter().enumerate() {
            let Some(v) = v else { continue };
            let col = j as u16;
            match exact_number(v) {
                Some(n) if numeric.get(j).copied().unwrap_or(false) => ws.write_number(r, col, n)?,
                // longer text would make the whole workbook fail
                _ if v.chars().count() > CELL_LIMIT => {
                    cut += 1;
                    ws.write_string(r, col, v.chars().take(CELL_LIMIT).collect::<String>())?
                }
                _ => ws.write_string(r, col, v)?,
            };
        }
    }
    if !columns.is_empty() {
        ws.autofilter(0, 0, rows.len() as u32, (columns.len() - 1) as u16)?;
    }
    ws.set_freeze_panes(1, 0)?;
    ws.autofit();
    wb.save(path)?;
    Ok(cut)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcelFile {
    path: String,
    /// Values cut to the 32,767 characters a cell holds.
    cut: usize,
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
) -> CmdResult<ExcelFile> {
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
    let cut = tauri::async_runtime::spawn_blocking(move || write_workbook(&target, &columns, &rows))
        .await
        .map_err(err)?
        .map_err(|e| format!("Could not write the workbook: {e}"))?;
    let shown = path.to_string_lossy().to_string();
    if open {
        app.opener()
            .open_path(&shown, None::<&str>)
            .map_err(|e| format!("The workbook was saved to {shown}, but no app opened it: {e}"))?;
    }
    Ok(ExcelFile { path: shown, cut })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_workbook() {
        let path = std::env::temp_dir().join(format!("sqlotus-test-{}.xlsx", std::process::id()));
        let columns = vec![
            SheetColumn {
                name: "Id".into(),
                type_name: "int".into(),
            },
            SheetColumn {
                name: "Name".into(),
                type_name: "nvarchar".into(),
            },
        ];
        let rows = vec![
            vec![Some("1".into()), Some("Nguyễn".into())],
            vec![Some("2".into()), None],
        ];
        write_workbook(&path, &columns, &rows).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..2], b"PK"); // a zip container
        std::fs::remove_file(&path).ok();
        assert!(is_numeric("decimal(18, 2)") && !is_numeric("varchar"));
        assert_eq!(exact_number("12.50"), Some(12.5));
        assert_eq!(exact_number("-0.001"), Some(-0.001));
        assert_eq!(exact_number("9007199254740993"), None); // bigint beyond double precision
        assert_eq!(exact_number("1234567890.1234567890"), None);
        // a value longer than an Excel cell holds
        let long = vec![vec![Some("1".into()), Some("x".repeat(40_000))]];
        let probe = std::env::temp_dir().join(format!("sqlotus-long-{}.xlsx", std::process::id()));
        let result = write_workbook(&probe, &columns, &long);
        std::fs::remove_file(&probe).ok();
        assert_eq!(result.unwrap(), 1, "cut to what a cell holds, not a failed workbook");
    }
}
