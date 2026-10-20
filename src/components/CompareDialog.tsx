import { useMemo, useState } from "react";
import type { ResultSet } from "../types";

type Row = (string | null)[];
type Kind = "added" | "removed" | "changed";

interface Diff {
  kind: Kind;
  key: string;
  /** Values per compared column: [before, after]. */
  cells: [string | null | undefined, string | null | undefined][];
}

const SHOW_MAX = 2000;

/**
 * Compare two result sets row by row, matched on key columns: rows only in
 * the pinned result (removed), only in the current one (added), and rows
 * whose other values differ (changed).
 */
export function CompareDialog({
  before,
  beforeLabel,
  after,
  onClose,
}: {
  before: ResultSet;
  beforeLabel: string;
  after: ResultSet;
  onClose: () => void;
}) {
  // columns present in both, matched by name (case-insensitive)
  const common = useMemo(
    () =>
      before.columns
        .map((c, i) => ({ name: c.name, b: i, a: after.columns.findIndex((x) => x.name.toLowerCase() === c.name.toLowerCase()) }))
        .filter((c) => c.a >= 0),
    [before, after],
  );
  const [keys, setKeys] = useState<string[]>(() => (common[0] ? [common[0].name] : []));
  const [filter, setFilter] = useState<Kind | "all">("all");

  const result = useMemo(() => {
    const keyCols = common.filter((c) => keys.includes(c.name));
    const keyOf = (r: Row, side: "a" | "b") => JSON.stringify(keyCols.map((c) => r[c[side]]));
    const left = new Map<string, Row>();
    let duplicateKeys = 0;
    for (const r of before.rows) {
      const k = keyOf(r, "b");
      if (left.has(k)) duplicateKeys++;
      left.set(k, r);
    }
    const diffs: Diff[] = [];
    let same = 0;
    const seen = new Set<string>();
    for (const r of after.rows) {
      const k = keyOf(r, "a");
      seen.add(k);
      const old = left.get(k);
      if (!old) {
        diffs.push({ kind: "added", key: k, cells: common.map((c) => [undefined, r[c.a]]) });
        continue;
      }
      const cells = common.map((c) => [old[c.b], r[c.a]] as [string | null, string | null]);
      if (cells.some(([x, y]) => x !== y)) diffs.push({ kind: "changed", key: k, cells });
      else same++;
    }
    for (const [k, r] of left) {
      if (!seen.has(k)) diffs.push({ kind: "removed", key: k, cells: common.map((c) => [r[c.b], undefined]) });
    }
    const count = (k: Kind) => diffs.filter((d) => d.kind === k).length;
    return { diffs, same, duplicateKeys, added: count("added"), removed: count("removed"), changed: count("changed") };
  }, [before, after, common, keys]);

  const shown = result.diffs.filter((d) => filter === "all" || d.kind === filter);
  const show = (v: string | null | undefined) => (v === undefined ? "" : v === null ? "NULL" : v);

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal compare-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Compare results — pinned “{beforeLabel}” → current</div>
        {common.length === 0 ? (
          <div className="panel-empty">The two results have no column names in common.</div>
        ) : (
          <>
            <div className="compare-keys">
              Match rows on:
              {common.map((c) => (
                <label key={c.name} className="check">
                  <input
                    type="checkbox"
                    checked={keys.includes(c.name)}
                    onChange={(e) => setKeys((k) => (e.target.checked ? [...k, c.name] : k.filter((x) => x !== c.name)))}
                  />
                  {c.name}
                </label>
              ))}
            </div>
            <div className="compare-summary">
              <button className={filter === "all" ? "active" : ""} onClick={() => setFilter("all")}>
                All differences ({result.diffs.length})
              </button>
              <button className={"diff-added" + (filter === "added" ? " active" : "")} onClick={() => setFilter("added")}>
                + Added {result.added}
              </button>
              <button className={"diff-removed" + (filter === "removed" ? " active" : "")} onClick={() => setFilter("removed")}>
                − Removed {result.removed}
              </button>
              <button className={"diff-changed" + (filter === "changed" ? " active" : "")} onClick={() => setFilter("changed")}>
                ≠ Changed {result.changed}
              </button>
              <span>= Same {result.same}</span>
              {result.duplicateKeys > 0 && (
                <span className="compare-warn">⚠ {result.duplicateKeys} duplicate key(s) in the pinned result — add key columns</span>
              )}
            </div>
            <div className="compare-table-wrap">
              {shown.length === 0 ? (
                <div className="panel-empty">{result.diffs.length === 0 ? "The results are identical." : "Nothing in this category."}</div>
              ) : (
                <table className="compare-table">
                  <thead>
                    <tr>
                      <th />
                      {common.map((c) => (
                        <th key={c.name}>{c.name}</th>
                      ))}
                    </tr>
                  </thead>
                  <tbody>
                    {shown.slice(0, SHOW_MAX).map((d, i) => (
                      <tr key={i} className={`diff-${d.kind}`}>
                        <td className="diff-mark">{d.kind === "added" ? "+" : d.kind === "removed" ? "−" : "≠"}</td>
                        {d.cells.map(([b, a], j) =>
                          d.kind === "changed" && b !== a ? (
                            <td key={j} className="diff-cell-changed" title={`${show(b)} → ${show(a)}`}>
                              <del>{show(b)}</del> → <ins>{show(a)}</ins>
                            </td>
                          ) : (
                            <td key={j}>{show(d.kind === "removed" ? b : a)}</td>
                          ),
                        )}
                      </tr>
                    ))}
                  </tbody>
                </table>
              )}
              {shown.length > SHOW_MAX && <div className="panel-empty">Showing the first {SHOW_MAX} of {shown.length}.</div>}
            </div>
          </>
        )}
        <div className="modal-actions">
          <small>
            Pinned: {before.rows.length} rows · Current: {after.rows.length} rows
          </small>
          <span className="spacer" />
          <button className="primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
