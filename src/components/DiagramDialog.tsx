import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { save } from "@tauri-apps/plugin-dialog";
import { api, errorText } from "../api";
import { toast } from "../notify";
import type { CatalogColumn, DbObject, DiagramModel, ForeignKey } from "../types";

const HEAD = 26;
const ROW = 18;
const PAD = 8;
const CHAR_W = 7.2;
const MAX_TABLES = 150;

type ColumnMode = "all" | "keys" | "none";
type Depth = 1 | 2 | 0; // 0 = whole database

interface Box {
  key: string;
  table: DbObject;
  columns: CatalogColumn[];
  fkColumns: Set<string>;
  w: number;
  h: number;
}

interface Point {
  x: number;
  y: number;
}

const keyOf = (schema: string, name: string) => `${schema}.${name}`.toLowerCase();

/** Theme colours as concrete values, so the exported SVG/PNG keeps them. */
function palette() {
  const css = getComputedStyle(document.documentElement);
  const v = (name: string, fallback: string) => css.getPropertyValue(name).trim() || fallback;
  return {
    bg: v("--bg", "#1e1e1e"),
    box: v("--bg2", "#252526"),
    head: v("--accent", "#0e639c"),
    headFg: v("--accent-fg", "#ffffff"),
    border: v("--border", "#3c3c3c"),
    fg: v("--fg", "#d4d4d4"),
    dim: v("--fg-dim", "#9a9a9a"),
    edge: "#e5a50a",
  };
}

/**
 * Force-directed layout of one connected group of tables, with rectangle
 * overlap removal: related tables end up close together, nothing overlaps.
 * Positions start at (0, 0). Deterministic for the same input.
 */
function layoutGroup(boxes: Box[], edges: [number, number][]): Point[] {
  const n = boxes.length;
  // start on a grid, most connected tables first, so neighbours start near each other
  const degree = boxes.map((_, i) => edges.filter(([a, b]) => a === i || b === i).length);
  const order = boxes.map((_, i) => i).sort((a, b) => degree[b] - degree[a]);
  const cols = Math.max(1, Math.ceil(Math.sqrt(n)));
  const pos = boxes.map(() => ({ x: 0, y: 0 }));
  order.forEach((i, k) => {
    pos[i] = { x: (k % cols) * 320, y: Math.floor(k / cols) * 260 };
  });
  const k = 260;
  let temp = 80;
  for (let iter = 0; n > 1 && iter < 300; iter++) {
    const disp = boxes.map(() => ({ x: 0, y: 0 }));
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        const dx = pos[i].x + boxes[i].w / 2 - (pos[j].x + boxes[j].w / 2);
        const dy = pos[i].y + boxes[i].h / 2 - (pos[j].y + boxes[j].h / 2);
        const d = Math.max(1, Math.hypot(dx, dy));
        const f = (k * k) / d;
        disp[i].x += (dx / d) * f;
        disp[i].y += (dy / d) * f;
        disp[j].x -= (dx / d) * f;
        disp[j].y -= (dy / d) * f;
      }
    }
    for (const [a, b] of edges) {
      const dx = pos[a].x - pos[b].x;
      const dy = pos[a].y - pos[b].y;
      const d = Math.max(1, Math.hypot(dx, dy));
      const f = (d * d) / k;
      disp[a].x -= (dx / d) * f;
      disp[a].y -= (dy / d) * f;
      disp[b].x += (dx / d) * f;
      disp[b].y += (dy / d) * f;
    }
    for (let i = 0; i < n; i++) {
      const d = Math.max(1, Math.hypot(disp[i].x, disp[i].y));
      pos[i].x += (disp[i].x / d) * Math.min(d, temp);
      pos[i].y += (disp[i].y / d) * Math.min(d, temp);
    }
    temp = Math.max(2, temp * 0.985);
  }
  // remove overlaps (with a margin for the relation lines)
  const gap = 40;
  for (let iter = 0; iter < 200; iter++) {
    let moved = false;
    for (let i = 0; i < n; i++) {
      for (let j = i + 1; j < n; j++) {
        const ox = Math.min(pos[i].x + boxes[i].w + gap, pos[j].x + boxes[j].w + gap) - Math.max(pos[i].x, pos[j].x);
        const oy = Math.min(pos[i].y + boxes[i].h + gap, pos[j].y + boxes[j].h + gap) - Math.max(pos[i].y, pos[j].y);
        if (ox <= 0 || oy <= 0) continue;
        moved = true;
        if (ox < oy) {
          const s = (pos[i].x < pos[j].x ? -1 : 1) * (ox / 2 + 1);
          pos[i].x += s;
          pos[j].x -= s;
        } else {
          const s = (pos[i].y < pos[j].y ? -1 : 1) * (oy / 2 + 1);
          pos[i].y += s;
          pos[j].y -= s;
        }
      }
    }
    if (!moved) break;
  }
  const minX = Math.min(...pos.map((p) => p.x));
  const minY = Math.min(...pos.map((p) => p.y));
  return pos.map((p) => ({ x: Math.round(p.x - minX), y: Math.round(p.y - minY) }));
}

