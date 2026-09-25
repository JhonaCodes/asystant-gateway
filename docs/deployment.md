# Deploy on Dokploy with SQLite

Deploy one application from `JhonaCodes/asystant-gateway`, branch `main`.
Choose Dockerfile build, repository-root build context `.` and `Dockerfile`.
The internal HTTP port is **8787**. There is no separate database service.

## Persistent storage and startup

Before the first deployment, add a **named volume** mounted at `/data`, for
example `asystant_gateway_data`. Keep the same volume for every deployment.
The image prepares `/data` for UID/GID **10001:10001** and runs without root.
A bind mount must be pre-created with that ownership. The entire directory must
be writable: SQLite also creates WAL and shared-memory files next to the DB.
Do not mount just the database file, use temporary storage, or share the volume
with another replica. Use a local disk, not NFS/SMB.

Remove the old `DATABASE_URL` variable; startup rejects it to prevent an
accidental silent switch from PostgreSQL to an empty SQLite database. Limit
bind-mount directory permissions to the service owner (`0700`).

Set these runtime environment variables in Dokploy (not Docker build arguments):

- `DATABASE_PATH=/data/asystant.db`
- `ASYSTANT_BIND=0.0.0.0:8787`
- `OPENROUTER_API_KEY`: your provider key, stored as a secret.
- `ASYSTANT_ORIGINS`: comma-separated exact browser origins, e.g. `https://app.turnosqr.com`.
- `ASYSTANT_PRODUCTS`: JSON product issuer, signing secret, model policy and budgets.
- `ASYSTANT_MODELS`: JSON model/provider mappings and conservative price ceilings.

Use [.env.example](../.env.example) as a template. It defaults to TurnosQR with
`openai/gpt-oss-120b` as the sole allowed model. There is currently no admin panel;
model policy is configured through these server variables and a restart.
The example daily caps are USD 10 per tenant and USD 1 per user, not subscription
charges. Adjust them to your intended budget. `client_models` and `budget_overrides`
are optional advanced settings; do not copy fictitious customer IDs.

Generate the product signing secret with `openssl rand -hex 32` and paste that
same value into the product JSON `secret` and TurnosQR API
`ASYSTANT_TICKET_SECRET`. This secret is separate from the OpenRouter API key.
 In individual Dokploy value
fields enter raw JSON without shell quote characters. Placeholder secrets must
be replaced. Never commit actual secrets or SQLite files.

Leave the start command unchanged. The default process applies embedded
migrations before starting HTTP. For manual operations, `--migrate-only` applies
migrations and exits; `--serve` starts without migration and readiness fails if
the schema is missing. Configure exactly **one replica** and stop the previous
container before starting its replacement (no rolling overlap).

## Domain and product integration

Add your HTTPS domain in Dokploy, pointing to container port 8787. Configure the
proxy for SSE: no response buffering, upstream timeout above 120 seconds,
request/header timeouts and shared client admission limits. Restrict direct
access to the container port. Do not expose the volume through a web server.

In TurnosQR API configure `ASYSTANT_GATEWAY_URL` to this HTTPS origin,
`ASYSTANT_ISSUER` to the product issuer (e.g. `turnosqr`), and
`ASYSTANT_TICKET_SECRET` to the same independent secret used in the product JSON.
Tools continue to execute exclusively inside Flutter. The gateway handles
credentials, provider access, model policy and accounting.

Health endpoints:

- `/health/live`: the process is running.
- `/health/ready`: database and migrated schema are accessible.
- `/openapi.yaml`: public API specification without keys or tenant settings.

Health does not validate provider credentials. Test login, renewal, model policy,
revocation, budget exhaustion and an actual model response before enabling users.

## Backup, restore and failure handling

Use SQLite's online backup API, not a copy of the live `.db` file alone. The
container includes `sqlite3`; for example, make a consistent snapshot inside the
volume, then copy it to a separately protected backup destination:

```sh
docker exec CONTAINER sqlite3 /data/asystant.db '.backup /data/asystant-backup.db'
docker cp CONTAINER:/data/asystant-backup.db ./asystant-backup.db
```

Protect backups as sensitive data and remove temporary snapshots only after
confirming off-volume backup success. For restore, stop the application, preserve
the current volume for recovery, restore into a fresh volume owned by 10001:10001,
and start one instance. Never combine a restored DB with old `-wal`/`-shm` files.
Check readiness and session/accounting state before enabling traffic. Restoring
an old snapshot can roll back revocations, consumed tickets and budgets: reconcile
provider usage and invalidate affected sessions before serving traffic.

WAL plus `synchronous=FULL` preserves committed writes. `BEGIN IMMEDIATE` makes
reservation and settlement atomic; each connection waits up to five seconds for
a writer. Keep transactions short. Lock exhaustion fails the request; it never
bypasses accounting. Provider calls happen outside database transactions.

Do not refund pending reservations just because the process stopped. Compare
uncertain usage against provider records. Monitor disk space and DB growth; no
automatic retention or reconciliation worker is included. A volume on the same
machine is persistence, not a backup.

This release creates a fresh SQLite schema. It does **not** automatically migrate
PostgreSQL records. If an older gateway served real requests, plan a data migration
before cutover to preserve budgets, replay prevention and revoked sessions.

## Local container validation

```sh
cp .env.example .env
# Replace placeholders through your secret manager.
docker compose up -d --build
curl --fail http://127.0.0.1:8787/health/ready
```

For a credential-free smoke test after building the image:

```sh
docker build -t asystant-gateway:local .
python3 scripts/check_container.py
```

## Build locally and deploy a prebuilt image

Use this flow when Dokploy should only pull an image instead of compiling Rust.
From a clean `main` checkout synchronized with GitHub:

```sh
./scripts/deploy-local-image.sh
```

Docker Desktop/Engine must be running, Buildx must be available, and `gh` must
be authenticated with GHCR `write:packages` permission. The script securely pipes
the existing GitHub token to Docker login; it never embeds that token or runtime
secrets in the image. It checks the branch/worktree/remote revision, builds
`linux/amd64` locally, and pushes both `:prod` and a full-commit-SHA tag to
`ghcr.io/jhonacodes/asystant-gateway`. Use `TARGET_PLATFORM=linux/arm64` only if
the Dokploy host uses ARM64, or `TARGET_PLATFORM=linux/amd64,linux/arm64` for both.

In Dokploy change the application provider/source to **Docker image** and use:

```text
ghcr.io/jhonacodes/asystant-gateway:prod
```

Keep the existing environment, `/data` volume, HTTPS domain and port `8787`.
If GHCR requires authentication, configure registry `ghcr.io`, your GitHub
username and a registry credential with `read:packages` in Dokploy's registry
settings. Do not put registry credentials in application variables or source.
Public repository visibility does not automatically guarantee public package
visibility; the package owner can make the container public in GitHub settings.

Click Deploy after each successful publication. This script publishes the image
but does not trigger a Dokploy restart. Configure one replica and stop-first
replacement so two processes do not overlap on the same SQLite volume.
For rollback, use a previously published SHA tag after verifying schema
compatibility, retaining the same volume. Never delete the volume to roll back.
