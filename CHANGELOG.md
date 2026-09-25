# Changelog

## 0.2.0

- Extract the gateway from JhonaCodes/asystant-ai into its own deployment repository.
- Replace PostgreSQL with bundled SQLite and a persistent local volume.
- Serialize accounting writes with immediate transactions, WAL and bounded lock waits.
- Apply embedded migrations by default at startup and keep manual migration modes.
- Add isolated SQLite tests, standalone CI and a container persistence/backup smoke test.
- Preserve the existing /v1 wire contract and Flutter-local tool execution.

This release does not import live PostgreSQL data automatically.
