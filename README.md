# Sqlotus

**English** · [Tiếng Việt](README.vi.md)

A fast desktop SQL editor inspired by Redgate SQL Prompt, SQL Search and SQL History — for
**SQL Server, PostgreSQL and MySQL/MariaDB**.

Built with Rust (Tauri 2), React/TypeScript and the Monaco editor. The SQL intelligence (tokenizer,
formatter, completion, code analysis, refactorings) is a pure-Rust library with no IO.

## Features

### Writing SQL fast
- **Context-aware completion**: keywords, schemas, tables, views, columns, procedures and functions;
  understands aliases, CTEs, derived tables, `#temp` tables and `@table` variables; assigns table aliases
  automatically (`OrderDetails` → `od`).
- **Foreign-key joins**: after `JOIN` it suggests related tables together with the `ON` clause, and after
  `ON` the join condition.
- **Statement generation like SQL Prompt**:
  - pick a table after `INSERT INTO` → full column list plus a commented `VALUES` block
    (identity/computed/rowversion columns skipped, Tab jumps between values);
  - `EXEC proc` → one parameter per line with sample values, type comments and `(has default)`;
  - `GROUP BY` → the non-aggregated SELECT items.
- **Wildcard expansion**: type `SELECT *` (or `t.*`) and press **Tab** to get the column list.
- **Cross-database completion**: `OtherDb.` → schemas, `OtherDb.dbo.` → objects, and columns of tables
  in other databases.
- **Hover** shows a table's columns, a column's type or a variable's type; **parameter hints** while typing
  function and procedure calls.
- **Snippets** with placeholders; SQL Prompt snippets are imported automatically on first run.

### Formatting and refactoring
- **Format SQL** (Ctrl+K, Ctrl+Y): keyword case, Redgate-style aligned or stacked lists, leading/trailing
  commas, JOIN indentation, ON/AND/OR on new lines. The formatter verifies that only whitespace and keyword
  case changed.
- **Actions** (right-click or F1): expand wildcards, qualify object names, insert semicolons, remove square
  brackets, upper/lower-case keywords — each a single undoable edit.
- **Rename** (F2) a variable, table alias or CTE everywhere it is used.
- **Go to definition** (F12) and **Script as ALTER** (Alt+F12).

### Safety and code analysis
- Squiggles while typing: UPDATE/DELETE without WHERE, `SELECT *`, `= NULL` comparisons, deprecated types,
  unused variables, unknown objects and columns, Unicode literals without the `N` prefix
  (`'Nguyễn'` would be stored as `Nguy?n`), `NOT IN (SELECT …)` over a nullable column, functions wrapped
  around a column in WHERE/ON (`YEAR(OrderDate) = 2024` cannot use an index), and comparisons that force an
  implicit conversion of the column (`varchar_col = N'x'`, `text_col = 123`).
- **Execution warnings**: confirmation before running UPDATE/DELETE without WHERE, TRUNCATE or DROP.
- **Read-only connections**: block INSERT/UPDATE/DELETE/MERGE/DDL/GRANT on a connection (e.g. production)
  while still running queries; such tabs show a 🔒.
- **Open transaction reminder**: when a run leaves a transaction open (SQL Server, PostgreSQL, MySQL) the tab
  shows **TRAN**, the toolbar shows **Commit** / **Roll back**, and closing the tab asks what to do.
- **Confirm large changes**: scripts that only insert, update or delete rows run inside a transaction; if they
  change more rows than a limit (1,000 by default, Options → Safety) or fail half-way, Sqlotus asks whether to
  commit or roll back. Smaller changes are committed automatically and the row count is reported.
- **Manual commit** per connection: every change starts a transaction that stays open until you commit.
- **Undeclared variables**: running `… WHERE Id = @Id` without a `DECLARE` asks for the value (and type) and
  declares it for you; values are remembered.
- **Find invalid objects**: views, procedures, functions and triggers that reference missing tables or columns.

