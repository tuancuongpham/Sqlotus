import { invoke } from "@tauri-apps/api/core";
import type {
  CatalogInfo,
  DbObject,
  Diagnostic,
  EditorArgs,
  RefactorAction,
  ScriptKind,
  SignatureInfo,
  CompletionResult,
  ConnectionConfig,
  Dialect,
  ExecuteResult,
  HistoryEntry,
  SavedConnection,
  SearchHit,
  SearchOptions,
  Settings,
  Snippet,
  TabConnection,
} from "./types";

export const api = {
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  loadSession: () => invoke<unknown | null>("load_session"),
  saveSession: (session: unknown) => invoke<void>("save_session", { session }),

  listConnections: () => invoke<SavedConnection[]>("list_connections"),
  saveConnection: (config: ConnectionConfig, password: string | null, forgetPassword: boolean) =>
    invoke<void>("save_connection", { config, password, forgetPassword }),
  deleteConnection: (id: string) => invoke<void>("delete_connection", { id }),
  testConnection: (config: ConnectionConfig, password: string | null) =>
    invoke<string[]>("test_connection", { config, password }),

  connectTab: (tabId: string, connectionId: string, database: string | null, password: string | null) =>
    invoke<TabConnection>("connect_tab", { tabId, connectionId, database, password }),
  disconnectTab: (tabId: string) => invoke<void>("disconnect_tab", { tabId }),
  changeDatabase: (tabId: string, database: string) => invoke<string>("change_database", { tabId, database }),
  execute: (tabId: string, sql: string) => invoke<ExecuteResult>("execute", { tabId, sql }),
  cancelQuery: (tabId: string) => invoke<void>("cancel_query", { tabId }),

  loadCatalog: (connectionId: string, database: string, refresh: boolean) =>
    invoke<CatalogInfo>("load_catalog_cmd", { connectionId, database, refresh }),
  complete: (args: {
    connectionId?: string;
    database?: string;
    dialect: Dialect;
    text: string;
    offset: number;
    server?: string;
  }) => invoke<CompletionResult>("complete", { args }),
  formatSql: (text: string, dialect: Dialect) => invoke<string>("format_sql", { text, dialect }),
  statementAt: (text: string, dialect: Dialect, offset: number) =>
    invoke<{ start: number; end: number } | null>("statement_at", { text, dialect, offset }),

  searchObjects: (connectionId: string, databases: string[], query: string, options: SearchOptions) =>
    invoke<SearchHit[]>("search_objects", { connectionId, databases, query, options }),
  objectScript: (connectionId: string, database: string, schema: string, name: string, dialect: Dialect) =>
    invoke<string>("object_script", { connectionId, database, schema, name, dialect }),

  listSnippets: () => invoke<Snippet[]>("list_snippets"),
  saveSnippet: (snippet: Snippet) => invoke<void>("save_snippet", { snippet }),
  deleteSnippet: (id: string) => invoke<void>("delete_snippet", { id }),
  importRedgateSnippets: (dir: string | null) => invoke<number>("import_redgate_snippets", { dir }),

  historyRecord: (entry: {
    tabId: string;
    title: string;
    connectionName: string | null;
    database: string | null;
    dialect: string;
    text: string;
    kind: "autosave" | "execute";
  }) => invoke<number>("history_record", { entry }),
  historySearch: (query: string, limit = 200) => invoke<HistoryEntry[]>("history_search", { query, limit }),
  historyTab: (tabId: string) => invoke<HistoryEntry[]>("history_tab", { tabId }),
  historyMarkClosed: (tabId: string) => invoke<void>("history_mark_closed", { tabId }),
  historyRecentlyClosed: (limit = 20) => invoke<HistoryEntry[]>("history_recently_closed", { limit }),
  historyDelete: (id: number) => invoke<void>("history_delete", { id }),

  diagnostics: (args: EditorArgs) => invoke<Diagnostic[]>("diagnostics", { args }),
  executionWarnings: (text: string, dialect: Dialect) => invoke<string[]>("execution_warnings", { text, dialect }),
  hover: (args: EditorArgs) => invoke<{ start: number; end: number; contents: string } | null>("hover", { args }),
  signatureHelp: (args: EditorArgs) => invoke<SignatureInfo | null>("signature_help", { args }),
  objectAt: (args: EditorArgs) => invoke<{ schema: string; name: string; kind: string } | null>("object_at", { args }),
  refactorText: (args: EditorArgs, action: RefactorAction) => invoke<string>("refactor_text", { args, action }),
  catalogObjects: (connectionId: string, database: string) => invoke<DbObject[]>("catalog_objects", { connectionId, database }),
  scriptObject: (connectionId: string, database: string, schema: string, name: string, dialect: Dialect, kind: ScriptKind) =>
    invoke<string>("script_object", { connectionId, database, schema, name, dialect, kind }),
};

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}

export function newId(): string {
  return crypto.randomUUID();
}
