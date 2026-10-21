import { useEffect, useMemo, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText } from "../api";
import { toast, update } from "../notify";
import { pin, unpin, usePinned } from "../pinned";
import type { Dialect, EditContext, EditSource, ResultSet, RowEdit } from "../types";
import { ChartDialog } from "./ChartDialog";
import { CompareDialog } from "./CompareDialog";

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

function toMarkdown(rs: ResultSet, rows: (string | null)[][]): string {
  const esc = (v: string | null) => (v == null ? "NULL" : v.replace(/\|/g, "\\|").replace(/\r?\n/g, "<br>"));
  const head = `| ${rs.columns.map((c) => esc(c.name)).join(" | ")} |`;
  const numeric = rs.columns.map((c) => NUMERIC.test(c.typeName));
  const sep = `| ${numeric.map((n) => (n ? "---:" : "---")).join(" | ")} |`;
  return [head, sep, ...rows.map((r) => `| ${r.map(esc).join(" | ")} |`)].join("\n");
}

function toJson(rs: ResultSet, rows: (string | null)[][]): string {
  const numeric = rs.columns.map((c) => NUMERIC.test(c.typeName) && !/^bit$/i.test(c.typeName));
  const objects = rows.map((r) =>
    Object.fromEntries(
      r.map((v, j) => {
        const n = v != null && numeric[j] && /^-?\d+(\.\d+)?$/.test(v) && Math.abs(Number(v)) <= Number.MAX_SAFE_INTEGER ? Number(v) : v;
        return [rs.columns[j].name || `column${j + 1}`, n];
      }),
    ),
  );
  return JSON.stringify(objects, null, 2);
}

