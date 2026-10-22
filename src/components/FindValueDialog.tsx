import { useState } from "react";
import { api, errorText } from "../api";
import { toast } from "../notify";
import type { Dialect, FindValueOptions } from "../types";

/**
 * Find a value in every text column of the database. Sqlotus writes the
 * search as a script and opens it for review: it only reads, but it reads
 * every text column in full, so the user decides when to run it.
 */
export function FindValueDialog({
  connectionId,
  database,
  dialect,
  schemas,
  onOpenScript,
  onClose,
}: {
  connectionId: string;
  database: string;
  dialect: Dialect;
  /** Schemas of the database, to search only one. */
  schemas: string[];
  onOpenScript: (title: string, text: string) => void;
  onClose: () => void;
}) {
  const [opts, setOpts] = useState<FindValueOptions>({ value: "", mode: "contains", schema: "", ignoreCase: true });
  const [error, setError] = useState("");
  const set = <K extends keyof FindValueOptions>(k: K, v: FindValueOptions[K]) => setOpts((o) => ({ ...o, [k]: v }));

  const create = async () => {
    setError("");
    try {
      const script = await api.findValueScript(connectionId, database, dialect, opts);
      onOpenScript(`Find '${opts.value.slice(0, 30)}'`, script);
      toast.info("Review the search, then run it (F5)", "It only reads, but it reads each text column in full: on big tables that takes a while.");
      onClose();
    } catch (e) {
      setError(errorText(e));
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal small-modal find-value-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Find a value — {database}</div>
        <div className="settings-form">
          <label>
            Value
            <input
              className="find-value"
              autoFocus
              value={opts.value}
              placeholder="text to look for"
              onChange={(e) => set("value", e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && void create()}
            />
          </label>
          <div className="row2">
            <label>
              Match
              <select value={opts.mode} onChange={(e) => set("mode", e.target.value as FindValueOptions["mode"])}>
                <option value="contains">Contains</option>
                <option value="startsWith">Starts with</option>
                <option value="exact">Exactly</option>
              </select>
            </label>
            <label>
              In
              <select value={opts.schema} onChange={(e) => set("schema", e.target.value)}>
                <option value="">Every schema</option>
                {schemas.map((s) => (
                  <option key={s} value={s}>
                    {s}
                  </option>
                ))}
              </select>
            </label>
          </div>
          {dialect === "postgres" ? (
            <label className="check">
              <input type="checkbox" checked={opts.ignoreCase} onChange={(e) => set("ignoreCase", e.target.checked)} />
              Ignore upper and lower case
            </label>
          ) : (
            <small className="settings-hint">Upper and lower case follow each column's collation.</small>
          )}
          <small className="settings-hint">
            Every text column of every table is searched. The result lists each table and column with matches, how many rows, and an
            example value.
          </small>
          {error && <div className="status-error">{error}</div>}
        </div>
        <div className="modal-actions">
          <span className="spacer" />
          <button className="primary" onClick={() => void create()} disabled={!opts.value}>
            Open script
          </button>
          <button onClick={onClose}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
