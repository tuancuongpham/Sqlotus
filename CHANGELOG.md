# Changelog

## 0.2.8

- **Objects reload by themselves** after a script creates, alters, drops or renames something (also `sp_rename` and
  `SELECT … INTO`): completion, analysis, Object Explorer and the diagram know the new table at once. A change made
  inside an open transaction is reloaded when the transaction ends (reading the objects meanwhile would wait for it).
- **F5 can run the statement at the cursor** (Options → Editor): Execute then runs the selection, or the statement at
  the cursor when nothing is selected. Select all (Ctrl+A) to run the whole script.
- **Theme follows Windows** (Options → Editor → Theme), switching as Windows switches. **Ctrl+mouse wheel** (or
  Ctrl+= / Ctrl+-) changes the editor's font size and keeps it; Ctrl+0 resets it.
- **Results as text**: Grid / Text above a result shows the rows as a fixed-width text table, as a terminal client
  prints them (numbers right-aligned; accented and wide characters keep the columns straight). The choice stays for
  later results. Copy as → Text table copies it.
- **Record**: the selected row as a form — every column with its type and whole value, a filter for wide tables,
  previous / next row; click a value to copy it.
- **Follow foreign keys** from a result: key columns are marked ↗ when the result can be traced to its tables (joins
  too). Ctrl+click a value to open the row it references in a new tab; right-click a row for the rows that reference
  it (Customers → their Orders).
- **Data as INSERT** (Object Explorer, a table): its rows as INSERT statements — the first N, optionally only those
  matching a condition — with identity values kept (IDENTITY_INSERT, OVERRIDING SYSTEM VALUE and the sequence moved
  on), computed and row-version columns left out. The script opens in a new tab.
- **Find value** (Object Explorer): a script that looks for a text in every text column of every table and lists the
  table, column, number of rows and an example. `%` and `_` in the text are matched as they are. The script opens for
  review: it only reads, but it reads each column in full.
- **SQLCMD variables** (SQL Server): `:setvar Name "value"` and `$(Name)`, plus `:on error exit`. Running a selection
  or the statement at the cursor keeps the variables defined above it; formatting, analysis and the estimated plan
  understand them; SQLCMD commands Sqlotus cannot run (`:r`, `:connect`) are refused before anything is sent.
- **Backup and restore** (SQL Server, Object Explorer → Backup…): full, differential or log backups with copy-only,
  compression, checksums and a check that the file can be restored; restore a backup, also as a copy next to the
  original with its files placed in the server's data and log folders. Nothing runs from the dialog: the script
  opens in a tab.
- **Connection folders**: give connections a folder (a customer, a project) in the Connect dialog; folders are
  listed together and fold, and a filter finds a connection by name, server, database or folder.
- **Query history for N days** (Options → Editor): older versions are deleted at start and when Options closes; the
  last version of an open tab is kept. Off (0) by default.
- **Ask before committing what procedures changed** (Options → Safety, off by default): a script that calls
  procedures runs in a transaction, and you commit or roll back their changes. A procedure that changed nothing is
  committed without a question.
- **CSV files safe for spreadsheets** (Options → Safety, on): text starting with `=`, `+`, `-` or `@` gets a `'` in
  front so Excel shows it instead of running it as a formula; numbers stay as they are.
- **Manual commit** connections now keep what called procedures change in the open transaction too (an `EXEC` was
  committed at once). BACKUP, RESTORE, CREATE / ALTER DATABASE and other statements a server refuses inside a
  transaction failed on such connections; they now run without one, and the notification says so.
- **MySQL** columns with a default such as `CURRENT_TIMESTAMP` were taken for computed columns, so editing rows could
  not change them.
- Shift+click on the row numbers after selecting all rows of a very large result (hundreds of thousands of rows)
  failed.
- **Execute current statement** (Shift+F5) inside a MySQL procedure, function, trigger or event written without
  `DELIMITER` ran the statement under the cursor on its own (a DELETE of the body emptied the table) instead of
  creating the routine. It now runs the whole definition — also between `DELIMITER` lines when the routine has no
  BEGIN … END, and with the comments that end it.
- **Edited rows waiting to be saved** were dropped without a word when you switched to another tab and back.
  They now stay with their tab until you save or cancel; a result grid is also never reused for another tab's
  result.
- **Your open tabs** were saved every 3 seconds even when nothing had changed (a 2 MB tab was written each time),
  and if the last session could not be read at start (a connection list that failed to load), the empty start was
  saved over it. The session is now saved only after it was restored, and only when it changed.
- **Reopen closed tab** (Ctrl+Shift+T) could bring back an older version of the tab when it was pressed right after
  closing it.
