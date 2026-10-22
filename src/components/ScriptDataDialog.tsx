import { useState } from "react";
import { api, errorText } from "../api";
import { toast, update } from "../notify";
import type { Dialect } from "../types";

/**
 * A table's rows as INSERT statements (to copy reference data to another
 * database, or keep rows before a change). The script opens in a new tab;
 * nothing is inserted until it is run.
 */
export function ScriptDataDialog({
  connectionId,
  database,
  schema,
  name,
  dialect,
  onOpenScript,
  onClose,
}: {
  connectionId: string;
  database: string;
  schema: string;
  name: string;
  dialect: Dialect;
  onOpenScript: (title: string, text: string) => void;
  onClose: () => void;
}) {
  const [maxRows, setMaxRows] = useState(1000);
  const [filter, setFilter] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");

  const create = async () => {
    setBusy(true);
    setError("");
    const id = toast.progress(`Reading ${schema}.${name}…`, filter.trim() ? `Rows where ${filter.trim()}` : `Up to ${maxRows.toLocaleString()} row(s)`);
    try {
      const r = await api.tableDataScript(connectionId, database, schema, name, dialect, { maxRows, filter });
      onOpenScript(`${name} (data)`, r.script);
      if (r.problems.length > 0) {
        update(id, "warning", `${r.rows.toLocaleString()} row(s) scripted, with ${r.problems.length} problem(s)`, r.problems[0]);
      } else if (r.truncated) {
        update(id, "warning", `The first ${r.rows.toLocaleString()} row(s) scripted`, "The table has more: raise the limit to script them all.");
      } else {
        update(id, "success", `${r.rows.toLocaleString()} row(s) scripted`, "The script opened in a new tab: nothing was inserted.");
      }
      onClose();
    } catch (e) {
      update(id, "error", "Could not script the data", errorText(e));
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal small-modal data-script-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">
          Data as INSERT — {schema}.{name}
        </div>
        <div className="settings-form">
          <label>
            At most (rows)
            <input
              className="data-rows"
              type="number"
              min={1}
              max={1000000}
              value={maxRows}
              onChange={(e) => setMaxRows(Math.max(1, Math.min(1_000_000, Number(e.target.value) || 1)))}
            />
          </label>
          <label>
            Only rows where (optional)
            <textarea
              className="data-filter"
              rows={3}
              value={filter}
              placeholder={dialect === "mssql" ? "e.g. CreatedAt >= '2024-01-01'" : "e.g. created_at >= '2024-01-01'"}
              onChange={(e) => setFilter(e.target.value)}
            />
          </label>
          <small className="settings-hint">
            Rows are read in key order. Identity values are kept; computed and row-version columns are left out, since the server sets
            them.
          </small>
          {error && <div className="status-error">{error}</div>}
        </div>
        <div className="modal-actions">
          <span className="spacer" />
          <button className="primary" onClick={() => void create()} disabled={busy}>
            {busy ? "Reading…" : "Open script"}
          </button>
          <button onClick={onClose}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
