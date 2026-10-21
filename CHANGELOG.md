# Changelog

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

- First preview: completion, formatting, refactorings, code analysis, SQL Search, SQL History,
  Object Explorer, multi-database runs.
