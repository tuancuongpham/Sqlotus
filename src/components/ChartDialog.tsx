import { useEffect, useMemo, useRef, useState } from "react";
import { Chart, registerables } from "chart.js";
import { toast } from "../notify";
import type { ResultSet } from "../types";

Chart.register(...registerables);

const MAX_POINTS = 1000;
const PALETTE = ["#4e79a7", "#f28e2b", "#e15759", "#76b7b2", "#59a14f", "#edc948", "#b07aa1", "#ff9da7", "#9c755f", "#bab0ac"];

type Kind = "bar" | "line" | "pie";

/** Columns whose non-null values are all numbers. */
function numericColumns(rs: ResultSet): number[] {
  return rs.columns
    .map((_, j) => j)
    .filter((j) => {
      const vals = rs.rows.slice(0, 200).map((r) => r[j]).filter((v): v is string => v != null);
      return vals.length > 0 && vals.every((v) => v.trim() !== "" && !Number.isNaN(Number(v)));
    });
}

/** A quick chart of a result set: one label column, one or more value columns. */
export function ChartDialog({ rs, onClose }: { rs: ResultSet; onClose: () => void }) {
  const numeric = useMemo(() => numericColumns(rs), [rs]);
  const [kind, setKind] = useState<Kind>("bar");
  const [labelCol, setLabelCol] = useState(() => rs.columns.findIndex((_, j) => !numeric.includes(j)));
  const [valueCols, setValueCols] = useState<number[]>(() => numeric.filter((j) => j !== labelCol).slice(0, 1));
  const canvas = useRef<HTMLCanvasElement>(null);
  const chart = useRef<Chart | null>(null);

  useEffect(() => {
    if (!canvas.current || valueCols.length === 0) return;
    const rows = rs.rows.slice(0, MAX_POINTS);
    const css = getComputedStyle(document.documentElement);
    const fg = css.getPropertyValue("--fg").trim() || "#ccc";
    const grid = css.getPropertyValue("--border").trim() || "#444";
    chart.current?.destroy();
    chart.current = new Chart(canvas.current, {
      type: kind,
      data: {
        labels: rows.map((r, i) => (labelCol >= 0 ? (r[labelCol] ?? "NULL") : String(i + 1))),
        datasets: valueCols.map((j, k) => ({
          label: rs.columns[j].name,
          data: rows.map((r) => (r[j] == null ? null : Number(r[j]))) as number[],
          backgroundColor: kind === "pie" ? rows.map((_, i) => PALETTE[i % PALETTE.length]) : PALETTE[k % PALETTE.length],
          borderColor: PALETTE[k % PALETTE.length],
          borderWidth: kind === "line" ? 2 : 0,
          tension: 0.2,
        })),
      },
      options: {
        responsive: true,
        maintainAspectRatio: false,
        animation: false,
        color: fg,
        plugins: { legend: { labels: { color: fg } } },
        scales:
          kind === "pie"
            ? {}
            : {
                x: { ticks: { color: fg, maxRotation: 60 }, grid: { color: grid } },
                y: { ticks: { color: fg }, grid: { color: grid }, beginAtZero: true },
              },
      },
    });
    return () => {
      chart.current?.destroy();
      chart.current = null;
    };
  }, [rs, kind, labelCol, valueCols]);

  const copyImage = () => {
    canvas.current?.toBlob((blob) => {
      if (!blob) return;
      navigator.clipboard.write([new ClipboardItem({ "image/png": blob })]).then(
        () => toast.success("Chart copied as an image", "Paste it into Excel, Word, Teams or an e-mail."),
        (e) => toast.error("Could not copy the chart", String(e)),
      );
    }, "image/png");
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal chart-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">Chart</div>
        <div className="chart-controls">
          <label>
            Type
            <select value={kind} onChange={(e) => setKind(e.target.value as Kind)}>
              <option value="bar">Bar</option>
              <option value="line">Line</option>
              <option value="pie">Pie</option>
            </select>
          </label>
          <label>
            Labels
            <select value={labelCol} onChange={(e) => setLabelCol(Number(e.target.value))}>
              <option value={-1}>(row number)</option>
              {rs.columns.map((c, j) => (
                <option key={j} value={j}>
                  {c.name || `column ${j + 1}`}
                </option>
              ))}
            </select>
          </label>
          <span className="chart-values">
            Values:
            {numeric.length === 0 && <i> no numeric columns</i>}
            {numeric.map((j) => (
              <label key={j} className="check">
                <input
                  type="checkbox"
                  checked={valueCols.includes(j)}
                  onChange={(e) => setValueCols((v) => (e.target.checked ? [...v, j] : v.filter((x) => x !== j)))}
                />
                {rs.columns[j].name || `column ${j + 1}`}
              </label>
            ))}
          </span>
        </div>
        <div className="chart-body">
          {valueCols.length === 0 ? <div className="panel-empty">Choose at least one numeric column.</div> : <canvas ref={canvas} />}
        </div>
        <div className="modal-actions">
          {rs.rows.length > MAX_POINTS && <small>Showing the first {MAX_POINTS} of {rs.rows.length} rows.</small>}
          <span className="spacer" />
          <button onClick={copyImage} disabled={valueCols.length === 0}>
            Copy image
          </button>
          <button className="primary" onClick={onClose}>
            Close
          </button>
        </div>
      </div>
    </div>
  );
}
