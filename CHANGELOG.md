# Changelog

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
