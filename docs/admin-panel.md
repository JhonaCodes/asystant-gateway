# Gateway administration

The gateway serves an embedded, responsive English panel at `/admin`. It requires
no separate container, Node runtime, or database service. The interface uses
ordinary server-rendered forms and contains no third-party scripts or fonts.

## Enable and sign in

Generate an independent administrator credential locally:

```sh
openssl rand -hex 32
```

Set the result as `ASYSTANT_ADMIN_TOKEN` on the **gateway** service in Dokploy,
then redeploy. Do not reuse an OpenRouter API key or a product ticket secret.
Open `https://ai.jhonacode.com/admin` and enter the token in the password field.
Without this variable the panel returns 404. A configured token shorter than
32 bytes or longer than 1024 bytes prevents startup.

HTTPS is required: administration uses `__Host-` cookies with `Secure`,
`HttpOnly`, and `SameSite=Strict`. Sessions expire after one hour and are
invalidated on restart. Every mutation requires a session-bound CSRF token;
login also requires a same-site CSRF cookie and hidden field. Five failed login
attempts globally lock sign-in for 15 minutes, including behind a reverse proxy.
Token comparison uses constant-time digests. Rotate the token and restart to
revoke all sessions. This first version uses a dedicated admin token, not TOTP.

## Model assignments

- **Product:** choose any model registered in `ASYSTANT_MODELS`. Choosing it
  enables that model for the product and sets it as the default.
- **Customer:** enter the exact tenant/company ID used in the product's login
  ticket. Choose one of the product's permitted models; this locks that customer
  to the selected model. Existing user-specific policies take precedence.
- **Restore customer defaults:** remove the tenant-level model and budget
  overrides. User-specific policies are retained.
- **Model limits:** change the maximum input upper bound and output token limit.
  The input estimator conservatively counts bytes, tool schemas and protocol
  overhead; it is not the provider's exact token count. Raising these limits also
  increases each turn's budget reservation.

Credentials, provider type, provider model ID, pricing ceilings and model
registry membership remain managed through environment configuration. The panel
never renders or persists API keys, ticket secrets or environment key names.

## Budgets

Budget fields use USD microdollars: **1,000,000 = $1**. Product policy controls
daily tenant and daily user budgets. Customer policy overrides the tenant daily
budget; the product's user budget and any more-specific user overrides remain
in effect. Changes do not erase prior spending or reservations.

## Allowed origins

**Allowed origins** lists the websites and apps whose browsers may call the
gateway's `/v1` API. Add as many as the products need (up to 64), for example
`https://app.turnosqr.com` and a second domain; remove one with its button.
Changes apply at once to new browser requests, without a restart.

Enter an exact origin: scheme and host, with a port only if it is not the
default. `HTTPS://App.Example.com/` is saved as `https://app.example.com`, the
form browsers send. Paths, queries, fragments and credentials are refused, and
plain `http` is only accepted for `localhost`, `127.0.0.1` and `[::1]`.

`ASYSTANT_ORIGINS` is only the initial list. Until the panel saves a policy the
gateway uses it; the first save stores that list together with the change, and
from then on the panel's list is the one that counts, also after a restart. Once
the origins are in the panel, the variable can be removed from Dokploy.

## Test a model

Each product has a **Run paid connection test** action. It sends a fixed,
tool-free greeting through ticket exchange, assistant registration, model
policy, the inference semaphore, budget reservation and settlement. It uses
stable tenant `__admin_playground__` and subject `admin-playground`, so repeated
tests share daily accounting. Tests consume provider tokens and are limited to
one start every ten seconds globally. Results are escaped before rendering.
The test verifies the product's default model, not an actual customer session
or application tools. A successful test does not grant application access.

## Persistence and live changes

Only nonsecret policy data is stored in the existing SQLite volume, in
`admin_policy`. Save operations read, merge, validate and write inside one
SQLite `BEGIN IMMEDIATE` transaction. Changes affect new registrations and
model listings, and inference rechecks policy before every new turn. Reconnect
an existing assistant after changing its assigned model. In-flight requests
complete using the policy snapshot they already reserved.

SQLite overrides take precedence over matching environment policy fields.
Keep the volume and backups when redeploying. If removing a model from the
registry, first move every affected assignment to another registered model.
Administration sessions and failed-login counters are intentionally memory-only;
run one replica with the SQLite volume and enforce ingress limits as well.
