//! SQL Server backup and restore as scripts to review: the Backup dialog's
//! choices become BACKUP / RESTORE statements that open in a new tab, and
//! nothing runs until the user runs them. Paths are on the server's disk,
//! where the SQL Server service reads and writes the files.

use serde::{Deserialize, Serialize};

use crate::Dialect;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BackupKind {
    #[default]
    Full,
    /// Changes since the last full backup.
    Differential,
    /// The transaction log (full or bulk-logged recovery only).
    Log,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackupOptions {
    pub database: String,
    pub kind: BackupKind,
    /// The backup file.
    pub path: String,
    /// Outside the regular backup chain: later differential and log
    /// backups do not depend on it.
    pub copy_only: bool,
    pub compression: bool,
    pub checksum: bool,
    /// Replace the backups already in the file (else add to them).
    pub overwrite: bool,
    /// Check afterwards that the file can be restored.
    pub verify: bool,
}

/// The BACKUP script.
pub fn backup_script(o: &BackupOptions) -> Result<String, String> {
    let db = o.database.trim();
    let path = o.path.trim();
    if db.is_empty() {
        return Err("Choose the database to back up.".into());
    }
    if path.is_empty() {
        return Err("Type the backup file's path (a folder on the server).".into());
    }
    let what = match o.kind {
        BackupKind::Full => "Full backup",
        BackupKind::Differential => "Differential backup",
        BackupKind::Log => "Log backup",
    };
    let mut with = Vec::new();
    if o.kind == BackupKind::Differential {
        with.push("DIFFERENTIAL".to_string());
    } else if o.copy_only {
        // a differential backup is never part of the chain anyway
        with.push("COPY_ONLY".into());
    }
    if o.compression {
        with.push("COMPRESSION".into());
    }
    if o.checksum {
        with.push("CHECKSUM".into());
    }
    with.push(if o.overwrite { "INIT" } else { "NOINIT" }.into());
    with.push(format!("NAME = {}", text(&format!("{db} {}", what.to_lowercase()))));
    with.push("STATS = 10".into());
    let target = if o.kind == BackupKind::Log { "LOG" } else { "DATABASE" };
    let mut out = format!(
        "-- {what} of {}, scripted by Sqlotus. Review it, then run it (F5).\n\
         -- SQL Server writes the file itself: the path is on the server, and its service account needs access to the folder.\n\
         BACKUP {target} {}\nTO DISK = {}\nWITH {};\nGO\n",
        name(db),
        name(db),
        text(path),
        with.join(", ")
    );
    if o.verify {
        if o.overwrite {
            out.push_str(&format!(
                "-- the file can be restored\nRESTORE VERIFYONLY FROM DISK = {}{};\n",
                text(path),
                if o.checksum { " WITH CHECKSUM" } else { "" }
            ));
        } else {
            out.push_str(&format!(
                "-- RESTORE VERIFYONLY checks the first backup of a file; this one was added after others.\n\
                 -- RESTORE HEADERONLY FROM DISK = {} lists them with their positions (FILE = n).\n",
                text(path)
            ));
        }
    }
    Ok(out)
}

/// A file of the database in a backup (RESTORE FILELISTONLY).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BackupFile {
    pub logical_name: String,
    /// Where the file was when the backup was made.
    pub physical_name: String,
    /// `D` data, `L` log, `F` full-text catalog, `S` FILESTREAM.
    pub kind: String,
}

/// Where a file of the restored database goes.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RestoreMove {
    pub logical_name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RestoreOptions {
    /// The backup file.
    pub path: String,
    /// Which backup of the file (1 = the first).
    pub file_number: u32,
    /// The database to restore into; a new name makes a copy.
    pub database: String,
    /// New places for its files (a copy next to the original needs them).
    pub moves: Vec<RestoreMove>,
    /// Overwrite the database if it exists.
    pub replace: bool,
    /// Open the database when done; off (NORECOVERY) waits for more log
    /// backups to be restored.
    pub recovery: bool,
    /// Disconnect other users of the database first (their open
    /// transactions roll back).
    pub close_connections: bool,
}

