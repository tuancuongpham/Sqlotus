import { useEffect, useMemo, useRef, useState } from "react";
import { api, errorText } from "../api";
import { toast, update } from "../notify";
import type { DbObject, Dialect, ObjectKind, ObjectProblem, Reference, ScriptKind } from "../types";
import { DiagramDialog } from "./DiagramDialog";
import { IndexHealthDialog } from "./IndexHealthDialog";
import { TestDataDialog } from "./TestDataDialog";

const GROUPS: { kind: ObjectKind[]; label: string }[] = [
  { kind: ["table"], label: "Tables" },
  { kind: ["view"], label: "Views" },
  { kind: ["procedure"], label: "Stored procedures" },
  { kind: ["scalarFunction", "tableFunction"], label: "Functions" },
  { kind: ["trigger", "synonym", "sequence"], label: "Other" },
];

const ICON: Record<ObjectKind, string> = {
  table: "▦",
  view: "◫",
  procedure: "ƒ",
  scalarFunction: "ƒx",
  tableFunction: "ƒ▦",
  trigger: "⚡",
  synonym: "↪",
  sequence: "#",
};

interface Props {
  connectionId?: string;
  database?: string;
  dialect: Dialect;
  /** Changes whenever the catalog was (re)loaded. */
  version: number;
  onOpenScript: (title: string, text: string, run?: boolean) => void;
  onInsert: (text: string) => void;
  onRefresh: () => void;
}