- **Ignore on this line** while a string or comment was still being typed at the end of the script put the
  `-- sqlotus-disable-line` comment inside it; it now goes on the line above.
- Formatting, Execute current statement, the checks before running, undeclared variables, query history and the
  session no longer run on the window's main thread, so a big script cannot stall the window.
- **Large scripts stay fast**: code analysis after each edit took 18 s on a 2 MB data script (20,000 INSERTs) and
  now takes 0.2 s; the checks made when it runs took 6 s (now 0.3 s); Ignore on this line, Insert semicolons,
  Expand wildcards, Qualify names and Rename slowed down the same way. Completion after JOIN took 17 s per
  keystroke in a database of 8,000 tables with foreign keys (now 40 ms). Schema compare built the sync script of
  1,000 views on views in 10–17 s (now 20 ms) and of 2,000 changed tables in about a second (now 50 ms).
- A query with subqueries nested thousands of levels deep (pasted, generated) closed the application with every
  unsaved tab: analysing it overflowed the stack.
- **Saving edited rows on SQL Server**: when the row had been changed by someone else, a trigger that writes
  elsewhere (an audit log) made the save look successful, and the change was silently lost. The save now checks the
  statement's own row count.
- **Statements inside a procedure, function, trigger or view** are not warned about when the script only creates
  it (a procedure that clears a staging table asked "DELETE has no WHERE clause" every time it was altered), and are
  not listed in the production confirmation. `ALTER TABLE … DISABLE TRIGGER` no longer passes for a trigger
  definition, which hid the undeclared variables of its batch.
- **Schema compare and Get latest** with tables of one name in two schemas (`dbo.Orders`, `sales.Orders`): a
  foreign key of the same name on both was dropped only once, so the script failed; a changed primary key left the
  foreign key of the other table in place; warnings name the schema.
- **Test data on MySQL 8**: unsigned integer columns (`int unsigned`, `smallint unsigned`) got codes such as
  `OR0001` or were left out, so the script failed. Editing rows checks their values as numbers too.
- A saved connection or snippet that this version cannot read (written by a newer one) no longer hides all the
  others: the connection list stayed empty, and completion stopped working.
- **PostgreSQL and MySQL connections** accept `host:port` (and `[::1]:5432`) in the host field, like `host,port`
  on SQL Server; it used to be looked up as a server name.
- **Formatting** keeps client lines as they are: `GO 5` lost its count (the number moved into the next batch) and
  MySQL `DELIMITER` scripts came out broken. T-SQL `label:` keeps its colon, PostgreSQL operators such as `<->`,
  `<=>`, `!~*` and `@@` (pgvector, full-text search) stay whole, and CRLF documents stay CRLF.
- **Execute current statement** (Shift+F5) runs whole blocks: a T-SQL CREATE / ALTER PROCEDURE, FUNCTION, TRIGGER or
  VIEW with the rest of its batch (a procedure without BEGIN … END could be created cut short), BEGIN … END,
  TRY / CATCH, IF / ELSE and WHILE, and PostgreSQL BEGIN ATOMIC bodies.
- **Production confirmations and read-only connections** recognise more changes: PostgreSQL DO blocks, COPY … FROM,
  REFRESH MATERIALIZED VIEW, VACUUM / CLUSTER / REINDEX, pg_terminate_backend, pg_cancel_backend and setval; SQL
  Server RESTORE, BACKUP, DBCC (other than the read-only checks), RECONFIGURE, ENABLE / DISABLE TRIGGER, UPDATETEXT /
  WRITETEXT; MySQL LOAD DATA, SET GLOBAL / PERSIST, SHUTDOWN, RESTART, INSTALL / UNINSTALL, PURGE, RESET, FLUSH,
  OPTIMIZE / REPAIR TABLE, and statements inside dump comments (`/*!50003 CREATE … */`). Temp tables and table
  variables (`#t`, `@t`, `CREATE TEMP TABLE`) and MySQL `SELECT … INTO @var` no longer count as changes.
- **SQL Server row counts** with NOCOUNT on (in the session or the script): saving edited rows failed with "changed
  by someone else", and the large-change confirmation saw 0 rows and committed without asking.
- **Saving edited rows**: SQL Server xml, sysname and other Unicode columns keep Vietnamese and emoji (they were
  saved as `?`); datetime / smalldatetime are written as ISO 8601 (a British, French or German login read
  2024-01-12 as 1 December); PostgreSQL bit, bytea and money and MySQL BIT and BLOB values can be saved.
- **MySQL results**: BIT columns show their number and binary columns always `0x…`; columns carry their type
  (bigint, varbinary …) like the other servers, so Excel, JSON and Copy as INSERT keep numbers as numbers. A single
  PostgreSQL statement now gets its column types too.
