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
  /** Groups connections; production asks before every change. */
  environment?: Environment | null;
  /** A folder of the user's in the connection list. */
  group?: string | null;
}

export type Environment = "dev" | "test" | "staging" | "prod";

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
  /** Values cut to 65,535 characters for display (editing and exports would use the cut text). */
  clipped?: number;
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
  pendingCommit?: { rows: number | null; limit: number; failed: boolean; calls?: string[] } | null;
  notice?: string | null;
  /** The script created, altered or dropped objects: loaded objects are stale. */
  schemaChanged?: boolean;
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

export type Theme = "dark" | "light" | "system";

export interface EditorSettings {
  fontSize: number;
  autosaveSeconds: number;
  maxRows: number;
  /** "system" follows the Windows light/dark setting. */
  theme: Theme;
  /** Execute (F5) runs the statement at the cursor when nothing is selected. */
  f5RunsStatement?: boolean;
  /** Query history older than this many days is deleted; 0 keeps it all. */
  historyDays?: number;
}

export interface SafetySettings {
  confirmRowsOver: number;
  promptVariables: boolean;
  /** Scripts that call procedures run in a transaction; ask before committing. */
  confirmProcedureCalls?: boolean;
  /** CSV files: a ' before text starting with = + - @, so spreadsheets do not run it. */
  csvFormulaGuard?: boolean;
}

export interface Settings {
  format: FormatOptions;
  completion: CompletionOptions;
  editor: EditorSettings;
  safety: SafetySettings;
  /** Code analysis rules that are switched off. */
  analysis?: { disabled: string[] };
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
  /** The SQL of the last run (to know which table an edited result comes from). */
  lastSql?: string;
}

export interface Reference {
  schema: string;
  name: string;
  kind: ObjectKind;
  line: number;
  text: string;
  how: "reads" | "changes" | "calls" | "trigger" | "mentions" | "foreign key";
}

export interface IndexFinding {
  kind: "missing" | "duplicate" | "overlapping" | "unused" | "scans" | "heap";
  schema: string;
  table: string;
  index: string | null;
  detail: string;
  score: number;
  script: string | null;
}

export interface IndexHealth {
  findings: IndexFinding[];
  notes: string[];
  errors: string[];
}

export interface SessionInfo {
  id: number;
  blockedBy: number;
  blocking: number;
  login: string;
  host: string;
  program: string;
  database: string;
  status: string;
  command: string;
  wait: string;
  elapsedMs: number;
  cpuMs: number | null;
  reads: number | null;
  openTransaction: boolean;
  sql: string;
  running: boolean;
  isSelf: boolean;
}

export interface EditColumn {
  column: string | null;
  dataType: string;
  nullable: boolean;
  isKey: boolean;
  readOnly: boolean;
}

export interface EditSource {
  schema: string;
  table: string;
  columns: EditColumn[];
}

export type RowEdit =
  | { kind: "update"; original: Record<string, string | null>; values: Record<string, string | null> }
  | { kind: "insert"; values: Record<string, string | null> }
  | { kind: "delete"; original: Record<string, string | null> };

export interface ObjectDiff {
  key: string;
  kind: ObjectKind;
  schema: string;
  name: string;
  change: "added" | "removed" | "changed";
  details: string[];
  sourceText: string | null;
  targetText: string | null;
  warnings: string[];
}

/** An object by name (the identical ones in a comparison). */
export interface ObjectRef {
  key: string;
  kind: ObjectKind;
  schema: string;
  name: string;
}

export interface SchemaCompareResult {
  id: string;
  dialect: Dialect;
  source: string;
  target: string;
  diff: { differences: ObjectDiff[]; identical: number; identicalObjects: ObjectRef[]; skipped: string[] };
}

/** A comparison set up once and opened again from the list. */
export interface SavedCompare {
  name: string;
  source: { connectionId: string; database: string };
  target: { connectionId: string; database: string };
  ignoreWhitespace: boolean;
  ignoreCase: boolean;
}

/** What a results grid needs to know to edit rows. */
export interface EditContext {
  tabId: string;
  connectionId: string;
  database: string;
  dialect: Dialect;
  sql: string;
  readOnly: boolean;
  /** A production connection: Save asks first, naming the server. */
  production?: { name: string; host: string };
  /** Run the query again after saving. */
  onSaved: () => void;
  /** Open a query in a new tab of the same database and run it. */
  openQuery: (title: string, sql: string) => void;
}

