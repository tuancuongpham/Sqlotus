import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, newId } from "../api";
import { toast } from "../notify";
import type { AnalysisRule, Dialect, Settings, Snippet, Theme } from "../types";

const SAMPLE = `select o.Id, c.Name, sum(i.Qty * i.Price) as Total from dbo.Orders o inner join dbo.Customers c on c.Id = o.CustomerId left join dbo.OrderItems i on i.OrderId = o.Id where o.CreatedAt >= '2024-01-01' and (c.Country = 'VN' or c.Vip = 1) group by o.Id, c.Name having sum(i.Qty) > 1 order by Total desc`;

interface Props {
  settings: Settings;
  dialect: Dialect;
  onClose: () => void;
  onSaved: (s: Settings) => void;
}

/** A whole number within `min..=max` (`fallback` for no number). */
const whole = (v: number | undefined, min: number, max: number, fallback: number) =>
  v != null && Number.isFinite(v) ? Math.min(max, Math.max(min, Math.floor(v))) : fallback;

/**
 * The settings as they are saved: numbers whole and in range. A typed -1 or
 * 1.5 failed the save, and the dialog would not close.
 */
function sanitized(s: Settings): Settings {
  return {
    ...s,
    format: { ...s.format, indentSize: whole(s.format.indentSize, 1, 8, 4) },
    editor: {
      ...s.editor,
      fontSize: whole(s.editor.fontSize, 9, 28, 14),
      autosaveSeconds: whole(s.editor.autosaveSeconds, 5, 600, 15),
      maxRows: whole(s.editor.maxRows, 100, 100_000_000, 100_000),
      historyDays: whole(s.editor.historyDays, 0, 36_500, 0),
    },
    safety: s.safety && { ...s.safety, confirmRowsOver: whole(s.safety.confirmRowsOver, 0, Number.MAX_SAFE_INTEGER, 1000) },
  };
}

