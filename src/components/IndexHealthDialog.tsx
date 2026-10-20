import { useEffect, useMemo, useState } from "react";
import { api, errorText } from "../api";
import { toast } from "../notify";
import type { Dialect, IndexFinding, IndexHealth } from "../types";

const GROUPS: { kind: IndexFinding["kind"]; title: string; hint: string }[] = [
  { kind: "missing", title: "Missing indexes", hint: "The server suggests these for queries it has run. Check that the key columns make sense before creating." },
  { kind: "scans", title: "Tables read by full scans", hint: "Queries read the whole table. Run them with Ctrl+L to see which filter needs an index." },
  { kind: "duplicate", title: "Duplicate indexes", hint: "Same key columns as another index: one of them only costs writes and space." },
  { kind: "overlapping", title: "Overlapping indexes", hint: "The keys are the leading part of a wider index, which can serve the same queries." },
  { kind: "unused", title: "Unused indexes", hint: "Not read since statistics were reset, but updated on every write." },
  { kind: "heap", title: "Heaps", hint: "Large tables without a clustered index." },
];

const keyOf = (f: IndexFinding, i: number) => `${f.kind}:${f.schema}.${f.table}:${f.index ?? i}`;

/** Missing, duplicate, overlapping and unused indexes of a database. */
export function IndexHealthDialog({
  connectionId,
  database,
  dialect,
  onOpenScript,
  onClose,
}: {
  connectionId: string;
  database: string;
  dialect: Dialect;
  onOpenScript: (title: string, text: string) => void;
  onClose: () => void;
}) {
  const [health, setHealth] = useState<IndexHealth | null>(null);
  const [error, setError] = useState("");
  const [picked, setPicked] = useState<Set<string>>(new Set());

  const load = () => {
    setHealth(null);
    setError("");
    api
      .indexHealth(connectionId, database, dialect)
      .then((h) => {
        setHealth(h);
        setPicked(new Set());
      })
      .catch((e) => setError(errorText(e)));
  };
  useEffect(load, [connectionId, database, dialect]); // eslint-disable-line react-hooks/exhaustive-deps

  const withScripts = useMemo(() => (health?.findings ?? []).map((f, i) => ({ f, key: keyOf(f, i) })), [health]);

  const openScripts = () => {
    const chosen = withScripts.filter((x) => picked.has(x.key) && x.f.script);
    if (chosen.length === 0) return;
    const text = [
      `-- Index changes for ${database}, from Sqlotus index health.`,
      "-- Review each statement before running it: dropping an index can slow queries that need it,",
      "-- and creating one takes locks and time on large tables.",
      "",
      ...chosen.map((x) => `-- ${x.f.kind}: ${x.f.detail}\n${x.f.script}\n`),
    ].join("\n");
    onOpenScript(`${database} (index changes)`, text);
    toast.success(`${chosen.length} index statement(s) opened in a new tab`, "Nothing was changed; review and run them yourself.");
    onClose();
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal health-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Index health — {database}</div>
        <div className="health-body">
          {error && <div className="status-error">{error}</div>}
          {!health && !error && (
            <div className="panel-empty">
              <span className="spinner" /> Reading index definitions and usage statistics…
            </div>
          )}
          {health && (
            <>
              {health.notes.map((n, i) => (
                <div key={i} className="settings-hint">
                  ℹ {n}
                </div>
              ))}
              {health.errors.map((e, i) => (
                <div key={i} className="health-error">
                  ⚠ {e}
                </div>
              ))}
              {health.findings.length === 0 && <div className="panel-empty">✓ No index problems found.</div>}
              {GROUPS.map((g) => {
                const items = withScripts.filter((x) => x.f.kind === g.kind);
                if (items.length === 0) return null;
                return (
                  <div key={g.kind} className="health-group">
                    <div className="health-group-head">
                      <b>
                        {g.title} ({items.length})
                      </b>
                      <small>{g.hint}</small>
                    </div>
                    {items.map(({ f, key }) => (
                      <label key={key} className="health-item">
                        <input
                          type="checkbox"
                          disabled={!f.script}
                          checked={picked.has(key)}
                          onChange={(e) =>
                            setPicked((p) => {
                              const next = new Set(p);
                              if (e.target.checked) next.add(key);
                              else next.delete(key);
                              return next;
                            })
                          }
                        />
                        <span className="health-text">
                          <b>
                            {f.schema ? `${f.schema}.` : ""}
                            {f.table}
                          </b>
                          {f.index && <span className="health-index"> · {f.index}</span>}
                          <div>{f.detail}</div>
                          {f.script && <code>{f.script}</code>}
                        </span>
                      </label>
                    ))}
                  </div>
                );
              })}
            </>
          )}
        </div>
        <div className="modal-actions">
          <small>Sqlotus only reads here; scripts open in a new tab.</small>
          <span className="spacer" />
          <button onClick={load} disabled={!health && !error}>
            Check again
          </button>
          <button className="primary" onClick={openScripts} disabled={picked.size === 0}>
            Open {picked.size || ""} script(s)
          </button>
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