/**
 * Lay out each group of related tables on its own, then pack the groups in
 * rows (largest first) so unrelated tables do not drift far away.
 */
function layout(boxes: Box[], fks: ForeignKey[]): Map<string, Point> {
  const index = new Map(boxes.map((b, i) => [b.key, i]));
  const parent = boxes.map((_, i) => i);
  const root = (i: number): number => (parent[i] === i ? i : (parent[i] = root(parent[i])));
  const edges: [number, number][] = [];
  for (const f of fks) {
    const a = index.get(keyOf(f.fromSchema, f.fromTable));
    const b = index.get(keyOf(f.toSchema, f.toTable));
    if (a == null || b == null || a === b) continue;
    edges.push([a, b]);
    parent[root(a)] = root(b);
  }
  const groups = new Map<number, number[]>();
  boxes.forEach((_, i) => groups.set(root(i), [...(groups.get(root(i)) ?? []), i]));
  const placed = [...groups.values()]
    .map((members) => {
      const local = new Map(members.map((m, i) => [m, i]));
      const groupEdges = edges.filter(([a, b]) => local.has(a) && local.has(b)).map(([a, b]) => [local.get(a)!, local.get(b)!] as [number, number]);
      const pos = layoutGroup(members.map((m) => boxes[m]), groupEdges);
      const w = Math.max(...members.map((m, i) => pos[i].x + boxes[m].w));
      const h = Math.max(...members.map((m, i) => pos[i].y + boxes[m].h));
      return { members, pos, w, h };
    })
    .sort((a, b) => b.members.length - a.members.length || b.h - a.h);
  const area = placed.reduce((s, g) => s + (g.w + 60) * (g.h + 60), 0);
  const rowLimit = Math.max(1200, Math.sqrt(area) * 1.6, ...placed.map((g) => g.w));
  const out = new Map<string, Point>();
  let x = 20;
  let y = 20;
  let rowH = 0;
  for (const g of placed) {
    if (x > 20 && x + g.w > rowLimit) {
      x = 20;
      y += rowH + 60;
      rowH = 0;
    }
    g.members.forEach((m, i) => out.set(boxes[m].key, { x: x + g.pos[i].x, y: y + g.pos[i].y }));
    x += g.w + 60;
    rowH = Math.max(rowH, g.h);
  }
  return out;
}

