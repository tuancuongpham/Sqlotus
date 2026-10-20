import { invoke } from "@tauri-apps/api/core";
import type {
  CatalogInfo,
  ColumnPlan,
  DbObject,
  EditSource,
  IndexHealth,
  Reference,
  RowEdit,
  SchemaCompareResult,
  SessionInfo,
  DiagramModel,
  Diagnosis,
  TestDataOptions,
  Fix,
  Diagnostic,
  EditorArgs,
  ObjectProblem,
  RefactorAction,
  ScriptKind,
  SignatureInfo,
  CompletionResult,
  ConnectionConfig,
  Dialect,
  ExecuteResult,
  ExplainOutput,
  HistoryEntry,
  ResultColumn,
  VariableValue,
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
  execute: (tabId: string, sql: string, variables: VariableValue[] | null = null) =>
    invoke<ExecuteResult>("execute", { tabId, sql, variables }),
  explain: (tabId: string, sql: string) => invoke<ExplainOutput>("explain", { tabId, sql }),
  undeclaredVariables: (text: string, dialect: Dialect) => invoke<string[]>("undeclared_variables", { text, dialect }),
  /** Write an .xlsx (to `path`, or a temp file) and optionally open it. Returns the path. */
  exportXlsx: (path: string | null, columns: ResultColumn[], rows: (string | null)[][], open: boolean) =>
    invoke<string>("export_xlsx", { path, columns, rows, open }),
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
  renameTarget: (args: EditorArgs) =>
    invoke<{ kind: string; name: string; ranges: [number, number][] } | null>("rename_target", { args }),
  findInvalidObjects: (connectionId: string, database: string, dialect: Dialect) =>
    invoke<ObjectProblem[]>("find_invalid_objects", { connectionId, database, dialect }),
  saveTextFile: (path: string, contents: string) => invoke<void>("save_text_file", { path, contents }),
  executeMulti: (tabId: string, sql: string, databases: string[]) =>
    invoke<ExecuteResult>("execute_multi", { tabId, sql, databases }),
  diagramModel: (connectionId: string, database: string) => invoke<DiagramModel>("diagram_model", { connectionId, database }),
  testDataPlan: (connectionId: string, database: string, schema: string, name: string, dialect: Dialect) =>
    invoke<ColumnPlan[]>("test_data_plan", { connectionId, database, schema, name, dialect }),
  testDataScript: (connectionId: string, database: string, schema: string, name: string, dialect: Dialect, options: TestDataOptions) =>
    invoke<string>("test_data_script", { connectionId, database, schema, name, dialect, options }),
  objectReferences: (connectionId: string, database: string, schema: string, name: string, column: string | null, dialect: Dialect) =>
    invoke<Reference[]>("object_references", { connectionId, database, schema, name, column, dialect }),
  modifyingStatements: (text: string, dialect: Dialect) => invoke<string[]>("modifying_statements", { text, dialect }),
  indexHealth: (connectionId: string, database: string, dialect: Dialect) => invoke<IndexHealth>("index_health", { connectionId, database, dialect }),
  serverActivity: (connectionId: string, database: string, dialect: Dialect) =>
    invoke<{ sessions: SessionInfo[]; note: string | null }>("server_activity", { connectionId, database, dialect }),
  killSession: (connectionId: string, database: string, dialect: Dialect, id: number, wholeSession: boolean) =>
    invoke<string>("kill_session", { connectionId, database, dialect, id, wholeSession }),
  closeActivity: (connectionId: string, database: string) => invoke<void>("close_activity", { connectionId, database }),
  editableSource: (connectionId: string, database: string, dialect: Dialect, sql: string, resultColumns: string[]) =>
    invoke<EditSource>("editable_source", { connectionId, database, dialect, sql, resultColumns }),
  editPreview: (source: EditSource, edits: RowEdit[], dialect: Dialect) =>
    invoke<{ statements: string[]; problems: string[] }>("edit_preview", { source, edits, dialect }),
  applyEdits: (tabId: string, source: EditSource, edits: RowEdit[]) =>
    invoke<{ applied: number; committed: boolean; openTransactions: number; notice: string }>("apply_edits", { tabId, source, edits }),
  compareSchemas: (
    source: { connectionId: string; database: string },
    target: { connectionId: string; database: string },
    options: { ignoreWhitespace: boolean; ignoreCase: boolean },
  ) => invoke<SchemaCompareResult>("compare_schemas", { source, target, options }),
  schemaSyncScript: (id: string, selected: string[]) =>
    invoke<{ script: string; warnings: string[]; statements: number }>("schema_sync_script", { id, selected }),
  takeStartupNotice: () => invoke<string | null>("take_startup_notice"),
  diagnoseConnection: (config: ConnectionConfig, error: string) => invoke<Diagnosis>("diagnose_connection", { config, error }),
  applyFix: (fix: Fix, config: ConnectionConfig, password: string | null) =>
    invoke<string>("apply_fix", { fix, config, password }),
};

export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}

export function newId(): string {
  return crypto.randomUUID();
}
