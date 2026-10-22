import { useEffect, useState } from "react";
import { api, errorText } from "../api";
import { toast } from "../notify";
import type { BackupContents, BackupInfo, BackupOptions, RestoreOptions } from "../types";

/** `yyyyMMdd_HHmm` for backup file names. */
function stamp(d = new Date()): string {
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}_${p(d.getHours())}${p(d.getMinutes())}`;
}

/** A file in a server folder: Windows or Linux separators, as the folder has them. */
function inFolder(dir: string, file: string): string {
  if (!dir) return file;
  const sep = dir.includes("\\") || /^[A-Za-z]:/.test(dir) ? "\\" : "/";
  return /[\\/]$/.test(dir) ? `${dir}${file}` : `${dir}${sep}${file}`;
}

const KIND_FILE: Record<BackupOptions["kind"], (db: string) => string> = {
  full: (db) => `${db}_${stamp()}.bak`,
  differential: (db) => `${db}_diff_${stamp()}.bak`,
  log: (db) => `${db}_log_${stamp()}.trn`,
};

/**
 * SQL Server backup and restore. The dialog writes the BACKUP or RESTORE
 * script and opens it in a new tab: nothing runs until it is run there.
 * Reading what a backup file holds is the only thing done here.
 */
export function BackupDialog({
  connectionId,
  database,
  onOpenScript,
  onClose,
}: {
  connectionId: string;
  database: string;
  onOpenScript: (title: string, text: string) => void;
  onClose: () => void;
}) {
  const [page, setPage] = useState<"backup" | "restore">("backup");
  const [info, setInfo] = useState<BackupInfo | null>(null);
  const [error, setError] = useState("");
  const [b, setB] = useState<BackupOptions>({
    database,
    kind: "full",
    path: "",
    copyOnly: true,
    compression: false,
    checksum: true,
    overwrite: true,
    verify: true,
  });
  // the file name follows the kind of backup until it is typed over
  const [pathTyped, setPathTyped] = useState(false);
  const [r, setR] = useState<RestoreOptions>({
    path: "",
    fileNumber: 1,
    database,
    moves: [],
    replace: false,
    recovery: true,
    closeConnections: false,
  });
  const [contents, setContents] = useState<BackupContents | null>(null);
  const [reading, setReading] = useState(false);

  useEffect(() => {
    api
      .backupInfo(connectionId, database)
      .then((i) => {
        setInfo(i);
        setB((o) => ({ ...o, path: inFolder(i.backupDir || i.dataDir, KIND_FILE.full(database)) }));
        setR((o) => ({ ...o, path: inFolder(i.backupDir || i.dataDir, `${database}.bak`) }));
      })
      .catch((e) => setError(errorText(e)));
  }, [connectionId, database]);

  const setKind = (kind: BackupOptions["kind"]) =>
    setB((o) => ({
      ...o,
      kind,
      path: pathTyped || !info ? o.path : inFolder(info.backupDir || info.dataDir, KIND_FILE[kind](database)),
    }));

  const simple = info?.recoveryModel.toUpperCase() === "SIMPLE";

  const openBackup = async () => {
    try {
      onOpenScript(`Backup ${database}`, await api.backupScript(b));
      toast.info("Review the backup script, then run it (F5)", "SQL Server writes the file on the server.");
      onClose();
    } catch (e) {
      setError(errorText(e));
    }
  };

  /** Read the backups in the file and where its database files would go. */
  const readFile = async (fileNumber = r.fileNumber, target = r.database) => {
    setReading(true);
    setError("");
    try {
      const c = await api.backupContents(connectionId, database, r.path, fileNumber);
      setContents(c);
      const moves = await api.restoreMoves(c.files, target, info?.dataDir ?? "", info?.logDir ?? "");
      setR((o) => ({ ...o, fileNumber, database: target, moves }));
    } catch (e) {
      setContents(null);
      setError(errorText(e));
    } finally {
      setReading(false);
    }
  };

  const setTarget = async (target: string) => {
    setR((o) => ({ ...o, database: target }));
    if (contents) {
      const moves = await api.restoreMoves(contents.files, target, info?.dataDir ?? "", info?.logDir ?? "").catch(() => r.moves);
      setR((o) => ({ ...o, moves }));
    }
  };

  const openRestore = async () => {
    try {
      onOpenScript(`Restore ${r.database}`, await api.restoreScript(r));
      toast.info("Review the restore script, then run it (F5)", "It switches to master: a database cannot be restored while it is in use.");
      onClose();
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal backup-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Backup and restore — SQL Server</div>
        <div className="settings-tabs">
          <button className={page === "backup" ? "active" : ""} onClick={() => setPage("backup")}>
            Back up {database}
          </button>
          <button className={page === "restore" ? "active" : ""} onClick={() => setPage("restore")}>
            Restore
          </button>
        </div>
        {page === "backup" ? (
          <div className="settings-form">
            <label>
              Backup
              <select className="backup-kind" value={b.kind} onChange={(e) => setKind(e.target.value as BackupOptions["kind"])}>
                <option value="full">Full</option>
                <option value="differential">Differential (changes since the last full backup)</option>
                <option value="log" disabled={simple}>
                  Transaction log{simple ? " (not with the SIMPLE recovery model)" : ""}
                </option>
              </select>
            </label>
            <label>
              File (on the server)
              <input
                className="backup-path"
                value={b.path}
                onChange={(e) => {
                  setPathTyped(true);
                  setB({ ...b, path: e.target.value });
                }}
              />
            </label>
            <small className="settings-hint">
              The SQL Server service writes the file, so the folder is on the server and its service account needs access to it.
            </small>
            {b.kind !== "differential" && (
              <label className="check" title="Later differential and log backups do not depend on this one">
                <input type="checkbox" checked={b.copyOnly} onChange={(e) => setB({ ...b, copyOnly: e.target.checked })} />
                Copy-only (leaves the regular backup chain as it is)
              </label>
            )}
            <label className="check" title={info?.express ? "Express edition cannot compress backups" : ""}>
              <input
                type="checkbox"
                checked={b.compression && !info?.express}
                disabled={info?.express}
                onChange={(e) => setB({ ...b, compression: e.target.checked })}
              />
              Compress{info?.express ? " (not in Express edition)" : ""}
            </label>
            <label className="check">
              <input type="checkbox" checked={b.checksum} onChange={(e) => setB({ ...b, checksum: e.target.checked })} />
              Checksums (finds damaged pages)
            </label>
            <label className="check">
              <input type="checkbox" checked={b.overwrite} onChange={(e) => setB({ ...b, overwrite: e.target.checked })} />
              Replace backups already in the file (else add this one to them)
            </label>
            <label className="check">
              <input type="checkbox" checked={b.verify} onChange={(e) => setB({ ...b, verify: e.target.checked })} />
              Check afterwards that the file can be restored
            </label>
          </div>
        ) : (
          <div className="settings-form">
            <label>
              Backup file (on the server)
              <div className="row2">
                <input className="restore-path" value={r.path} onChange={(e) => setR({ ...r, path: e.target.value })} />
                <button onClick={() => void readFile()} disabled={reading || !r.path.trim()}>
                  {reading ? "Reading…" : "Read the file"}
                </button>
              </div>
            </label>
            {contents && contents.sets.length > 1 && (
              <label>
                Backup in the file
                <select value={r.fileNumber} onChange={(e) => void readFile(Number(e.target.value))}>
                  {contents.sets.map((s) => (
                    <option key={s.position} value={s.position}>
                      {s.position}: {s.kind} backup of {s.databaseName}, {s.finished}
                    </option>
                  ))}
                </select>
              </label>
            )}
            {contents && contents.sets.length === 1 && (
              <small className="settings-hint">
                A {contents.sets[0].kind} backup of {contents.sets[0].databaseName}, made {contents.sets[0].finished}.
              </small>
            )}
            <label>
              Restore as database
              <input className="restore-target" value={r.database} onChange={(e) => void setTarget(e.target.value)} />
            </label>
            {r.moves.length > 0 && (
              <div className="restore-moves">
                <small className="settings-hint">Where its files go (on the server):</small>
                {r.moves.map((m, i) => (
                  <label key={m.logicalName} className="restore-move">
                    <span>{m.logicalName}</span>
                    <input
                      value={m.path}
                      onChange={(e) => setR({ ...r, moves: r.moves.map((x, k) => (k === i ? { ...x, path: e.target.value } : x)) })}
                    />
                  </label>
                ))}
              </div>
            )}
            {!contents && (
              <small className="settings-hint">Read the file to put the restored database's files next to the server's others (needed for a copy).</small>
            )}
            <label className="check">
              <input type="checkbox" checked={r.replace} onChange={(e) => setR({ ...r, replace: e.target.checked })} />
              Overwrite the database if it exists
            </label>
            <label className="check">
              <input type="checkbox" checked={r.closeConnections} onChange={(e) => setR({ ...r, closeConnections: e.target.checked })} />
              Disconnect its other users first (their open transactions roll back)
            </label>
            <label className="check" title="Off: the database waits for more log backups to be restored (NORECOVERY)">
              <input type="checkbox" checked={r.recovery} onChange={(e) => setR({ ...r, recovery: e.target.checked })} />
              Ready to use when done (off: wait for more log backups)
            </label>
          </div>
        )}
        {error && <div className="status-error backup-error">{error}</div>}
        <div className="modal-actions">
          <small>Nothing runs here: the script opens in a new tab for you to review and run.</small>
          <span className="spacer" />
          {page === "backup" ? (
            <button className="primary" onClick={() => void openBackup()} disabled={!b.path.trim()}>
              Open backup script
            </button>
          ) : (
            <button className="primary" onClick={() => void openRestore()} disabled={!r.path.trim() || !r.database.trim()}>
              Open restore script
            </button>
          )}
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
