# Changelog

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
