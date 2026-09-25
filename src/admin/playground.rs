use std::sync::Arc;
use chrono::Utc;
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::Value;
use uuid::Uuid;
use crate::{
    error::AppError,
    model::{Manifest, Message, TicketClaims, Turn},
    service::GatewayService,
};

/// Exercise the real policy and accounting pipeline with a fixed, tool-free prompt.
/// A stable synthetic tenant keeps all admin tests under the product's daily budgets.
pub async fn run(service: Arc<GatewayService>, issuer: &str) -> Result<String, AppError> {
    let config = service.effective_config().await?;
    let product = config
        .products
        .iter()
        .find(|p| p.issuer == issuer)
        .ok_or(AppError::Invalid)?;
    let now = Utc::now().timestamp();
    let claims = TicketClaims {
        iss: issuer.into(),
        aud: "asystant-gateway".into(),
        sub: "admin-playground".into(),
        tenant: "__admin_playground__".into(),
        sid: "admin-playground".into(),
        jti: Uuid::new_v4().to_string(),
        iat: now,
        exp: now + 60,
        session_exp: now + 600,
    };
    let ticket = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(product.secret.as_bytes()),
    )
    .map_err(|_| AppError::Internal)?;
    let exchanged = service.exchange(&ticket).await?;
    let session = service.authenticate(&exchanged.token).await?;
    let (registration_id, _, policy) = service
        .register_with_policy(
            session.clone(),
            Manifest {
                tools: vec![],
                prompts: vec!["Reply briefly and do not use tools.".into()],
                models: vec![],
            },
        )
        .await?;
    let turn = Turn {
        registration_id,
        request_id: Uuid::new_v4().to_string(),
        model: policy.default_model,
        messages: vec![Message {
            role: "user".into(),
            content: "Reply with exactly: Connection successful.".into(),
            calls: vec![],
            call_id: String::new(),
        }],
    };
    let mut receiver = service.start_turn(session, turn).await?;
    while let Some(event) = receiver.recv().await {
        let event = event.map_err(|_| AppError::Provider)?;
        let text = std::str::from_utf8(&event).map_err(|_| AppError::Provider)?;
        for line in text.lines().filter_map(|line| line.strip_prefix("data: ")) {
            let event: Value = serde_json::from_str(line).map_err(|_| AppError::Provider)?;
            if event["type"] == "failed" {
                return Err(AppError::Provider);
            }
            if event["type"] == "completed" {
                return Ok(event["message"]["content"]
                    .as_str()
                    .unwrap_or("Completed without text.")
                    .chars()
                    .take(8192)
                    .collect());
            }
        }
    }
    Err(AppError::Provider)
}
