import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, newId } from "./api";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { ExplorerPanel } from "./components/ExplorerPanel";
import { HistoryPanel } from "./components/HistoryPanel";
import { MultiDbDialog } from "./components/MultiDbDialog";
import { ResultsPanel } from "./components/ResultsPanel";
import { SearchPanel } from "./components/SearchPanel";
import { SettingsDialog } from "./components/SettingsDialog";
import {
  bindModelContext,
  editorArgs,
  expandWildcard,
  languageFor,
  monaco,
  registerProviders,
  unbindModelContext,
  updateDiagnostics,
} from "./monaco";
import type { Dialect, HistoryEntry, RefactorAction, SavedConnection, Settings, Tab } from "./types";

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
  search: "SQL Search",
  history: "SQL History",
  explorer: "Object Explorer",
};

interface ConfirmState {
  messages: string[];
  resolve: (ok: boolean) => void;
}

let untitled = 0;
const nextTitle = () => `SQLQuery${++untitled}.sql`;

export default function App() {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeId, setActiveId] = useState<string>("");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [connections, setConnections] = useState<SavedConnection[]>([]);
  const [dialog, setDialog] = useState<"connect" | "settings" | "multi" | null>(null);
  const [panel, setPanel] = useState<Panel>(null);
  const [resultsVisible, setResultsVisible] = useState(true);
  const [resultsHeight, setResultsHeight] = useState(280);
  const [panelWidth, setPanelWidth] = useState(520);
  const [status, setStatus] = useState("");
  const [catalogVersion, setCatalogVersion] = useState(0);
  const [confirmState, setConfirmState] = useState<ConfirmState | null>(null);
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
    try {
      const info = await api.loadCatalog(tab.connectionId, tab.database, refresh);
      setStatus(`${where}: ${info.objects} objects loaded`);
      setCatalogVersion((v) => v + 1);
    } catch (e) {
      setStatus(`${where}: object load failed: ${errorText(e)}`);
    }
  }, []);

  const connectTab = useCallback(
    async (id: string, conn: SavedConnection, password: string | null, database?: string) => {
      patchTab(id, { connecting: true, connectionId: conn.id });
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
        void loadCatalog(tab);
      } catch (e) {
        patchTab(id, { connecting: false, connected: false });
        setStatus(`${conn.name}: ${errorText(e)}`);
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
      void loadCatalog({ ...activeTab, database: d });
    } catch (e) {
      setStatus(errorText(e));
    }
  };

  // -- execution ------------------------------------------------------------

  const execute = useCallback(
    async (mode: "auto" | "statement") => {
      const id = activeRef.current;
      const tab = tabsRef.current.find((t) => t.id === id);
      const ed = editor.current;
      const model = models.current.get(id);
      if (!tab || !ed || !model) return;
      if (!tab.connected) {
        setDialog("connect");
        return;
      }
      if (tab.running) return;
      let sql = "";
      const sel = ed.getSelection();
      if (mode === "auto" && sel && !sel.isEmpty()) {
        sql = model.getValueInRange(sel);
      } else if (mode === "statement") {
        const pos = ed.getPosition();
        const text = model.getValue();
        const r = pos ? await api.statementAt(text, tab.dialect, model.getOffsetAt(pos)) : null;
        if (!r) return;
        sql = text.slice(r.start, r.end);
        const s = model.getPositionAt(r.start);
        const e = model.getPositionAt(r.end);
        ed.setSelection(new monaco.Selection(s.lineNumber, s.column, e.lineNumber, e.column));
      } else {
        sql = model.getValue();
      }
      if (!sql.trim()) return;
      // like SQL Prompt's execution warnings: confirm destructive statements
      const warnings = await api.executionWarnings(sql, tab.dialect).catch(() => [] as string[]);
      if (warnings.length > 0) {
        const ok = await new Promise<boolean>((resolve) => setConfirmState({ messages: warnings, resolve }));
        setConfirmState(null);
        if (!ok) return;
      }
      recordHistory(id, "execute");
      patchTab(id, { running: true });
      setResultsVisible(true);
      try {
        const result = await api.execute(id, sql);
        patchTab(id, {
          running: false,
          result,
          executedAt: Date.now(),
          database: result.database || tab.database,
        });
        if (result.database && result.database !== tab.database) {
          void loadCatalog({ ...tab, database: result.database });
        }
      } catch (e) {
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
        const result = await api.executeMulti(id, sql, databases);
        patchTab(id, { running: false, result, executedAt: Date.now() });
        setStatus(`Ran on ${databases.length} database(s)`);
      } catch (e) {
        patchTab(id, { running: false });
        setStatus(errorText(e));
      }
    },
    [patchTab, recordHistory],
  );

  const cancel = useCallback(() => {
    const id = activeRef.current;
    void api.cancelQuery(id);
  }, []);

  const format = useCallback(() => {
    const ed = editor.current;
    if (!ed) return;
    const sel = ed.getSelection();
    const action = sel && !sel.isEmpty() ? "editor.action.formatSelection" : "editor.action.formatDocument";
    void ed.getAction(action)?.run();
  }, []);

  const openText = useCallback(
    async (title: string, text: string, run = false) => {
      const base = tabsRef.current.find((t) => t.id === activeRef.current);
      const tab = createTab({ title, text, dialect: base?.dialect, connectionId: base?.connectionId, database: base?.database });
      const conn = connectionsRef.current.find((c) => c.id === base?.connectionId);
      if (conn && base?.connected) {
        try {
          await connectTab(tab.id, conn, null, base.database);
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
        setStatus("Nothing to change");
        return;
      }
      ed.pushUndoStop();
      ed.executeEdits("refactor", [{ range: model.getFullModelRange(), text }]);
      ed.pushUndoStop();
    } catch (e) {
      setStatus(errorText(e));
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
        setStatus("Connect the tab to open object definitions");
        return;
      }
      try {
        const obj = await api.objectAt(editorArgs(model, pos));
        if (!obj) {
          setStatus("No database object under the cursor");
          return;
        }
        const text = await api.scriptObject(tab.connectionId, tab.database, obj.schema, obj.name, tab.dialect, kind);
        await openText(`${obj.name}${kind === "alter" ? " (alter)" : ""}`, text);
      } catch (e) {
        setStatus(errorText(e));
      }
    },
    [openText],
  );

  const openHistory = (e: HistoryEntry) => {
    const conn = connections.find((c) => c.name === e.connectionName);
    const dialect = (["mssql", "postgres", "mysql"].includes(e.dialect) ? e.dialect : "mssql") as Dialect;
    const tab = createTab({ title: e.title, text: e.text, dialect, connectionId: conn?.id, database: e.database ?? undefined });
    if (conn) connectTab(tab.id, conn, null, e.database ?? undefined).catch(() => undefined);
  };

  const reopenClosed = useCallback(async () => {
    const [last] = await api.historyRecentlyClosed(1);
    if (!last) return;
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

  const actions = useRef({ execute, cancel, format, closeTab, createTab, reopenClosed, refactor, goToDefinition, openMulti });
  actions.current = { execute, cancel, format, closeTab, createTab, reopenClosed, refactor, goToDefinition, openMulti };

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
    ed.addCommand(
      KC.Tab,
      () => {
        void expandWildcard(ed).then((done) => {
          if (!done) ed.trigger("keyboard", "tab", null);
        });
      },
      "editorTextFocus && !suggestWidgetVisible && !inSnippetMode && !editorTabMovesFocus && !editorHasSelection",
    );
    ed.onDidChangeCursorPosition((e) => setCursor({ line: e.position.lineNumber, col: e.position.column }));

    // context menu / F1 actions, SQL Prompt style
    const action = (
      id: string,
      label: string,
      run: () => void,
      keybindings: number[] = [],
      group = "1_sqlq",
      order = 1,
    ) => ed.addAction({ id, label, keybindings, contextMenuGroupId: group, contextMenuOrder: order, run });
    action("sqlq.definition", "Go to Definition (script object)", () => void actions.current.goToDefinition("create"), [KC.F12], "0_nav", 1);
    action("sqlq.alter", "Script as ALTER", () => void actions.current.goToDefinition("alter"), [KM.Alt | KC.F12], "0_nav", 2);
    action("sqlq.format", "Format SQL", () => actions.current.format(), [], "1_sqlq", 1);
    action("sqlq.multi", "Run on Multiple Databases…", () => actions.current.openMulti(), [KM.CtrlCmd | KM.Shift | KC.F5], "0_nav", 3);
    action(
      "sqlq.expandWildcards",
      "Expand Wildcards",
      () => void actions.current.refactor("expandWildcards"),
      [KM.chord(KM.CtrlCmd | KC.KeyB, KM.CtrlCmd | KC.KeyW)],
      "1_sqlq",
      2,
    );
    action(
      "sqlq.qualify",
      "Qualify Object Names",
      () => void actions.current.refactor("qualifyNames"),
      [KM.chord(KM.CtrlCmd | KC.KeyB, KM.CtrlCmd | KC.KeyQ)],
      "1_sqlq",
      3,
    );
    action("sqlq.semicolons", "Insert Semicolons", () => void actions.current.refactor("insertSemicolons"), [], "1_sqlq", 4);
    action("sqlq.brackets", "Remove Square Brackets", () => void actions.current.refactor("removeBrackets"), [], "1_sqlq", 5);
    action("sqlq.upper", "Uppercase Keywords", () => void actions.current.refactor("upperKeywords"), [], "1_sqlq", 6);
    action("sqlq.lower", "Lowercase Keywords", () => void actions.current.refactor("lowerKeywords"), [], "1_sqlq", 7);

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
    });

    (async () => {
      const [s, conns] = await Promise.all([api.getSettings(), api.listConnections()]);
      setSettings(s);
      setConnections(conns);
      connectionsRef.current = conns;
      const session = (await api.loadSession().catch(() => null)) as { tabs: SessionTab[]; active?: string } | null;
      const saved = session?.tabs ?? [];
      if (saved.length === 0) {
        createTab();
        return;
      }
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
        <span className="sep" />
        <button onClick={format} title="Format SQL (Ctrl+K, Ctrl+Y)">
          Format
        </button>
        <button className={panel === "search" ? "active" : ""} onClick={() => setPanel(panel === "search" ? null : "search")} title="SQL Search (Ctrl+Alt+D)">
          Search
        </button>
        <button className={panel === "history" ? "active" : ""} onClick={() => setPanel(panel === "history" ? null : "history")} title="SQL History (Ctrl+Shift+H)">
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
                <ResultsPanel result={activeTab?.result} running={!!activeTab?.running} dialect={activeTab?.dialect} />
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
            connectTab(id, c, pw).catch(() => undefined);
            // back to typing right away
            window.setTimeout(() => editor.current?.focus(), 0);
          }}
        />
      )}
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
        <SettingsDialog settings={settings} dialect={activeTab?.dialect ?? "mssql"} onClose={() => setDialog(null)} onSaved={setSettings} />
      )}
    </div>
  );
}