- **Copy as INSERT**: binary values as `X'…'` (MySQL) / `0x…` (SQL Server), SQL Server datetime as ISO 8601, and on
  PostgreSQL every value is quoted unless the column is numeric (text such as `'00123'` lost its zeros).
- **Excel**: a value longer than the 32,767 characters a cell holds is cut, with a warning, instead of failing the
  whole export.
- **Test data**: decimals fit their precision (`decimal(5,2)` failed the whole script), MySQL tinyint stays signed,
  SQL Server datetime is ISO 8601 and xml / sysname values keep Unicode, PostgreSQL bit columns get bit strings.
- **Schema compare and Get latest**: views, routines and triggers are dropped and created in dependency order (a
  view on a view could fail); PostgreSQL views and triggers built on a view or column that changes are dropped and
  created again; SQL Server drops the indexes, keys, checks and computed columns that use a column whose type
  changes and puts them back, and drops columns before changing others; PostgreSQL generated columns over a
  changing column are rebuilt.
- **Rename** (F2): renaming a CTE leaves columns and column aliases of the same name alone; a bare name under the
  cursor is a column, not the table alias; the alias of a function in FROM (`generate_series(…) AS g`) is not
  renamed, since it also names the column.
- **Analysis**: SQ001 no longer reports trigger events (BEFORE UPDATE, INSERT OR UPDATE) or UPDATE / DELETE limited
  by an inner join; SQ009 not when the subquery filters `col IS NOT NULL`.
- **Undeclared variables**: a table variable used as a table (`FROM @t`) is not asked for; datetime values are
  ISO 8601.
- Statements made only of comments are not sent on their own.
- DOMPurify 3.4.16 (bundled with the editor).
- Tests: properties over a corpus of tricky SQL for all three servers (formatting, refactorings, statement
  splitting, quick fixes, Ignore on this line, rename), live checks on real servers of every rewrite's syntax,
  edited rows (also on tables with triggers), Copy as INSERT, generated test data and schema syncs of awkward
  schemas, and timing checks that editor features and schema sync grow no faster than the script or schema.
  Simulated editing sessions type every corpus case keystroke by keystroke (typos, Backspace, pastes, edits in
  the middle) and check every editor feature at each moment; a new end-to-end suite drives the app as a person
  does (connections typed by hand, edits saved while someone else changes the row, tabs closed with an open
  transaction, pasted 2 MB and deeply nested scripts). Live checks on the three servers script a table of every
  common type as INSERT statements and get every row back (identity values and keys with gaps included), find
  planted values (and nothing that only looks alike), run SQLCMD scripts, back up a database and restore it as a
  copy; another end-to-end suite covers every new feature above.

## 0.2.7

- **Production connections** now also confirm runs on multiple databases (Ctrl+Shift+F5) and Save of edited rows,
  showing the statements and naming the server. A script that cannot be analysed asks too.
- **Long values**: values over 65,535 characters are shown cut. The grid now says how many were cut, copies and
  exports warn that they hold the cut text, and such results cannot be edited (saving could store the cut text).
- **Quick fixes** apply only while the analysis matches the text: right after typing, Fix / Fix all wait for the
  new analysis instead of editing at old positions.
- **Ignore on this line** never writes into a string or comment: a line that already ends in a comment gets a
  `/* sqlotus-disable-line … */` before it, and a line ending inside a multi-line string gets
  `-- sqlotus-disable-next-line …` above it.
- **Source control**: a folder that holds only the deploy scripts of an earlier link can be linked again; a linked
  folder that was moved or deleted is reported; files git has not committed (for example after a commit git
  refused) are listed with **Commit these files**; the first Push of a new branch publishes it to `origin`, and a
  missing remote is explained. Git runs with English messages, file names with accents stay readable, and objects
  named like Windows devices (`COM4`, `LPT9`, `nul.backup`) get a file name Windows accepts.
- **sqlotus-lint**: reads UTF-16 big-endian files, keeps the UTF-8 BOM when fixing, never rewrites a file that is
  not UTF-8 or UTF-16 (it would lose its accented characters), and does not follow links to folders (a link to a
  parent folder looped).
- **Schema compare**: All / None act on the objects the filter shows, the list says how many selected objects are
  hidden by the filter, and opening a saved comparison whose connection was deleted says so.

## 0.2.6

- **Source control (git)**: link a database to a folder, usually a git repository. Sqlotus writes one `.sql` file per
  object plus a snapshot, sorts differences into changes in the database (commit), changes in the folder (get
  latest) and conflicts, commits with git, and builds Get latest scripts between pre- and post-deploy scripts —
  opened for review, never run. Pull, Push and History; hand-edited files are reported.
