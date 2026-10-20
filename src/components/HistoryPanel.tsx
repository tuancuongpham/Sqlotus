import { useEffect, useRef, useState } from "react";
import { api } from "../api";
import type { HistoryEntry } from "../types";

function when(ms: number) {
  const d = new Date(ms);
  const today = new Date();
  const sameDay = d.toDateString() === today.toDateString();
  return sameDay ? d.toLocaleTimeString() : d.toLocaleString();
}

interface Props {
  onOpen: (entry: HistoryEntry) => void;
  onInsert: (text: string) => void;
}

export function HistoryPanel({ onOpen, onInsert }: Props) {
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
    </div>
  );
}