export function ExplorerPanel({ connectionId, database, dialect, version, onOpenScript, onInsert, onRefresh }: Props) {
  const [objects, setObjects] = useState<DbObject[]>([]);
  const [filter, setFilter] = useState("");
  const [open, setOpen] = useState<Set<string>>(new Set(["Tables"]));
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [selected, setSelected] = useState<string>("");
  const [error, setError] = useState("");
  const [problems, setProblems] = useState<ObjectProblem[] | null>(null);
  const [checking, setChecking] = useState(false);
  const [diagram, setDiagram] = useState<{ schema: string; name: string } | "all" | null>(null);
  const [testData, setTestData] = useState<DbObject | null>(null);
  const [health, setHealth] = useState(false);
  const [refs, setRefs] = useState<{ title: string; list: Reference[] } | null>(null);

  /** Who uses an object (or one of its columns). */
  const findReferences = async (o: DbObject, column?: string) => {
    if (!connectionId || !database) return;
    const title = `${o.schema}.${o.name}${column ? `.${column}` : ""}`;
    try {
      const list = await api.objectReferences(connectionId, database, o.schema, o.name, column ?? null, dialect);
      setRefs({ title, list });
      if (list.length === 0) toast.info(`Nothing uses ${title}`, "No view, procedure, function, trigger or foreign key refers to it.");
    } catch (e) {
      toast.error(`Could not find what uses ${title}`, errorText(e));
    }
  };

  const openReference = async (r: Reference) => {
    if (!connectionId || !database || r.how === "foreign key") return;
    try {
      const text = await api.scriptObject(connectionId, database, r.schema, r.name, dialect, "create");
      onOpenScript(`${r.name} (line ${r.line})`, text);
    } catch (e) {
      toast.error(`Could not open ${r.name}`, errorText(e));
    }
  };

  const findInvalid = async () => {
    if (!connectionId || !database) return;
    setChecking(true);
    setError("");
    try {
      const id = toast.progress("Checking every view, procedure, function and trigger…", database);
      const found = await api.findInvalidObjects(connectionId, database, dialect).catch((e) => {
        update(id, "error", "Could not check the objects", errorText(e));
        throw e;
      });
      setProblems(found);
      if (found.length === 0) update(id, "success", "No invalid objects", `Every module in ${database} references existing tables and columns.`);
      else update(id, "warning", `${found.length} problem(s) found`, "Click a problem to open the object's definition.");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setChecking(false);
    }
  };

  const openProblem = async (p: ObjectProblem) => {
    if (!connectionId || !database) return;
    try {
      const text = await api.scriptObject(connectionId, database, p.schema, p.name, dialect, "create");
      onOpenScript(`${p.name} (line ${p.line})`, text);
    } catch (e) {
      setError(errorText(e));
    }
  };
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => inputRef.current?.focus(), []);

  useEffect(() => {
    if (!connectionId || !database) {
      setObjects([]);
      return;
    }
    api.catalogObjects(connectionId, database).then(setObjects).catch((e) => setError(errorText(e)));
  }, [connectionId, database, version]);

  const visible = useMemo(() => {
    const f = filter.trim().toLowerCase();
    if (!f) return objects;
    return objects.filter(
      (o) => `${o.schema}.${o.name}`.toLowerCase().includes(f) || o.columns.some((c) => c.name.toLowerCase().includes(f)),
    );
  }, [objects, filter]);

  const key = (o: DbObject) => `${o.schema}.${o.name}`;

  const script = async (o: DbObject, kind: ScriptKind, run = false) => {
    if (!connectionId || !database) return;
    try {
      const text = await api.scriptObject(connectionId, database, o.schema, o.name, dialect, kind);
      onOpenScript(`${o.name} (${kind})`, text, run);
      toast.success(run ? `Running SELECT on ${o.name}` : `Opened ${kind.toUpperCase()} script for ${o.name}`);
    } catch (e) {
      setError(errorText(e));
      toast.error(`Could not script ${o.name}`, errorText(e));
    }
  };

  const toggle = (set: Set<string>, value: string, update: (s: Set<string>) => void) => {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    update(next);
  };

  if (!connectionId) {
    return <div className="panel-empty">Connect the current tab to browse its objects.</div>;
  }

  return (
    <div className="explorer-panel">
      <div className="panel-search-row">
        <input ref={inputRef} value={filter} placeholder="Filter objects and columns…" onChange={(e) => setFilter(e.target.value)} />
        <button onClick={onRefresh} title="Reload objects">
          ⟳
        </button>
        <button onClick={findInvalid} disabled={checking} title="Find views, procedures, functions and triggers that reference missing tables or columns">
          {checking ? "Checking…" : "Find invalid objects"}
        </button>
        <button onClick={() => setDiagram("all")} disabled={!database} title="Entity-relationship diagram of the tables">
          Diagram
        </button>
        <button onClick={() => setHealth(true)} disabled={!database} title="Missing, duplicate and unused indexes">
          Index health
        </button>
      </div>
      {refs && (
        <div className="problems references">
          <div className="problems-head">
            <b>
              {refs.list.length === 0 ? `Nothing uses ${refs.title}` : `${refs.list.length} use(s) of ${refs.title}`}
            </b>
            <span className="spacer" />
            <button onClick={() => setRefs(null)}>×</button>
          </div>
          {refs.list.map((r, i) => (
            <div key={i} className="problem" onClick={() => void openReference(r)} title={r.how === "foreign key" ? r.text : "Open the definition"}>
              <span className="tree-icon">{ICON[r.kind]}</span>
              <span>
                <small>{r.schema}.</small>
                {r.name} <span className={`ref-how ref-${r.how.replace(" ", "-")}`}>{r.how}</span>
                {r.line > 0 && <small> line {r.line}</small>}
                <div className="problem-msg">{r.text}</div>
              </span>
            </div>
          ))}
        </div>
      )}
      {error && <div className="status-error">{error}</div>}
      {problems && (
        <div className="problems">
          <div className="problems-head">
            <b>{problems.length === 0 ? "No invalid objects found" : `${problems.length} problem(s)`}</b>
            <span className="spacer" />
            <button onClick={() => setProblems(null)}>×</button>
          </div>
          {problems.map((p, i) => (
            <div key={i} className="problem" onClick={() => openProblem(p)} title="Open the definition">
              <span className="tree-icon">{ICON[p.kind]}</span>
              <span>
                <small>{p.schema}.</small>
                {p.name} <small>line {p.line}</small>
                <div className="problem-msg">{p.message}</div>
              </span>
            </div>
          ))}
        </div>
      )}
      <div className="explorer-tree">
        {GROUPS.map((g) => {
          const items = visible.filter((o) => g.kind.includes(o.kind));
          if (items.length === 0) return null;
          const isOpen = open.has(g.label) || filter.trim() !== "";
          return (
            <div key={g.label}>
              <div className="tree-group" onClick={() => toggle(open, g.label, setOpen)}>
                {isOpen ? "▾" : "▸"} {g.label} <small>({items.length})</small>
              </div>
              {isOpen &&
                items.map((o) => {
                  const k = key(o);
                  const isExpanded = expanded.has(k);
                  const isRoutine = o.kind === "procedure" || o.kind === "scalarFunction" || o.kind === "tableFunction";
                  const isRelation = o.kind === "table" || o.kind === "view" || o.kind === "tableFunction";
                  return (
                    <div key={k}>
                      <div
                        className={"tree-item" + (selected === k ? " selected" : "")}
                        onClick={() => setSelected(k)}
                        onDoubleClick={() => onInsert(k)}
                        title="Double-click to insert the name"
                      >
                        <span className="tree-toggle" onClick={(e) => (e.stopPropagation(), toggle(expanded, k, setExpanded))}>
                          {o.columns.length || o.params.length ? (isExpanded ? "▾" : "▸") : " "}
                        </span>
                        <span className="tree-icon">{ICON[o.kind]}</span>
                        <span className="tree-name">
                          <small>{o.schema}.</small>
                          {o.name}
                        </span>
                      </div>
                      {selected === k && (
                        <div className="tree-actions">
                          {o.kind === "table" || o.kind === "view" ? (
                            <button className="primary" onClick={() => script(o, "select", true)} title="Open SELECT TOP 100 and run it">
                              Select top 100
                            </button>
                          ) : null}
                          {isRelation && <button onClick={() => script(o, "select")}>SELECT</button>}
                          {o.kind === "table" && (
                            <>
                              <button onClick={() => script(o, "insert")}>INSERT</button>
                              <button onClick={() => script(o, "update")}>UPDATE</button>
                              <button onClick={() => script(o, "delete")}>DELETE</button>
                              <button onClick={() => setDiagram({ schema: o.schema, name: o.name })} title="Diagram of this table and its related tables">
                                Diagram
                              </button>
                              <button onClick={() => setTestData(o)} title="Generate realistic rows as an INSERT script">
                                Test data…
                              </button>
                            </>
                          )}
                          {o.kind === "procedure" && <button onClick={() => script(o, "exec")}>EXEC</button>}
                          <button onClick={() => script(o, "create")}>{o.kind === "table" ? "CREATE" : "Definition"}</button>
                          <button onClick={() => void findReferences(o)} title="Views, procedures, functions, triggers and foreign keys that use it">
                            Used by
                          </button>
                          {(isRoutine || o.kind === "view" || o.kind === "trigger") && dialect === "mssql" && (
                            <button onClick={() => script(o, "alter")}>ALTER</button>
                          )}
                        </div>
                      )}
                      {isExpanded && (
                        <div className="tree-children">
                          {o.columns.map((c) => (
                            <div key={c.name} className="tree-leaf" onDoubleClick={() => onInsert(c.name)} title="Double-click to insert">
                              <span className="tree-icon">{c.isPrimaryKey ? "🔑" : "·"}</span>
                              {c.name} <small>{c.dataType}{c.nullable ? "" : ", not null"}{c.isIdentity ? ", identity" : ""}{c.isComputed ? ", computed" : ""}</small>
                              <button className="leaf-action" onClick={(e) => (e.stopPropagation(), void findReferences(o, c.name))} title="What uses this column?">
                                used by
                              </button>
                            </div>
                          ))}
                          {o.params.map((p) => (
                            <div key={p.name} className="tree-leaf">
                              <span className="tree-icon">@</span>
                              {p.name} <small>{p.dataType}{p.isOutput ? " output" : ""}</small>
                            </div>
                          ))}
                        </div>
                      )}
                    </div>
                  );
                })}
            </div>
          );
        })}
        {visible.length === 0 && <div className="panel-empty">{objects.length ? "No match" : "No objects loaded"}</div>}
      </div>
      {diagram && database && (
        <DiagramDialog
          connectionId={connectionId}
          database={database}
          focus={diagram === "all" ? undefined : diagram}
          onClose={() => setDiagram(null)}
        />
      )}
      {health && database && (
        <IndexHealthDialog
          connectionId={connectionId}
          database={database}
          dialect={dialect}
          onOpenScript={(title, text) => onOpenScript(title, text)}
          onClose={() => setHealth(false)}
        />
      )}
      {testData && database && (
        <TestDataDialog
          connectionId={connectionId}
          database={database}
          schema={testData.schema}
          name={testData.name}
          dialect={dialect}
          onOpenScript={(title, text) => onOpenScript(title, text)}
          onClose={() => setTestData(null)}
        />
      )}
    </div>
  );
}
