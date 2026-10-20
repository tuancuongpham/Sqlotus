import { useMemo, useState } from "react";

interface Props {
  databases: string[];
  current?: string;
  onRun: (databases: string[]) => void;
  onClose: () => void;
}

/** Pick databases to run the current script against. */
export function MultiDbDialog({ databases, current, onRun, onClose }: Props) {
  const [picked, setPicked] = useState<Set<string>>(() => new Set(current ? [current] : []));
  const [filter, setFilter] = useState("");
  const visible = useMemo(
    () => databases.filter((d) => d.toLowerCase().includes(filter.trim().toLowerCase())),
    [databases, filter],
  );
  const toggle = (d: string) =>
    setPicked((p) => {
      const n = new Set(p);
      if (n.has(d)) n.delete(d);
      else n.add(d);
      return n;
    });
  const allVisible = visible.length > 0 && visible.every((d) => picked.has(d));

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal multi-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Run on multiple databases</div>
        <div className="panel-search-row">
          <input value={filter} placeholder="Filter databases…" onChange={(e) => setFilter(e.target.value)} autoFocus />
        </div>
        <div className="multi-list">
          <label className="check">
            <input
              type="checkbox"
              checked={allVisible}
              onChange={() =>
                setPicked((p) => {
                  const n = new Set(p);
                  visible.forEach((d) => (allVisible ? n.delete(d) : n.add(d)));
                  return n;
                })
              }
            />
            <b>All shown</b>
          </label>
          {visible.map((d) => (
            <label key={d} className="check">
              <input type="checkbox" checked={picked.has(d)} onChange={() => toggle(d)} />
              {d}
              {d === current && <small> (current)</small>}
            </label>
          ))}
        </div>
        <div className="modal-actions">
          <small>Results get a Database column; same-shaped results are combined.</small>
          <span className="spacer" />
          <button onClick={onClose}>Cancel</button>
          <button
            className="primary"
            disabled={picked.size === 0}
            onClick={() => onRun(databases.filter((d) => picked.has(d)))}
          >
            Run on {picked.size} database{picked.size === 1 ? "" : "s"}
          </button>
        </div>
      </div>
    </div>
  );
}
