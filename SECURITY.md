# Security policy and deployment controls

## Scope

This repository contains the self-hosted Rust gateway; the Flutter SDK is maintained separately. The review covers code, protocol boundaries, database transactions, configuration and local tests. It is not an OWASP certification or an external penetration test. Host product backends, local business authorization, provider infrastructure and production ingress configuration are outside this repository's verification boundary.

The gateway should be reachable over public HTTPS **only behind a controlled ingress**. Public API documentation does not mean anonymous inference: session exchange requires a signed ticket and all business routes require a bearer credential.

## Reporting

Report vulnerabilities privately through [GitHub private vulnerability reporting](https://github.com/JhonaCodes/asystant-gateway/security/advisories/new). Do not post credentials, customer data or an exploit containing secrets in a public issue. Include the affected version, reproduction steps and expected impact. Version 0.2.x is the currently maintained line.

## Trust boundaries and controls

| OWASP API Security Top 10 2023 concern | Implemented control | Required operator or host action |
| --- | --- | --- |
| API1 / API5: object and function authorization | Registration ownership includes product, tenant, subject and login ID. Model policy is derived from authenticated identity and rechecked at inference. No public administration routes. | Derive ticket claims from a verified product login. Authorize every local tool in the host repository. |
| API2: authentication | Fixed HS256 algorithm and audience, per-product secret, tickets up to 120 seconds, single-use jti, opaque credentials up to ten minutes, stored hashes only, persistent login revocation. | Store independent random secrets in a secret manager. Revoke before logout where required; rotate product secrets with a planned reauthentication rollout. |
| API3: property authorization | Closed top-level request DTOs, bounded JSON inputs, typed SDK arguments, no frontend budget or tenant override. | Treat model text and tool results as untrusted input; never evaluate returned source code or HTML. |
| API4 / API6: resource consumption and sensitive flows | 256 KiB request limit; bounded schemas, transcripts and provider responses; daily tenant/user reservations; 32 concurrent inferences per process; 120-second inference timeout; bounded process-local peer admission. | Apply shared rate limits and request/header timeouts at ingress, account for all replicas, restrict origin-server access and monitor storage growth. |
| API7: SSRF | Fixed provider endpoints with redirects disabled; client requests cannot supply a provider URL. | Restrict server egress and filesystem access to the database volume. |
| API8: configuration | Explicit CORS allowlist, generic errors, no-store credential responses, no-sniff and restrictive browser headers, non-root container. | Terminate TLS, configure HSTS at HTTPS ingress, restrict access to the SQLite volume, keep secrets out of logs and images. |
| API9: inventory | Versioned `/v1` routes and an OpenAPI contract; documented health endpoints and operational limits. | Inventory deployed revisions and disable obsolete hosts/credentials. |
| API10: unsafe provider consumption | Bounded reads, deadlines, protocol validation, no automatic replay of uncertain writes and conservative pending reservations. | Confirm model availability, price ceilings and provider terms; reconcile uncertain usage against provider records. |

Reference: [OWASP API Security Top 10 2023](https://owasp.org/API-Security/editions/2023/en/0x11-t10/).

## Admission details

The process-local guard permits ten session exchange attempts per minute per TCP peer and 120 other `/v1` requests per minute per peer. The table is bounded to 10,000 entries and fails closed at capacity. It ignores `Forwarded` and `X-Forwarded-For`; callers cannot spoof an address to bypass it. Health and documentation routes are outside these counters.

Behind a reverse proxy, all traffic may share the proxy peer and therefore the same conservative limit. Configure trusted ingress limits and capacity accordingly; do not trust arbitrary forwarding headers. These counters are per process and reset on restart. They are not a distributed quota or a DDoS defense. Durable daily money limits remain in SQLite across restarts of the single supported replica.

A concurrency rejection occurs before budget reservation. A timeout, interrupted stream or uncertain accounting retains the reservation pending reconciliation. Slow or disconnected clients cannot hold the inference task beyond its deadline, but process termination can still leave pending records.

## Operational limitations

- No automatic usage reconciliation or retention worker is included. Monitor database growth, pending requests and budget denials; use a reviewed retention policy that preserves active revocations and accounting evidence.
- Prompt injection remains possible. Tool approval, input validation and host authorization are mandatory boundaries; prompts do not confer permissions.
- Do not render model-provided HTML or execute model-provided code. The bundled cards contain text and structured options.
- Run dependency security checks in CI and rebuild images for security updates. A successful package score does not establish backend security.
- Run one replica using a local persistent volume owned by UID 10001. Use SQLite online backups or stop the service before copying all database files; test restore procedures. Do not share WAL databases across hosts or network filesystems.
- The external provider smoke test requires a valid key. No production penetration test, load test or multi-replica ingress verification has been performed.

## Secrets

`.env`, `.env.*`, local state and build artifacts are ignored by Git. `.env.example` contains placeholders only. Provider and product signing secrets must be supplied by the deployment secret manager; neither Flutter packages nor Docker build arguments should contain them. Avoid logging bearer headers, JWT tickets, prompts or customer data.
