# SQLite extraction verification — 0.2.0

## Context status

GO. Extracted from `JhonaCodes/asystant-ai` main into a standalone Rust repository.
The /v1 HTTP and Flutter-local tool execution contracts are preserved. SQLite
replaces PostgreSQL; no database service is required in Dokploy.

## Business-rule status

GO. Immediate transactions serialize reservations before reading balances.
Tenant and user accounting roll back together. Duplicate ticket/request IDs,
exactly-once settlement, login revocation, model policy and durable pending
reservations retain their existing behavior. Provider calls stay outside SQL
transactions and SQL work stays off the async HTTP executor.

## Code-quality findings

No blocking findings in the migration review. Each pooled connection applies a
bounded busy timeout, foreign-key enforcement and synchronous FULL commits.
WAL is configured before pooling. In-memory and remote URL configurations are
rejected. Legacy DATABASE_URL is rejected at startup rather than silently
ignoring an existing PostgreSQL configuration. Runtime is non-root and the image
initializes a private writable volume directory.

## Test verification

- `cargo test --locked`: 16 tests passed, including four SQLite-specific contracts.
- Concurrent independent pools accept exactly the permitted number of reservations.
- Failed user-limit reservations leave neither tenant charges nor request IDs.
- Concurrent settlement accepts one writer and rejects the duplicate.
- Reopening storage preserves pending budgets; WAL permits reads during a writer.
- Existing HTTP, session, replay, model policy and prompt tests pass with isolated files.
- `cargo fmt --check` and `cargo clippy --locked --all-targets -- -D warnings`: pass.
- `cargo audit`: no reported vulnerabilities.
- OpenAPI specification validates with openapi-spec-validator 0.7.2.
- Docker build passes. Container smoke test verifies UID 10001, read-only root,
  unmigrated readiness, default migration, WAL, data after container replacement,
  online backup and database integrity. All resources are disposable; no provider calls.
- Tracked-file scan found no matching provider/private-key patterns or database files.

## Confidence report and decision

GO for a single-instance deployment with the documented persistent volume.
Confidence is high for tested storage, accounting and protocol behavior. This is
an internal source/test review, not a security certification or production load test.

## Residual risks

No automatic PostgreSQL data import is provided. Do not discard existing live
accounting or revoked-session history. SQLite requires local disk and one deployed
replica; no shared network filesystem or rolling replica overlap is supported.
Configure ingress, runtime secrets, backups and provider tariffs before deployment.
No live provider credentials or production environment were tested. Pending usage
still requires operator reconciliation; no retention/reconciliation worker exists.
