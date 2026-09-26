//! Browser origins allowed to call the public API. They are part of the
//! administration policy, so adding one takes effect without a restart.
use std::sync::{Arc, PoisonError, RwLock};

use actix_cors::Cors;
use reqwest::Url;

use crate::error::AppError;

/// Upper bound on configured origins; a longer list is a mistake, not a need.
pub const MAX_ORIGINS: usize = 64;

/// Turn user input into the exact form browsers send in the Origin header:
/// `HTTPS://App.Example.com/` becomes `https://app.example.com`. Anything with a
/// path, query, fragment or credentials is refused, and plain HTTP is only
/// accepted for local development hosts.
pub fn normalize(raw: &str) -> Result<String, AppError> {
    let raw = raw.trim();
    if raw.is_empty() || raw.len() > 200 {
        return Err(AppError::Invalid);
    }
    let url = Url::parse(raw).map_err(|_| AppError::Invalid)?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && local);
    if !scheme_ok
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(AppError::Invalid);
    }
    Ok(url.origin().ascii_serialization())
}

/// The origins the CORS layer checks on every request, shared between the
/// HTTP workers and replaced whenever administration saves the policy.
#[derive(Debug, Clone, Default)]
pub struct AllowedOrigins(Arc<RwLock<Vec<String>>>);

impl AllowedOrigins {
    pub fn allows(&self, origin: &str) -> bool {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .any(|allowed| allowed == origin)
    }
    pub fn replace(&self, origins: Vec<String>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = origins;
    }
}

/// The CORS layer of the public API, reading the live origin list.
pub fn cors(origins: AllowedOrigins) -> Cors {
    Cors::default()
        .allowed_origin_fn(move |origin, _| origin.to_str().is_ok_and(|o| origins.allows(o)))
        .allowed_methods(vec!["GET", "POST"])
        .allowed_headers(vec!["Authorization", "Content-Type"])
        .max_age(600)
}
