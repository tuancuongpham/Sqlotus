import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, newId } from "./api";
import { ActivityDialog } from "./components/ActivityDialog";
import { SchemaCompareDialog } from "./components/SchemaCompareDialog";
import { SourceControlDialog } from "./components/SourceControlDialog";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { DoctorDialog } from "./components/DoctorDialog";
import { Toaster } from "./components/Toaster";
import { dismiss, toast, update } from "./notify";
import { ExplorerPanel } from "./components/ExplorerPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import { MultiDbDialog } from "./components/MultiDbDialog";
import { ProblemsView } from "./components/ProblemsView";
import { ResultsPanel } from "./components/ResultsPanel";
import { SearchPanel } from "./components/SearchPanel";
import { SettingsDialog } from "./components/SettingsDialog";
import { VariablesDialog } from "./components/VariablesDialog";
import {
  applyFixes,
  bindModelContext,
  diagnosticsOf,
  editorArgs,
  expandWildcard,
  languageFor,
  monaco,
  registerProviders,
  unbindModelContext,
  updateDiagnostics,
} from "./monaco";
import { problemsMarkdown, type Problem } from "./problems";
import type {
  ConnectionConfig,
  Dialect,
  ExecuteResult,
  HistoryEntry,
  RefactorAction,
  SavedConnection,
  Settings,
  Tab,
  VariableValue,
} from "./types";

interface SessionTab {
  id: string;
  title: string;
  dialect: Dialect;
  connectionId?: string;
  database?: string;
  text: string;
}

type Panel = "search" | "history" | "explorer" | null;

const PANEL_TITLE: Record<Exclude<Panel, null>, string> = {
  search: "Object Search",
  history: "Query History",
  explorer: "Object Explorer",
};

interface ConfirmState {
  messages: string[];
  resolve: (ok: boolean) => void;
}

interface ChoiceState {
  title: string;
  message: string;
  options: { label: string; value: string; kind?: "primary" | "danger" }[];
  resolve: (value: string) => void;
}

interface VariablesState {
  names: string[];
  dialect: Dialect;
  resolve: (values: VariableValue[] | null) => void;
}

interface DoctorState {
  config: ConnectionConfig;
  password: string | null;
  error: string;
  tabId: string;
}

const REFACTOR_DONE: Record<RefactorAction, string> = {
  expandWildcards: "Wildcards expanded",
  qualifyNames: "Object names qualified",
  insertSemicolons: "Semicolons inserted",
  removeBrackets: "Square brackets removed",
  upperKeywords: "Keywords upper-cased",
  lowerKeywords: "Keywords lower-cased",
};

let untitled = 0;
const nextTitle = () => `SQLQuery${++untitled}.sql`;

