# Contributing to Sqlotus

Thanks for taking the time to help! Bug reports, ideas and pull requests are all welcome.

## Reporting a bug

Open an issue with:

- what you did, what you expected and what happened instead;
- the database (SQL Server / PostgreSQL / MySQL and version) and the Sqlotus version;
- the SQL text, if the bug is about completion, formatting or analysis (remove anything private).

Security problems: please follow [SECURITY.md](SECURITY.md) instead of opening a public issue.

## Development setup

Requirements: Windows 10/11, Rust (MSVC toolchain), Node 22+, Visual Studio Build Tools (C++ workload).

```bash
npm install
npx tauri dev
```

`npx tauri build` produces the installers in `target/release/bundle`.

## Project layout

- `crates/sqlotus-core` — the SQL engine: tokenizer, formatter, completion, analysis, refactorings, plans,
  schema compare. Pure Rust, no IO — most features and almost all tests live here.
- `crates/sqlotus-db` — connections, query execution and metadata loading for each server.
- `crates/sqlotus-store` — local SQLite storage (connections, settings, snippets, history).
- `src-tauri` — the desktop app's commands.
- `src` — the React + Monaco user interface.
- `scripts` — end-to-end UI tests, Docker test databases and the third-party license generator.
- `docs` — README translations and screenshots; `licenses` — third-party licenses shipped with the app.

## Before you open a pull request

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm test
npx tsc --noEmit
```

CI runs the same checks on every pull request.

- Add or update tests for what you change. Logic in `sqlotus-core` should come with unit tests; anything that
  talks to a server can have a live test in `crates/sqlotus-db/tests` (see below).
- Keep changes focused: one fix or feature per pull request.
- Anything that changes data or schema must stay explicit: Sqlotus shows scripts for review and asks before
  committing large changes — new features should follow the same rule.

### Testing against real databases

`scripts/test-db/README.md` starts SQL Server, PostgreSQL and MySQL in Docker with a sample schema and shows how
to run the live tests and the UI scripts against them. Live tests are skipped when the `SQLOTUS_TEST_*`
environment variables are not set, so `cargo test` works without Docker.

### Dependencies

When you add or update a dependency, regenerate `licenses/THIRD_PARTY_LICENSES.md` (it ships with the installer):

```bash
cargo install cargo-about --locked --features cli
node scripts/third-party-licenses.mjs
```

New dependencies need a permissive license accepted in `licenses/about.toml`.

## Code of conduct

This project follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## License

By contributing you agree that your contributions are licensed under the [MIT License](../LICENSE).
