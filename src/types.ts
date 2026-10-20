export type Dialect = "mssql" | "postgres" | "mysql";

export interface ConnectionConfig {
  id: string;
  name: string;
  dialect: Dialect;
  host: string;
  port?: number | null;
  database?: string | null;
  auth: "sql" | "windows";
  user?: string | null;
  encrypt: boolean;
  trustServerCertificate: boolean;
  color?: string | null;
}

export interface SavedConnection extends ConnectionConfig {
  hasPassword: boolean;
}

export interface TabConnection {
  database: string;
  databases: string[];
  dialect: Dialect;
}

export interface ResultColumn {
  name: string;
  typeName: string;
}

export interface ResultSet {
  columns: ResultColumn[];
  rows: (string | null)[][];
  truncated: boolean;
}

export interface Message {
  kind: "info" | "error";
  text: string;
}

export interface BatchOutput {
  resultSets: ResultSet[];
  messages: Message[];
  elapsedMs: number;
  failed: boolean;
}

export interface ExecuteResult {
  batches: { start: number; end: number; output: BatchOutput }[];
  database: string;
  cancelled: boolean;
}

export type CompletionKind =
  | "keyword"
  | "function"
  | "schema"
  | "table"
  | "view"
  | "tableFunction"
  | "scalarFunction"
  | "procedure"
  | "column"
  | "alias"
  | "localTable"
  | "variable"
  | "snippet"
  | "join"
  | "expansion";

export interface CompletionItem {
  label: string;
  kind: CompletionKind;
  detail: string;
  insertText: string;
  isSnippet: boolean;
  sortText: string;
  replaceStart: number | null;
  filterText: string | null;
}

export interface CompletionResult {
  replaceStart: number;
  replaceEnd: number;
  items: CompletionItem[];
}

export type ObjectKind =
  | "table"
  | "view"
  | "procedure"
  | "scalarFunction"
  | "tableFunction"
  | "trigger"
  | "synonym"
  | "sequence";

export interface CatalogInfo {
  objects: number;
  definitions: number;
  schemas: string[];
  tables: { schema: string; name: string; kind: ObjectKind }[];
}

export interface SearchOptions {
  exactMatch: boolean;
  caseSensitive: boolean;
  kinds: ObjectKind[];
  searchNames: boolean;
  searchColumns: boolean;
  searchDefinitions: boolean;
  maxResults: number;
}

export interface SearchHit {
  database: string;
  schema: string;
  name: string;
  kind: ObjectKind;
  matchedIn: "name" | "column" | "definition";
  detail: string;
  line: number;
  matchCount: number;
}

export interface Snippet {
  id: string;
  prefix: string;
  description: string;
  body: string;
  placeholders: { name: string; defaultValue: string }[];
}

export interface HistoryEntry {
  id: number;
  tabId: string;
  title: string;
  connectionName: string | null;
  database: string | null;
  dialect: string;
  text: string;
  kind: string;
  createdAt: number;
  updatedAt: number;
  closed: boolean;
}

export interface FormatOptions {
  keywordCase: "upper" | "lower" | "preserve";
  indentSize: number;
  listLayout: "aligned" | "stacked";
  commaStyle: "trailing" | "leading";
  indentJoins: boolean;
  onNewLine: boolean;
  andOrNewLine: boolean;
  blankLineBetweenStatements: boolean;
}

export interface CompletionOptions {
  keywordCase: "upper" | "lower" | "preserve";
  qualifyObjects: boolean;
  assignAliases: boolean;
  maxItems: number;
}

export interface Settings {
  format: FormatOptions;
  completion: CompletionOptions;
  editor: { fontSize: number; autosaveSeconds: number; maxRows: number; theme: "dark" | "light" };
}

/** An editor tab. Text lives in the Monaco model; this is the metadata. */
export interface Tab {
  id: string;
  title: string;
  dialect: Dialect;
  connectionId?: string;
  database?: string;
  databases: string[];
  connected: boolean;
  connecting: boolean;
  running: boolean;
  result?: ExecuteResult;
  executedAt?: number;
}
