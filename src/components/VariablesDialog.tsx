import { useState } from "react";
import type { Dialect, VariableValue } from "../types";

const STORE_KEY = "sqlotus.variables";
const TYPES = ["auto", "int", "bigint", "decimal(38, 10)", "nvarchar(4000)", "varchar(8000)", "date", "datetime2", "bit", "uniqueidentifier"];

interface Entry {
  name: string;
  value: string;
  type: string;
  isNull: boolean;
}

function remembered(): Record<string, { value: string; type: string; isNull: boolean }> {
  try {
    return JSON.parse(localStorage.getItem(STORE_KEY) ?? "{}");
  } catch {
    return {};
  }
}

/** SQL Server type for a value typed by the user ("auto"). */
export function inferType(value: string): string {
  const v = value.trim();
  if (/^-?\d+$/.test(v)) return Math.abs(Number(v)) > 2147483647 ? "bigint" : "int";
  if (/^-?\d*\.\d+$/.test(v)) return "decimal(38, 10)";
  if (/^\d{4}-\d{2}-\d{2}$/.test(v)) return "date";
  if (/^\d{4}-\d{2}-\d{2}[ T]\d{2}:\d{2}(:\d{2}(\.\d+)?)?$/.test(v)) return "datetime2";
  if (/^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(v)) return "uniqueidentifier";
  return "nvarchar(4000)";
}

/**
 * Ask for the values of variables a script uses without declaring them;
 * Sqlotus declares them in front of the batches that use them.
 */
export function VariablesDialog({
  names,
  dialect,
  onRun,
  onCancel,
}: {
  names: string[];
  dialect: Dialect;
  onRun: (values: VariableValue[]) => void;
  onCancel: () => void;
}) {
  const [entries, setEntries] = useState<Entry[]>(() => {
    const saved = remembered();
    return names.map((name) => {
      const s = saved[name.toLowerCase()];
      return { name, value: s?.value ?? "", type: s?.type ?? "auto", isNull: s?.isNull ?? false };
    });
  });
  const mssql = dialect === "mssql";
  const patch = (i: number, p: Partial<Entry>) => setEntries((es) => es.map((e, k) => (k === i ? { ...e, ...p } : e)));

  const run = () => {
    try {
      const saved = remembered();
      for (const e of entries) saved[e.name.toLowerCase()] = { value: e.value, type: e.type, isNull: e.isNull };
      localStorage.setItem(STORE_KEY, JSON.stringify(saved));
    } catch {
      // remembering values is a convenience only
    }
    onRun(
      entries.map((e) => ({
        name: e.name,
        dataType: !mssql ? "" : e.type === "auto" ? inferType(e.value) : e.type,
        value: e.isNull ? null : e.value,
      })),
    );
  };

  return (
    <div className="modal-backdrop" onMouseDown={onCancel}>
      <div
        className="modal variables-modal"
        onMouseDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") onCancel();
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault();
            run();
          }
        }}
      >
        <div className="modal-title">Values for undeclared variables</div>
        <div className="variables-body">
          <p className="settings-hint">
            The script uses {names.length === 1 ? "a variable" : "variables"} it does not declare. Enter values to run it with;
            {mssql ? " Sqlotus adds DECLARE statements in front of the batches that use them." : " Sqlotus sets them before the script runs."}
          </p>
          <table className="variables-table">
            <tbody>
              {entries.map((e, i) => (
                <tr key={e.name}>
                  <td>
                    <code>{e.name}</code>
                  </td>
                  <td>
                    <input
                      className="variable-value"
                      autoFocus={i === 0}
                      value={e.value}
                      disabled={e.isNull}
                      placeholder={e.isNull ? "NULL" : "value"}
                      onChange={(ev) => patch(i, { value: ev.target.value })}
                    />
                  </td>
                  {mssql && (
                    <td>
                      <select value={e.type} onChange={(ev) => patch(i, { type: ev.target.value })} title="Type of the variable">
                        {TYPES.map((t) => (
                          <option key={t} value={t}>
                            {t === "auto" ? `auto (${e.isNull ? "nvarchar" : inferType(e.value)})` : t}
                          </option>
                        ))}
                      </select>
                    </td>
                  )}
                  <td>
                    <label className="check">
                      <input type="checkbox" checked={e.isNull} onChange={(ev) => patch(i, { isNull: ev.target.checked })} />
                      NULL
                    </label>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        <div className="modal-actions">
          <small>Values are remembered for next time. Turn this off in Options → Safety.</small>
          <span className="spacer" />
          <button className="primary" onClick={run}>
            Run
          </button>
          <button onClick={onCancel}>Cancel</button>
        </div>
      </div>
    </div>
  );
}
