import { useEffect, useMemo, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText } from "../api";
import { compareReportHtml, compareReportMarkdown, matchesFilter } from "../compareReport";
import { monaco } from "../monaco";
import { toast, update } from "../notify";
import type { Dialect, ObjectDiff, SavedCompare, SavedConnection, SchemaCompareResult } from "../types";

const LANGUAGE: Record<Dialect, string> = { mssql: "sql", postgres: "pgsql", mysql: "mysql" };
const CHANGE_LABEL: Record<ObjectDiff["change"], string> = {
  changed: "Different",
  added: "Only in source (created in target)",
  removed: "Only in target (dropped from target)",
};

interface Side {
  connectionId: string;
  database: string;
}

/** Side-by-side diff of one object's definition in both databases. */
export function ObjectDiffView({ diff, dialect }: { diff: Pick<ObjectDiff, "sourceText" | "targetText">; dialect: Dialect }) {
  const el = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const left = monaco.editor.createModel(diff.targetText ?? "", LANGUAGE[dialect]);
    const right = monaco.editor.createModel(diff.sourceText ?? "", LANGUAGE[dialect]);
    const editor = monaco.editor.createDiffEditor(el.current!, {
      automaticLayout: true,
      readOnly: true,
      renderSideBySide: true,
      minimap: { enabled: false },
      fontSize: 12,
    });
    editor.setModel({ original: left, modified: right });
    return () => {
      editor.dispose();
      left.dispose();
      right.dispose();
    };
  }, [diff, dialect]);
  return <div className="compare-diff-editor" ref={el} />;
}

/**
 * Compare the schema of two databases and build a script that makes the
 * target like the source. The script opens in a tab connected to the
 * target; nothing is run here.
 */
