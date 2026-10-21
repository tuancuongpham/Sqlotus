# Security policy

## Reporting a vulnerability

Please do **not** open a public issue for security problems. Use GitHub's private reporting instead:
**Security → Report a vulnerability** on this repository. You will get an answer within a few days.

Useful details: the Sqlotus version, what an attacker could do, and steps to reproduce.

## Supported versions

Fixes go into the latest release only.

## How Sqlotus handles your data

- Saved passwords are stored in the Windows Credential Manager, never in plain files.
- Connections, settings, snippets and query history are stored locally in the app data folder
  (`%APPDATA%\com.sqlotus.app`). Nothing is sent anywhere: Sqlotus has no telemetry and talks only to the
  database servers you connect to.
- Fixes that change the computer (for example enabling TCP/IP for SQL Server) run only after Windows asks for
  administrator permission.
