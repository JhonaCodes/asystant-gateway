# Changelog

## Unreleased

- Manage the browser origins allowed to call `/v1` from the admin panel: add
  and remove several, applied without a restart and kept in the SQLite policy.
  Origins are normalized and must be exact (no path, query or credentials;
  plain http only for localhost). `ASYSTANT_ORIGINS` becomes the initial list
  until the panel saves a policy.

## 0.2.0

- Extract the gateway from JhonaCodes/asystant-ai into its own deployment repository.
- Replace PostgreSQL with bundled SQLite and a persistent local volume.
- Serialize accounting writes with immediate transactions, WAL and bounded lock waits.
- Apply embedded migrations by default at startup and keep manual migration modes.
- Add isolated SQLite tests, standalone CI and a container persistence/backup smoke test.
- Preserve the existing /v1 wire contract and Flutter-local tool execution.

This release does not import live PostgreSQL data automatically.
