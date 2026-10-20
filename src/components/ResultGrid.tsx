import { useEffect, useMemo, useRef, useState } from "react";
import type { Dialect, ResultSet } from "../types";

const ROW_H = 24;
const OVERSCAN = 10;
const CHAR_W = 7.4;

function columnWidths(rs: ResultSet): number[] {
  const sample = rs.rows.slice(0, 200);
  return rs.columns.map((c, i) => {
    let max = c.name.length;
    for (const r of sample) {
      const v = r[i];
      max = Math.max(max, v == null ? 4 : Math.min(v.length, 60));
    }
    return Math.max(60, Math.min(420, Math.round(max * CHAR_W + 20)));
  });
}

function toTsv(rs: ResultSet, rows: (string | null)[][], headers: boolean): string {
  const esc = (v: string | null) => (v == null ? "NULL" : v.replace(/\t/g, " ").replace(/\r?\n/g, " "));
  const lines = rows.map((r) => r.map(esc).join("\t"));
  if (headers) lines.unshift(rs.columns.map((c) => c.name).join("\t"));
  return lines.join("\r\n");
}

const NUMERIC = /^(tinyint|smallint|int|bigint|decimal|numeric|money|smallmoney|float|real|bit|integer|double|serial|bigserial|int[248]?|float[48]|LONGLONG|LONG|SHORT|TINY|NEWDECIMAL|DOUBLE|FLOAT)/i;

function sqlLiteral(v: string | null, typeName: string, dialect: Dialect): string {
  if (v == null) return "NULL";
  const numeric = typeName ? NUMERIC.test(typeName) : /^-?\d+(\.\d+)?$/.test(v);
  if (numeric && /^-?\d+(\.\d+)?(e-?\d+)?$/i.test(v)) return v;
  if (typeName === "varbinary" || typeName === "binary") return v;
  const quoted = `'${v.replace(/'/g, "''")}'`;
  return dialect === "mssql" ? `N${quoted}` : quoted;
}

/** Rows as INSERT statements (1000-row VALUES batches, the SQL Server limit). */
function toInsert(rs: ResultSet, rows: (string | null)[][], dialect: Dialect): string {
  const q = (n: string) => (dialect === "mssql" ? `[${n.replace(/]/g, "]]")}]` : dialect === "mysql" ? `\`${n}\`` : `"${n}"`);
  const target = dialect === "mssql" ? "#Results" : "results";
  const cols = rs.columns.map((c) => q(c.name || "column")).join(", ");
  const parts: string[] = [];
  for (let i = 0; i < rows.length; i += 1000) {
    const values = rows
      .slice(i, i + 1000)
      .map((r) => `(${r.map((v, j) => sqlLiteral(v, rs.columns[j].typeName, dialect)).join(", ")})`)
      .join(",\n       ");
    parts.push(`INSERT INTO ${target} (${cols})\nVALUES ${values};`);
  }
  return parts.join("\n\n");
}

