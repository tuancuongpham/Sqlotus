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
  readOnly?: boolean;
  /** Changes wait in an open transaction until the user commits. */
  manualCommit?: boolean;
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
  rowsAffected?: number;
}

export interface ExecuteResult {
  batches: { start: number; end: number; output: BatchOutput }[];
  database: string;
  cancelled: boolean;
  openTransactions?: number;
  /** Rows changed by INSERT/UPDATE/DELETE/MERGE, when reported. */
  rowsAffected?: number | null;
  /** Sqlotus kept the changes in a transaction and needs a decision. */
  pendingCommit?: { rows: number; limit: number; failed: boolean } | null;
  notice?: string | null;
}

export interface PlanNode {
  op: string;
  object: string | null;
  detail: string | null;
  rows: number | null;
  cost: number | null;
  costPercent: number | null;
  warnings: string[];
  children: PlanNode[];
}

export interface StatementPlan {
  statement: string;
  cost: number | null;
  root: PlanNode | null;
  warnings: string[];
  missingIndexes: { table: string; impact: number | null; createStatement: string }[];
}

export interface ExplainOutput {
  plans: StatementPlan[];
  errors: string[];
}

export interface VariableValue {
  name: string;
  dataType: string;
  value: string | null;
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
  safety: { confirmRowsOver: number; promptVariables: boolean };
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
  /** Transactions left open after the last run. */
  openTransactions?: number;
  /** Estimated plan shown in the results area. */
  plan?: ExplainOutput;
  /** Which results view to show: rows or the plan. */
  view?: "results" | "plan";
}

export interface Diagnostic {
  start: number;
  end: number;
  severity: "error" | "warning" | "info";
  code: string;
  message: string;
}

export interface SignatureInfo {
  label: string;
  parameters: string[];
  activeParameter: number;
}

export interface CatalogColumn {
  name: string;
  dataType: string;
  nullable: boolean;
  isPrimaryKey: boolean;
  isIdentity: boolean;
  isComputed: boolean;
}

export interface DbObject {
  schema: string;
  name: string;
  kind: ObjectKind;
  columns: CatalogColumn[];
  params: { name: string; dataType: string; isOutput: boolean; hasDefault: boolean }[];
}

export type RefactorAction =
  | "expandWildcards"
  | "qualifyNames"
  | "insertSemicolons"
  | "removeBrackets"
  | "upperKeywords"
  | "lowerKeywords";

export type ScriptKind = "select" | "insert" | "update" | "delete" | "exec" | "create" | "alter";

export interface EditorArgs {
  connectionId?: string;
  database?: string;
  dialect: Dialect;
  text: string;
  offset?: number;
}

export interface ObjectProblem {
  schema: string;
  name: string;
  kind: ObjectKind;
  line: number;
  code: string;
  message: string;
}

export type Fix =
  | { kind: "enableTcp"; instanceId: string; service: string }
  | { kind: "startService"; service: string }
  | { kind: "enableBrowser" }
  | { kind: "enableMixedAuth"; instanceId: string; service: string }
  | { kind: "usePort"; port: number }
  | { kind: "useHost"; host: string }
  | { kind: "useWindowsAuth" }
  | { kind: "trustCertificate" }
  | { kind: "useDatabase"; database: string | null }
  | { kind: "openFirewall"; port: number; browser: boolean }
  | { kind: "enableLogin"; login: string };

export interface Finding {
  level: "ok" | "info" | "warning" | "error";
  title: string;
  detail: string;
  fix: Fix | null;
  fixLabel: string | null;
  needsAdmin: boolean;
}

export interface Diagnosis {
  summary: string;
  findings: Finding[];
}