- **Problems** tab: every analysis problem of the tab, grouped and filtered, with quick fixes (`= NULL` → `IS NULL`,
  the N prefix, unused declarations), Fix all as one undo step, and copy as Markdown. The light bulb (Ctrl+.)
  offers the fix and "Ignore on this line".
- **Rules**: switch them off in Options → Analysis, or in the script with `-- sqlotus-disable-line`,
  `-- sqlotus-disable-next-line` and `-- sqlotus-disable` / `-- sqlotus-enable` comments.
- **`sqlotus-lint`**: the same analysis for `.sql` files on the command line — text, JSON or GitHub annotations,
  `--fix`, `--fail-on`, and `--schema` to check names against a source-control folder.
- **Schema compare**: filter the list, see the identical objects, save comparisons, export an HTML or Markdown report.

## 0.2.5

- **MySQL `DELIMITER` scripts** (dumps, procedures, triggers) now run: each statement ending in the custom
  delimiter is sent on its own, as the mysql client does, and Shift+F5 runs a whole procedure.
- **Backslashes on MySQL**: edited cells, values for undeclared variables, test data, copied INSERT statements and
  schema-compare defaults now escape `\`, which MySQL reads as an escape character (`C:\temp` was saved as
  `C:<tab>emp`).
- **Copy as JSON** keeps long decimals and big integers as strings instead of rounding them; copied INSERT
  statements quote column names containing `` ` `` or `"`.
- **Code analysis** reports variables that are assigned but never read, and understands `DECLARE @a AS int, @b …`.
- **Execution warnings** also cover statements that reach other sessions or the server: `WITH ROLLBACK
  IMMEDIATE`, `SET OFFLINE`, `KILL`, `SHUTDOWN`. Read-only connections refuse `KILL`, also from the activity monitor.
- Edited numeric cells accept plain numbers only (`NaN`, `inf` are refused before reaching the server).
- MySQL: `--` starts a comment only when a space follows (`5--2` is arithmetic).
- Frontend unit tests (`npm test`) run in CI.

## 0.2.4

- **Read-only connections** on PostgreSQL and MySQL are now read-only on the server too, so procedures and
  functions cannot write either. (SQL Server has no read-only session; the README says so.)
- **Production connections** also confirm procedure calls (`EXEC`, `CALL`), which may change data.
- **Execution warnings** now cover `TRUNCATE t` without the TABLE keyword (PostgreSQL, MySQL) and
  `ALTER TABLE … DROP COLUMN`.
- **Tab after `*`** without a `FROM` clause now says to write `FROM` first instead of silently inserting a tab;
  the READMEs explain that the columns come from the tables in `FROM`.
- README: states exactly which statements ask first, the SQL Server read-only limit, and that MariaDB is untested.

## 0.2.3

- New app icon: earlier builds still used the default icon of the Tauri framework.
- The search and history panels are now called **Object Search** and **Query History**.
- Snippets: **Import snippets…** (Options → Snippets) reads any folder of `.json` snippet files; snippets are no
  longer imported automatically on first run.
- Test data: e-mail addresses use reserved example domains only, and the dialog notes that generated phone numbers
  may belong to real people.
- Third-party licenses now say where to get the source code of every component (as MPL-2.0 requires).

## 0.2.2

- **Schema compare**: compare two databases (tables, keys, indexes, foreign keys, checks, views, routines,
  triggers) and generate a dependency-ordered sync script with data-loss warnings. SQL Server requires
  VIEW DEFINITION, so hidden objects are never mistaken for missing ones.
- Content Security Policy enabled for release builds.
- The installer includes the license and third-party licenses (`THIRD_PARTY_LICENSES.md`).

## 0.2.1

- Used by: what uses a table, column or module.
- Index health: missing, duplicate, overlapping and unused indexes; full-scan tables; heaps.
- Activity monitor: sessions, waits, open transactions, blocking chains; cancel / kill.
- Edit rows of a single-table result, saved in one transaction with optimistic checks.
- ER diagram and realistic test data.
- Copy results as Markdown, JSON or HTML; connection environments (production asks before changes).
- Fixes: commit guard with `SET NOCOUNT ON`, quoting of reserved words, PostgreSQL failed transactions,
  Excel precision, reconnect after the server ends a session.

## 0.2.0

- Renamed to Sqlotus (data from SqlSupportQuery is moved over).
- Connection Doctor with one-click fixes; notifications for every action.
- Ask before committing large changes; manual commit and read-only connections.
- Estimated execution plans, new analysis rules, values for undeclared variables.
- Excel export, charts, result comparison, range aggregates.

## 0.1.0

- First preview: completion, formatting, refactorings, code analysis, object search, query history,
  Object Explorer, multi-database runs.