function toCsv(rs: ResultSet, rows: (string | null)[][]): string {
  const esc = (v: string | null) => (v == null ? "" : /[",\r\n]/.test(v) ? `"${v.replace(/"/g, '""')}"` : v);
  return [rs.columns.map((c) => esc(c.name)).join(","), ...rows.map((r) => r.map(esc).join(","))].join("\r\n");
}

interface Cell {
  row: number;
  col: number;
}

export function ResultGrid({ rs, dialect = "mssql" }: { rs: ResultSet; dialect?: Dialect }) {
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(300);
  const [selected, setSelected] = useState<Cell | null>(null);
  const [selRows, setSelRows] = useState<Set<number>>(new Set());
  const [widths, setWidths] = useState<number[]>(() => columnWidths(rs));
  const ref = useRef<HTMLDivElement>(null);
  const resizing = useRef<{ col: number; x: number; w: number } | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const rowNumW = Math.max(40, String(rs.rows.length).length * 8 + 16);
  const totalW = useMemo(() => widths.reduce((a, b) => a + b, rowNumW), [widths, rowNumW]);
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const last = Math.min(rs.rows.length, Math.ceil((scrollTop + height) / ROW_H) + OVERSCAN);

  const copy = (headers: boolean) => {
    let text: string;
    if (selRows.size > 0) {
      text = toTsv(rs, [...selRows].sort((a, b) => a - b).map((i) => rs.rows[i]), headers);
    } else if (selected) {
      text = rs.rows[selected.row]?.[selected.col] ?? "NULL";
    } else {
      text = toTsv(rs, rs.rows, headers);
    }
    void navigator.clipboard.writeText(text);
  };

  const targetRows = () => (selRows.size > 0 ? [...selRows].sort((a, b) => a - b).map((i) => rs.rows[i]) : rs.rows);

  const copyAs = (format: string) => {
    let text = "";
    if (format === "insert") text = toInsert(rs, targetRows(), dialect);
    else if (format === "csv") text = toCsv(rs, targetRows());
    else if (format === "in") {
      const col = selected?.col ?? 0;
      const values = [...new Set(targetRows().map((r) => r[col]))];
      text = `(${values.map((v) => sqlLiteral(v, rs.columns[col].typeName, dialect)).join(", ")})`;
    }
    if (text) void navigator.clipboard.writeText(text);
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.ctrlKey && e.key.toLowerCase() === "c") {
      e.preventDefault();
      copy(e.shiftKey);
      return;
    }
    if (e.ctrlKey && e.key.toLowerCase() === "a") {
      e.preventDefault();
      setSelRows(new Set(rs.rows.map((_, i) => i)));
      return;
    }
    if (!selected) return;
    const moves: Record<string, [number, number]> = {
      ArrowUp: [-1, 0],
      ArrowDown: [1, 0],
      ArrowLeft: [0, -1],
      ArrowRight: [0, 1],
      PageUp: [-Math.floor(height / ROW_H), 0],
      PageDown: [Math.floor(height / ROW_H), 0],
    };
    const m = moves[e.key];
    if (!m) return;
    e.preventDefault();
    const row = Math.max(0, Math.min(rs.rows.length - 1, selected.row + m[0]));
    const col = Math.max(0, Math.min(rs.columns.length - 1, selected.col + m[1]));
    setSelected({ row, col });
    setSelRows(new Set());
    const el = ref.current;
    if (el) {
      const top = row * ROW_H;
      if (top < el.scrollTop) el.scrollTop = top;
      else if (top + ROW_H * 2 > el.scrollTop + el.clientHeight) el.scrollTop = top + ROW_H * 2 - el.clientHeight;
    }
  };

  const startResize = (e: React.MouseEvent, col: number) => {
    e.preventDefault();
    e.stopPropagation();
    resizing.current = { col, x: e.clientX, w: widths[col] };
    const move = (ev: MouseEvent) => {
      const r = resizing.current;
      if (!r) return;
      setWidths((ws) => ws.map((w, i) => (i === r.col ? Math.max(40, r.w + ev.clientX - r.x) : w)));
    };
    const up = () => {
      resizing.current = null;
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const clickRowNumber = (i: number, e: React.MouseEvent) => {
    setSelected(null);
    setSelRows((prev) => {
      const next = new Set(e.ctrlKey ? prev : []);
      if (e.shiftKey && prev.size > 0) {
        const anchor = Math.min(...prev);
        for (let k = Math.min(anchor, i); k <= Math.max(anchor, i); k++) next.add(k);
      } else if (next.has(i)) next.delete(i);
      else next.add(i);
      return next;
    });
  };

  return (
    <div className="grid-wrap">
      <div className="grid-toolbar">
        <span>
          {rs.rows.length.toLocaleString()} rows{rs.truncated ? " (truncated)" : ""} · {rs.columns.length} columns
        </span>
        <button onClick={() => copy(false)} title="Ctrl+C">Copy</button>
        <button onClick={() => copy(true)} title="Ctrl+Shift+C">Copy with headers</button>
        <select
          className="copy-as"
          value=""
          title="Copy the selected rows (or all rows) in another format"
          onChange={(e) => {
            copyAs(e.target.value);
            e.target.value = "";
          }}
        >
          <option value="" disabled>
            Copy as…
          </option>
          <option value="insert">INSERT statements</option>
          <option value="csv">CSV</option>
          <option value="in">IN (…) list of the column</option>
        </select>
      </div>
      <div
        className="grid"
        ref={ref}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
      >
        <div className="grid-header" style={{ width: totalW }}>
          <div className="grid-cell grid-rownum" style={{ width: rowNumW }} />
          {rs.columns.map((c, i) => (
            <div key={i} className="grid-cell grid-head" style={{ width: widths[i] }} title={`${c.name} ${c.typeName}`}>
              {c.name || <i>(No column name)</i>}
              <span className="grid-resize" onMouseDown={(e) => startResize(e, i)} />
            </div>
          ))}
        </div>
        <div style={{ height: rs.rows.length * ROW_H, width: totalW, position: "relative" }}>
          {rs.rows.slice(first, last).map((row, k) => {
            const i = first + k;
            return (
              <div
                key={i}
                className={"grid-row" + (selRows.has(i) ? " selected" : "")}
                style={{ top: i * ROW_H, width: totalW }}
              >
                <div className="grid-cell grid-rownum" style={{ width: rowNumW }} onClick={(e) => clickRowNumber(i, e)}>
                  {i + 1}
                </div>
                {row.map((v, j) => (
                  <div
                    key={j}
                    className={"grid-cell" + (selected?.row === i && selected.col === j ? " active" : "") + (v == null ? " null" : "")}
                    style={{ width: widths[j] }}
                    onMouseDown={() => {
                      setSelected({ row: i, col: j });
                      setSelRows(new Set());
                    }}
                    title={v != null && v.length > 40 ? v.slice(0, 2000) : undefined}
                  >
                    {v == null ? "NULL" : v}
                  </div>
                ))}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
