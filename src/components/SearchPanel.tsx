import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorText } from "../api";
import type { Dialect, ObjectKind, SearchHit, SearchOptions } from "../types";

const KINDS: { kind: ObjectKind; label: string }[] = [
  { kind: "table", label: "Tables" },
  { kind: "view", label: "Views" },
  { kind: "procedure", label: "Procedures" },
  { kind: "scalarFunction", label: "Functions" },
  { kind: "tableFunction", label: "Table functions" },
  { kind: "trigger", label: "Triggers" },
];

const KIND_LABEL: Record<ObjectKind, string> = {
  table: "Table",
  view: "View",
  procedure: "Procedure",
  scalarFunction: "Function",
  tableFunction: "Table function",
  trigger: "Trigger",
  synonym: "Synonym",
  sequence: "Sequence",
};

interface Props {
  connectionId?: string;
  database?: string;
  databases: string[];
  dialect: Dialect;
  onOpenScript: (title: string, text: string) => void;
}

function escapeRe(s: string) {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

function Highlighted({ text, query, opts }: { text: string; query: string; opts: SearchOptions }) {
  const parts = useMemo(() => {
    if (!query) return [text];
    const re = new RegExp(
      opts.exactMatch ? `(?<![\\w@#$])${escapeRe(query)}(?![\\w@#$])` : escapeRe(query),
      opts.caseSensitive ? "g" : "gi",
    );
    const out: (string | { m: string })[] = [];
    let last = 0;
    for (const m of text.matchAll(re)) {
      out.push(text.slice(last, m.index), { m: m[0] });
      last = (m.index ?? 0) + m[0].length;
    }
    out.push(text.slice(last));
    return out;
  }, [text, query, opts.exactMatch, opts.caseSensitive]);
  return (
    <>
      {parts.map((p, i) => (typeof p === "string" ? p : <mark key={i}>{p.m}</mark>))}
    </>
  );
}

export function SearchPanel({ connectionId, database, databases, dialect, onOpenScript }: Props) {
  const [query, setQuery] = useState("");
  const [allDbs, setAllDbs] = useState(false);
  const [opts, setOpts] = useState<SearchOptions>({
    exactMatch: false,
    caseSensitive: false,
    kinds: [],
    searchNames: true,
    searchColumns: true,
    searchDefinitions: true,
    maxResults: 1000,
  });
  const [hits, setHits] = useState<SearchHit[]>([]);
  const [selected, setSelected] = useState<SearchHit | null>(null);
  const [script, setScript] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const inputRef = useRef<HTMLInputElement>(null);
  const previewRef = useRef<HTMLPreElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  const run = async () => {
    if (!connectionId || !query.trim()) return;
    setBusy(true);
    setError("");
    try {
      const dbs = allDbs ? databases : database ? [database] : [];
      const r = await api.searchObjects(connectionId, dbs, query, opts);
      setHits(r);
      setSelected(r[0] ?? null);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  useEffect(() => {
    if (!selected || !connectionId) {
      setScript("");
      return;
    }
    api
      .objectScript(connectionId, selected.database, selected.schema, selected.name, dialect)
      .then(setScript)
      .catch((e) => setScript(`-- ${errorText(e)}`));
  }, [selected, connectionId, dialect]);

  useEffect(() => {
    // scroll the first highlight into view
    previewRef.current?.querySelector("mark")?.scrollIntoView({ block: "center" });
  }, [script]);

  const toggleKind = (k: ObjectKind) =>
    setOpts((o) => ({ ...o, kinds: o.kinds.includes(k) ? o.kinds.filter((x) => x !== k) : [...o.kinds, k] }));

  if (!connectionId) {
    return <div className="panel-empty">Connect the current tab to search its database.</div>;
  }

  return (
    <div className="search-panel">
      <div className="panel-search-row">
        <input
          ref={inputRef}
          value={query}
          placeholder="Search object names, columns and definitions…"
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => e.key === "Enter" && run()}
        />
        <button className="primary" onClick={run} disabled={busy}>
          {busy ? "…" : "Search"}
        </button>
      </div>
      <div className="search-options">
        <label className="check">
          <input type="checkbox" checked={opts.exactMatch} onChange={(e) => setOpts({ ...opts, exactMatch: e.target.checked })} />
          Exact match
        </label>
        <label className="check">
          <input
            type="checkbox"
            checked={opts.caseSensitive}
            onChange={(e) => setOpts({ ...opts, caseSensitive: e.target.checked })}
          />
          Match case
        </label>
        <label className="check">
          <input type="checkbox" checked={allDbs} onChange={(e) => setAllDbs(e.target.checked)} />
          All databases
        </label>
      </div>
      <div className="search-options">
        {KINDS.map((k) => (
          <label key={k.kind} className="check">
            <input type="checkbox" checked={opts.kinds.length === 0 || opts.kinds.includes(k.kind)} onChange={() => toggleKind(k.kind)} />
            {k.label}
          </label>
        ))}
      </div>
      {error && <div className="status-error">{error}</div>}
      <div className="search-results">
        <table>
          <thead>
            <tr>
              <th>Object</th>
              <th>Type</th>
              <th>Match</th>
              <th>Detail</th>
            </tr>
          </thead>
          <tbody>
            {hits.map((h, i) => (
              <tr
                key={i}
                className={selected === h ? "selected" : ""}
                onClick={() => setSelected(h)}
                onDoubleClick={() => script && onOpenScript(`${h.schema}.${h.name}`, script)}
              >
                <td>
                  {allDbs && <small>{h.database}.</small>}
                  {h.schema}.<b>{h.name}</b>
                </td>
                <td>{KIND_LABEL[h.kind]}</td>
                <td>
                  {h.matchedIn}
                  {h.matchCount > 1 ? ` ×${h.matchCount}` : ""}
                </td>
                <td className="detail">
                  <Highlighted text={h.detail} query={query} opts={opts} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {hits.length === 0 && !busy && <div className="panel-empty">No results</div>}
      </div>
      {selected && (
        <div className="search-preview">
          <div className="preview-bar">
            <b>
              {selected.schema}.{selected.name}
            </b>
            <span className="spacer" />
            <button onClick={() => navigator.clipboard.writeText(script)}>Copy</button>
            <button onClick={() => onOpenScript(`${selected.schema}.${selected.name}`, script)}>Open in new tab</button>
          </div>
          <pre ref={previewRef}>
            <Highlighted text={script} query={query} opts={opts} />
          </pre>
        </div>
      )}
    </div>
  );
}
