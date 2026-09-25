use std::{collections::HashSet, sync::Arc};
use chrono::{Duration, Utc};
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
use serde_json::json;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use crate::{
    config::Config,
    error::AppError,
    model::{ExchangeOutput, Manifest, Session, TicketClaims, Turn},
    provider::{InferenceProvider, ProviderClient},
    repository::{GatewayRepository, PoolConfig},
};

pub struct GatewayService {
    pub inference_slots: Arc<tokio::sync::Semaphore>,
    pub config: Config,
    pub pool: PoolConfig,
    pub provider: Arc<dyn InferenceProvider>,
}
impl GatewayService {
    pub async fn check_ready(&self) -> Result<(), AppError> {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || pool.check_ready())
            .await
            .map_err(|_| AppError::Internal)?
    }
    pub fn hash(value: &str) -> String {
        format!("{:x}", Sha256::digest(value.as_bytes()))
    }
    pub async fn exchange(&self, ticket: &str) -> Result<ExchangeOutput, AppError> {
        if ticket.len() > 8192 {
            return Err(AppError::Authentication);
        }
        // Try only configured issuer keys with fixed algorithm and audience; no untrusted key lookup.
        let claims = self
            .config
            .products
            .iter()
            .find_map(|product| {
                let mut validation = Validation::new(Algorithm::HS256);
                validation.leeway = 0;
                validation.set_audience(&["asystant-gateway"]);
                validation.set_issuer(&[&product.issuer]);
                decode::<TicketClaims>(
                    ticket,
                    &DecodingKey::from_secret(product.secret.as_bytes()),
                    &validation,
                )
                .ok()
                .map(|t| t.claims)
            })
            .ok_or(AppError::Authentication)?;
        claims.validate()?;
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let expires_at = (Utc::now() + Duration::minutes(10)).min(
            chrono::DateTime::from_timestamp(claims.session_exp, 0)
                .ok_or(AppError::Authentication)?,
        );
        let session = Session {
            token_hash: Self::hash(&token),
            identity: claims.identity(),
            sid: claims.session_key(),
            issuer: claims.iss.clone(),
            tenant: claims.tenant.clone(),
            subject: claims.sub.clone(),
            expires_at,
        };
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || pool.exchange(&claims, &session))
            .await
            .map_err(|_| AppError::Internal)??;
        Ok(ExchangeOutput { token, expires_at })
    }
    pub async fn authenticate(&self, token: &str) -> Result<Session, AppError> {
        if token.len() != 64 {
            return Err(AppError::Authentication);
        }
        let hash = Self::hash(token);
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || pool.authenticate(&hash))
            .await
            .map_err(|_| AppError::Internal)?
    }
    pub async fn register(
        &self,
        session: Session,
        mut manifest: Manifest,
    ) -> Result<(String, Vec<String>), AppError> {
        Self::validate_manifest(&manifest)?;
        let product = self
            .config
            .products
            .iter()
            .find(|p| p.issuer == session.issuer)
            .ok_or(AppError::Authentication)?;
        let policy = product.model_policy(&session.tenant, &session.subject)?;
        // Server policy is authoritative; init model IDs are optional UI preferences.
        // Always include the assigned default, even if the old app requested another model.
        let requested = manifest.models;
        manifest.models = vec![policy.default_model.clone()];
        if policy.allow_selection {
            for model in policy.models {
                if (requested.is_empty() || requested.contains(&model))
                    && !manifest.models.contains(&model)
                {
                    manifest.models.push(model)
                }
            }
        }
        let id = Uuid::new_v4().to_string();
        let output = (id.clone(), manifest.models.clone());
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || pool.register(&id, &session, &manifest))
            .await
            .map_err(|_| AppError::Internal)??;
        Ok(output)
    }
    pub fn validate_manifest(manifest: &Manifest) -> Result<(), AppError> {
        if manifest.tools.len() > 64
            || manifest.prompts.len() > 16
            || manifest.models.len() > 32
            || serde_json::to_vec(manifest)
                .map_err(|_| AppError::Invalid)?
                .len()
                > 64000
        {
            return Err(AppError::Invalid);
        }
        let mut names = HashSet::new();
        for tool in &manifest.tools {
            let name = tool["name"].as_str().ok_or(AppError::Invalid)?;
            if name.is_empty()
                || name.len() > 64
                || !name.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
                || !names.insert(name)
                || tool["description"].as_str().is_none()
                || tool["parameters"]["type"] != "object"
            {
                return Err(AppError::Invalid);
            }
        }
        Ok(())
    }
    pub fn validate_turn(turn: &Turn) -> Result<(), AppError> {
        if turn.request_id.is_empty()
            || turn.request_id.len() > 100
            || turn.messages.is_empty()
            || turn.messages.len() > 256
        {
            return Err(AppError::Invalid);
        }
        let mut pending = HashSet::new();
        for m in &turn.messages {
            match m.role.as_str() {
                "user" => {
                    if !pending.is_empty() || !m.calls.is_empty() {
                        return Err(AppError::Invalid);
                    }
                }
                "assistant" => {
                    if !pending.is_empty() {
                        return Err(AppError::Invalid);
                    }
                    for c in &m.calls {
                        if c.id.is_empty()
                            || !c.arguments.is_object()
                            || !pending.insert(c.id.clone())
                        {
                            return Err(AppError::Invalid);
                        }
                    }
                }
                "tool" => {
                    if !m.calls.is_empty() || !pending.remove(&m.call_id) {
                        return Err(AppError::Invalid);
                    }
                }
                _ => return Err(AppError::Invalid),
            }
        }
        if !pending.is_empty() {
            return Err(AppError::Invalid);
        }
        Ok(())
    }
    pub async fn start_turn(
        self: Arc<Self>,
        session: Session,
        turn: Turn,
    ) -> Result<
        tokio::sync::mpsc::Receiver<Result<actix_web::web::Bytes, actix_web::Error>>,
        AppError,
    > {
        Self::validate_turn(&turn)?;
        let permit = Arc::clone(&self.inference_slots)
            .try_acquire_owned()
            .map_err(|_| AppError::Limited)?;
        let pool = self.pool.clone();
        let registration = turn.registration_id.clone();
        let identity = session.identity.clone();
        let manifest = tokio::task::spawn_blocking(move || pool.manifest(&registration, &identity))
            .await
            .map_err(|_| AppError::Internal)??;
        if !manifest.models.contains(&turn.model) {
            return Err(AppError::Invalid);
        }
        let model = self
            .config
            .models
            .iter()
            .find(|m| m.id == turn.model)
            .ok_or(AppError::Invalid)?
            .clone();
        let product = self
            .config
            .products
            .iter()
            .find(|p| p.issuer == session.issuer)
            .ok_or(AppError::Authentication)?;
        if !product
            .model_policy(&session.tenant, &session.subject)?
            .permits(&model.id)
        {
            return Err(AppError::Invalid);
        }
        // Byte-based upper bound plus protocol overhead; text-only SDK, no hidden files/images.
        let input = serde_json::to_vec(&ProviderClient::openai_messages(&manifest, &turn.messages))
            .map_err(|_| AppError::Invalid)?
            .len()
            + serde_json::to_vec(&manifest.tools)
                .map_err(|_| AppError::Invalid)?
                .len()
            + turn.messages.len() * 128
            + manifest.tools.len() * 1024
            + 4096;
        if input > model.max_input_tokens as usize {
            return Err(AppError::Invalid);
        }
        let reservation = model.reservation()?;
        let id = Self::hash(&json!([session.identity, turn.request_id]).to_string());
        let pool = self.pool.clone();
        let reserve_id = id.clone();
        let reserve_session = session.clone();
        let (tenant_limit, user_limit) = product.limits(&session.tenant, &session.subject);
        tokio::task::spawn_blocking(move || {
            pool.reserve(
                &reserve_id,
                &reserve_session,
                reservation,
                tenant_limit,
                user_limit,
            )
        })
        .await
        .map_err(|_| AppError::Internal)??;
        let (sender, receiver) = tokio::sync::mpsc::channel(128);
        // Continue accounting after client disconnect; pending reservations survive process failure.
        actix_web::rt::spawn(async move {
            let inference = tokio::time::timeout(
                std::time::Duration::from_secs(120),
                self.provider.complete(
                    &model,
                    &manifest,
                    &turn.messages,
                    &Self::hash(&session.identity),
                    &sender,
                ),
            )
            .await;
            let event = match inference {
                Ok(Ok((message, charge))) => {
                    let pool = self.pool.clone();
                    match tokio::task::spawn_blocking(move || pool.settle(&id, charge)).await {
                        Ok(Ok(())) => json!({"type":"completed","message":message}),
                        _ => json!({"type":"failed","failure":{"code":"unavailable","detail":""}}),
                    }
                }
                _ => json!({"type":"failed","failure":{"code":"unavailable","detail":""}}),
            };
            let _ = sender.try_send(Ok(actix_web::web::Bytes::from(format!(
                "data: {event}\n\n"
            ))));
            drop(permit);
        });
        Ok(receiver)
    }
}