/** A foreign key a result row can be followed along. */
export interface RowLink {
  direction: "references" | "referencedBy";
  constraint: string;
  /** Result columns holding the key's values, in key order. */
  columns: number[];
  schema: string;
  table: string;
  targetColumns: string[];
}

export interface DataScript {
  script: string;
  rows: number;
  truncated: boolean;
  problems: string[];
}

export interface FindValueOptions {
  value: string;
  mode: "contains" | "startsWith" | "exact";
  schema: string;
  ignoreCase: boolean;
}

export interface BackupInfo {
  backupDir: string;
  dataDir: string;
  logDir: string;
  express: boolean;
  recoveryModel: string;
}

export interface BackupOptions {
  database: string;
  kind: "full" | "differential" | "log";
  path: string;
  copyOnly: boolean;
  compression: boolean;
  checksum: boolean;
  overwrite: boolean;
  verify: boolean;
}

export interface BackupFile {
  logicalName: string;
  physicalName: string;
  /** D data, L log, F full-text catalog, S FILESTREAM. */
  kind: string;
}

export interface RestoreMove {
  logicalName: string;
  path: string;
}

export interface RestoreOptions {
  path: string;
  fileNumber: number;
  database: string;
  moves: RestoreMove[];
  replace: boolean;
  recovery: boolean;
  closeConnections: boolean;
}

export interface BackupContents {
  sets: { position: number; databaseName: string; kind: string; finished: string }[];
  files: BackupFile[];
}

export interface Diagnostic {
  start: number;
  end: number;
  severity: "error" | "warning" | "info";
  code: string;
  message: string;
  /** A safe rewrite of start..end (UTF-16 offsets). */
  fix?: { title: string; start: number; end: number; text: string };
  /** "Ignore on this line": the comment that switches the rule off there (same shape). */
  ignore?: { title: string; start: number; end: number; text: string };
}

/** A code analysis rule (Options → Analysis). */
export interface AnalysisRule {
  code: string;
  title: string;
  severity: Diagnostic["severity"];
  needsCatalog: boolean;
  only: Dialect | null;
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

export interface ForeignKey {
  name: string;
  fromSchema: string;
  fromTable: string;
  fromColumns: string[];
  toSchema: string;
  toTable: string;
  toColumns: string[];
}

export interface DiagramModel {
  tables: DbObject[];
  foreignKeys: ForeignKey[];
}

export type Generator =
  | "skip"
  | "null"
  | "fullName"
  | "firstName"
  | "lastName"
  | "email"
  | "phone"
  | "address"
  | "city"
  | "country"
  | "company"
  | "product"
  | "code"
  | "status"
  | "gender"
  | "url"
  | "text"
  | "integer"
  | "decimal"
  | "date"
  | "birthDate"
  | "dateTime"
  | "boolean"
  | "guid"
  | "sequence"
  | "foreignKey"
  | "fixed";

export interface ColumnPlan {
  column: string;
  dataType: string;
  nullable: boolean;
  generator: Generator;
  note: string;
}

export interface TestDataOptions {
  rows: number;
  locale: "vi" | "en";
  nullPercent: number;
  seed: number;
  overrides: Record<string, Generator>;
  fixed: Record<string, string>;
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

/** The folder a database's schema is kept in (Source control). */
export interface ScLinkInfo {
  folder: string;
  git: boolean;
  branch: string | null;
  hasSnapshot: boolean;
}

/** One object that differs between the database and its folder. */
export interface ScItem {
  key: string;
  kind: ObjectKind;
  schema: string;
  name: string;
  /** local: changed in the database; incoming: changed in the folder; conflict: both. */
  state: "local" | "incoming" | "conflict";
  /** The database compared with the folder (added = only in the database). */
  change: "added" | "removed" | "changed";
  details: string[];
  dbText: string | null;
  folderText: string | null;
}

export interface ScStatus {
  id: string;
  folder: string;
  dialect: Dialect;
  items: ScItem[];
  inSync: number;
  skipped: string[];
  handEdited: string[];
  /** Files in the folder git has not committed yet. */
  uncommitted: string[];
  git: boolean;
  branch: string | null;
  firstCommit: boolean;
  /** A common version of database and folder is known (else differences are all conflicts). */
  hasBase: boolean;
}

export interface ScCommitResult {
  written: number;
  removed: number;
  commit: string | null;
  note: string;
}
