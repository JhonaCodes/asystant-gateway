# Asystant Gateway

A product-independent Rust/Actix API with SQLite/Diesel persistence. Use it directly or as a reference implementation for a compatible assistant backend. It manages provider access; **it never executes application tools**.

## Start

Supply the environment variables in [.env.example](.env.example). The binary reads process environment variables; it does not automatically load a dotenv file.

```sh
cargo run --bin asystant_api
```

Startup applies embedded migrations before listening. Run one replica with a persistent local volume; no PostgreSQL service is needed. `--migrate-only` and `--serve` remain available for manual operations. The default listener is `127.0.0.1:8787`; the container listens on `0.0.0.0:8787`. See [deployment](docs/deployment.md).

The [OpenAPI specification](openapi.yaml) describes all public endpoints, request/response schemas and SSE events. The service exposes the same contract at `/openapi.yaml`. A public endpoint still requires a valid signed ticket or bearer credential; no anonymous inference is provided.

## Authentication

Register each product with an issuer and an independent random secret of at least 32 bytes. After validating its existing login, the product backend issues a JWT using **HS256** with these claims:

```json
{
  "iss": "my-product",
  "aud": "asystant-api",
  "sub": "user-id",
  "tenant": "customer-id",
  "sid": "stable-login-id",
  "jti": "unique-ticket-id",
  "iat": 1790000000,
  "exp": 1790000060,
  "session_exp": 1790003600
}
```

Timestamps are illustrative Unix seconds; generate fresh values. Tickets last at most 120 seconds and can be consumed once. `session_exp` is the actual host session expiry. Derive all identity claims from the verified login, never from caller-selected tenant/user values. Keep the signing secret outside Flutter.

1. `POST /v1/sessions/exchange` with `{ "ticket": "..." }` returns an opaque credential and expiry.
2. The credential lasts at most ten minutes and cannot outlive the host session. Only its SHA-256 hash is stored.
3. The SDK requests a fresh ticket before expiry and exchanges it. Rotation preserves registration and budget identity.
4. `POST /v1/sessions/revoke` revokes all credentials for that login ID and rejects future tickets for it. A new login must use a new `sid`.

Each product must implement ticket issuance on its own backend. The reference API does not replace the product login or accept a provider key from the frontend.

## Tools and conversations

`POST /v1/assistants/init` registers tool schemas, system prompts and optional model preferences. The gateway always prepends its own English security baseline before application prompts, for every provider wire format. An identical SDK baseline is deduplicated. These instructions are guidance; server authorization and local tool validation remain mandatory. It returns an identity-bound registration valid for 24 hours plus the effective model policy. Tool implementations remain in the app.

`POST /v1/turns` accepts `{registration_id, request_id, model, messages}` and returns `text/event-stream`. Each `data:` payload is one JSON envelope: `text_delta`, `completed` or `failed`. A completed message can contain proposed calls; the SDK validates them, obtains permission when required, executes locally and returns each result in the next inference.

Request IDs are unique per identity/inference. Reuse returns 409; there is no cached-response replay. Messages only accept user, assistant and tool roles, with matched call/result pairs. Top-level unknown request fields are rejected. The SDK bounds each turn to eight rounds and sixteen calls per model response.

## Assign a model per customer

Configure product `models`, `default_model` and `client_models` in `ASYSTANT_PRODUCTS`:

```json
{
  "models": ["fast", "advanced"],
  "default_model": "fast",
  "client_models": [
    {"tenant":"customer-a","models":["fast"],"default_model":"fast","allow_selection":false},
    {"tenant":"customer-b","models":["fast","advanced"],"default_model":"advanced","allow_selection":true},
    {"tenant":"customer-b","subject":"limited-user","models":["fast"],"default_model":"fast","allow_selection":false}
  ]
}
```

This fragment supplements the required issuer, secret and budget fields. Each model alias must exist in `ASYSTANT_MODELS` and can target a different provider/model.

User rules override tenant rules, which override product defaults. `GET /v1/models` derives identity from the bearer credential and returns `{models, default_model, allow_selection}`. It never accepts a caller-selected tenant. Fixed assignments expose only the assigned model. Inference rechecks policy, so changing the HTTP payload cannot bypass a fixed assignment.

Initialization with `models: []` delegates the catalog to the server. The assigned default is always included even when the app requested a different model. Configuration changes require restart. If a model is removed, existing clients must reinitialize. No public policy-administration route is included.