export function SchemaCompareDialog({
  connections: knownConnections,
  initial,
  onOpenScript,
  onClose,
}: {
  connections: SavedConnection[];
  initial?: Side;
  onOpenScript: (title: string, text: string, target: Side) => void;
  onClose: () => void;
}) {
  // the saved connections, fresh (one may have been added since the app loaded them)
  const [connections, setConnections] = useState<SavedConnection[]>(knownConnections);
  useEffect(() => {
    api.listConnections().then(setConnections).catch(() => undefined);
  }, []);
  const [source, setSource] = useState<Side>(initial ?? { connectionId: knownConnections[0]?.id ?? "", database: "" });
  const [target, setTarget] = useState<Side>(initial ?? { connectionId: knownConnections[0]?.id ?? "", database: "" });
  const [databases, setDatabases] = useState<Record<string, string[] | string>>({});
  const [ignoreWhitespace, setIgnoreWhitespace] = useState(true);
  const [ignoreCase, setIgnoreCase] = useState(false);
  const [result, setResult] = useState<SchemaCompareResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [current, setCurrent] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [showIdentical, setShowIdentical] = useState(false);
  const [saved, setSaved] = useState<SavedCompare[]>([]);
  const [savedName, setSavedName] = useState("");
  useEffect(() => {
    api.savedCompares().then(setSaved).catch(() => undefined);
  }, []);

  const connName = (id: string) => connections.find((c) => c.id === id)?.name ?? "?";
  const gone = (s: Side) => !!s.connectionId && !connections.some((c) => c.id === s.connectionId);
  const storeSaved = async (list: SavedCompare[]) => {
    setSaved(list);
    await api.saveSavedCompares(list).catch((e) => toast.error("Could not save the list", errorText(e)));
  };
  const saveCurrent = async () => {
    const name = `${connName(source.connectionId)} / ${source.database} → ${connName(target.connectionId)} / ${target.database}`;
    const entry: SavedCompare = { name, source, target, ignoreWhitespace, ignoreCase };
    await storeSaved([...saved.filter((x) => x.name !== name), entry].sort((a, b) => a.name.localeCompare(b.name)));
    setSavedName(name);
    toast.success("Comparison saved", name);
  };
  const openSaved = (name: string) => {
    setSavedName(name);
    const x = saved.find((s) => s.name === name);
    if (!x) return;
    setSource(x.source);
    setTarget(x.target);
    setIgnoreWhitespace(x.ignoreWhitespace);
    setIgnoreCase(x.ignoreCase);
    setResult(null);
    if (gone(x.source) || gone(x.target))
      toast.warning("A connection of this comparison no longer exists", "It was deleted: choose the server again, then Save to update the comparison.");
  };
  const exportReport = async () => {
    if (!result) return;
    const base = `schema-compare-${source.database}-${target.database}`.replace(/[^\w.-]+/g, "_");
    const path = await save({
      defaultPath: `${base}.html`,
      filters: [
        { name: "HTML page", extensions: ["html"] },
        { name: "Markdown", extensions: ["md"] },
      ],
    });
    if (!path) return;
    const now = new Date();
    const text = /\.md$/i.test(path) ? compareReportMarkdown(result, now) : compareReportHtml(result, now);
    try {
      await api.saveTextFile(path, text);
      toast.success("Report saved", path);
    } catch (e) {
      toast.error("Could not save the report", errorText(e));
    }
  };

  // databases of each connection used (from a test connection)
  useEffect(() => {
    for (const id of new Set([source.connectionId, target.connectionId])) {
      const conn = connections.find((c) => c.id === id);
      if (!conn || databases[id] !== undefined) continue;
      setDatabases((d) => ({ ...d, [id]: [] }));
      api
        .testConnection(conn, null)
        .then((list) => setDatabases((d) => ({ ...d, [id]: list })))
        .catch((e) => setDatabases((d) => ({ ...d, [id]: errorText(e) })));
    }
  }, [source.connectionId, target.connectionId]); // eslint-disable-line react-hooks/exhaustive-deps

  // default target: another database of the same server
  useEffect(() => {
    const list = databases[target.connectionId];
    if (Array.isArray(list) && list.length > 0 && (!target.database || (target.database === source.database && target.connectionId === source.connectionId && !result))) {
      const other = list.find((d) => d !== source.database) ?? list[0];
      if (other !== target.database) setTarget((t) => ({ ...t, database: other }));
    }
  }, [databases]); // eslint-disable-line react-hooks/exhaustive-deps

  const dialectOf = (id: string) => connections.find((c) => c.id === id)?.dialect;
  const missing = gone(source) || gone(target);
  const sameKind = missing || dialectOf(source.connectionId) === dialectOf(target.connectionId);
  const sameDb = source.connectionId === target.connectionId && source.database === target.database;

  const run = async () => {
    setBusy(true);
    setError("");
    setResult(null);
    setCurrent(null);
    const id = toast.progress("Comparing schemas…", `${source.database} → ${target.database}`);
    try {
      const r = await api.compareSchemas(source, target, { ignoreWhitespace, ignoreCase });
      setResult(r);
      // drops are opt-in: they lose data
      setPicked(new Set(r.diff.differences.filter((d) => d.change !== "removed").map((d) => d.key)));
      setCurrent(r.diff.differences[0]?.key ?? null);
      const n = r.diff.differences.length;
      if (n === 0) update(id, "success", "The schemas are identical", `${r.diff.identical} object(s) compared.`);
      else update(id, "info", `${n} difference(s)`, `${r.diff.identical} identical. Objects only in the target are not selected by default.`);
    } catch (e) {
      setError(errorText(e));
      update(id, "error", "Could not compare", errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const generate = async () => {
    if (!result) return;
    try {
      const s = await api.schemaSyncScript(result.id, [...picked]);
      onOpenScript(`Sync ${target.database}`, s.script, target);
      const w = s.warnings.length;
      toast[w ? "warning" : "success"](
        `Sync script: ${s.statements} statement(s)`,
        `${w ? `${w} warning(s) at the top of the script. ` : ""}It opened in a tab connected to ${result.target}; review it, then run it there.`,
      );
      onClose();
    } catch (e) {
      toast.error("Could not build the script", errorText(e));
    }
  };

  const groups = useMemo(() => {
    const out: Record<ObjectDiff["change"], ObjectDiff[]> = { changed: [], added: [], removed: [] };
    for (const d of result?.diff.differences ?? []) if (matchesFilter(d, filter)) out[d.change].push(d);
    return out;
  }, [result, filter]);
  const identical = useMemo(() => (result?.diff.identicalObjects ?? []).filter((o) => matchesFilter(o, filter)), [result, filter]);
  const selected = result?.diff.differences.find((d) => d.key === current) ?? null;
  // All / None act on what the filter shows; the script still takes every picked one
  const shown = useMemo(() => new Set([...groups.changed, ...groups.added, ...groups.removed].map((d) => d.key)), [groups]);
  const pickedHidden = [...picked].filter((k) => !shown.has(k)).length;
  const toggle = (key: string, on: boolean) =>
    setPicked((p) => {
      const next = new Set(p);
      if (on) next.add(key);
      else next.delete(key);
      return next;
    });

  const sidePicker = (label: string, side: Side, set: (s: Side) => void) => {
    const list = databases[side.connectionId];
    return (
      <div className="compare-side">
        <b>{label}</b>
        <select value={side.connectionId} onChange={(e) => set({ connectionId: e.target.value, database: "" })}>
          {gone(side) && <option value={side.connectionId}>(deleted connection)</option>}
          {connections.map((c) => (
            <option key={c.id} value={c.id}>
              {c.name} ({c.dialect})
            </option>
          ))}
        </select>
        {typeof list === "string" ? (
          <span className="status-error" title={list}>
            cannot list databases
          </span>
        ) : (
          <select value={side.database} onChange={(e) => set({ ...side, database: e.target.value })}>
            {!side.database && <option value="">(choose)</option>}
            {side.database && !(list ?? []).includes(side.database) && <option value={side.database}>{side.database}</option>}
            {(list ?? []).map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        )}
      </div>
    );
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal schema-compare-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Schema compare</div>
        <div className="compare-saved">
          <select value={savedName} onChange={(e) => openSaved(e.target.value)} title="Comparisons you saved">
            <option value="">{saved.length ? "Saved comparisons…" : "No saved comparisons"}</option>
            {saved.map((x) => (
              <option key={x.name} value={x.name}>
                {x.name}
              </option>
            ))}
          </select>
          <button onClick={() => void saveCurrent()} disabled={!source.database || !target.database} title="Save this source, target and options">
            Save
          </button>
          <button
            onClick={() => {
              void storeSaved(saved.filter((x) => x.name !== savedName));
              setSavedName("");
            }}
            disabled={!savedName}
          >
            Delete
          </button>
        </div>
        <div className="compare-setup">
          {sidePicker("Source", source, setSource)}
          <button
            className="compare-swap"
            title="Swap source and target"
            onClick={() => {
              setSource(target);
              setTarget(source);
              setResult(null);
            }}
          >
            ⇄
          </button>
          {sidePicker("Target (changed by the script)", target, setTarget)}
          <label className="check">
            <input type="checkbox" checked={ignoreWhitespace} onChange={(e) => setIgnoreWhitespace(e.target.checked)} />
            Ignore whitespace
          </label>
          <label className="check">
            <input type="checkbox" checked={ignoreCase} onChange={(e) => setIgnoreCase(e.target.checked)} />
            Ignore case in definitions
          </label>
          <button className="primary" onClick={() => void run()} disabled={busy || !source.database || !target.database || !sameKind || missing}>
            {busy ? "Comparing…" : "Compare"}
          </button>
        </div>
        {missing && <div className="status-error compare-note">A saved connection of this comparison was deleted: choose the server again.</div>}
        {!sameKind && <div className="status-error compare-note">Both databases must be on the same kind of server.</div>}
        {sameDb && <div className="settings-hint compare-note">Source and target are the same database: they will be identical.</div>}
        {error && <div className="status-error compare-note">{error}</div>}
        {result && (
          <div className="compare-results">
            <div className="compare-list">
              <div className="compare-summary">
                <span>
                  {result.source} → {result.target}
                </span>
                <span>· {result.diff.identical} identical</span>
                {result.diff.skipped.length > 0 && <span title={result.diff.skipped.join(", ")}>· {result.diff.skipped.length} not readable (skipped)</span>}
                <span className="spacer" />
                <input className="compare-filter" placeholder="Filter objects…" value={filter} onChange={(e) => setFilter(e.target.value)} />
                {pickedHidden > 0 && (
                  <span className="compare-hidden" title="Selected differences the filter hides are in the script too">
                    {pickedHidden} selected, not shown
                  </span>
                )}
                <button onClick={() => setPicked((p) => new Set([...p, ...shown]))} title={filter ? "Select the differences shown" : "Select every difference"}>
                  All
                </button>
                <button onClick={() => setPicked((p) => new Set([...p].filter((k) => !shown.has(k))))} title={filter ? "Clear the differences shown" : "Clear every difference"}>
                  None
                </button>
              </div>
              {result.diff.differences.length === 0 && <div className="panel-empty">✓ No differences.</div>}
              {(Object.keys(groups) as ObjectDiff["change"][]).map((change) =>
                groups[change].length === 0 ? null : (
                  <div key={change} className={`compare-group change-${change}`}>
                    <div className="compare-group-head">
                      <input
                        type="checkbox"
                        checked={groups[change].every((d) => picked.has(d.key))}
                        onChange={(e) => groups[change].forEach((d) => toggle(d.key, e.target.checked))}
                      />
                      {CHANGE_LABEL[change]} ({groups[change].length})
                    </div>
                    {groups[change].map((d) => (
                      <div key={d.key} className={"compare-item" + (d.key === current ? " active" : "")} onClick={() => setCurrent(d.key)}>
                        <input type="checkbox" checked={picked.has(d.key)} onClick={(e) => e.stopPropagation()} onChange={(e) => toggle(d.key, e.target.checked)} />
                        <span className="compare-kind">{d.kind === "table" ? "▦" : d.kind === "view" ? "◫" : d.kind === "trigger" ? "⚡" : "ƒ"}</span>
                        <span className="compare-name">
                          {d.schema ? <small>{d.schema}.</small> : null}
                          {d.name}
                        </span>
                        {d.warnings.length > 0 && (
                          <span className="compare-warn" title={d.warnings.join("\n")}>
                            ⚠ {d.warnings.length}
                          </span>
                        )}
                      </div>
                    ))}
                  </div>
                ),
              )}
              {identical.length > 0 && (
                <div className="compare-group change-identical">
                  <div className="compare-group-head compare-toggle" onClick={() => setShowIdentical((v) => !v)}>
                    {showIdentical ? "▾" : "▸"} Identical ({identical.length})
                  </div>
                  {showIdentical &&
                    identical.map((o) => (
                      <div key={o.key} className="compare-item identical" title="The same in both databases">
                        <span className="compare-kind">{o.kind === "table" ? "▦" : o.kind === "view" ? "◫" : o.kind === "trigger" ? "⚡" : "ƒ"}</span>
                        <span className="compare-name">
                          {o.schema ? <small>{o.schema}.</small> : null}
                          {o.name}
                        </span>
                      </div>
                    ))}
                </div>
              )}
            </div>
            <div className="compare-detail">
              {selected ? (
                <>
                  <div className="compare-detail-head">
                    <b>
                      {selected.schema ? `${selected.schema}.` : ""}
                      {selected.name}
                    </b>
                    <span>{CHANGE_LABEL[selected.change]}</span>
                  </div>
                  {selected.details.length > 0 && (
                    <ul className="compare-details">
                      {selected.details.map((x, i) => (
                        <li key={i}>{x}</li>
                      ))}
                    </ul>
                  )}
                  {selected.warnings.map((w, i) => (
                    <div key={i} className="health-error">
                      ⚠ {w}
                    </div>
                  ))}
                  <div className="diff-labels">
                    <span>Target (now)</span>
                    <span>Source (wanted)</span>
                  </div>
                  <ObjectDiffView diff={selected} dialect={result.dialect} />
                </>
              ) : (
                <div className="panel-empty">Select a difference to see it side by side.</div>
              )}
            </div>
          </div>
        )}
        <div className="modal-actions">
          <small>Nothing is changed here: the script opens in a tab connected to the target for you to review and run.</small>
          <span className="spacer" />
          <button onClick={() => void exportReport()} disabled={!result} title="Save the comparison as an HTML page or Markdown">
            Export report…
          </button>
          <button className="primary" onClick={() => void generate()} disabled={!result || picked.size === 0}>
            Generate script ({picked.size})
          </button>
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