/** Entity-relationship diagram of the tables of a database. */
export function DiagramDialog({
  connectionId,
  database,
  focus: initialFocus,
  onClose,
}: {
  connectionId: string;
  database: string;
  focus?: { schema: string; name: string };
  onClose: () => void;
}) {
  const [model, setModel] = useState<DiagramModel | null>(null);
  const [error, setError] = useState("");
  const [focus, setFocus] = useState<string | null>(initialFocus ? keyOf(initialFocus.schema, initialFocus.name) : null);
  const [depth, setDepth] = useState<Depth>(initialFocus ? 1 : 0);
  const [mode, setMode] = useState<ColumnMode>("all");
  const [filter, setFilter] = useState("");
  const [positions, setPositions] = useState<Map<string, Point>>(new Map());
  const [selected, setSelected] = useState<string | null>(null);
  const [view, setView] = useState({ x: 0, y: 0, zoom: 1 });
  const svgRef = useRef<SVGSVGElement>(null);
  const contentRef = useRef<SVGGElement>(null);
  const drag = useRef<{ kind: "pan" | "node"; key?: string; start: Point; origin: Point; moved: boolean } | null>(null);
  const colors = useMemo(palette, []);

  useEffect(() => {
    api
      .diagramModel(connectionId, database)
      .then(setModel)
      .catch((e) => setError(errorText(e)));
  }, [connectionId, database]);

  // which tables to show: a table and its neighbours, a filter, or everything
  const { boxes, fks, truncated } = useMemo(() => {
    if (!model) return { boxes: [] as Box[], fks: [] as ForeignKey[], truncated: 0 };
    const byKey = new Map(model.tables.map((t) => [keyOf(t.schema, t.name), t]));
    const neighbours = new Map<string, Set<string>>();
    for (const f of model.foreignKeys) {
      const a = keyOf(f.fromSchema, f.fromTable);
      const b = keyOf(f.toSchema, f.toTable);
      if (!neighbours.has(a)) neighbours.set(a, new Set());
      if (!neighbours.has(b)) neighbours.set(b, new Set());
      neighbours.get(a)!.add(b);
      neighbours.get(b)!.add(a);
    }
    let keys: string[];
    if (focus && byKey.has(focus) && depth > 0) {
      const seen = new Set([focus]);
      let frontier = [focus];
      for (let d = 0; d < depth; d++) {
        const next: string[] = [];
        for (const k of frontier) for (const nb of neighbours.get(k) ?? []) if (!seen.has(nb)) (seen.add(nb), next.push(nb));
        frontier = next;
      }
      keys = [...seen];
    } else {
      keys = [...byKey.keys()];
    }
    const f = filter.trim().toLowerCase();
    if (f) keys = keys.filter((k) => k.includes(f));
    const degree = (k: string) => neighbours.get(k)?.size ?? 0;
    keys.sort((a, b) => degree(b) - degree(a) || a.localeCompare(b));
    const truncated = Math.max(0, keys.length - MAX_TABLES);
    keys = keys.slice(0, MAX_TABLES);
    const shown = new Set(keys);
    const fks = model.foreignKeys.filter((x) => shown.has(keyOf(x.fromSchema, x.fromTable)) && shown.has(keyOf(x.toSchema, x.toTable)));
    const boxes = keys.map((k) => {
      const t = byKey.get(k)!;
      const fkColumns = new Set(
        model.foreignKeys.filter((x) => keyOf(x.fromSchema, x.fromTable) === k).flatMap((x) => x.fromColumns.map((c) => c.toLowerCase())),
      );
      const columns =
        mode === "none" ? [] : mode === "keys" ? t.columns.filter((c) => c.isPrimaryKey || fkColumns.has(c.name.toLowerCase())) : t.columns;
      const longest = Math.max(`${t.schema}.${t.name}`.length + 2, ...columns.map((c) => c.name.length + c.dataType.length + 5));
      return {
        key: k,
        table: t,
        columns,
        fkColumns,
        w: Math.round(Math.min(360, Math.max(170, longest * CHAR_W + PAD * 2))),
        h: HEAD + columns.length * ROW + (columns.length ? PAD / 2 : 0),
      };
    });
    return { boxes, fks, truncated };
  }, [model, focus, depth, mode, filter]);

  const fit = useCallback(
    (pos: Map<string, Point>) => {
      const svg = svgRef.current;
      if (!svg || boxes.length === 0) return;
      const xs = boxes.map((b) => pos.get(b.key)!);
      const maxX = Math.max(...boxes.map((b, i) => xs[i].x + b.w)) + 20;
      const maxY = Math.max(...boxes.map((b, i) => xs[i].y + b.h)) + 20;
      const { width, height } = svg.getBoundingClientRect();
      const zoom = Math.max(0.2, Math.min(1.25, Math.min(width / maxX, height / maxY)));
      setView({ zoom, x: (width - maxX * zoom) / 2, y: (height - maxY * zoom) / 2 });
    },
    [boxes],
  );

  useEffect(() => {
    if (boxes.length === 0) return;
    const pos = layout(boxes, fks);
    setPositions(pos);
    fit(pos);
  }, [boxes, fks]); // eslint-disable-line react-hooks/exhaustive-deps

  const toDiagram = (e: { clientX: number; clientY: number }): Point => {
    const r = svgRef.current!.getBoundingClientRect();
    return { x: (e.clientX - r.left - view.x) / view.zoom, y: (e.clientY - r.top - view.y) / view.zoom };
  };

  const onMouseDown = (e: React.MouseEvent, key?: string) => {
    e.stopPropagation();
    if (e.button !== 0) return;
    const start = key ? toDiagram(e) : { x: e.clientX, y: e.clientY };
    const origin = key ? positions.get(key)! : { x: view.x, y: view.y };
    drag.current = { kind: key ? "node" : "pan", key, start, origin, moved: false };
  };

  const onMouseMove = (e: React.MouseEvent) => {
    const d = drag.current;
    if (!d) return;
    if (d.kind === "pan") {
      const dx = e.clientX - d.start.x;
      const dy = e.clientY - d.start.y;
      if (Math.abs(dx) + Math.abs(dy) > 2) d.moved = true;
      setView((v) => ({ ...v, x: d.origin.x + dx, y: d.origin.y + dy }));
    } else if (d.key) {
      const p = toDiagram(e);
      const dx = p.x - d.start.x;
      const dy = p.y - d.start.y;
      if (Math.abs(dx) + Math.abs(dy) > 2) d.moved = true;
      setPositions((m) => new Map(m).set(d.key!, { x: d.origin.x + dx, y: d.origin.y + dy }));
    }
  };

  const onMouseUp = () => {
    const d = drag.current;
    drag.current = null;
    if (!d || d.moved) return;
    setSelected(d.kind === "node" ? (d.key ?? null) : null);
  };

  const onWheel = (e: React.WheelEvent) => {
    const r = svgRef.current!.getBoundingClientRect();
    const cx = e.clientX - r.left;
    const cy = e.clientY - r.top;
    setView((v) => {
      const zoom = Math.max(0.15, Math.min(3, v.zoom * (e.deltaY < 0 ? 1.1 : 1 / 1.1)));
      return { zoom, x: cx - ((cx - v.x) * zoom) / v.zoom, y: cy - ((cy - v.y) * zoom) / v.zoom };
    });
  };

  // relation lines from the child column to the parent column
  const edgePath = (f: ForeignKey) => {
    const from = boxes.find((b) => b.key === keyOf(f.fromSchema, f.fromTable));
    const to = boxes.find((b) => b.key === keyOf(f.toSchema, f.toTable));
    const pf = from && positions.get(from.key);
    const pt = to && positions.get(to.key);
    if (!from || !to || !pf || !pt) return null;
    const rowY = (b: Box, p: Point, col: string) => {
      const i = b.columns.findIndex((c) => c.name.toLowerCase() === col.toLowerCase());
      return i >= 0 ? p.y + HEAD + i * ROW + ROW / 2 : p.y + HEAD / 2;
    };
    const y1 = rowY(from, pf, f.fromColumns[0] ?? "");
    const y2 = rowY(to, pt, f.toColumns[0] ?? "");
    if (from === to) {
      const x = pf.x + from.w;
      return `M ${x} ${y1} C ${x + 50} ${y1}, ${x + 50} ${y2}, ${x} ${y2}`;
    }
    const rightward = pt.x + to.w / 2 >= pf.x + from.w / 2;
    const x1 = rightward ? pf.x + from.w : pf.x;
    const x2 = rightward ? pt.x : pt.x + to.w;
    const bend = Math.max(40, Math.abs(x2 - x1) / 2);
    const s = rightward ? 1 : -1;
    return `M ${x1} ${y1} C ${x1 + s * bend} ${y1}, ${x2 - s * bend} ${y2}, ${x2} ${y2}`;
  };

  const related = useMemo(() => {
    if (!selected) return null;
    const set = new Set([selected]);
    for (const f of fks) {
      const a = keyOf(f.fromSchema, f.fromTable);
      const b = keyOf(f.toSchema, f.toTable);
      if (a === selected) set.add(b);
      if (b === selected) set.add(a);
    }
    return set;
  }, [selected, fks]);

  /** The diagram as a standalone SVG document. */
  const svgDocument = () => {
    const content = contentRef.current;
    if (!content || boxes.length === 0) return null;
    const maxX = Math.max(...boxes.map((b) => (positions.get(b.key)?.x ?? 0) + b.w)) + 40;
    const maxY = Math.max(...boxes.map((b) => (positions.get(b.key)?.y ?? 0) + b.h)) + 40;
    const inner = content.innerHTML;
    return {
      width: maxX,
      height: maxY,
      text: `<svg xmlns="http://www.w3.org/2000/svg" width="${maxX}" height="${maxY}" viewBox="0 0 ${maxX} ${maxY}" font-family="Cascadia Mono, Consolas, monospace" font-size="12"><defs><marker id="fk-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse"><path d="M 0 0 L 10 5 L 0 10 z" fill="${colors.edge}"/></marker></defs><rect width="100%" height="100%" fill="${colors.bg}"/>${inner}</svg>`,
    };
  };

  const copyImage = async () => {
    const doc = svgDocument();
    if (!doc) return;
    try {
      const url = URL.createObjectURL(new Blob([doc.text], { type: "image/svg+xml" }));
      const img = new Image();
      await new Promise((resolve, reject) => {
        img.onload = resolve;
        img.onerror = reject;
        img.src = url;
      });
      const scale = 2;
      const canvas = document.createElement("canvas");
      canvas.width = doc.width * scale;
      canvas.height = doc.height * scale;
      const ctx = canvas.getContext("2d")!;
      ctx.scale(scale, scale);
      ctx.drawImage(img, 0, 0);
      URL.revokeObjectURL(url);
      const blob = await new Promise<Blob | null>((r) => canvas.toBlob(r, "image/png"));
      if (!blob) throw new Error("could not render the image");
      await navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]);
      toast.success("Diagram copied as an image", "Paste it into a document, Teams or an e-mail.");
    } catch (e) {
      toast.error("Could not copy the diagram", errorText(e));
    }
  };

  const saveSvg = async () => {
    const doc = svgDocument();
    if (!doc) return;
    try {
      const path = await save({ defaultPath: `${database}-diagram.svg`, filters: [{ name: "SVG image", extensions: ["svg"] }] });
      if (!path) return;
      await api.saveTextFile(path, doc.text);
      toast.success("Diagram saved", path);
    } catch (e) {
      toast.error("Could not save the diagram", errorText(e));
    }
  };

  const focusName = focus ? model?.tables.find((t) => keyOf(t.schema, t.name) === focus) : undefined;

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal diagram-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">
          Diagram — {database}
          {focusName && depth > 0 && (
            <small>
              {" "}
              · around {focusName.schema}.{focusName.name}
            </small>
          )}
        </div>
        <div className="diagram-controls">
          <input value={filter} placeholder="Filter tables…" onChange={(e) => setFilter(e.target.value)} />
          <label>
            Show
            <select value={depth} onChange={(e) => setDepth(Number(e.target.value) as Depth)} disabled={!focus}>
              <option value={1}>related tables</option>
              <option value={2}>2 levels of relations</option>
              <option value={0}>whole database</option>
            </select>
          </label>
          <label>
            Columns
            <select value={mode} onChange={(e) => setMode(e.target.value as ColumnMode)}>
              <option value="all">all</option>
              <option value="keys">keys only</option>
              <option value="none">none</option>
            </select>
          </label>
          <button onClick={() => fit(positions)}>Fit</button>
          <button onClick={() => setPositions(layout(boxes, fks))} title="Arrange the tables again">
            Re-arrange
          </button>
          <span className="spacer" />
          <small>
            {boxes.length} table(s), {fks.length} relation(s)
            {truncated > 0 && ` · ${truncated} more hidden — filter or pick a table`}
          </small>
        </div>
        <div className="diagram-body">
          {error ? (
            <div className="status-error">{error}</div>
          ) : !model ? (
            <div className="panel-empty">
              <span className="spinner" /> Loading tables and relations…
            </div>
          ) : boxes.length === 0 ? (
            <div className="panel-empty">No tables match.</div>
          ) : (
            <svg
              ref={svgRef}
              className="diagram-svg"
              onMouseDown={(e) => onMouseDown(e)}
              onMouseMove={onMouseMove}
              onMouseUp={onMouseUp}
              onMouseLeave={() => (drag.current = null)}
              onWheel={onWheel}
            >
              <defs>
                <marker id="fk-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
                  <path d="M 0 0 L 10 5 L 0 10 z" fill={colors.edge} />
                </marker>
              </defs>
              <g transform={`translate(${view.x},${view.y}) scale(${view.zoom})`}>
                <g ref={contentRef}>
                  {fks.map((f, i) => {
                    const d = edgePath(f);
                    if (!d) return null;
                    const dim = related && !(related.has(keyOf(f.fromSchema, f.fromTable)) && related.has(keyOf(f.toSchema, f.toTable)));
                    return (
                      <path key={i} d={d} fill="none" stroke={colors.edge} strokeWidth={1.6} opacity={dim ? 0.15 : 0.85} markerEnd="url(#fk-arrow)">
                        <title>
                          {f.name}: {f.fromTable}({f.fromColumns.join(", ")}) → {f.toTable}({f.toColumns.join(", ")})
                        </title>
                      </path>
                    );
                  })}
                  {boxes.map((b) => {
                    const p = positions.get(b.key);
                    if (!p) return null;
                    const dim = related && !related.has(b.key);
                    const isSel = selected === b.key;
                    return (
                      <g
                        key={b.key}
                        className="diagram-table"
                        transform={`translate(${p.x},${p.y})`}
                        opacity={dim ? 0.3 : 1}
                        onMouseDown={(e) => onMouseDown(e, b.key)}
                        onDoubleClick={() => {
                          setFocus(b.key);
                          setDepth((d) => (d === 0 ? 1 : d));
                        }}
                      >
                        <rect width={b.w} height={b.h} rx={5} fill={colors.box} stroke={isSel ? colors.edge : colors.border} strokeWidth={isSel ? 2 : 1} />
                        <path d={`M0 5 a5 5 0 0 1 5 -5 h${b.w - 10} a5 5 0 0 1 5 5 v${HEAD - 5} h-${b.w} z`} fill={colors.head} />
                        <text x={PAD} y={17} fill={colors.headFg} fontWeight="bold">
                          <tspan opacity={0.75}>{b.table.schema}.</tspan>
                          {b.table.name}
                        </text>
                        {b.columns.map((c, i) => {
                          const fk = b.fkColumns.has(c.name.toLowerCase());
                          return (
                            <g key={c.name} transform={`translate(0,${HEAD + i * ROW})`}>
                              <text x={PAD} y={13} fill={c.isPrimaryKey ? colors.edge : fk ? colors.head : colors.dim} fontSize={10}>
                                {c.isPrimaryKey ? "PK" : fk ? "FK" : ""}
                              </text>
                              <text x={PAD + 22} y={13} fill={colors.fg} fontWeight={c.isPrimaryKey ? "bold" : "normal"}>
                                {c.name}
                              </text>
                              <text x={b.w - PAD} y={13} fill={colors.dim} textAnchor="end" fontSize={11}>
                                {c.dataType}
                                {c.nullable ? "" : " ●"}
                              </text>
                            </g>
                          );
                        })}
                        <title>
                          {b.table.schema}.{b.table.name} — click to highlight relations, double-click to show its neighbours
                        </title>
                      </g>
                    );
                  })}
                </g>
              </g>
            </svg>
          )}
        </div>
        <div className="modal-actions">
          <small>Drag tables or the background · wheel to zoom · ● = NOT NULL · double-click a table to focus on it</small>
          <span className="spacer" />
          {focus && (
            <button
              onClick={() => {
                setFocus(null);
                setDepth(0);
              }}
            >
              Whole database
            </button>
          )}
          <button onClick={() => void copyImage()} disabled={boxes.length === 0}>
            Copy image
          </button>
          <button onClick={() => void saveSvg()} disabled={boxes.length === 0}>
            Save SVG…
          </button>
          <button className="primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