## Accounting

Limits are integer USD micros: one USD equals 1,000,000. Configure `daily_tenant_micros`, `daily_user_micros` and optional `budget_overrides` by tenant and subject. Days use UTC. A zero override blocks new spending for that identity.

Before inference, a SQLite `BEGIN IMMEDIATE` transaction reserves a conservative maximum against tenant and user accounts. WAL, a five-second busy timeout and full synchronous commits protect concurrent updates. Credentials and logins share durable daily accounts across process restarts. Network calls run outside database transactions. Known costs settle once; a second settlement is rejected.

OpenRouter receives configured price ceilings and its reported cost is preferred when available. Direct-provider accounting uses configured conservative tariffs, without cache discounts. Maintain these tariffs as provider pricing changes. A known charge above the reserve is recorded in full and blocks future spending if it exhausts the limit.

Unknown usage, timeout or incomplete streaming leaves the reservation pending. Client disconnect does not automatically refund it. There is no reconciliation worker: compare pending records with provider evidence before closing them. Do not release funds based only on record age.

## Providers

| Provider | Wire protocol and current behavior |
| --- | --- |
| `openrouter` | Chat Completions streaming, tools, reported usage and price ceilings |
| `openai` | Chat Completions or Responses |
| `gemini` | Official OpenAI-compatible endpoint with streaming/tools |
| `anthropic` | Messages with tools; complete response rather than incremental text |
| `opencode_zen` | Chat Completions, Messages or Responses according to model |
| `opencode_go` | Corresponding wire adapter and session header; enable only for permitted use cases |

`wire_api` is `chat_completions` by default, or `messages` / `responses`. It must match the model endpoint. Messages/Responses currently return complete responses. OpenCode Go targets coding-agent traffic; verify suitability before enabling it for a general-purpose product assistant.

Official references: [OpenRouter](https://openrouter.ai/docs/api/api-reference/chat/send-chat-completion-request), [Gemini](https://ai.google.dev/gemini-api/docs/openai), [OpenCode Zen](https://opencode.ai/docs/zen/), [OpenCode Go](https://opencode.ai/docs/go/).

## Security and verification

See [SECURITY.md](SECURITY.md) for OWASP-mapped controls, process-local admission, trusted-ingress requirements and residual risks. Security posture is based on code and local tests; it is not a certification or production penetration test.

```sh
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
```

Database and HTTP tests create isolated temporary SQLite files automatically.
See [Dokploy deployment](docs/deployment.md) for volume mounts, configuration,
backups and rollout instructions. The Flutter SDK lives in
[JhonaCodes/asystant-ai](https://github.com/JhonaCodes/asystant-ai).

Version 0.2.0 starts a new SQLite database. It does not import an existing
PostgreSQL database; do not reset live budget or revocation history by swapping
storage engines without a separately reviewed data migration.

## Input admission limits

`max_input_tokens` is checked against a conservative byte-based upper bound,
not an exact tokenizer count. It includes the system prompts, serialized tool
schemas, conversation and protocol overhead (including 1,024 per registered
tool). Even a short user message can exceed a small configured limit.

The example GPT OSS 120B policy uses 65,536 input and 2,048 output tokens.
The TurnosQR 18-tool manifest with a short first message has a bound of about
37,500, exceeding the previous 32,768 example. Existing deployments must change
`ASYSTANT_MODELS` themselves and redeploy; updating the example does not alter
runtime settings. Keep input plus output within the provider model context.

An authenticated oversized turn returns HTTP 400 with `code` equal to
`input_limit_exceeded`, `estimated_input_upper_bound` and `max_input_tokens`.
It is rejected before provider inference or budget reservation. Raising this
limit also increases the maximum budget reservation; daily spending limits
remain enforced.

## Embedded administration panel

Enable `/admin` with a separate `ASYSTANT_ADMIN_TOKEN` (generate with
`openssl rand -hex 32`) and use HTTPS. The panel configures product/customer
model assignments, budgets and model input/output limits in the same SQLite
volume. Its paid connection test exercises the normal inference and accounting
pipeline without executing application tools. API keys and product signing
secrets remain environment-only. No additional service is required.

See [admin setup and protections](docs/admin-panel.md),
[desktop preview](docs/admin-panel-desktop.png) and
[mobile preview](docs/admin-panel-mobile.png). Previews use fixture data.