export function SettingsDialog({ settings, dialect, onClose, onSaved }: Props) {
  const [s, setS] = useState<Settings>(settings);
  const [page, setPage] = useState<"format" | "editor" | "analysis" | "safety" | "snippets">("format");
  const [rules, setRules] = useState<AnalysisRule[]>([]);
  useEffect(() => {
    api.analysisRules().then(setRules).catch(() => undefined);
  }, []);
  const disabled = new Set((s.analysis?.disabled ?? []).map((c) => c.toUpperCase()));
  const setRule = (code: string, on: boolean) => {
    const next = new Set(disabled);
    if (on) next.delete(code);
    else next.add(code);
    setS({ ...s, analysis: { disabled: [...next].sort() } });
  };
  const [preview, setPreview] = useState("");
  const [snippets, setSnippets] = useState<Snippet[]>([]);
  const [snip, setSnip] = useState<Snippet | null>(null);
  const [note, setNote] = useState("");

  // live preview: settings must be saved for the backend formatter to use them
  useEffect(() => {
    const t = setTimeout(async () => {
      try {
        await api.saveSettings(sanitized(s));
        setPreview(await api.formatSql(SAMPLE, dialect));
      } catch {
        // shown when the dialog closes
      }
    }, 150);
    return () => clearTimeout(t);
  }, [s, dialect]);

  const loadSnippets = async () => setSnippets(await api.listSnippets());
  useEffect(() => {
    loadSnippets().then(() => undefined);
  }, []);

  const f = s.format;
  const setF = <K extends keyof Settings["format"]>(k: K, v: Settings["format"][K]) => setS({ ...s, format: { ...f, [k]: v } });
  const c = s.completion;
  const setC = <K extends keyof Settings["completion"]>(k: K, v: Settings["completion"][K]) =>
    setS({ ...s, completion: { ...c, [k]: v } });
  const e = s.editor;
  const setE = <K extends keyof Settings["editor"]>(k: K, v: Settings["editor"][K]) => setS({ ...s, editor: { ...e, [k]: v } });
  const sf = s.safety ?? { confirmRowsOver: 1000, promptVariables: true, confirmProcedureCalls: false, csvFormulaGuard: true };
  const setSf = <K extends keyof Settings["safety"]>(k: K, v: Settings["safety"][K]) => setS({ ...s, safety: { ...sf, [k]: v } });

  const close = async () => {
    const clean = sanitized(s);
    try {
      await api.saveSettings(clean);
    } catch (err) {
      toast.error("Could not save the settings", errorText(err));
      return;
    }
    onSaved(clean);
    onClose();
  };

  const importSnippets = async () => {
    try {
      const dir = await open({ directory: true, title: "Folder with snippet .json files" });
      if (typeof dir !== "string") return;
      const n = await api.importSnippets(dir);
      setNote(`Imported ${n} snippets.`);
      toast.success(`Imported ${n} snippets`);
      await loadSnippets();
    } catch (err) {
      setNote(errorText(err));
      toast.error("Could not import snippets", errorText(err));
    }
  };

  const saveSnip = async () => {
    if (!snip || !snip.prefix.trim()) return;
    await api.saveSnippet(snip);
    toast.success(`Snippet "${snip.prefix}" saved`, "Type its prefix in the editor to use it.");
    await loadSnippets();
  };

  return (
    <div className="modal-backdrop" onMouseDown={close}>
      <div className="modal settings-modal" onMouseDown={(ev) => ev.stopPropagation()}>
        <div className="modal-title">Options</div>
        <div className="settings-tabs">
          {(["format", "editor", "analysis", "safety", "snippets"] as const).map((p) => (
            <button key={p} className={page === p ? "active" : ""} onClick={() => setPage(p)}>
              {{ format: "Formatting", editor: "Editor & completion", analysis: "Analysis", safety: "Safety", snippets: "Snippets" }[p]}
            </button>
          ))}
        </div>
        {page === "format" && (
          <div className="settings-body two-col">
            <div className="settings-form">
              <label>
                Keyword case
                <select value={f.keywordCase} onChange={(ev) => setF("keywordCase", ev.target.value as typeof f.keywordCase)}>
                  <option value="upper">UPPER</option>
                  <option value="lower">lower</option>
                  <option value="preserve">Preserve</option>
                </select>
              </label>
              <label>
                List layout
                <select value={f.listLayout} onChange={(ev) => setF("listLayout", ev.target.value as typeof f.listLayout)}>
                  <option value="aligned">Aligned with keyword</option>
                  <option value="stacked">Keyword on its own line</option>
                </select>
              </label>
              <label>
                Commas
                <select value={f.commaStyle} onChange={(ev) => setF("commaStyle", ev.target.value as typeof f.commaStyle)}>
                  <option value="trailing">Trailing</option>
                  <option value="leading">Leading</option>
                </select>
              </label>
              <label>
                Indent size
                <input type="number" min={1} max={8} value={f.indentSize} onChange={(ev) => setF("indentSize", Number(ev.target.value) || 4)} />
              </label>
              <label className="check">
                <input type="checkbox" checked={f.indentJoins} onChange={(ev) => setF("indentJoins", ev.target.checked)} />
                Indent JOINs under FROM
              </label>
              <label className="check">
                <input type="checkbox" checked={f.onNewLine} onChange={(ev) => setF("onNewLine", ev.target.checked)} />
                ON on a new line
              </label>
              <label className="check">
                <input type="checkbox" checked={f.andOrNewLine} onChange={(ev) => setF("andOrNewLine", ev.target.checked)} />
                AND / OR on new lines
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={f.blankLineBetweenStatements}
                  onChange={(ev) => setF("blankLineBetweenStatements", ev.target.checked)}
                />
                Blank line between statements
              </label>
            </div>
            <pre className="format-preview">{preview}</pre>
          </div>
        )}
        {page === "editor" && (
          <div className="settings-body settings-form">
            <label>
              Theme
              <select value={e.theme} onChange={(ev) => setE("theme", ev.target.value as Theme)}>
                <option value="dark">Dark</option>
                <option value="light">Light</option>
                <option value="system">Follow Windows</option>
              </select>
            </label>
            <label>
              Font size
              <input type="number" min={9} max={28} value={e.fontSize} onChange={(ev) => setE("fontSize", Number(ev.target.value) || 14)} />
            </label>
            <small className="settings-hint">Ctrl+mouse wheel (or Ctrl+= / Ctrl+-) changes it in the editor; Ctrl+0 resets it.</small>
            <label className="check">
              <input
                className="f5-statement"
                type="checkbox"
                checked={!!e.f5RunsStatement}
                onChange={(ev) => setE("f5RunsStatement", ev.target.checked)}
              />
              Execute (F5) runs the statement at the cursor when nothing is selected
            </label>
            <small className="settings-hint">To run the whole script then, select it all first (Ctrl+A, F5).</small>
            <label>
              History autosave (seconds)
              <input
                type="number"
                min={5}
                max={600}
                value={e.autosaveSeconds}
                onChange={(ev) => setE("autosaveSeconds", Number(ev.target.value) || 15)}
              />
            </label>
            <label>
              Keep query history for (days, 0 = always)
              <input
                className="history-days"
                type="number"
                min={0}
                value={e.historyDays ?? 0}
                onChange={(ev) => setE("historyDays", Math.max(0, Math.floor(Number(ev.target.value) || 0)))}
              />
            </label>
            <small className="settings-hint">Older versions are deleted when you press Done and at each start; the last version of an open tab is kept.</small>
            <label>
              Max rows per result
              <input type="number" min={100} value={e.maxRows} onChange={(ev) => setE("maxRows", Number(ev.target.value) || 100000)} />
            </label>
            <label>
              Completion keyword case
              <select value={c.keywordCase} onChange={(ev) => setC("keywordCase", ev.target.value as typeof c.keywordCase)}>
                <option value="upper">UPPER</option>
                <option value="lower">lower</option>
              </select>
            </label>
            <label className="check">
              <input type="checkbox" checked={c.qualifyObjects} onChange={(ev) => setC("qualifyObjects", ev.target.checked)} />
              Qualify object names with schema
            </label>
            <label className="check">
              <input type="checkbox" checked={c.assignAliases} onChange={(ev) => setC("assignAliases", ev.target.checked)} />
              Assign table aliases automatically
            </label>
          </div>
        )}
        {page === "analysis" && (
          <div className="settings-body settings-form analysis-rules">
            {rules.map((r) => (
              <label key={r.code} className="check">
                <input type="checkbox" checked={!disabled.has(r.code)} onChange={(ev) => setRule(r.code, ev.target.checked)} />
                <b>{r.code}</b>&nbsp;{r.title}
                <small className="rule-notes">
                  {r.severity}
                  {r.needsCatalog ? " · needs a connected database" : ""}
                  {r.only === "mssql" ? " · SQL Server" : ""}
                </small>
              </label>
            ))}
            <small className="settings-hint">
              Switch a rule off for one place with a comment: <code>-- sqlotus-disable-line SQ002</code> at the end of a line,{" "}
              <code>-- sqlotus-disable-next-line SQ001</code> above it, or <code>-- sqlotus-disable SQ005</code> …{" "}
              <code>-- sqlotus-enable</code> around a block. The same rules run in CI with <code>sqlotus-lint</code>.
            </small>
          </div>
        )}
        {page === "safety" && (
          <div className="settings-body settings-form">
            <label>
              Ask before committing changes to more than (rows)
              <input
                className="confirm-rows"
                type="number"
                min={0}
                value={sf.confirmRowsOver}
                onChange={(ev) => setSf("confirmRowsOver", Math.max(0, Number(ev.target.value) || 0))}
              />
            </label>
            <small className="settings-hint">
              Scripts that only insert, update or delete rows run inside a transaction. If they change more rows than this, or
              fail half-way, Sqlotus asks whether to commit or roll back. 0 turns this off.
            </small>
            <label className="check">
              <input
                className="confirm-calls"
                type="checkbox"
                checked={!!sf.confirmProcedureCalls}
                onChange={(ev) => setSf("confirmProcedureCalls", ev.target.checked)}
              />
              Ask before committing what called procedures changed
            </label>
            <small className="settings-hint">
              Sqlotus cannot count the rows a procedure changes (EXEC, CALL). With this on, scripts that call procedures run in a
              transaction, and you commit or roll back what they did. Procedures that change nothing are committed without asking.
            </small>
            <label className="check">
              <input type="checkbox" checked={sf.promptVariables} onChange={(ev) => setSf("promptVariables", ev.target.checked)} />
              Ask for values of variables the script uses without declaring them
            </label>
            <label className="check">
              <input
                className="csv-guard"
                type="checkbox"
                checked={sf.csvFormulaGuard ?? true}
                onChange={(ev) => setSf("csvFormulaGuard", ev.target.checked)}
              />
              Keep spreadsheets from running text in CSV files as formulas
            </label>
            <small className="settings-hint">
              Text starting with =, +, - or @ gets a ' in front in CSV files (Excel then shows it as text). Numbers stay as they are.
            </small>
            <small className="settings-hint">
              Read-only and manual-commit modes are set per connection in the Connect dialog.
            </small>
          </div>
        )}
        {page === "snippets" && (
          <div className="settings-body two-col">
            <div className="snippet-list">
              <div className="preview-bar">
                <button onClick={() => setSnip({ id: newId(), prefix: "", description: "", body: "", placeholders: [] })}>New</button>
                <button onClick={importSnippets} title="Each .json file holds prefix, description, body and placeholders">Import snippets…</button>
              </div>
              {note && <small>{note}</small>}
              {snippets.map((x) => (
                <div key={x.id} className={"conn-item" + (snip?.id === x.id ? " active" : "")} onClick={() => setSnip(x)}>
                  <b>{x.prefix}</b>&nbsp;<small>{x.description}</small>
                </div>
              ))}
            </div>
            {snip ? (
              <div className="settings-form">
                <label>
                  Prefix
                  <input value={snip.prefix} onChange={(ev) => setSnip({ ...snip, prefix: ev.target.value })} />
                </label>
                <label>
                  Description
                  <input value={snip.description} onChange={(ev) => setSnip({ ...snip, description: ev.target.value })} />
                </label>
                <label>
                  Body ($CURSOR$, $SELECTEDTEXT$, $name$ placeholders)
                  <textarea rows={10} value={snip.body} onChange={(ev) => setSnip({ ...snip, body: ev.target.value })} />
                </label>
                <div className="modal-actions">
                  <button
                    className="danger"
                    onClick={async () => {
                      await api.deleteSnippet(snip.id);
                      toast.success(`Snippet "${snip.prefix}" deleted`);
                      setSnip(null);
                      await loadSnippets();
                    }}
                  >
                    Delete
                  </button>
                  <span className="spacer" />
                  <button className="primary" onClick={saveSnip}>
                    Save snippet
                  </button>
                </div>
              </div>
            ) : (
              <div className="panel-empty">Select or create a snippet</div>
            )}
          </div>
        )}
        <div className="modal-actions">
          <span className="spacer" />
          <button className="primary" onClick={close}>
            Done
          </button>
        </div>
      </div>
    </div>
  );
}
