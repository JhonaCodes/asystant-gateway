# Embedded administration verification

- 20 Rust tests passed, including login/CSRF, policy persistence, input limits,
  existing-session policy enforcement, test-model accounting and SQLite storage.
- Clippy (all targets, warnings denied), formatting and cargo audit passed.
- Browser fixture: signed in with a separate test credential, changed the
  product model and confirmed persistence after reload; assigned a customer
  model/budget through the form; mobile viewport 390 had no horizontal overflow.
- HTML CSP permits same-origin forms and styles but blocks injected fetch scripts.
- No provider keys, signing secrets or environment key names appear in the panel.
- Real provider invocation was not performed; the provider integration test uses
  a deterministic local implementation. The deployed paid-test button lets the
  administrator verify their configured provider after rollout.

Release status: ready for deployment after setting an independent admin token.
Existing installations using a 32,768 input limit must update it to 65,536 in
ASYSTANT_MODELS or through the model-limits form; publishing an image does not
rewrite environment variables or saved SQLite policy.