function toHtml(rs: ResultSet, rows: (string | null)[][]): string {
  const esc = (v: string | null) => (v == null ? "<i>NULL</i>" : v.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;"));
  const head = rs.columns.map((c) => `<th>${esc(c.name)}</th>`).join("");
  const body = rows.map((r) => `<tr>${r.map((v) => `<td>${esc(v)}</td>`).join("")}</tr>`).join("\n");
  return `<table border="1" cellspacing="0" cellpadding="4">\n<thead><tr>${head}</tr></thead>\n<tbody>\n${body}\n</tbody>\n</table>`;
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

/** Pending edits of a result: changed cells, deleted and new rows. */
interface EditState {
  source: EditSource;
  /** Changed values per row (row arrays are compared by identity). */
  changes: Map<Row, Map<number, string | null>>;
  deleted: Set<Row>;
  inserted: Row[];
}

export function ResultGrid({
  rs,
  dialect = "mssql",
  label = "Results",
  edit: editContext,
}: {
  rs: ResultSet;
  dialect?: Dialect;
  label?: string;
  edit?: EditContext;
}) {
  const [dialog, setDialog] = useState<"chart" | "compare" | "script" | null>(null);
  const [edit, setEdit] = useState<EditState | null>(null);
  const [cellEdit, setCellEdit] = useState<{ row: Row; col: number; value: string } | null>(null);
  const [preview, setPreview] = useState<{ statements: string[]; problems: string[] } | null>(null);
  const [saving, setSaving] = useState(false);
  const pinned = usePinned();
  const isPinned = pinned?.rs === rs;
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
    if (edit) return [...rs.rows, ...edit.inserted]; // no sort or filter while editing
    const f = filter.trim().toLowerCase();
    let out = f ? rs.rows.filter((r) => r.some((v) => (v ?? "null").toLowerCase().includes(f))) : rs.rows;
    if (sort) {
      out = [...out].sort((a, b) => compareCells(a[sort.col], b[sort.col]) * (sort.desc ? -1 : 1));
    }
    return out;
  }, [rs, filter, sort, edit]);

  // -- editing ---------------------------------------------------------------

  const valueOf = (row: Row, j: number): string | null | undefined => {
    const changed = edit?.changes.get(row);
    if (changed?.has(j)) return changed.get(j);
    if (edit?.inserted.includes(row)) return undefined; // not set: the column default
    return row[j];
  };
  const editableCol = (j: number) => !!edit && !!edit.source.columns[j]?.column && !edit.source.columns[j].readOnly;
  const pending = edit
    ? [...edit.changes.entries()].filter(([r, m]) => m.size > 0 && !edit.deleted.has(r) && !edit.inserted.includes(r)).length +
      edit.deleted.size -
      edit.inserted.filter((r) => edit.deleted.has(r)).length +
      edit.inserted.filter((r) => !edit.deleted.has(r)).length
    : 0;

  const startEditing = async () => {
    if (!editContext) {
      toast.info("Run the query in a connected tab to edit its rows");
      return;
    }
    if (editContext.readOnly) {
      toast.info("This connection is read-only", "Turn off Read-only in the connection settings to edit rows.");
      return;
    }
    try {
      const source = await api.editableSource(
        editContext.connectionId,
        editContext.database,
        editContext.dialect,
        editContext.sql,
        rs.columns.map((c) => c.name),
      );
      setSort(null);
      setFilter("");
      setSelRows(new Set());
      setRangeEnd(null);
      setEdit({ source, changes: new Map(), deleted: new Set(), inserted: [] });
      const readOnlyCols = source.columns.filter((c) => !c.column || c.readOnly).length;
      toast.info(
        `Editing ${source.schema}.${source.table}`,
        `Double-click a cell to change it.${readOnlyCols ? ` ${readOnlyCols} column(s) are read-only (keys set by the server, expressions).` : ""} Nothing is saved until you press Save.`,
      );
    } catch (e) {
      toast.warning("These rows cannot be edited", errorText(e));
    }
  };

  const setCell = (row: Row, col: number, value: string | null) => {
    setEdit((ed) => {
      if (!ed) return ed;
      const changes = new Map(ed.changes);
      const m = new Map(changes.get(row) ?? []);
      const isNew = ed.inserted.includes(row);
      if (!isNew && value === row[col]) m.delete(col);
      else m.set(col, value);
      changes.set(row, m);
      return { ...ed, changes };
    });
  };

  const commitCell = () => {
    if (!cellEdit) return;
    setCell(cellEdit.row, cellEdit.col, cellEdit.value);
    setCellEdit(null);
  };

  const beginCell = (i: number, j: number) => {
    const row = rows[i];
    if (!edit || !row || edit.deleted.has(row)) return;
    if (!editableCol(j)) {
      toast.info("This column cannot be changed here", edit.source.columns[j]?.column ? "It is set by the server." : "It is calculated by the query.");
      return;
    }
    const v = valueOf(row, j);
    setCellEdit({ row, col: j, value: v ?? "" });
  };

  const addRow = () => {
    if (!edit) return;
    const row: Row = rs.columns.map(() => null);
    setEdit({ ...edit, inserted: [...edit.inserted, row] });
    setSelected({ row: rs.rows.length + edit.inserted.length, col: edit.source.columns.findIndex((c, j) => editableCol(j) && c) });
    window.setTimeout(() => ref.current && (ref.current.scrollTop = ref.current.scrollHeight), 0);
  };

  const toggleDelete = () => {
    if (!edit) return;
    const targets = selRows.size > 0 ? [...selRows].map((i) => rows[i]) : selected ? [rows[selected.row]] : [];
    if (targets.length === 0) {
      toast.info("Select a row first", "Click a cell or a row number.");
      return;
    }
    const deleted = new Set(edit.deleted);
    for (const r of targets) {
      if (deleted.has(r)) deleted.delete(r);
      else deleted.add(r);
    }
    setEdit({ ...edit, deleted });
  };

  const setNull = () => {
    if (!edit || !selected) return;
    const row = rows[selected.row];
    if (!editableCol(selected.col)) return;
    if (edit.source.columns[selected.col].nullable === false) {
      toast.info(`${edit.source.columns[selected.col].column} cannot be NULL`);
      return;
    }
    setCell(row, selected.col, null);
  };

  const buildEdits = (): RowEdit[] => {
    if (!edit) return [];
    const cols = edit.source.columns;
    const original = (row: Row) => Object.fromEntries(cols.flatMap((c, j) => (c.column ? [[c.column, row[j]]] : [])));
    const values = (m: Map<number, string | null> | undefined) =>
      Object.fromEntries([...(m ?? new Map<number, string | null>()).entries()].flatMap(([j, v]) => (cols[j]?.column ? [[cols[j].column!, v]] : [])));
    const out: RowEdit[] = [];
    for (const row of rs.rows) {
      if (edit.deleted.has(row)) out.push({ kind: "delete", original: original(row) });
      else if ((edit.changes.get(row)?.size ?? 0) > 0) out.push({ kind: "update", original: original(row), values: values(edit.changes.get(row)) });
    }
    for (const row of edit.inserted) if (!edit.deleted.has(row)) out.push({ kind: "insert", values: values(edit.changes.get(row)) });
    return out;
  };

  const showScript = async () => {
    if (!edit) return;
    try {
      setPreview(await api.editPreview(edit.source, buildEdits(), dialect));
      setDialog("script");
    } catch (e) {
      toast.error("Could not build the script", errorText(e));
    }
  };

  const saveEdits = async () => {
    if (!edit || !editContext) return;
    commitCell();
    const edits = buildEdits();
    if (edits.length === 0) {
      toast.info("Nothing to save");
      return;
    }
    setSaving(true);
    try {
      const r = await api.applyEdits(editContext.tabId, edit.source, edits);
      setEdit(null);
      if (r.committed) toast.success(r.notice, `${edit.source.schema}.${edit.source.table}`);
      else toast.warning(r.notice, "The tab's transaction is still open.");
      editContext.onSaved();
    } catch (e) {
      toast.error("The changes were not saved", errorText(e));
    } finally {
      setSaving(false);
    }
  };

  const cancelEditing = () => {
    setEdit(null);
    setCellEdit(null);
    toast.info(pending ? `${pending} change(s) discarded` : "Editing ended");
  };

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
  const excel = async (open: boolean) => {
    let path: string | null = null;
    if (!open) {
      path = await save({ defaultPath: "results.xlsx", filters: [{ name: "Excel workbook", extensions: ["xlsx"] }] });
      if (!path) return;
    }
    const n = targetRows().length;
    const id = toast.progress(open ? "Opening in Excel…" : "Saving the workbook…", `${n.toLocaleString()} row(s)`);
    try {
      const file = await api.exportXlsx(path, rs.columns, targetRows(), open);
      update(id, "success", open ? `Opened ${n.toLocaleString()} row(s) in Excel` : `Saved ${n.toLocaleString()} row(s) as Excel`, file);
    } catch (e) {
      update(id, "error", "Could not export to Excel", errorText(e));
    }
  };

  const togglePin = () => {
    if (isPinned) {
      unpin();
      toast.info("Result unpinned");
    } else {
      pin(label, rs);
      toast.success("Result pinned for comparison", "Run the query again (or another one), then press Compare on the new result.");
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

  // aggregates of the selected range
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
    else if (format === "markdown") text = toMarkdown(rs, targetRows());
    else if (format === "json") text = toJson(rs, targetRows());
    else if (format === "html") text = toHtml(rs, targetRows());
    else if (format === "in") {
      const col = selected?.col ?? 0;
      const values = [...new Set(targetRows().map((r) => r[col]))];
      text = `(${values.map((v) => sqlLiteral(v, rs.columns[col].typeName, dialect)).join(", ")})`;
    }
    const label =
      { insert: "as INSERT statements", csv: "as CSV", in: "as an IN (…) list", markdown: "as a Markdown table", json: "as JSON", html: "as an HTML table" }[format] ?? "";
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
    if (edit && (e.key === "Enter" || e.key === "F2")) {
      e.preventDefault();
      beginCell(selected.row, selected.col);
      return;
    }
    if (edit && e.ctrlKey && e.key === "0") {
      e.preventDefault();
      setNull();
      return;
    }
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
        {edit ? (
          <span className="grid-edit-bar">
            <b>
              ✎ {edit.source.schema}.{edit.source.table}
            </b>
            <button onClick={addRow} title="Add a row (columns you leave empty get their default)">
              ＋ Row
            </button>
            <button onClick={toggleDelete} title="Mark the selected row(s) for deletion (again to undo)">
              Delete row
            </button>
            <button onClick={setNull} disabled={!selected} title="Set the selected cell to NULL (Ctrl+0)">
              Set NULL
            </button>
            <button
              onClick={() => setEdit({ ...edit, changes: new Map(), deleted: new Set(), inserted: [] })}
              disabled={pending === 0}
              title="Undo every change"
            >
              Revert
            </button>
            <button onClick={() => void showScript()} disabled={pending === 0}>
              Script…
            </button>
            <button className="primary" onClick={() => void saveEdits()} disabled={pending === 0 || saving}>
              {saving ? "Saving…" : `Save (${pending})`}
            </button>
            <button onClick={cancelEditing}>Done</button>
          </span>
        ) : (
          <>
            <input className="grid-filter" value={filter} placeholder="Filter rows…" onChange={(e) => setFilter(e.target.value)} />
            {editContext && (
              <button onClick={() => void startEditing()} title="Edit, add or delete rows of the table this result comes from">
                ✎ Edit
              </button>
            )}
          </>
        )}
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
          <option value="markdown">Markdown table</option>
          <option value="json">JSON</option>
          <option value="html">HTML table</option>
        </select>
        <button onClick={saveCsv} title="Save the selected rows (or all rows) as a CSV file">
          Save CSV…
        </button>
        <button onClick={() => void excel(true)} title="Open the selected rows (or all rows) in Excel">
          Excel
        </button>
        <button onClick={() => void excel(false)} title="Save the selected rows (or all rows) as an Excel workbook">
          Save Excel…
        </button>
        <button onClick={() => setDialog("chart")} title="Chart the result">
          Chart…
        </button>
        <button className={isPinned ? "active" : ""} onClick={togglePin} title="Keep this result to compare a later result with">
          {isPinned ? "📌 Pinned" : "📌 Pin"}
        </button>
        <button
          onClick={() => setDialog("compare")}
          disabled={!pinned || isPinned}
          title={pinned ? `Compare with the pinned result (${pinned.label})` : "Pin a result first"}
        >
          Compare
        </button>
      </div>
      {dialog === "chart" && <ChartDialog rs={rs} onClose={() => setDialog(null)} />}
      {dialog === "compare" && pinned && (
        <CompareDialog before={pinned.rs} beforeLabel={pinned.label} after={rs} onClose={() => setDialog(null)} />
      )}
      {dialog === "script" && preview && (
        <div className="modal-backdrop" onMouseDown={() => setDialog(null)}>
          <div className="modal edit-script-modal" onMouseDown={(e) => e.stopPropagation()}>
            <div className="modal-title">Statements Save will run ({preview.statements.length})</div>
            {preview.problems.length > 0 && (
              <div className="confirm-body">
                {preview.problems.map((p, i) => (
                  <div key={i}>⚠ {p}</div>
                ))}
              </div>
            )}
            <pre className="edit-script">{preview.statements.join("\n")}</pre>
            <div className="modal-actions">
              <small>All of them run in one transaction; if any row was changed by someone else meanwhile, nothing is saved.</small>
              <span className="spacer" />
              <button onClick={() => void navigator.clipboard.writeText(preview.statements.join("\n")).then(() => toast.success("Script copied"))}>
                Copy
              </button>
              <button className="primary" onClick={() => setDialog(null)}>
                Close
              </button>
            </div>
          </div>
        </div>
      )}
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
              title={edit ? `${c.name} ${c.typeName}${editableCol(i) ? "" : " (read-only)"}` : `${c.name} ${c.typeName} — click to sort`}
              onClick={() => !edit && toggleSort(i)}
            >
              {edit && !editableCol(i) && <span className="grid-lock">🔒 </span>}
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
                className={
                  "grid-row" +
                  (selRows.has(i) ? " selected" : "") +
                  (edit?.deleted.has(row) ? " row-deleted" : "") +
                  (edit?.inserted.includes(row) ? " row-inserted" : "")
                }
                style={{ top: i * ROW_H, width: totalW }}
              >
                <div className="grid-cell grid-rownum" style={{ width: rowNumW }} onClick={(e) => clickRowNumber(i, e)}>
                  {i + 1}
                </div>
                {row.map((raw, j) => {
                  const v = edit ? valueOf(row, j) : raw;
                  const changed = !!edit?.changes.get(row)?.has(j);
                  if (cellEdit && cellEdit.row === row && cellEdit.col === j) {
                    return (
                      <div key={j} className="grid-cell cell-editing" style={{ width: widths[j] }}>
                        <input
                          autoFocus
                          value={cellEdit.value}
                          onChange={(e) => setCellEdit({ ...cellEdit, value: e.target.value })}
                          onBlur={commitCell}
                          onKeyDown={(e) => {
                            e.stopPropagation();
                            if (e.key === "Enter" || e.key === "Tab") {
                              e.preventDefault();
                              commitCell();
                              if (e.key === "Tab") setSelected({ row: i, col: Math.min(rs.columns.length - 1, j + 1) });
                              ref.current?.focus();
                            } else if (e.key === "Escape") {
                              setCellEdit(null);
                              ref.current?.focus();
                            }
                          }}
                        />
                      </div>
                    );
                  }
                  return (
                  <div
                    key={j}
                    className={
                      "grid-cell" +
                      (selected?.row === i && selected.col === j ? " active" : "") +
                      (inRange(i, j) ? " in-range" : "") +
                      (v == null ? " null" : "") +
                      (changed ? " cell-changed" : "")
                    }
                    style={{ width: widths[j] }}
                    onDoubleClick={() => edit && beginCell(i, j)}
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
                    {v === undefined ? <i className="cell-default">default</i> : v == null ? "NULL" : v}
                  </div>
                  );
                })}
              </div>
            );
          })}
        </div>
      </div>
    </div>
  );
}