export default function App() {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeId, setActiveId] = useState<string>("");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [connections, setConnections] = useState<SavedConnection[]>([]);
  const [dialog, setDialog] = useState<"connect" | "settings" | "multi" | "activity" | "compare" | "source" | null>(null);
  // the explorer is open by default: the database structure is always in view
  const [panel, setPanel] = useState<Panel>("explorer");
  const [resultsVisible, setResultsVisible] = useState(true);
  const [resultsHeight, setResultsHeight] = useState(280);
  const [panelWidth, setPanelWidth] = useState(520);
  const [status, setStatus] = useState("");
  const [problems, setProblems] = useState<Problem[]>([]);
  const [catalogVersion, setCatalogVersion] = useState(0);
  const [confirmState, setConfirmState] = useState<ConfirmState | null>(null);
  const [choice, setChoice] = useState<ChoiceState | null>(null);
  const [doctor, setDoctor] = useState<DoctorState | null>(null);
  const [variables, setVariables] = useState<VariablesState | null>(null);
  const settingsRef = useRef<Settings | null>(null);
  /** "Transaction still open" notification per tab, closed once it ends. */
  const txReminders = useRef(new Map<string, number>());
  settingsRef.current = settings;

  const askChoice = (title: string, message: string, options: ChoiceState["options"]) =>
    new Promise<string>((resolve) =>
      setChoice({
        title,
        message,
        options,
        resolve: (v) => {
          setChoice(null);
          resolve(v);
        },
      }),
    );
  const [cursor, setCursor] = useState({ line: 1, col: 1 });

  const editorEl = useRef<HTMLDivElement>(null);
  const editor = useRef<monaco.editor.IStandaloneCodeEditor | null>(null);
  const models = useRef(new Map<string, monaco.editor.ITextModel>());
  const viewStates = useRef(new Map<string, monaco.editor.ICodeEditorViewState | null>());
  const lastSaved = useRef(new Map<string, string>());
  const tabsRef = useRef<Tab[]>([]);
  const activeRef = useRef("");
  const connectionsRef = useRef<SavedConnection[]>([]);
  tabsRef.current = tabs;
  activeRef.current = activeId;
  connectionsRef.current = connections;

  const activeTab = tabs.find((t) => t.id === activeId);

  /** Resolve once the rendered state of tab `id` satisfies `pred`. */
  const waitForTab = (id: string, pred: (t: Tab) => boolean, ms = 5000) =>
    new Promise<boolean>((resolve) => {
      const end = Date.now() + ms;
      const tick = () => {
        const t = tabsRef.current.find((x) => x.id === id);
        if (t && pred(t)) resolve(true);
        else if (Date.now() > end) resolve(false);
        else window.setTimeout(tick, 30);
      };
      tick();
    });
  const connOf = (t?: Tab) => connections.find((c) => c.id === t?.connectionId);

  const patchTab = useCallback((id: string, patch: Partial<Tab>) => {
    setTabs((ts) => ts.map((t) => (t.id === id ? { ...t, ...patch } : t)));
  }, []);

  // -- tabs ---------------------------------------------------------------

  const createTab = useCallback(
    (init: Partial<SessionTab> & { text?: string } = {}) => {
      const id = init.id ?? newId();
      const dialect: Dialect = init.dialect ?? "mssql";
      const model = monaco.editor.createModel(init.text ?? "", languageFor[dialect]);
      models.current.set(id, model);
      lastSaved.current.set(id, init.text ?? "");
      bindModelContext(model, () => {
        const t = tabsRef.current.find((x) => x.id === id);
        const c = connectionsRef.current.find((x) => x.id === t?.connectionId);
        return {
          dialect: t?.dialect ?? dialect,
          connectionId: t?.connected ? t.connectionId : undefined,
          database: t?.database,
          server: c?.host,
        };
      });
      const tab: Tab = {
        id,
        title: init.title ?? nextTitle(),
        dialect,
        connectionId: init.connectionId,
        database: init.database,
        databases: [],
        connected: false,
        connecting: false,
        running: false,
      };
      setTabs((ts) => [...ts, tab]);
      setActiveId(id);
      return tab;
    },
    [],
  );

  const recordHistory = useCallback((id: string, kind: "autosave" | "execute") => {
    const t = tabsRef.current.find((x) => x.id === id);
    const model = models.current.get(id);
    if (!t || !model) return;
    const text = model.getValue();
    if (kind === "autosave" && lastSaved.current.get(id) === text) return;
    lastSaved.current.set(id, text);
    const c = connectionsRef.current.find((x) => x.id === t.connectionId);
    api
      .historyRecord({
        tabId: id,
        title: t.title,
        connectionName: c?.name ?? null,
        database: t.database ?? null,
        dialect: t.dialect,
        text,
        kind,
      })
      .catch(() => undefined);
  }, []);

  const closeTab = useCallback(
    async (id: string) => {
      const closing = tabsRef.current.find((t) => t.id === id);
      if (closing?.connected && (closing.openTransactions ?? 0) > 0) {
        const answer = await askChoice(
          "Open transaction",
          `${closing.title} has an open transaction. Closing the tab without COMMIT rolls back its uncommitted changes.`,
          [
            { label: "Commit and close", value: "COMMIT", kind: "primary" },
            { label: "Roll back and close", value: "ROLLBACK", kind: "danger" },
            { label: "Cancel", value: "" },
          ],
        );
        if (!answer) {
          toast.info("Tab kept open", "The transaction is still open.");
          return;
        }
        try {
          await api.execute(id, answer);
          toast.success(answer === "COMMIT" ? "Transaction committed" : "Transaction rolled back");
        } catch (e) {
          toast.error(`${answer} failed — the tab stays open`, errorText(e));
          return;
        }
      }
      recordHistory(id, "autosave");
      api.historyMarkClosed(id).catch(() => undefined);
      api.disconnectTab(id).catch(() => undefined);
      const model = models.current.get(id);
      if (model) {
        unbindModelContext(model);
        model.dispose();
      }
      models.current.delete(id);
      viewStates.current.delete(id);
      const remaining = tabsRef.current.filter((t) => t.id !== id);
      setTabs(remaining);
      if (activeRef.current === id) {
        const idx = tabsRef.current.findIndex((t) => t.id === id);
        const next = remaining[Math.min(idx, remaining.length - 1)];
        if (next) setActiveId(next.id);
        else createTab();
      }
    },
    [createTab, recordHistory],
  );

  // swap the editor model when the active tab changes
  useEffect(() => {
    const ed = editor.current;
    const model = models.current.get(activeId);
    if (!ed || !model) return;
    const current = ed.getModel();
    if (current === model) return;
    const prevId = [...models.current.entries()].find(([, m]) => m === current)?.[0];
    if (prevId) viewStates.current.set(prevId, ed.saveViewState());
    ed.setModel(model);
    const vs = viewStates.current.get(activeId);
    if (vs) ed.restoreViewState(vs);
    ed.focus();
  }, [activeId, tabs.length]);

  // -- connections ----------------------------------------------------------

  const loadCatalog = useCallback(async (tab: Tab, refresh = false) => {
    if (!tab.connectionId || !tab.database) return;
    const name = connectionsRef.current.find((c) => c.id === tab.connectionId)?.name ?? "";
    const where = `${name} / ${tab.database}`;
    setStatus(`${where}: ${refresh ? "refreshing" : "loading"} objects…`);
    const progress = refresh ? toast.progress("Reloading objects…", where) : null;
    try {
      const info = await api.loadCatalog(tab.connectionId, tab.database, refresh);
      setStatus(`${where}: ${info.objects} objects loaded`);
      setCatalogVersion((v) => v + 1);
      if (progress) update(progress, "success", "Objects reloaded", `${where}: ${info.objects} objects`);
    } catch (e) {
      setStatus(`${where}: object load failed: ${errorText(e)}`);
      const msg = "Completion, analysis and search will be limited until objects load.";
      if (progress) update(progress, "error", "Could not load objects", `${errorText(e)}\n${msg}`);
      else toast.warning(`Could not load objects of ${where}`, msg, [{ label: "Retry", run: () => void loadCatalog(tab, true) }]);
    }
  }, []);

  const connectTab = useCallback(
    async (id: string, conn: SavedConnection, password: string | null, database?: string, interactive = false) => {
      patchTab(id, { connecting: true, connectionId: conn.id });
      const progress = interactive ? toast.progress(`Connecting to ${conn.name}…`, conn.host) : null;
      try {
        const r = await api.connectTab(id, conn.id, database ?? conn.database ?? null, password);
        const tab = { ...tabsRef.current.find((t) => t.id === id)! };
        Object.assign(tab, {
          connected: true,
          connecting: false,
          connectionId: conn.id,
          database: r.database,
          databases: r.databases,
          dialect: r.dialect,
        });
        patchTab(id, tab);
        const model = models.current.get(id);
        if (model) monaco.editor.setModelLanguage(model, languageFor[r.dialect]);
        setStatus(`Connected to ${conn.name}`);
        if (progress) update(progress, "success", `Connected to ${conn.name}`, `Database: ${r.database}${conn.readOnly ? " · read-only" : ""}`);
        void loadCatalog(tab);
      } catch (e) {
        patchTab(id, { connecting: false, connected: false });
        const error = errorText(e);
        setStatus(`${conn.name}: ${error}`);
        const openDoctor = () => setDoctor({ config: conn, password, error, tabId: id });
        if (progress) {
          update(progress, "error", `Could not connect to ${conn.name}`, "The Connection Doctor is checking why.", [
            { label: "Diagnose", run: openDoctor },
          ]);
          openDoctor();
        } else {
          toast.error(`Could not connect to ${conn.name}`, error.split("\n")[0], [{ label: "Diagnose", run: openDoctor }]);
        }
        throw e;
      }
    },
    [patchTab, loadCatalog],
  );

  const changeDatabase = async (db: string) => {
    if (!activeTab) return;
    try {
      const d = await api.changeDatabase(activeTab.id, db);
      patchTab(activeTab.id, { database: d });
      toast.success(`Switched to database ${d}`);
      void loadCatalog({ ...activeTab, database: d });
    } catch (e) {
      toast.error(`Could not switch to ${db}`, errorText(e));
    }
  };

  // -- execution ------------------------------------------------------------

  /** COMMIT or ROLLBACK the open transaction of a tab. */
  const finishTransaction = useCallback(
    async (tabId: string, sql: "COMMIT" | "ROLLBACK") => {
      try {
        const r = await api.execute(tabId, sql);
        patchTab(tabId, { openTransactions: r.openTransactions ?? 0 });
        const failure = r.batches.flatMap((b) => b.output.messages).find((m) => m.kind === "error");
        if (failure) {
          toast.error(`${sql} failed`, r.notice ? `${failure.text}
${r.notice}` : failure.text);
          return;
        }
        const reminder = txReminders.current.get(tabId);
        if (reminder && !r.openTransactions) {
          dismiss(reminder);
          txReminders.current.delete(tabId);
        }
        toast.success(sql === "COMMIT" ? "Transaction committed" : "Transaction rolled back", sql === "COMMIT" ? "The changes are saved." : "The changes were undone.");
      } catch (e) {
        toast.error(`${sql} failed`, errorText(e));
        // the transaction may have ended anyway (e.g. rolled back): refresh the badge
        const r = await api.execute(tabId, "SELECT 1").catch(() => null);
        if (r) patchTab(tabId, { openTransactions: r.openTransactions ?? 0 });
      }
    },
    [patchTab],
  );

  /** Toast after a run: rows / errors / open transaction / commit decision. */
  const reportExecution = (result: ExecuteResult, title: string, tabId: string) => {
    if (result.cancelled) {
      toast.info("Query cancelled", "The connection was reset; it reconnects on the next run.");
      return;
    }
    const outs = result.batches.map((b) => b.output);
    const failed = outs.some((o) => o.failed);
    const rows = outs.reduce((n, o) => n + o.resultSets.reduce((m, r) => m + r.rows.length, 0), 0);
    const secs = (outs.reduce((n, o) => n + o.elapsedMs, 0) / 1000).toFixed(2);
    const changed = result.rowsAffected != null ? ` · ${result.rowsAffected.toLocaleString()} row(s) changed` : "";
    if (failed) {
      const first = outs.flatMap((o) => o.messages).find((m) => m.kind === "error")?.text ?? "";
      toast.error("The query finished with errors", first.split("\n").slice(-1)[0]);
    } else {
      toast.success(
        `Query finished in ${secs}s`,
        `${rows.toLocaleString()} row(s) in ${outs.reduce((n, o) => n + o.resultSets.length, 0)} result set(s)${changed}`,
      );
    }
    if (result.notice && !result.pendingCommit) toast.info(result.notice);
    const remind = () => {
      const old = txReminders.current.get(tabId);
      if (old) dismiss(old);
      const id = toast.warning(`${title}: transaction still open`, "Changes are not saved until COMMIT. Other sessions may be blocked meanwhile.", [
        { label: "Commit", run: () => void finishTransaction(tabId, "COMMIT") },
        { label: "Roll back", run: () => void finishTransaction(tabId, "ROLLBACK") },
      ]);
      txReminders.current.set(tabId, id);
    };
    const pending = result.pendingCommit;
    if (pending) {
      const message = pending.failed
        ? `Some statements failed. The changes made so far${pending.rows != null ? ` (${pending.rows.toLocaleString()} row(s))` : ""} are kept in an open transaction.`
        : pending.rows == null
          ? "SQL Server did not report how many rows changed (the script uses SET NOCOUNT ON or USE), so Sqlotus cannot tell whether it is within your limit. The changes are kept in an open transaction."
          : `This run changed ${pending.rows.toLocaleString()} rows — more than your limit of ${pending.limit.toLocaleString()} (Options → Safety). The changes are kept in an open transaction.`;
      void askChoice("Commit these changes?", message, [
        { label: "Commit", value: "COMMIT", kind: "primary" },
        { label: "Roll back", value: "ROLLBACK", kind: "danger" },
        { label: "Decide later", value: "" },
      ]).then((answer) => {
        if (answer === "COMMIT" || answer === "ROLLBACK") void finishTransaction(tabId, answer);
        else remind();
      });
    } else if ((result.openTransactions ?? 0) > 0) {
      remind();
    }
  };

  const execute = useCallback(
    async (mode: "auto" | "statement") => {
      const id = activeRef.current;
      const tab = tabsRef.current.find((t) => t.id === id);
      const ed = editor.current;
      const model = models.current.get(id);
      if (!tab || !ed || !model) return;
      if (!tab.connected) {
        toast.info("Connect first", "Choose a server for this tab, then run the query again.");
        setDialog("connect");
        return;
      }
      if (tab.running) {
        toast.info("A query is already running in this tab", "Wait for it or press Cancel.");
        return;
      }
      let sql = "";
      const sel = ed.getSelection();
      if (mode === "auto" && sel && !sel.isEmpty()) {
        sql = model.getValueInRange(sel);
      } else if (mode === "statement") {
        const pos = ed.getPosition();
        const text = model.getValue();
        const r = pos ? await api.statementAt(text, tab.dialect, model.getOffsetAt(pos)) : null;
        if (!r) {
          toast.info("No statement at the cursor");
          return;
        }
        sql = text.slice(r.start, r.end);
        const s = model.getPositionAt(r.start);
        const e = model.getPositionAt(r.end);
        ed.setSelection(new monaco.Selection(s.lineNumber, s.column, e.lineNumber, e.column));
      } else {
        sql = model.getValue();
      }
      if (!sql.trim()) {
        toast.info("Nothing to run", "The editor is empty.");
        return;
      }
      // execution warnings: confirm destructive statements
      const warnings = await api.executionWarnings(sql, tab.dialect).catch(() => [] as string[]);
      if (warnings.length > 0) {
        const ok = await new Promise<boolean>((resolve) => setConfirmState({ messages: warnings, resolve }));
        setConfirmState(null);
        if (!ok) {
          toast.info("Execution cancelled", "Nothing was run.");
          return;
        }
      }
      // production: confirm every change, naming the server and database
      const tabConn = connectionsRef.current.find((c) => c.id === tab.connectionId);
      if (tabConn?.environment === "prod") {
        const changes = await api.modifyingStatements(sql, tab.dialect).catch(() => [] as string[]);
        if (changes.length > 0) {
          const answer = await askChoice(
            "Change PRODUCTION?",
            `${tabConn.name} (${tabConn.host} / ${tab.database}) is a production connection. This script changes it: ${changes.slice(0, 5).join(", ")}${changes.length > 5 ? ", …" : ""}.`,
            [
              { label: "Run on production", value: "run", kind: "danger" },
              { label: "Cancel", value: "" },
            ],
          );
          if (answer !== "run") {
            toast.info("Execution cancelled", "Nothing was run.");
            return;
          }
        }
      }
      // values for variables the script uses without declaring them
      let values: VariableValue[] | null = null;
      if (settingsRef.current?.safety?.promptVariables ?? true) {
        const names = await api.undeclaredVariables(sql, tab.dialect).catch(() => [] as string[]);
        if (names.length > 0) {
          values = await new Promise<VariableValue[] | null>((resolve) => setVariables({ names, dialect: tab.dialect, resolve }));
          setVariables(null);
          if (!values) {
            toast.info("Execution cancelled", "Nothing was run.");
            return;
          }
        }
      }
      recordHistory(id, "execute");
      patchTab(id, { running: true, view: "results" });
      setResultsVisible(true);
      try {
        const result = await api.execute(id, sql, values);
        patchTab(id, {
          running: false,
          result,
          lastSql: sql,
          executedAt: Date.now(),
          database: result.database || tab.database,
          openTransactions: result.openTransactions ?? 0,
        });
        if (result.database && result.database !== tab.database) {
          void loadCatalog({ ...tab, database: result.database });
        }
        reportExecution(result, tab.title, id);
      } catch (e) {
        toast.error("The query was not run", errorText(e));
        patchTab(id, {
          running: false,
          result: {
            batches: [{ start: 0, end: 0, output: { resultSets: [], messages: [{ kind: "error", text: errorText(e) }], elapsedMs: 0, failed: true } }],
            database: tab.database ?? "",
            cancelled: false,
          },
        });
      }
    },
    [patchTab, recordHistory, loadCatalog],
  );

  /** Run the last query of a tab again (after its rows were edited). */
  const refreshResult = useCallback(
    async (id: string) => {
      const tab = tabsRef.current.find((t) => t.id === id);
      if (!tab?.lastSql) return;
      try {
        const result = await api.execute(id, tab.lastSql);
        patchTab(id, { result, openTransactions: result.openTransactions ?? 0 });
      } catch (e) {
        toast.error("Could not reload the rows", errorText(e));
      }
    },
    [patchTab],
  );

  /** Estimated execution plan of the selection or the whole tab. */
  const explain = useCallback(async () => {
    const id = activeRef.current;
    const tab = tabsRef.current.find((t) => t.id === id);
    const ed = editor.current;
    const model = models.current.get(id);
    if (!tab || !ed || !model) return;
    if (!tab.connected) {
      toast.info("Connect first", "The plan comes from the server.");
      setDialog("connect");
      return;
    }
    const sel = ed.getSelection();
    const sql = sel && !sel.isEmpty() ? model.getValueInRange(sel) : model.getValue();
    if (!sql.trim()) {
      toast.info("Nothing to explain", "The editor is empty.");
      return;
    }
    const progress = toast.progress("Getting the estimated plan…", "The query is not run.");
    try {
      const plan = await api.explain(id, sql);
      patchTab(id, { plan, view: "plan" });
      setResultsVisible(true);
      const warnings = plan.plans.reduce((n, p) => n + p.warnings.length + p.missingIndexes.length, 0);
      if (plan.plans.length === 0) update(progress, plan.errors.length ? "error" : "info", "No plan", plan.errors[0] ?? "Only SELECT, INSERT, UPDATE, DELETE and MERGE have plans.");
      else if (warnings > 0) update(progress, "warning", `Plan ready: ${warnings} thing(s) to look at`, "See the ⚠ lines in the Plan tab.");
      else update(progress, "success", "Plan ready", `${plan.plans.length} statement(s), no obvious problems.`);
    } catch (e) {
      update(progress, "error", "Could not get the plan", errorText(e));
    }
  }, [patchTab]);

  /** Run the selection (or the whole tab) on several databases. */
  const executeMulti = useCallback(
    async (databases: string[]) => {
      const id = activeRef.current;
      const tab = tabsRef.current.find((t) => t.id === id);
      const ed = editor.current;
      const model = models.current.get(id);
      if (!tab || !ed || !model || !tab.connected || tab.running) return;
      const sel = ed.getSelection();
      const sql = sel && !sel.isEmpty() ? model.getValueInRange(sel) : model.getValue();
      if (!sql.trim()) return;
      const warnings = await api.executionWarnings(sql, tab.dialect).catch(() => [] as string[]);
      if (warnings.length > 0) {
        const msgs = [...warnings, `This will run on ${databases.length} database(s).`];
        const ok = await new Promise<boolean>((resolve) => setConfirmState({ messages: msgs, resolve }));
        setConfirmState(null);
        if (!ok) return;
      }
      recordHistory(id, "execute");
      patchTab(id, { running: true });
      setResultsVisible(true);
      try {
        const progress = toast.progress(`Running on ${databases.length} database(s)…`);
        const result = await api.executeMulti(id, sql, databases).catch((e) => {
          update(progress, "error", "Multi-database run failed", errorText(e));
          throw e;
        });
        patchTab(id, { running: false, result, executedAt: Date.now() });
        setStatus(`Ran on ${databases.length} database(s)`);
        const failed = result.batches.some((b) => b.output.failed);
        update(progress, failed ? "warning" : "success", `Ran on ${databases.length} database(s)`, failed ? "Some databases reported errors — see Messages." : databases.join(", "));
      } catch {
        patchTab(id, { running: false });
      }
    },
    [patchTab, recordHistory],
  );

  const cancel = useCallback(() => {
    const id = activeRef.current;
    if (!tabsRef.current.find((t) => t.id === id)?.running) {
      toast.info("Nothing is running in this tab");
      return;
    }
    toast.info("Cancelling the query…", "The server is asked to stop; the connection is reset if it does not.");
    void api.cancelQuery(id);
  }, []);

  const format = useCallback(() => {
    const ed = editor.current;
    if (!ed) return;
    const sel = ed.getSelection();
    const selection = sel && !sel.isEmpty();
    const action = selection ? "editor.action.formatSelection" : "editor.action.formatDocument";
    const before = ed.getModel()?.getValue();
    void ed
      .getAction(action)
      ?.run()
      .then(() => {
        const after = ed.getModel()?.getValue();
        if (before === after) toast.info("Already formatted", "Nothing changed.");
        else toast.success(selection ? "Selection formatted" : "Document formatted", "Ctrl+Z undoes it.");
      });
  }, []);

  const openText = useCallback(
    async (title: string, text: string, run = false, target?: { connectionId: string; database: string }) => {
      const base = tabsRef.current.find((t) => t.id === activeRef.current);
      const connectionId = target?.connectionId ?? base?.connectionId;
      const database = target?.database ?? base?.database;
      const conn = connectionsRef.current.find((c) => c.id === connectionId);
      const tab = createTab({ title, text, dialect: conn?.dialect ?? base?.dialect, connectionId, database });
      if (conn && (target || base?.connected)) {
        try {
          await connectTab(tab.id, conn, null, database);
          // execute reads tab state from the last render; wait for it
          if (run && (await waitForTab(tab.id, (t) => t.connected))) await actions.current.execute("auto");
        } catch {
          // reported in the status bar
        }
      }
    },
    [createTab, connectTab],
  );

  /** Apply a whole-document refactoring as one undoable edit. */
  const refactor = useCallback(async (action: RefactorAction) => {
    const ed = editor.current;
    const model = ed?.getModel();
    if (!ed || !model) return;
    try {
      const text = await api.refactorText(editorArgs(model), action);
      if (text === model.getValue()) {
        toast.info(`${REFACTOR_DONE[action]}: nothing to change`);
        return;
      }
      ed.pushUndoStop();
      ed.executeEdits("refactor", [{ range: model.getFullModelRange(), text }]);
      ed.pushUndoStop();
      toast.success(REFACTOR_DONE[action], "Ctrl+Z undoes it.");
    } catch (e) {
      toast.error("Could not apply the change", errorText(e));
    }
  }, []);

  /** F12: open the definition / CREATE script of the object under the cursor. */
  const goToDefinition = useCallback(
    async (kind: "create" | "alter") => {
      const ed = editor.current;
      const model = ed?.getModel();
      const pos = ed?.getPosition();
      const tab = tabsRef.current.find((t) => t.id === activeRef.current);
      if (!ed || !model || !pos || !tab?.connected || !tab.connectionId || !tab.database) {
        toast.info("Connect the tab first", "Object definitions come from the connected database.");
        return;
      }
      try {
        const obj = await api.objectAt(editorArgs(model, pos));
        if (!obj) {
          toast.info("No database object under the cursor", "Put the cursor on a table, view, procedure or function name.");
          return;
        }
        const text = await api.scriptObject(tab.connectionId, tab.database, obj.schema, obj.name, tab.dialect, kind);
        await openText(`${obj.name}${kind === "alter" ? " (alter)" : ""}`, text);
      } catch (e) {
        toast.error("Could not open the definition", errorText(e));
      }
    },
    [openText],
  );

  const openHistory = (e: HistoryEntry) => {
    const conn = connections.find((c) => c.name === e.connectionName);
    const dialect = (["mssql", "postgres", "mysql"].includes(e.dialect) ? e.dialect : "mssql") as Dialect;
    const tab = createTab({ title: e.title, text: e.text, dialect, connectionId: conn?.id, database: e.database ?? undefined });
    toast.success(`Opened "${e.title}" from history`, conn ? `Reconnecting to ${conn.name}…` : "Not connected — choose a server to run it.");
    if (conn) connectTab(tab.id, conn, null, e.database ?? undefined).catch(() => undefined);
  };

  const reopenClosed = useCallback(async () => {
    const [last] = await api.historyRecentlyClosed(1);
    if (!last) {
      toast.info("No recently closed tab to reopen");
      return;
    }
    toast.success(`Reopened "${last.title}"`);
    const conn = connectionsRef.current.find((c) => c.name === last.connectionName);
    const tab = createTab({ title: last.title, text: last.text, dialect: last.dialect as Dialect, connectionId: conn?.id });
    await api.historyDelete(last.id).catch(() => undefined);
    if (conn) connectTab(tab.id, conn, null, last.database ?? undefined).catch(() => undefined);
  }, [createTab, connectTab]);

  const insertText = (text: string) => {
    const ed = editor.current;
    const sel = ed?.getSelection();
    if (!ed || !sel) return;
    ed.executeEdits("history", [{ range: sel, text }]);
    ed.focus();
  };

  // keep the latest handlers reachable from Monaco commands registered once
  const openMulti = useCallback(() => {
    const tab = tabsRef.current.find((t) => t.id === activeRef.current);
    if (!tab?.connected) {
      setDialog("connect");
      return;
    }
    setDialog("multi");
  }, []);

  const actions = useRef({ execute, cancel, format, closeTab, createTab, reopenClosed, refactor, goToDefinition, openMulti, explain });
  actions.current = { execute, cancel, format, closeTab, createTab, reopenClosed, refactor, goToDefinition, openMulti, explain };

  // -- startup ----------------------------------------------------------------

  useEffect(() => {
    registerProviders();
    const ed = monaco.editor.create(editorEl.current!, {
      model: null,
      automaticLayout: true,
      fontFamily: "Cascadia Mono, Consolas, monospace",
      fontSize: 14,
      minimap: { enabled: false },
      tabSize: 4,
      insertSpaces: true,
      wordBasedSuggestions: "off",
      suggest: { showWords: false, snippetsPreventQuickSuggestions: false, filterGraceful: true, localityBonus: true },
      quickSuggestions: { other: true, comments: false, strings: false },
      quickSuggestionsDelay: 50,
      acceptSuggestionOnEnter: "on",
      tabCompletion: "on",
      renderWhitespace: "selection",
      scrollBeyondLastLine: false,
      fixedOverflowWidgets: true,
    });
    editor.current = ed;
    const KM = monaco.KeyMod;
    const KC = monaco.KeyCode;
    ed.addCommand(KC.F5, () => actions.current.execute("auto"));
    ed.addCommand(KM.CtrlCmd | KC.KeyE, () => actions.current.execute("auto"));
    ed.addCommand(KM.Shift | KC.F5, () => actions.current.execute("statement"));
    ed.addCommand(KM.chord(KM.CtrlCmd | KC.KeyK, KM.CtrlCmd | KC.KeyY), () => actions.current.format());
    ed.addCommand(KM.Alt | KC.PauseBreak, () => actions.current.cancel());
    ed.addCommand(KM.CtrlCmd | KC.KeyR, () => setResultsVisible((v) => !v));
    ed.addCommand(KM.CtrlCmd | KC.KeyL, () => void actions.current.explain());
    ed.addCommand(
      KC.Tab,
      () => {
        void expandWildcard(ed).then((r) => {
          if (r === "no-tables") {
            toast.info("No columns to expand yet", "Write the FROM clause first, then press Tab right after * to list the columns.");
          } else if (r === "none") {
            ed.trigger("keyboard", "tab", null);
          }
        });
      },
      "editorTextFocus && !suggestWidgetVisible && !inSnippetMode && !editorTabMovesFocus && !editorHasSelection",
    );
    ed.onDidChangeCursorPosition((e) => setCursor({ line: e.position.lineNumber, col: e.position.column }));

    // context menu / F1 actions
    const action = (
      id: string,
      label: string,
      run: () => void,
      keybindings: number[] = [],
      group = "1_sqlotus",
      order = 1,
    ) => ed.addAction({ id, label, keybindings, contextMenuGroupId: group, contextMenuOrder: order, run });
    action("sqlotus.definition", "Go to Definition (script object)", () => void actions.current.goToDefinition("create"), [KC.F12], "0_nav", 1);
    action("sqlotus.alter", "Script as ALTER", () => void actions.current.goToDefinition("alter"), [KM.Alt | KC.F12], "0_nav", 2);
    action("sqlotus.format", "Format SQL", () => actions.current.format(), [], "1_sqlotus", 1);
    action("sqlotus.multi", "Run on Multiple Databases…", () => actions.current.openMulti(), [KM.CtrlCmd | KM.Shift | KC.F5], "0_nav", 3);
    action("sqlotus.explain", "Display Estimated Execution Plan", () => void actions.current.explain(), [], "0_nav", 4);
    action(
      "sqlotus.expandWildcards",
      "Expand Wildcards",
      () => void actions.current.refactor("expandWildcards"),
      [KM.chord(KM.CtrlCmd | KC.KeyB, KM.CtrlCmd | KC.KeyW)],
      "1_sqlotus",
      2,
    );
    action(
      "sqlotus.qualify",
      "Qualify Object Names",
      () => void actions.current.refactor("qualifyNames"),
      [KM.chord(KM.CtrlCmd | KC.KeyB, KM.CtrlCmd | KC.KeyQ)],
      "1_sqlotus",
      3,
    );
    action("sqlotus.semicolons", "Insert Semicolons", () => void actions.current.refactor("insertSemicolons"), [], "1_sqlotus", 4);
    action("sqlotus.brackets", "Remove Square Brackets", () => void actions.current.refactor("removeBrackets"), [], "1_sqlotus", 5);
    action("sqlotus.upper", "Uppercase Keywords", () => void actions.current.refactor("upperKeywords"), [], "1_sqlotus", 6);
    action("sqlotus.lower", "Lowercase Keywords", () => void actions.current.refactor("lowerKeywords"), [], "1_sqlotus", 7);

    // code analysis while typing
    let timer: number | undefined;
    ed.onDidChangeModelContent(() => {
      window.clearTimeout(timer);
      const model = ed.getModel();
      timer = window.setTimeout(() => model && void updateDiagnostics(model), 500);
    });
    ed.onDidChangeModel(() => {
      const model = ed.getModel();
      if (model) void updateDiagnostics(model);
      showProblems();
    });
    // the Problems list follows the active tab's analysis
    const showProblems = () => {
      const model = ed.getModel();
      setProblems(
        model
          ? diagnosticsOf(model).map((d) => {
              const p = model.getPositionAt(d.start);
              return { ...d, line: p.lineNumber, column: p.column };
            })
          : [],
      );
    };
    monaco.editor.onDidChangeMarkers((uris) => {
      const model = ed.getModel();
      if (model && uris.some((u) => u.toString() === model.uri.toString())) showProblems();
    });

    (async () => {
      const [s, conns] = await Promise.all([api.getSettings(), api.listConnections()]);
      setSettings(s);
      setConnections(conns);
      connectionsRef.current = conns;
      const session = (await api.loadSession().catch(() => null)) as { tabs: SessionTab[]; active?: string } | null;
      const saved = session?.tabs ?? [];
      const notice = await api.takeStartupNotice().catch(() => null);
      if (notice) toast.success("Welcome to Sqlotus", notice);
      if (saved.length === 0) {
        createTab();
        return;
      }
      toast.info(`Restored ${saved.length} tab(s) from your last session`, "Saved connections reconnect automatically.");
      for (const t of saved) {
        const m = /^SQLQuery(\d+)\.sql$/.exec(t.title);
        if (m) untitled = Math.max(untitled, Number(m[1]));
        createTab(t);
      }
      if (session?.active && saved.some((t) => t.id === session.active)) setActiveId(session.active);
      // reconnect restored tabs that have a saved connection
      for (const t of saved) {
        const conn = conns.find((c) => c.id === t.connectionId);
        if (conn && (conn.hasPassword || conn.auth === "windows")) {
          connectTab(t.id, conn, null, t.database).catch(() => undefined);
        }
      }
    })();
    return () => ed.dispose();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // apply editor settings
  useEffect(() => {
    if (!settings) return;
    editor.current?.updateOptions({ fontSize: settings.editor.fontSize });
    monaco.editor.setTheme(settings.editor.theme === "light" ? "vs" : "vs-dark");
    document.documentElement.dataset.theme = settings.editor.theme;
  }, [settings]);

  // re-run code analysis when objects were (re)loaded
  useEffect(() => {
    const model = editor.current?.getModel();
    if (model) void updateDiagnostics(model);
  }, [catalogVersion, activeId]);

  // history autosave
  useEffect(() => {
    const secs = settings?.editor.autosaveSeconds ?? 15;
    const timer = setInterval(() => tabsRef.current.forEach((t) => recordHistory(t.id, "autosave")), secs * 1000);
    return () => clearInterval(timer);
  }, [settings, recordHistory]);

  // persist open tabs (text included) so a restart restores the workspace
  useEffect(() => {
    const save = () => {
      const data = {
        active: activeRef.current,
        tabs: tabsRef.current.map<SessionTab>((t) => ({
          id: t.id,
          title: t.title,
          dialect: t.dialect,
          connectionId: t.connectionId,
          database: t.database,
          text: models.current.get(t.id)?.getValue() ?? "",
        })),
      };
      api.saveSession(data).catch(() => undefined);
    };
    const timer = setInterval(save, 3000);
    window.addEventListener("beforeunload", save);
    return () => {
      clearInterval(timer);
      window.removeEventListener("beforeunload", save);
    };
  }, []);

  // global shortcuts
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const k = e.key.toLowerCase();
      if (e.key === "F8") {
        e.preventDefault();
        setPanel((p) => (p === "explorer" ? null : "explorer"));
      } else if (e.ctrlKey && e.altKey && k === "d") {
        e.preventDefault();
        setPanel((p) => (p === "search" ? null : "search"));
      } else if (e.ctrlKey && e.shiftKey && k === "h") {
        e.preventDefault();
        setPanel((p) => (p === "history" ? null : "history"));
      } else if (e.ctrlKey && e.shiftKey && k === "t") {
        e.preventDefault();
        void actions.current.reopenClosed();
      } else if (e.ctrlKey && !e.shiftKey && k === "n") {
        e.preventDefault();
        actions.current.createTab();
      } else if (e.ctrlKey && k === "w") {
        e.preventDefault();
        void actions.current.closeTab(activeRef.current);
      } else if (e.ctrlKey && k === "tab") {
        e.preventDefault();
        const ts = tabsRef.current;
        const i = ts.findIndex((t) => t.id === activeRef.current);
        const next = ts[(i + (e.shiftKey ? -1 : 1) + ts.length) % ts.length];
        if (next) setActiveId(next.id);
      } else if (e.ctrlKey && !e.shiftKey && k === "r") {
        // also stops the webview from reloading
        e.preventDefault();
        setResultsVisible((v) => !v);
      } else if (e.key === "F5" && !e.shiftKey) {
        e.preventDefault();
        void actions.current.execute("auto");
      } else if (e.key === "Escape") {
        setDialog(null);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  const dragResults = (e: React.MouseEvent) => {
    const startY = e.clientY;
    const startH = resultsHeight;
    const move = (ev: MouseEvent) => setResultsHeight(Math.max(80, Math.min(window.innerHeight - 200, startH - (ev.clientY - startY))));
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const dragPanel = (e: React.MouseEvent) => {
    const startX = e.clientX;
    const startW = panelWidth;
    const move = (ev: MouseEvent) => setPanelWidth(Math.max(300, Math.min(window.innerWidth - 300, startW - (ev.clientX - startX))));
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  const conn = connOf(activeTab);
  const rows = activeTab?.result?.batches.reduce((a, b) => a + b.output.resultSets.reduce((x, r) => x + r.rows.length, 0), 0);

  return (
    <div className="app">
      <div className="toolbar">
        <button onClick={() => setDialog("connect")} title="Connect">
          {activeTab?.connecting ? "Connecting…" : activeTab?.connected ? "⇄ " + (conn?.name ?? "") : "Connect…"}
        </button>
        {activeTab?.connected && (
          <select value={activeTab.database ?? ""} onChange={(e) => changeDatabase(e.target.value)} title="Database">
            {!activeTab.databases.includes(activeTab.database ?? "") && <option value={activeTab.database}>{activeTab.database}</option>}
            {activeTab.databases.map((d) => (
              <option key={d} value={d}>
                {d}
              </option>
            ))}
          </select>
        )}
        <button className="primary" onClick={() => execute("auto")} disabled={activeTab?.running} title="Execute (F5 / Ctrl+E)">
          ▶ Execute
        </button>
        <button onClick={() => execute("statement")} disabled={activeTab?.running} title="Execute current statement (Shift+F5)">
          ▷ Statement
        </button>
        <button onClick={openMulti} disabled={activeTab?.running} title="Run on multiple databases (Ctrl+Shift+F5)">
          ⧉ Multi-DB
        </button>
        <button onClick={cancel} disabled={!activeTab?.running} title="Cancel (Alt+Break)">
          ■ Cancel
        </button>
        <button onClick={() => void explain()} disabled={activeTab?.running} title="Estimated execution plan (Ctrl+L) — the query is not run">
          ◇ Plan
        </button>
        <button
          onClick={() => (connections.length ? setDialog("compare") : toast.info("Save a connection first", "Schema compare works between saved connections."))}
          title="Compare the schema of two databases and build a script to sync them"
        >
          Compare
        </button>
        <button
          onClick={() =>
            activeTab?.connected && activeTab.database
              ? setDialog("source")
              : toast.info("Connect first", "Source control keeps the schema of the tab's database in a folder (a git repository).")
          }
          title="Keep this database's schema in a folder (git): commit changes, get your team's changes"
        >
          Source control
        </button>
        <button
          onClick={() => (activeTab?.connected ? setDialog("activity") : toast.info("Connect first", "The activity monitor shows the sessions of the tab's server."))}
          title="Sessions, running queries and blocking on this server"
        >
          Activity
        </button>
        {activeTab?.connected && (activeTab.openTransactions ?? 0) > 0 && (
          <>
            <button className="tx-commit" onClick={() => void finishTransaction(activeTab.id, "COMMIT")} title="COMMIT the open transaction">
              ✓ Commit
            </button>
            <button className="tx-rollback" onClick={() => void finishTransaction(activeTab.id, "ROLLBACK")} title="ROLLBACK the open transaction">
              ↶ Roll back
            </button>
          </>
        )}
        <span className="sep" />
        <button onClick={format} title="Format SQL (Ctrl+K, Ctrl+Y)">
          Format
        </button>
        <button className={panel === "search" ? "active" : ""} onClick={() => setPanel(panel === "search" ? null : "search")} title="Object Search (Ctrl+Alt+D)">
          Search
        </button>
        <button className={panel === "history" ? "active" : ""} onClick={() => setPanel(panel === "history" ? null : "history")} title="Query History (Ctrl+Shift+H)">
          History
        </button>
        <button
          className={panel === "explorer" ? "active" : ""}
          onClick={() => setPanel(panel === "explorer" ? null : "explorer")}
          title="Object Explorer (F8)"
        >
          Explorer
        </button>
        {activeTab?.connected && (
          <button onClick={() => loadCatalog(activeTab, true)} title="Reload objects for completion, analysis and search">
            ⟳
          </button>
        )}
        <span className="spacer" />
        <button onClick={() => setDialog("settings")} title="Options">
          ⚙ Options
        </button>
      </div>

      <div className="tabbar">
        {tabs.map((t) => {
          const c = connOf(t);
          return (
            <div
              key={t.id}
              className={"tab" + (t.id === activeId ? " active" : "")}
              style={{ borderTopColor: c?.color || undefined }}
              onMouseDown={(e) => {
                if (e.button === 1) {
                  e.preventDefault();
                  void closeTab(t.id);
                } else setActiveId(t.id);
              }}
              onDoubleClick={() => {
                const name = prompt("Tab name", t.title);
                if (name) patchTab(t.id, { title: name });
              }}
              title={c ? `${c.name} / ${t.database ?? ""}` : "Not connected"}
            >
              {t.running && <span className="spinner" />}
              {(t.openTransactions ?? 0) > 0 && (
                <span className="tab-badge tx" title="Open transaction — COMMIT or ROLLBACK">
                  TRAN
                </span>
              )}
              {c?.readOnly && (
                <span className="tab-badge ro" title="Read-only connection">
                  🔒
                </span>
              )}
              {c?.environment === "prod" && (
                <span className="tab-badge prod" title="Production connection: every change asks first">
                  PROD
                </span>
              )}
              <span className="tab-title">{t.title}</span>
              {c && <small className="tab-db">{t.database}</small>}
              <span
                className="tab-close"
                onMouseDown={(e) => e.stopPropagation()}
                onClick={(e) => {
                  e.stopPropagation();
                  void closeTab(t.id);
                }}
              >
                ×
              </span>
            </div>
          );
        })}
        <button className="tab-new" onClick={() => createTab()} title="New query (Ctrl+N)">
          +
        </button>
      </div>

      <div className="main">
        <div className="editor-col">
          <div className="editor" ref={editorEl} />
          {resultsVisible && (
            <>
              <div className="splitter-h" onMouseDown={dragResults} />
              <div className="results-pane" style={{ height: resultsHeight }}>
                <ResultsPanel
                  editContext={
                    activeTab?.connected && activeTab.lastSql && conn
                      ? {
                          tabId: activeTab.id,
                          connectionId: conn.id,
                          database: activeTab.database ?? "",
                          dialect: activeTab.dialect,
                          sql: activeTab.lastSql,
                          readOnly: !!conn.readOnly,
                          onSaved: () => void refreshResult(activeTab.id),
                        }
                      : undefined
                  }
                  result={activeTab?.result}
                  plan={activeTab?.plan}
                  showPlan={activeTab?.view === "plan"}
                  running={!!activeTab?.running}
                  dialect={activeTab?.dialect}
                  problems={{
                    count: problems.length,
                    worst: problems.some((p) => p.severity === "error")
                      ? "error"
                      : problems.some((p) => p.severity === "warning")
                        ? "warning"
                        : problems.length
                          ? "info"
                          : undefined,
                    view: (
                      <ProblemsView
                        problems={problems}
                        onJump={(p) => {
                          const ed = editor.current;
                          const model = ed?.getModel();
                          if (!ed || !model) return;
                          const range = monaco.Range.fromPositions(model.getPositionAt(p.start), model.getPositionAt(p.end));
                          ed.setSelection(range);
                          ed.revealRangeInCenter(range);
                          ed.focus();
                        }}
                        onFix={(p) => {
                          if (editor.current) applyFixes(editor.current, [p]);
                        }}
                        onFixAll={() => {
                          const n = editor.current ? applyFixes(editor.current, problems) : 0;
                          toast.success(`${n} problem(s) fixed`, "One undo step (Ctrl+Z) brings them back.");
                        }}
                        onCopy={() => {
                          void navigator.clipboard.writeText(problemsMarkdown(problems, activeTab?.title ?? "Query"));
                          toast.success("Problems copied", "A Markdown table, ready for a review or a ticket.");
                        }}
                      />
                    ),
                  }}
                />
              </div>
            </>
          )}
        </div>
        {panel && (
          <>
            <div className="splitter-v" onMouseDown={dragPanel} />
            <div className="side-panel" style={{ width: panelWidth }}>
              <div className="side-head">
                <b>{PANEL_TITLE[panel]}</b>
                <span className="spacer" />
                <button onClick={() => setPanel(null)}>×</button>
              </div>
              {panel === "search" && (
                <SearchPanel
                  connectionId={activeTab?.connected ? activeTab.connectionId : undefined}
                  database={activeTab?.database}
                  databases={activeTab?.databases ?? []}
                  dialect={activeTab?.dialect ?? "mssql"}
                  onOpenScript={(title, text) => void openText(title, text)}
                />
              )}
              {panel === "history" && <HistoryPanel
                  onOpen={openHistory}
                  onInsert={insertText}
                  currentText={() => editor.current?.getModel()?.getValue() ?? ""}
                />}
              {panel === "explorer" && (
                <ExplorerPanel
                  connectionId={activeTab?.connected ? activeTab.connectionId : undefined}
                  database={activeTab?.database}
                  dialect={activeTab?.dialect ?? "mssql"}
                  version={catalogVersion}
                  onOpenScript={(title, text, run) => void openText(title, text, run)}
                  onInsert={insertText}
                  onRefresh={() => activeTab && void loadCatalog(activeTab, true)}
                />
              )}
            </div>
          </>
        )}
      </div>

      <div className="statusbar" style={{ background: conn?.color || undefined }}>
        <span>{activeTab?.dialect === "postgres" ? "PostgreSQL" : activeTab?.dialect === "mysql" ? "MySQL" : "SQL Server"}</span>
        {conn && (
          <span>
            {conn.host} / {activeTab?.database}
          </span>
        )}
        <span className="status-msg">{status}</span>
        <span className="spacer" />
        {rows != null && <span>{rows.toLocaleString()} rows</span>}
        <span>
          Ln {cursor.line}, Col {cursor.col}
        </span>
      </div>

      {dialog === "connect" && (
        <ConnectionDialog
          onClose={() => {
            setDialog(null);
            api.listConnections().then(setConnections);
            window.setTimeout(() => editor.current?.focus(), 0);
          }}
          onConnect={async (c, pw) => {
            const conns = await api.listConnections();
            setConnections(conns);
            connectionsRef.current = conns;
            setDialog(null);
            const id = activeRef.current || createTab().id;
            // failures are reported in the status bar by connectTab
            connectTab(id, c, pw, undefined, true).catch(() => undefined);
            // back to typing right away
            window.setTimeout(() => editor.current?.focus(), 0);
          }}
        />
      )}
      {doctor && (
        <DoctorDialog
          config={doctor.config}
          password={doctor.password}
          error={doctor.error}
          onClose={() => setDoctor(null)}
          onRetry={async (cfg) => {
            const conns = await api.listConnections();
            setConnections(conns);
            connectionsRef.current = conns;
            const saved = conns.find((c) => c.id === cfg.id);
            if (!saved) return;
            setDoctor(null);
            connectTab(doctor.tabId, saved, doctor.password, cfg.database ?? undefined, true).catch(() => undefined);
          }}
        />
      )}
      {choice && (
        <div className="modal-backdrop">
          <div className="modal confirm-modal" onKeyDown={(e) => e.key === "Escape" && choice.resolve("")}>
            <div className="modal-title">{choice.title}</div>
            <div className="confirm-body">
              <p>{choice.message}</p>
            </div>
            <div className="modal-actions">
              <span className="spacer" />
              {choice.options.map((o) => (
                <button key={o.label} className={o.kind ?? ""} autoFocus={o.value === ""} onClick={() => choice.resolve(o.value)}>
                  {o.label}
                </button>
              ))}
            </div>
          </div>
        </div>
      )}
      {dialog === "compare" && (
        <SchemaCompareDialog
          connections={connections}
          initial={activeTab?.connected && conn ? { connectionId: conn.id, database: activeTab.database ?? "" } : undefined}
          onOpenScript={(title, text, target) => void openText(title, text, false, target)}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === "source" && activeTab?.connected && conn && activeTab.database && (
        <SourceControlDialog
          connectionId={conn.id}
          database={activeTab.database}
          connectionName={conn.name}
          dialect={activeTab.dialect}
          onOpenScript={(title, text) => void openText(title, text, false, { connectionId: conn.id, database: activeTab.database ?? "" })}
          onClose={() => setDialog(null)}
        />
      )}
      {dialog === "activity" && activeTab?.connected && conn && (
        <ActivityDialog
          connectionId={conn.id}
          database={activeTab.database ?? ""}
          dialect={activeTab.dialect}
          server={`${conn.name} (${conn.host})`}
          onOpenScript={(title, text) => void openText(title, text)}
          onClose={() => setDialog(null)}
        />
      )}
      {variables && (
        <VariablesDialog
          names={variables.names}
          dialect={variables.dialect}
          onRun={(v) => variables.resolve(v)}
          onCancel={() => variables.resolve(null)}
        />
      )}
      <Toaster />
      {confirmState && (
        <div className="modal-backdrop">
          <div className="modal confirm-modal" onKeyDown={(e) => e.key === "Escape" && confirmState.resolve(false)}>
            <div className="modal-title">⚠ Execution warning</div>
            <div className="confirm-body">
              {confirmState.messages.map((m, i) => (
                <div key={i}>{m}</div>
              ))}
              <p>Do you want to run it anyway?</p>
            </div>
            <div className="modal-actions">
              <span className="spacer" />
              <button className="danger" onClick={() => confirmState.resolve(true)}>
                Execute anyway
              </button>
              <button className="primary" autoFocus onClick={() => confirmState.resolve(false)}>
                Cancel
              </button>
            </div>
          </div>
        </div>
      )}
      {dialog === "multi" && activeTab && (
        <MultiDbDialog
          databases={activeTab.databases}
          current={activeTab.database}
          onClose={() => setDialog(null)}
          onRun={(dbs) => {
            setDialog(null);
            void executeMulti(dbs);
          }}
        />
      )}
      {dialog === "settings" && settings && (
        <SettingsDialog
          settings={settings}
          dialect={activeTab?.dialect ?? "mssql"}
          onClose={() => setDialog(null)}
          onSaved={(s) => {
            setSettings(s);
            // analysis rules may have changed
            const model = editor.current?.getModel();
            if (model) void updateDiagnostics(model);
            toast.success("Settings saved", "Formatting, completion, analysis and editor options are applied.");
          }}
        />
      )}
    </div>
  );
}
