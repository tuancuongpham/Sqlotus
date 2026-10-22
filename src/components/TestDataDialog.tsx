import { useEffect, useMemo, useState } from "react";
import { api, errorText } from "../api";
import { toast } from "../notify";
import type { ColumnPlan, Dialect, Generator, TestDataOptions } from "../types";

const LABELS: Record<Generator, string> = {
  skip: "— not inserted —",
  null: "NULL",
  fullName: "Full name",
  firstName: "First name",
  lastName: "Last name",
  email: "E-mail",
  phone: "Phone",
  address: "Address",
  city: "City",
  country: "Country",
  company: "Company",
  product: "Product name",
  code: "Code (unique)",
  status: "Status",
  gender: "Gender",
  url: "Web address",
  text: "Words",
  integer: "Number",
  decimal: "Amount",
  date: "Date (last 3 years)",
  birthDate: "Birth date",
  dateTime: "Date and time",
  boolean: "True / false",
  guid: "GUID",
  sequence: "Next key (after max)",
  foreignKey: "Existing parent value",
  fixed: "Fixed value…",
};

/**
 * Generate realistic rows for a table as an INSERT script. The script opens
 * in a new tab to review and run (Sqlotus never inserts by itself).
 */
export function TestDataDialog({
  connectionId,
  database,
  schema,
  name,
  dialect,
  onOpenScript,
  onClose,
}: {
  connectionId: string;
  database: string;
  schema: string;
  name: string;
  dialect: Dialect;
  onOpenScript: (title: string, text: string) => void;
  onClose: () => void;
}) {
  const [plans, setPlans] = useState<ColumnPlan[] | null>(null);
  const [error, setError] = useState("");
  const [options, setOptions] = useState<TestDataOptions>({
    rows: 100,
    locale: "vi",
    nullPercent: 10,
    seed: 1,
    overrides: {},
    fixed: {},
  });
  const [preview, setPreview] = useState("");
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api
      .testDataPlan(connectionId, database, schema, name, dialect)
      .then(setPlans)
      .catch((e) => setError(errorText(e)));
  }, [connectionId, database, schema, name, dialect]);

  // live preview with 5 rows
  const previewKey = useMemo(() => JSON.stringify({ ...options, rows: 5 }), [options]);
  useEffect(() => {
    if (!plans) return;
    const t = window.setTimeout(() => {
      api
        .testDataScript(connectionId, database, schema, name, dialect, { ...options, rows: 5 })
        .then((s) => {
          setPreview(s);
          setError("");
        })
        .catch((e) => {
          setPreview("");
          setError(errorText(e));
        });
    }, 250);
    return () => window.clearTimeout(t);
  }, [previewKey, plans]); // eslint-disable-line react-hooks/exhaustive-deps

  const generatorOf = (p: ColumnPlan) => options.overrides[p.column] ?? p.generator;
  const setGenerator = (column: string, g: Generator) => setOptions((o) => ({ ...o, overrides: { ...o.overrides, [column]: g } }));

  const create = async () => {
    setBusy(true);
    try {
      const script = await api.testDataScript(connectionId, database, schema, name, dialect, options);
      onOpenScript(`${name} (test data)`, script);
      toast.success(
        `Test data script for ${name}: ${options.rows.toLocaleString()} row(s)`,
        "Review it, then press F5. Changes over your row limit ask before committing.",
      );
      onClose();
    } catch (e) {
      toast.error("Could not generate the test data", errorText(e));
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="modal-backdrop" onMouseDown={onClose} onKeyDown={(e) => e.key === "Escape" && onClose()}>
      <div className="modal testdata-modal" onMouseDown={(e) => e.stopPropagation()}>
        <div className="modal-title">
          Test data — {schema}.{name}
        </div>
        <div className="testdata-controls">
          <label>
            Rows
            <input
              type="number"
              min={1}
              max={100000}
              value={options.rows}
              onChange={(e) => setOptions((o) => ({ ...o, rows: Math.max(1, Math.min(100000, Math.floor(Number(e.target.value)) || 1)) }))}
            />
          </label>
          <label>
            Language
            <select value={options.locale} onChange={(e) => setOptions((o) => ({ ...o, locale: e.target.value as "vi" | "en" }))}>
              <option value="vi">Tiếng Việt</option>
              <option value="en">English</option>
            </select>
          </label>
          <label title="Share of NULLs in columns that allow NULL">
            NULLs %
            <input
              type="number"
              min={0}
              max={100}
              value={options.nullPercent}
              onChange={(e) => setOptions((o) => ({ ...o, nullPercent: Math.max(0, Math.min(100, Math.floor(Number(e.target.value)) || 0)) }))}
            />
          </label>
          <label title="The same seed always produces the same rows">
            Seed
            <input type="number" value={options.seed} onChange={(e) => setOptions((o) => ({ ...o, seed: Math.abs(Math.floor(Number(e.target.value))) || 0 }))} />
          </label>
          <button onClick={() => setOptions((o) => ({ ...o, seed: Math.floor(Math.random() * 1e6) }))}>New values</button>
        </div>
        <div className="testdata-body">
          <div className="testdata-columns">
            {!plans && !error && (
              <div className="panel-empty">
                <span className="spinner" /> Reading the table…
              </div>
            )}
            {plans && (
              <table>
                <thead>
                  <tr>
                    <th>Column</th>
                    <th>Type</th>
                    <th>Values</th>
                  </tr>
                </thead>
                <tbody>
                  {plans.map((p) => {
                    const g = generatorOf(p);
                    return (
                      <tr key={p.column} className={g === "skip" ? "skipped" : ""}>
                        <td title={p.note}>{p.column}</td>
                        <td>
                          <small>
                            {p.dataType}
                            {p.nullable ? "" : " not null"}
                          </small>
                        </td>
                        <td>
                          <select value={g} onChange={(e) => setGenerator(p.column, e.target.value as Generator)}>
                            {(Object.keys(LABELS) as Generator[]).map((k) => (
                              <option key={k} value={k}>
                                {LABELS[k]}
                                {k === p.generator ? " (suggested)" : ""}
                              </option>
                            ))}
                          </select>
                          {g === "fixed" && (
                            <input
                              className="testdata-fixed"
                              placeholder="value or NULL"
                              value={options.fixed[p.column] ?? ""}
                              onChange={(e) => setOptions((o) => ({ ...o, fixed: { ...o.fixed, [p.column]: e.target.value } }))}
                            />
                          )}
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            )}
          </div>
          <div className="testdata-preview">
            <div className="preview-bar">
              <b>Preview (5 rows)</b>
            </div>
            {error ? <div className="status-error">{error}</div> : <pre>{preview}</pre>}
          </div>
        </div>
        <div className="modal-actions">
          <small>Foreign keys use rows that exist in the parent tables. E-mails use reserved example domains; phone numbers follow real formats and may belong to real people, so never send messages to them. Nothing is inserted until you run the script.</small>
          <span className="spacer" />
          <button className="primary" onClick={() => void create()} disabled={busy || !plans || !!error}>
            {busy ? "Generating…" : "Open script"}
          </button>
          <button onClick={onClose}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
