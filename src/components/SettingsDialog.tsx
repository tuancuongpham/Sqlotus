import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, newId } from "../api";
import { toast } from "../notify";
import type { Dialect, Settings, Snippet } from "../types";

const SAMPLE = `select o.Id, c.Name, sum(i.Qty * i.Price) as Total from dbo.Orders o inner join dbo.Customers c on c.Id = o.CustomerId left join dbo.OrderItems i on i.OrderId = o.Id where o.CreatedAt >= '2024-01-01' and (c.Country = 'VN' or c.Vip = 1) group by o.Id, c.Name having sum(i.Qty) > 1 order by Total desc`;

interface Props {
  settings: Settings;
  dialect: Dialect;
  onClose: () => void;
  onSaved: (s: Settings) => void;
}

export function SettingsDialog({ settings, dialect, onClose, onSaved }: Props) {
  const [s, setS] = useState<Settings>(settings);
  const [page, setPage] = useState<"format" | "editor" | "safety" | "snippets">("format");
  const [preview, setPreview] = useState("");
  const [snippets, setSnippets] = useState<Snippet[]>([]);
  const [snip, setSnip] = useState<Snippet | null>(null);
  const [note, setNote] = useState("");

  // live preview: settings must be saved for the backend formatter to use them
  useEffect(() => {
    const t = setTimeout(async () => {
      await api.saveSettings(s);
      setPreview(await api.formatSql(SAMPLE, dialect));
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
  const sf = s.safety ?? { confirmRowsOver: 1000, promptVariables: true };
  const setSf = <K extends keyof Settings["safety"]>(k: K, v: Settings["safety"][K]) => setS({ ...s, safety: { ...sf, [k]: v } });

  const close = async () => {
    await api.saveSettings(s);
    onSaved(s);
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
          {(["format", "editor", "safety", "snippets"] as const).map((p) => (
            <button key={p} className={page === p ? "active" : ""} onClick={() => setPage(p)}>
              {{ format: "Formatting", editor: "Editor & completion", safety: "Safety", snippets: "Snippets" }[p]}
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
              <select value={e.theme} onChange={(ev) => setE("theme", ev.target.value as "dark" | "light")}>
                <option value="dark">Dark</option>
                <option value="light">Light</option>
              </select>
            </label>
            <label>
              Font size
              <input type="number" min={9} max={28} value={e.fontSize} onChange={(ev) => setE("fontSize", Number(ev.target.value) || 14)} />
            </label>
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
              <input type="checkbox" checked={sf.promptVariables} onChange={(ev) => setSf("promptVariables", ev.target.checked)} />
              Ask for values of variables the script uses without declaring them
            </label>
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