impl Default for RestoreOptions {
    fn default() -> Self {
        Self {
            path: String::new(),
            file_number: 1,
            database: String::new(),
            moves: Vec::new(),
            replace: false,
            recovery: true,
            close_connections: false,
        }
    }
}

/// The RESTORE script.
pub fn restore_script(o: &RestoreOptions) -> Result<String, String> {
    let db = o.database.trim();
    let path = o.path.trim();
    if path.is_empty() {
        return Err("Type the backup file's path (on the server).".into());
    }
    if db.is_empty() {
        return Err("Type the name of the database to restore into.".into());
    }
    if let Some(m) = o.moves.iter().find(|m| m.path.trim().is_empty()) {
        return Err(format!("Type where the file {} goes.", m.logical_name));
    }
    let file = o.file_number.max(1);
    let mut out = format!(
        "-- Restore {} from {} (backup {file} of the file), scripted by Sqlotus.\n\
         -- Review it, then run it (F5). It runs from master: a database cannot be restored while it is in use.\n\
         USE master;\nGO\n",
        name(db),
        text(path)
    );
    if o.close_connections {
        out.push_str(&format!(
            "-- disconnect other users of {} (their open transactions roll back)\n\
             IF DB_ID({}) IS NOT NULL\n    ALTER DATABASE {} SET SINGLE_USER WITH ROLLBACK IMMEDIATE;\nGO\n",
            name(db),
            text(db),
            name(db)
        ));
    }
    let mut with = vec![format!("FILE = {file}")];
    with.extend(
        o.moves
            .iter()
            .map(|m| format!("MOVE {} TO {}", text(&m.logical_name), text(m.path.trim()))),
    );
    if o.replace {
        with.push("REPLACE".into());
    }
    with.push(if o.recovery { "RECOVERY" } else { "NORECOVERY" }.into());
    with.push("STATS = 10".into());
    out.push_str(&format!(
        "RESTORE DATABASE {}\nFROM DISK = {}\nWITH {};\nGO\n",
        name(db),
        text(path),
        with.join(",\n     ")
    ));
    if o.close_connections && o.recovery {
        out.push_str(&format!(
            "-- open to everyone again (also if the restore failed)\nIF DB_ID({}) IS NOT NULL\n    ALTER DATABASE {} SET MULTI_USER;\n",
            text(db),
            name(db)
        ));
    }
    Ok(out)
}

/// Where the files of `files` go when restored as `database`: the data
/// folder for data files, the log folder for logs (the files' old folders
/// when unknown), named after the database.
pub fn default_moves(files: &[BackupFile], database: &str, data_dir: &str, log_dir: &str) -> Vec<RestoreMove> {
    let base: String = database
        .trim()
        .chars()
        .map(|c| if "\\/:*?\"<>|".contains(c) { '_' } else { c })
        .collect();
    let (mut data, mut logs) = (0, 0);
    files
        .iter()
        .map(|f| {
            let is_log = f.kind.eq_ignore_ascii_case("L");
            let dir = if is_log { log_dir } else { data_dir };
            let dir = if dir.trim().is_empty() {
                folder_of(&f.physical_name)
            } else {
                dir.trim()
            };
            let file = match f.kind.to_ascii_uppercase().as_str() {
                "L" => {
                    logs += 1;
                    if logs == 1 {
                        format!("{base}_log.ldf")
                    } else {
                        format!("{base}_log{logs}.ldf")
                    }
                }
                "D" => {
                    data += 1;
                    if data == 1 {
                        format!("{base}.mdf")
                    } else {
                        format!("{base}_{data}.ndf")
                    }
                }
                // full-text catalogs and FILESTREAM data are folders
                _ => format!("{base}_{}", f.logical_name),
            };
            RestoreMove {
                logical_name: f.logical_name.clone(),
                path: join(dir, &file),
            }
        })
        .collect()
}

