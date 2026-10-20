import { useEffect, useMemo, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText } from "../api";
import { toast } from "../notify";
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

type Row = (string | null)[];

function compareCells(a: string | null, b: string | null): number {
  if (a == null || b == null) return a == null ? (b == null ? 0 : -1) : 1;
  const na = Number(a);
  const nb = Number(b);
  if (a.trim() !== "" && b.trim() !== "" && !Number.isNaN(na) && !Number.isNaN(nb)) return na - nb;
  return a.localeCompare(b, undefined, { numeric: true, sensitivity: "base" });
}

interface Cell {
  row: number;
  col: number;
}

export function ResultGrid({ rs, dialect = "mssql" }: { rs: ResultSet; dialect?: Dialect }) {
  const [scrollTop, setScrollTop] = useState(0);
  const [height, setHeight] = useState(300);
  const [selected, setSelected] = useState<Cell | null>(null);
  // Shift+click extends a range down/up the active column
  const [rangeEnd, setRangeEnd] = useState<number | null>(null);
  const [selRows, setSelRows] = useState<Set<number>>(new Set());
  const [widths, setWidths] = useState<number[]>(() => columnWidths(rs));
  const [sort, setSort] = useState<{ col: number; desc: boolean } | null>(null);
  const [filter, setFilter] = useState("");
  const [note, setNote] = useState("");

  // filtered + sorted view of the rows; selections index into it
  const rows: Row[] = useMemo(() => {
    const f = filter.trim().toLowerCase();
    let out = f ? rs.rows.filter((r) => r.some((v) => (v ?? "null").toLowerCase().includes(f))) : rs.rows;
    if (sort) {
      out = [...out].sort((a, b) => compareCells(a[sort.col], b[sort.col]) * (sort.desc ? -1 : 1));
    }
    return out;
  }, [rs, filter, sort]);

  const toggleSort = (col: number) => {
    setSelRows(new Set());
    setSelected(null);
    setSort((s) => (s?.col !== col ? { col, desc: false } : s.desc ? null : { col, desc: true }));
  };

  const saveCsv = async () => {
    try {
      const path = await save({ defaultPath: "results.csv", filters: [{ name: "CSV", extensions: ["csv"] }] });
      if (!path) return;
      await api.saveTextFile(path, toCsv(rs, targetRows()));
      setNote(`Saved ${targetRows().length} rows`);
      toast.success(`Saved ${targetRows().length} rows as CSV`, path);
    } catch (e) {
      setNote(errorText(e));
      toast.error("Could not save the CSV file", errorText(e));
    }
  };
  const ref = useRef<HTMLDivElement>(null);
  const resizing = useRef<{ col: number; x: number; w: number } | null>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(() => setHeight(el.clientHeight));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const rowNumW = Math.max(40, String(rows.length).length * 8 + 16);
  const totalW = useMemo(() => widths.reduce((a, b) => a + b, rowNumW), [widths, rowNumW]);
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((scrollTop + height) / ROW_H) + OVERSCAN);

  const range = selected && rangeEnd != null ? [Math.min(selected.row, rangeEnd), Math.max(selected.row, rangeEnd)] : null;
  const inRange = (i: number, j: number) => !!range && selected?.col === j && i >= range[0] && i <= range[1];

  // aggregates of the selected range (like SSMS add-ins' status bar totals)
  const aggregates = useMemo(() => {
    if (!range || !selected) return null;
    const values = rows.slice(range[0], range[1] + 1).map((r) => r[selected.col]);
    const nonNull = values.filter((v): v is string => v != null);
    const nums = nonNull.map(Number).filter((n) => !Number.isNaN(n));
    const fmt = (n: number) => (Number.isInteger(n) ? n.toLocaleString() : n.toLocaleString(undefined, { maximumFractionDigits: 4 }));
    if (nums.length === nonNull.length && nums.length > 0) {
      const sum = nums.reduce((a, b) => a + b, 0);
      return `Count ${values.length} · Sum ${fmt(sum)} · Avg ${fmt(sum / nums.length)} · Min ${fmt(Math.min(...nums))} · Max ${fmt(Math.max(...nums))}`;
    }
    return `Count ${values.length} · Distinct ${new Set(values).size} · Null ${values.length - nonNull.length}`;
  }, [range?.[0], range?.[1], selected?.col, rows]); // eslint-disable-line react-hooks/exhaustive-deps

  const copy = (headers: boolean) => {
    let text: string;
    if (range && selected) {
      text = rows
        .slice(range[0], range[1] + 1)
        .map((r) => r[selected.col] ?? "NULL")
        .join("\r\n");
    } else if (selRows.size > 0) {
      text = toTsv(rs, [...selRows].sort((a, b) => a - b).map((i) => rows[i]), headers);
    } else if (selected) {
      text = rows[selected.row]?.[selected.col] ?? "NULL";
    } else {
      text = toTsv(rs, rows, headers);
    }
    const what = range ? `${range[1] - range[0] + 1} cell(s)` : selRows.size > 0 ? `${selRows.size} row(s)` : selected ? "1 cell" : `${rows.length} row(s)`;
    void navigator.clipboard.writeText(text).then(
      () => toast.success(`Copied ${what}`, headers ? "With column headers" : undefined),
      (e) => toast.error("Copy failed", String(e)),
    );
  };

  const targetRows = () => (selRows.size > 0 ? [...selRows].sort((a, b) => a - b).map((i) => rows[i]) : rows);

  const copyAs = (format: string) => {
    let text = "";
    if (format === "insert") text = toInsert(rs, targetRows(), dialect);
    else if (format === "csv") text = toCsv(rs, targetRows());
    else if (format === "in") {
      const col = selected?.col ?? 0;
      const values = [...new Set(targetRows().map((r) => r[col]))];
      text = `(${values.map((v) => sqlLiteral(v, rs.columns[col].typeName, dialect)).join(", ")})`;
    }
    const label = { insert: "as INSERT statements", csv: "as CSV", in: "as an IN (…) list" }[format] ?? "";
    if (text)
      void navigator.clipboard.writeText(text).then(
        () => toast.success(`Copied ${format === "in" ? "column values" : `${targetRows().length} row(s)`} ${label}`),
        (e) => toast.error("Copy failed", String(e)),
      );
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.ctrlKey && e.key.toLowerCase() === "c") {
      e.preventDefault();
      copy(e.shiftKey);
      return;
    }
    if (e.ctrlKey && e.key.toLowerCase() === "a") {
      e.preventDefault();
      setSelRows(new Set(rows.map((_, i) => i)));
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
    const row = Math.max(0, Math.min(rows.length - 1, selected.row + m[0]));
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
          {rows.length === rs.rows.length ? rows.length.toLocaleString() : `${rows.length.toLocaleString()} of ${rs.rows.length.toLocaleString()}`} rows
          {rs.truncated ? " (truncated)" : ""} · {rs.columns.length} columns {note && <small>· {note}</small>}
          {aggregates && (
            <b className="grid-aggregates" title="Selected range (Shift+click to extend)">
              {" "}
              · {aggregates}
            </b>
          )}
        </span>
        <input className="grid-filter" value={filter} placeholder="Filter rows…" onChange={(e) => setFilter(e.target.value)} />
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
        <button onClick={saveCsv} title="Save the selected rows (or all rows) as a CSV file">
          Save CSV…
        </button>
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
            <div
              key={i}
              className="grid-cell grid-head"
              style={{ width: widths[i] }}
              title={`${c.name} ${c.typeName} — click to sort`}
              onClick={() => toggleSort(i)}
            >
              {c.name || <i>(No column name)</i>}
              {sort?.col === i && <span className="sort-mark">{sort.desc ? " ▼" : " ▲"}</span>}
              <span className="grid-resize" onMouseDown={(e) => startResize(e, i)} />
            </div>
          ))}
        </div>
        <div style={{ height: rows.length * ROW_H, width: totalW, position: "relative" }}>
          {rows.slice(first, last).map((row, k) => {
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
                    className={
                      "grid-cell" +
                      (selected?.row === i && selected.col === j ? " active" : "") +
                      (inRange(i, j) ? " in-range" : "") +
                      (v == null ? " null" : "")
                    }
                    style={{ width: widths[j] }}
                    onMouseDown={(e) => {
                      if (e.shiftKey && selected && selected.col === j) {
                        setRangeEnd(i);
                        return;
                      }
                      setRangeEnd(null);
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
