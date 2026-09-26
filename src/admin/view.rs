use crate::config::Config;

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
fn page(content: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>Asystant · Administration</title><link rel=\"stylesheet\" href=\"/admin/style.css\"></head><body><main><header><span class=\"brand\">asystant<span class=\"accent\">·</span>ai</span><span class=\"badge\">Gateway administration</span></header>{content}</main></body></html>"
    )
}
fn hidden(name: &str, value: &str) -> String {
    format!(
        "<input type=\"hidden\" name=\"{name}\" value=\"{}\">",
        escape(value)
    )
}
fn number(name: &str, label: &str, value: impl std::fmt::Display, min: u32, max: u64) -> String {
    format!(
        "<label>{label}<input required type=\"number\" name=\"{name}\" value=\"{value}\" min=\"{min}\" max=\"{max}\" step=\"1\"></label>"
    )
}
fn models(ids: &[String], selected: &str) -> String {
    let options = ids
        .iter()
        .map(|id| {
            format!(
                "<option value=\"{}\" {}>{}</option>",
                escape(id),
                if id == selected { "selected" } else { "" },
                escape(id)
            )
        })
        .collect::<String>();
    format!("<label>Assigned model<select name=\"model\">{options}</select></label>")
}
pub fn login(csrf: &str) -> String {
    page(&format!(
        "<section class=\"intro\"><p class=\"eyebrow\">CONTROL CENTER</p><h1>Welcome back.</h1><p>Manage model assignments and spending policies for your embedded assistants.</p></section><section class=\"card login\"><h2>Administrator sign in</h2><p>Use the dedicated ASYSTANT_ADMIN_TOKEN configured on this gateway.</p><form method=\"post\" action=\"/admin/login\">{}<label>Admin token<input type=\"password\" name=\"token\" required minlength=\"32\" maxlength=\"1024\" autocomplete=\"current-password\"></label><button>Sign in securely</button></form><small>Your session expires after one hour.</small></section>",
        hidden("csrf", csrf)
    ))
}
pub fn dashboard(config: &Config, csrf: &str) -> String {
    let mut content = format!(
        "<section class=\"intro\"><div><p class=\"eyebrow\">POLICY WORKSPACE</p><h1>Your assistants, configured.</h1><p>Choose a model per product or customer. Credentials stay on the server.</p></div><form method=\"post\" action=\"/admin/logout\">{}<button class=\"secondary\">Sign out</button></form></section><aside>Changes apply to new assistant registrations and are rechecked before every inference. Reopen or reconnect the assistant after changing its assigned model. Daily budgets are USD microdollars: 1,000,000 = $1.</aside><h2>Products &amp; customers</h2><div class=\"grid\">",
        hidden("csrf", csrf)
    );
    for p in &config.products {
        let default = p
            .default_model
            .as_deref()
            .or_else(|| p.models.first().map(String::as_str))
            .unwrap_or("");
        let common = format!("{}{}", hidden("csrf", csrf), hidden("issuer", &p.issuer));
        let available_models = config
            .models
            .iter()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>();
        content.push_str(&format!(
            r#"
<section class="card">
  <p class="eyebrow">PRODUCT</p>
  <h3>{issuer}</h3>
  <form method="post" action="/admin/policy">
    {common}{product_kind}{product_models}
    <div class="fields">{tenant_budget}{user_budget}</div>
    <button>Save product policy</button>
  </form>
  <details>
    <summary>Assign a customer model &amp; budget</summary>
    <p>Enter the exact company / tenant ID sent by the product login.</p>
    <form method="post" action="/admin/policy">
      {common}{client_kind}
      <label>Tenant ID<input name="tenant" required maxlength="200" placeholder="Company UUID"></label>
      {client_models}{client_budget}
      <button>Save customer assignment</button>
    </form>
  </details>
"#,
            issuer = escape(&p.issuer),
            product_kind = hidden("kind", "product"),
            product_models = models(&available_models, default),
            tenant_budget = number("daily_tenant_micros", "Daily tenant budget (µUSD)", p.daily_tenant_micros, 1, 1000000000000),
            user_budget = number("daily_user_micros", "Daily user budget (µUSD)", p.daily_user_micros, 1, 1000000000000),
            client_kind = hidden("kind", "client"),
            client_models = models(&p.models, default),
            client_budget = number("daily_tenant_micros", "Daily tenant budget (µUSD)", p.daily_tenant_micros, 0, 1000000000000),
        ));
        for client in &p.client_models {
            content.push_str(&format!(
                "<div class=\"assignment\"><strong>{}</strong><span>{}</span><small>{}</small></div>",
                escape(&client.tenant),
                escape(&client.default_model),
                if client.subject.is_some() { "User-specific assignment" } else { "Customer assignment" },
            ));
        }
        content.push_str(&format!(r#"
<details>
<summary>Restore customer defaults</summary>
<form method="post" action="/admin/policy">{}{}<label>Tenant ID<input name="tenant" required maxlength="200">
</label>
<button class="secondary">Remove customer override</button>
</form>
</details>
"#,common,hidden("kind","inherit")));
        content.push_str(&format!(r#"
<details>
<summary>Test the assigned default model</summary>
<p>Sends a fixed greeting through the real provider and budget pipeline. Uses the __admin_playground__ tenant and consumes tokens.</p>
<form method="post" action="/admin/test">{}<button>Run paid connection test</button>
</form>
</details>
"#,common));
        content.push_str("</section>");
    }
    content.push_str("</div><h2>Model limits</h2><aside>Input limits use a conservative byte-based upper bound, including tool schemas and protocol overhead; this is not the provider's exact token count. Raising limits also increases the reserved budget for every turn.</aside><div class=\"grid\">");
    for m in &config.models {
        content.push_str(&format!(
            r#"
<section class="card">
<p class="eyebrow">{:?}</p>
<h3>{}</h3>
<form method="post" action="/admin/policy">{}{}{}<div class="fields">{}{}</div>
<button>Save model limits</button>
</form>
</section>
"#,
            m.provider,
            escape(&m.id),
            hidden("csrf", csrf),
            hidden("kind", "model"),
            hidden("model", &m.id),
            number(
                "max_input_tokens",
                "Maximum input upper bound",
                m.max_input_tokens,
                1,
                1000000
            ),
            number(
                "max_output_tokens",
                "Maximum output tokens",
                m.max_output_tokens,
                1,
                32768
            )
        ));
    }
    content.push_str("</div><h2>Allowed origins</h2><aside>Websites and apps whose browsers may call this gateway, written as scheme and host, for example https://app.turnosqr.com. Changes apply at once. Plain http is only accepted for localhost.</aside><section class=\"card\">");
    if config.origins.is_empty() {
        content.push_str("<p>No origins yet: browsers cannot call the gateway.</p>");
    }
    for origin in &config.origins {
        content.push_str(&format!(
            "<form class=\"assignment\" method=\"post\" action=\"/admin/policy\">{}{}{}<strong>{}</strong><button class=\"secondary\">Remove</button></form>",
            hidden("csrf", csrf),
            hidden("kind", "origin_remove"),
            hidden("origin", origin),
            escape(origin),
        ));
    }
    content.push_str(&format!(
        "<form method=\"post\" action=\"/admin/policy\">{}{}<label>New origin<input name=\"origin\" required maxlength=\"200\" placeholder=\"https://app.example.com\"></label><button>Add origin</button></form></section>",
        hidden("csrf", csrf),
        hidden("kind", "origin_add"),
    ));
    content.push_str("<footer>SQLite stores operational policies. API keys and signing secrets remain in environment variables.</footer>");
    page(&content)
}

pub fn test_result(message: &str) -> String {
    page(&format!(
        "<section class=\"intro\"><h1>Connection test</h1></section><section class=\"card\"><pre>{}</pre><a href=\"/admin\">Back to administration</a></section>",
        escape(message)
    ))
}