/// The folder of a Windows or Linux path, with its separator.
fn folder_of(path: &str) -> &str {
    path.rfind(['\\', '/']).map_or("", |i| &path[..=i])
}

fn join(dir: &str, file: &str) -> String {
    if dir.is_empty() {
        return file.to_string();
    }
    let sep = if dir.contains('\\') || (!dir.contains('/') && dir.contains(':')) {
        '\\'
    } else {
        '/'
    };
    if dir.ends_with(['\\', '/']) {
        format!("{dir}{file}")
    } else {
        format!("{dir}{sep}{file}")
    }
}

fn name(s: &str) -> String {
    format!("[{}]", s.replace(']', "]]"))
}

fn text(s: &str) -> String {
    format!("N{}", Dialect::MsSql.string_literal(s))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backup(kind: BackupKind) -> BackupOptions {
        BackupOptions {
            database: "Shop".into(),
            kind,
            path: "D:\\Backups\\Shop 'nightly'.bak".into(),
            copy_only: true,
            compression: false,
            checksum: true,
            overwrite: true,
            verify: true,
        }
    }

    #[test]
    fn backup_scripts() {
        assert_eq!(
            backup_script(&backup(BackupKind::Full)).unwrap(),
            "-- Full backup of [Shop], scripted by Sqlotus. Review it, then run it (F5).\n\
             -- SQL Server writes the file itself: the path is on the server, and its service account needs access to the folder.\n\
             BACKUP DATABASE [Shop]\nTO DISK = N'D:\\Backups\\Shop ''nightly''.bak'\n\
             WITH COPY_ONLY, CHECKSUM, INIT, NAME = N'Shop full backup', STATS = 10;\nGO\n\
             -- the file can be restored\nRESTORE VERIFYONLY FROM DISK = N'D:\\Backups\\Shop ''nightly''.bak' WITH CHECKSUM;\n"
        );
        // a differential backup is never copy-only; a log backup can be
        let diff = backup_script(&backup(BackupKind::Differential)).unwrap();
        assert!(
            diff.contains("BACKUP DATABASE [Shop]") && diff.contains("WITH DIFFERENTIAL, CHECKSUM, INIT"),
            "{diff}"
        );
        let log = backup_script(&BackupOptions {
            overwrite: false,
            compression: true,
            ..backup(BackupKind::Log)
        })
        .unwrap();
        assert!(
            log.contains("BACKUP LOG [Shop]\n")
                && log.contains("WITH COPY_ONLY, COMPRESSION, CHECKSUM, NOINIT, NAME = N'Shop log backup'"),
            "{log}"
        );
        // an added backup is not the first of its file: no VERIFYONLY of the wrong one
        assert!(
            !log.lines().any(|l| l.starts_with("RESTORE")) && log.contains("RESTORE HEADERONLY"),
            "{log}"
        );
        assert!(
            backup_script(&BackupOptions {
                path: " ".into(),
                ..backup(BackupKind::Full)
            })
            .is_err()
        );
        let odd = backup_script(&BackupOptions {
            database: "a]b".into(),
            ..backup(BackupKind::Full)
        })
        .unwrap();
        assert!(odd.contains("BACKUP DATABASE [a]]b]"), "{odd}");
    }

    #[test]
    fn restore_scripts() {
        let o = RestoreOptions {
            path: "/var/opt/mssql/backup/Shop.bak".into(),
            file_number: 2,
            database: "Shop_Copy".into(),
            moves: vec![
                RestoreMove {
                    logical_name: "Shop".into(),
                    path: "/var/opt/mssql/data/Shop_Copy.mdf".into(),
                },
                RestoreMove {
                    logical_name: "Shop_log".into(),
                    path: "/var/opt/mssql/data/Shop_Copy_log.ldf".into(),
                },
            ],
            replace: true,
            recovery: true,
            close_connections: true,
        };
        assert_eq!(
            restore_script(&o).unwrap(),
            "-- Restore [Shop_Copy] from N'/var/opt/mssql/backup/Shop.bak' (backup 2 of the file), scripted by Sqlotus.\n\
             -- Review it, then run it (F5). It runs from master: a database cannot be restored while it is in use.\n\
             USE master;\nGO\n\
             -- disconnect other users of [Shop_Copy] (their open transactions roll back)\n\
             IF DB_ID(N'Shop_Copy') IS NOT NULL\n    ALTER DATABASE [Shop_Copy] SET SINGLE_USER WITH ROLLBACK IMMEDIATE;\nGO\n\
             RESTORE DATABASE [Shop_Copy]\nFROM DISK = N'/var/opt/mssql/backup/Shop.bak'\n\
             WITH FILE = 2,\n     MOVE N'Shop' TO N'/var/opt/mssql/data/Shop_Copy.mdf',\n     \
             MOVE N'Shop_log' TO N'/var/opt/mssql/data/Shop_Copy_log.ldf',\n     REPLACE,\n     RECOVERY,\n     STATS = 10;\nGO\n\
             -- open to everyone again (also if the restore failed)\nIF DB_ID(N'Shop_Copy') IS NOT NULL\n    ALTER DATABASE [Shop_Copy] SET MULTI_USER;\n"
        );
        // waiting for log backups: nothing to open yet
        let waiting = restore_script(&RestoreOptions {
            recovery: false,
            ..o.clone()
        })
        .unwrap();
        assert!(
            waiting.contains("NORECOVERY") && !waiting.contains("MULTI_USER"),
            "{waiting}"
        );
        let plain = restore_script(&RestoreOptions {
            moves: vec![],
            replace: false,
            close_connections: false,
            ..o.clone()
        })
        .unwrap();
        assert!(
            plain.contains("WITH FILE = 2,\n     RECOVERY,\n     STATS = 10;") && !plain.contains("SINGLE_USER"),
            "{plain}"
        );
        assert!(
            restore_script(&RestoreOptions {
                database: "".into(),
                ..o.clone()
            })
            .is_err()
        );
        let missing = RestoreOptions {
            moves: vec![RestoreMove {
                logical_name: "Shop".into(),
                path: "".into(),
            }],
            ..o
        };
        assert_eq!(restore_script(&missing).unwrap_err(), "Type where the file Shop goes.");
    }

    #[test]
    fn files_go_to_the_server_folders_named_after_the_database() {
        let f = |l: &str, p: &str, k: &str| BackupFile {
            logical_name: l.into(),
            physical_name: p.into(),
            kind: k.into(),
        };
        let files = vec![
            f("Shop", "C:\\Data\\Shop.mdf", "D"),
            f("Shop_log", "C:\\Logs\\Shop_log.ldf", "L"),
            f("Shop_2", "C:\\Data\\Shop_2.ndf", "D"),
            f("ftcat", "C:\\Data\\ft", "F"),
        ];
        let moves: Vec<String> = default_moves(&files, "Shop: Copy", "E:\\SQLData", "F:\\SQLLogs\\")
            .into_iter()
            .map(|m| m.path)
            .collect();
        assert_eq!(
            moves,
            vec![
                "E:\\SQLData\\Shop_ Copy.mdf",
                "F:\\SQLLogs\\Shop_ Copy_log.ldf",
                "E:\\SQLData\\Shop_ Copy_2.ndf",
                "E:\\SQLData\\Shop_ Copy_ftcat",
            ]
        );
        // a Linux server, and the files' own folders when the defaults are unknown
        let linux = default_moves(&files[..2], "Copy", "/var/opt/mssql/data/", "");
        assert_eq!(linux[0].path, "/var/opt/mssql/data/Copy.mdf");
        assert_eq!(linux[1].path, "C:\\Logs\\Copy_log.ldf");
    }
}