### Connection Doctor and notifications
- When a connection fails, the **Connection Doctor** explains why: server refused, timeout, unknown host or
  instance, login failed, certificate, database, Windows authentication. For a SQL Server on this computer it
  checks the service, TCP/IP, listening ports, SQL Server Browser and the authentication mode.
- One-click fixes: enable TCP/IP, start the service, start SQL Server Browser, enable SQL Server
  authentication (these ask Windows for administrator permission), use the right port or host, switch to
  Windows authentication, trust the certificate, pick another database, allow the port in Windows Firewall,
  enable a disabled login. It reconnects afterwards and reports the result.
- **Login failed (18456)** on a local server: the real reason is read from the Windows event log (wrong
  password, unknown login, disabled login, expired password, database unavailable). For remote servers it
  explains how to read the server's error log and what each state number means.
- **Check server** in the Connect dialog runs the same checks without an error, e.g. to let other computers
  connect (TCP/IP, firewall, SQL Server Browser).
- Every action and setting shows a notification: connecting, query finished (time and rows), errors with
  **Diagnose** / **Retry**, format and refactor, copy, CSV export, saving connections and options, restored tabs.

### Search, history, explorer
- **SQL Search** (Ctrl+Alt+D): find objects by name, column name or definition text in one or all databases,
  with a highlighted preview.
- **SQL History** (Ctrl+Shift+H): every tab is versioned automatically (periodically and on each execute),
  full-text search, reopen closed tabs (Ctrl+Shift+T), diff against the previous version or the current tab.
  Open tabs are restored on restart.
- **Object Explorer** (F8, open by default): filter objects and columns, Select top 100, script
  SELECT/INSERT/UPDATE/DELETE/EXEC/CREATE/ALTER.
- **ER diagram**: tables and foreign keys of a database, or one table and its related tables, laid out
  automatically; drag, zoom, highlight relations, copy as image or save as SVG.
- **Used by**: views, procedures, functions, triggers and foreign keys that use a table, a column or a module,
  with the line — before you rename or drop something.
- **Schema compare** (toolbar → Compare): two databases on the same or different saved connections — tables
  (columns, types, NULL, defaults, identity, computed), primary keys, indexes, unique constraints, foreign keys,
  checks, views, procedures, functions and triggers, with a side-by-side diff per object. The sync script is
  ordered by dependencies (foreign keys dropped first and re-created last, missing schemas created), lists
  data-loss warnings at the top, and opens in a tab connected to the target — it is never run for you. Objects
  only in the target are not selected unless you choose to drop them. SQL Server requires VIEW DEFINITION:
  without it hidden objects would look missing, so the comparison refuses instead.
- **Index health**: missing indexes the server suggests, duplicate and overlapping indexes, indexes nobody reads,
  tables read by full scans and large heaps. Fixes are CREATE/DROP scripts opened in a tab, never run directly.
- **Test data**: realistic rows for a table (Vietnamese or English names, e-mails, phones, addresses, amounts,
  dates); foreign keys use rows that exist in the parent table and non-identity keys continue after the current
  maximum. The INSERT script opens in a new tab to review and run.

### Running queries
- F5 / Ctrl+E runs the selection or the tab, Shift+F5 the statement at the cursor; `GO` / `GO n` batches;
  cancel; multiple result sets.
- **Run on multiple databases** (Ctrl+Shift+F5): every row gets a `Database` column; results with the same
  shape are combined.
- Virtualized result grid: sort by column, filter, copy (with headers), **copy as INSERT statements / CSV /
  IN (…) list**, save as CSV. Shift+click a range in a column to see Count / Sum / Avg / Min / Max.
- **Excel**: open the result in Excel or save it as .xlsx (numbers stay numbers, header row, filters).
- **Chart**: bar, line or pie chart of a result; copy it as an image.
- **Compare results**: pin a result, run again (or another query), then compare — added, removed and changed
  rows matched on the key columns you choose.
