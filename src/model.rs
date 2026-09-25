use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::{error::AppError, schema::sessions};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TicketClaims {
    pub iss: String,
    pub aud: String,
    pub sub: String,
    pub tenant: String,
    pub sid: String,
    pub jti: String,
    pub iat: i64,
    pub exp: i64,
    pub session_exp: i64,
}
impl TicketClaims {
    pub fn identity(&self) -> String {
        serde_json::json!([self.iss, self.tenant, self.sub, self.sid]).to_string()
    }
    pub fn session_key(&self) -> String {
        serde_json::json!([self.iss, self.tenant, self.sub, self.sid]).to_string()
    }
    pub fn validate(&self) -> Result<(), AppError> {
        let now = Utc::now().timestamp();
        if [&self.iss, &self.sub, &self.tenant, &self.sid, &self.jti]
            .iter()
            .any(|s| s.is_empty() || s.len() > 200)
            || self.session_exp <= now
            || self.exp <= now
            || self.iat > now + 5
            || self.iat < now - 120
            || self.exp - self.iat > 120
            || self.exp <= self.iat
        {
            return Err(AppError::Authentication);
        }
        Ok(())
    }
}
#[derive(
    Debug, Clone, Serialize, Deserialize, diesel::Queryable, diesel::Selectable, diesel::Insertable,
)]
#[diesel(table_name=sessions)]
pub struct Session {
    pub token_hash: String,
    pub identity: String,
    pub sid: String,
    pub issuer: String,
    pub tenant: String,
    pub subject: String,
    pub expires_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub tools: Vec<Value>,
    pub prompts: Vec<String>,
    pub models: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub calls: Vec<ToolCall>,
    #[serde(default)]
    pub call_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Turn {
    pub registration_id: String,
    pub request_id: String,
    pub model: String,
    pub messages: Vec<Message>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExchangeInput {
    pub ticket: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct ExchangeOutput {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}
impl std::fmt::Debug for ExchangeOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExchangeOutput")
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for ExchangeInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ExchangeInput([redacted])")
    }
}
