import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import { languageFor } from "../monaco";
import type { Dialect, HistoryEntry } from "../types";
import { DiffDialog } from "./DiffDialog";

function when(ms: number) {
  const d = new Date(ms);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  return sameDay ? d.toLocaleTimeString() : d.toLocaleString();
}

interface Props {
  onOpen: (entry: HistoryEntry) => void;
  onInsert: (text: string) => void;
  /** Text of the active editor tab, for "compare with current". */
  currentText: () => string;
}

export function HistoryPanel({ onOpen, onInsert, currentText }: Props) {
  const [diff, setDiff] = useState<{ title: string; a: string; b: string; aLabel: string; bLabel: string; dialect: string } | null>(null);
  const [note, setNote] = useState("");

  const compareWithPrevious = async (e: HistoryEntry) => {
    const versions = await api.historyTab(e.tabId);
    const prev = versions.filter((v) => v.id < e.id).sort((a, b) => b.id - a.id)[0];
    if (!prev) {
      setNote("This is the first version of that tab.");
      return;
    }
    setNote("");
    setDiff({
      title: `${e.title}: previous version → selected`,
      a: prev.text,
      b: e.text,
      aLabel: `${when(prev.updatedAt)} (${prev.kind})`,
      bLabel: `${when(e.updatedAt)} (${e.kind})`,
      dialect: e.dialect,
    });
  };

  const [query, setQuery] = useState("");
  const [mode, setMode] = useState<"all" | "closed">("all");
  const [items, setItems] = useState<HistoryEntry[]>([]);
  const [selected, setSelected] = useState<HistoryEntry | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  useEffect(() => {
    const t = setTimeout(async () => {
      const r = mode === "closed" ? await api.historyRecentlyClosed(100) : await api.historySearch(query, 300);
      const filtered =
        mode === "closed" && query
          ? r.filter((e) => e.text.toLowerCase().includes(query.toLowerCase()) || e.title.toLowerCase().includes(query.toLowerCase()))
          : r;
      setItems(filtered);
      setSelected((s) => filtered.find((x) => x.id === s?.id) ?? filtered[0] ?? null);
    }, 150);
    return () => clearTimeout(t);
  }, [query, mode]);

  const remove = async (e: HistoryEntry) => {
    await api.historyDelete(e.id);
    setItems((xs) => xs.filter((x) => x.id !== e.id));
    if (selected?.id === e.id) setSelected(null);
  };

  return (
    <div className="history-panel">
      <div className="panel-search-row">
        <input ref={inputRef} value={query} placeholder="Search query history…" onChange={(e) => setQuery(e.target.value)} />
      </div>
      <div className="search-options">
        <label className="check">
          <input type="radio" checked={mode === "all"} onChange={() => setMode("all")} />
          All history
        </label>
        <label className="check">
          <input type="radio" checked={mode === "closed"} onChange={() => setMode("closed")} />
          Recently closed tabs
        </label>
      </div>
      <div className="history-list">
        {items.map((e) => (
          <div
            key={e.id}
            className={"history-item" + (selected?.id === e.id ? " selected" : "")}
            onClick={() => setSelected(e)}
            onDoubleClick={() => onOpen(e)}
          >
            <div className="history-head">
              <b>{e.title}</b>
              <span className={"badge " + e.kind}>{e.kind === "execute" ? "executed" : "saved"}</span>
              <span className="spacer" />
              <small>{when(e.updatedAt)}</small>
            </div>
            <small className="history-meta">
              {[e.connectionName, e.database].filter(Boolean).join(" / ") || "not connected"}
            </small>
            <div className="history-snippet">{e.text.trim().split("\n").slice(0, 2).join(" ").slice(0, 160)}</div>
          </div>
        ))}
        {items.length === 0 && <div className="panel-empty">Nothing found</div>}
      </div>
      {selected && (
        <div className="search-preview">
          <div className="preview-bar">
            <b>{selected.title}</b>
            <span className="spacer" />
            <button onClick={() => navigator.clipboard.writeText(selected.text)}>Copy</button>
            <button onClick={() => compareWithPrevious(selected)} title="Diff against the version saved before it">
              Diff previous
            </button>
            <button
              onClick={() =>
                setDiff({
                  title: `${selected.title} → current tab`,
                  a: selected.text,
                  b: currentText(),
                  aLabel: `history: ${when(selected.updatedAt)}`,
                  bLabel: "current tab",
                  dialect: selected.dialect,
                })
              }
              title="Diff against the text in the current tab"
            >
              Diff current
            </button>
            <button onClick={() => onInsert(selected.text)}>Insert at cursor</button>
            <button className="primary" onClick={() => onOpen(selected)}>
              Open in new tab
            </button>
            <button className="danger" onClick={() => remove(selected)}>
              Delete
            </button>
          </div>
          <pre>{selected.text}</pre>
        </div>
      )}
      {note && <div className="panel-empty">{note}</div>}
      {diff && (
        <DiffDialog
          title={diff.title}
          original={diff.a}
          modified={diff.b}
          originalLabel={diff.aLabel}
          modifiedLabel={diff.bLabel}
          language={languageFor[(diff.dialect as Dialect) in languageFor ? (diff.dialect as Dialect) : "mssql"]}
          onClose={() => setDiff(null)}
        />
      )}
    </div>
  );
}