- **Estimated execution plan** (Ctrl+L): a tree with the cost share of each step and warnings for scans that
  read a whole table, key lookups, implicit conversions, missing join predicates, plus missing-index
  suggestions with a ready CREATE INDEX statement. SQL Server, PostgreSQL and MySQL; the query is not run.
- **Edit rows** (✎ Edit on a result from one table with a primary key): change cells, add and delete rows, see the
  statements first; Save runs them in one transaction and keeps nothing if a row was changed by someone else meanwhile.
- **Copy as** INSERT statements, CSV, IN (…) list, Markdown table, JSON or HTML table.
- **Activity monitor**: sessions, running queries, waits, open transactions and blocking chains (who blocks whom);
  cancel a query or kill a session after confirming. SQL Server, PostgreSQL and MySQL.
- **Environments**: mark connections Development / Test / Staging / Production. Production gets a red tab, a PROD
  badge, manual commit, and a confirmation naming the server before every change.
- Per-connection tab colours. Passwords are stored in the Windows Credential Manager.

## Keyboard shortcuts

| Keys | Action |
|---|---|
| F5 / Ctrl+E | Execute |
| Shift+F5 | Execute current statement |
| Ctrl+Shift+F5 | Run on multiple databases |
| Alt+Break | Cancel |
| Ctrl+L | Estimated execution plan |
| Ctrl+K, Ctrl+Y | Format |
| Tab after `*` | Expand to column list |
| F2 | Rename variable / alias / CTE |
| F12 / Alt+F12 | Go to definition / Script as ALTER |
| Ctrl+B, Ctrl+W | Expand wildcards |
| Ctrl+B, Ctrl+Q | Qualify object names |
| F8 | Object Explorer |
| Ctrl+Alt+D | SQL Search |
| Ctrl+Shift+H | SQL History |
| F1 | Command palette |
| Ctrl+N / Ctrl+W / Ctrl+Tab | New / close / next tab |
| Ctrl+Shift+T | Reopen closed tab |
| Ctrl+R | Toggle results |

## Development

Requirements: Rust (MSVC toolchain), Node 22+, Visual Studio Build Tools (C++). Windows is the primary
target (WebView2).

```bash
npm install
npx tauri dev
```

```bash
cargo test --workspace
```

```bash
npx tauri build
```

### Layout

- `crates/sqlotus-core` — fault-tolerant tokenizer, formatter, completion, scope analysis, code analysis,
  refactorings, rename, snippets, search. No IO.
- `crates/sqlotus-db` — connections and query execution (tiberius / tokio-postgres / mysql_async), metadata loading.
- `crates/sqlotus-store` — SQLite storage: connections, settings, snippets, history (FTS5 trigram), catalog cache.
- `src-tauri` — Tauri commands, per-tab database sessions.
- `src` — React + Monaco UI.
- `scripts/` — UI tests over WebView2's DevTools Protocol (`ui-smoke.mjs`, `e2e-db.mjs`, `e2e-features.mjs`)
  and Docker test databases (`test-db/`).
- `docs/redgate-comparison.md` — feature comparison with Redgate and ideas (Vietnamese).
- `docs/pain-points.md` — common SQL tool pain points, which paid tools address them, and what Sqlotus does (Vietnamese).
- `docs/feature-proposals.md` — researched feature proposals with an assessment of each (Vietnamese).
- `docs/review-2026-10.md` — code review: bugs found and fixed, remaining risks (Vietnamese).

### Testing against real databases

`scripts/test-db/README.md` starts SQL Server, PostgreSQL and MySQL in Docker with a sample schema. Then set
`SQLOTUS_TEST_MSSQL` / `SQLOTUS_TEST_PG` / `SQLOTUS_TEST_MYSQL` (see `crates/sqlotus-db/tests/live.rs`) and run
`cargo test -p sqlotus-db --test live`. The UI scripts need the app running with
`WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222`.

## Known limitations

- SQL Server: `PRINT` messages and DML row counts are not shown yet (not exposed by tiberius).
- Named-pipe connections (LocalDB) are not supported; use TCP.
- No AI assistant yet.

## License

[MIT](LICENSE)
