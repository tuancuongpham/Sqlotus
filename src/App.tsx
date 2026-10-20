import { useCallback, useEffect, useRef, useState } from "react";
import { api, errorText, newId } from "./api";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { HistoryPanel } from "./components/HistoryPanel";
import { ResultsPanel } from "./components/ResultsPanel";
import { SearchPanel } from "./components/SearchPanel";
import { SettingsDialog } from "./components/SettingsDialog";
import { bindModelContext, expandWildcard, languageFor, monaco, registerProviders, unbindModelContext } from "./monaco";
import type { Dialect, HistoryEntry, SavedConnection, Settings, Tab } from "./types";

interface SessionTab {
  id: string;
  title: string;
  dialect: Dialect;
  connectionId?: string;
  database?: string;
  text: string;
}

type Panel = "search" | "history" | null;

let untitled = 0;
const nextTitle = () => `SQLQuery${++untitled}.sql`;

export default function App() {
  const [tabs, setTabs] = useState<Tab[]>([]);
  const [activeId, setActiveId] = useState<string>("");
  const [settings, setSettings] = useState<Settings | null>(null);
  const [connections, setConnections] = useState<SavedConnection[]>([]);
  const [dialog, setDialog] = useState<"connect" | "settings" | null>(null);
  const [panel, setPanel] = useState<Panel>(null);
  const [resultsVisible, setResultsVisible] = useState(true);
  const [resultsHeight, setResultsHeight] = useState(280);
  const [panelWidth, setPanelWidth] = useState(520);
  const [status, setStatus] = useState("");
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
    setStatus(refresh ? "Refreshing objects…" : "Loading objects…");
    try {
      const info = await api.loadCatalog(tab.connectionId, tab.database, refresh);
      setStatus(`${info.objects} objects loaded`);
    } catch (e) {
      setStatus(`Object load failed: ${errorText(e)}`);
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
        setStatus(`Connection failed: ${errorText(e)}`);
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
    (title: string, text: string, from?: Tab) => {
      const base = from ?? tabsRef.current.find((t) => t.id === activeRef.current);
      const tab = createTab({ title, text, dialect: base?.dialect, connectionId: base?.connectionId, database: base?.database });
      const conn = connectionsRef.current.find((c) => c.id === base?.connectionId);
      if (conn && base?.connected) connectTab(tab.id, conn, null, base.database).catch(() => undefined);
    },
    [createTab, connectTab],
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
  const actions = useRef({ execute, cancel, format, closeTab, createTab, reopenClosed });
  actions.current = { execute, cancel, format, closeTab, createTab, reopenClosed };

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
      if (e.ctrlKey && e.altKey && k === "d") {
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
        {activeTab?.connected && (
          <button onClick={() => loadCatalog(activeTab, true)} title="Reload objects for completion and search">
            ⟳ Objects
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
                <ResultsPanel result={activeTab?.result} running={!!activeTab?.running} />
              </div>
            </>
          )}
        </div>
        {panel && (
          <>
            <div className="splitter-v" onMouseDown={dragPanel} />
            <div className="side-panel" style={{ width: panelWidth }}>
              <div className="side-head">
                <b>{panel === "search" ? "SQL Search" : "SQL History"}</b>
                <span className="spacer" />
                <button onClick={() => setPanel(null)}>×</button>
              </div>
              {panel === "search" ? (
                <SearchPanel
                  connectionId={activeTab?.connected ? activeTab.connectionId : undefined}
                  database={activeTab?.database}
                  databases={activeTab?.databases ?? []}
                  dialect={activeTab?.dialect ?? "mssql"}
                  onOpenScript={(title, text) => openText(title, text)}
                />
              ) : (
                <HistoryPanel onOpen={openHistory} onInsert={insertText} />
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
          }}
          onConnect={async (c, pw) => {
            const conns = await api.listConnections();
            setConnections(conns);
            connectionsRef.current = conns;
            setDialog(null);
            const id = activeRef.current || createTab().id;
            connectTab(id, c, pw).catch((e) => alert(errorText(e)));
          }}
        />
      )}
      {dialog === "settings" && settings && (
        <SettingsDialog settings={settings} dialect={activeTab?.dialect ?? "mssql"} onClose={() => setDialog(null)} onSaved={setSettings} />
      )}
    </div>
  );
}
