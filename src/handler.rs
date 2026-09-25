use std::sync::Arc;
use actix_web::{HttpRequest, HttpResponse, web};
use serde_json::json;
use tokio_stream::wrappers::ReceiverStream;
use crate::{
    error::AppError,
    model::{ExchangeInput, Manifest, Session, Turn},
    repository::GatewayRepository,
    service::GatewayService,
};

async fn session(request: &HttpRequest, service: &GatewayService) -> Result<Session, AppError> {
    let token = request
        .headers()
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(AppError::Authentication)?;
    service.authenticate(token).await
}
pub async fn exchange(
    service: web::Data<Arc<GatewayService>>,
    input: web::Json<ExchangeInput>,
) -> Result<HttpResponse, AppError> {
    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .json(service.exchange(&input.ticket).await?))
}
pub async fn init(
    request: HttpRequest,
    service: web::Data<Arc<GatewayService>>,
    input: web::Json<Manifest>,
) -> Result<HttpResponse, AppError> {
    let session = session(&request, &service).await?;
    let policy = service
        .config
        .products
        .iter()
        .find(|p| p.issuer == session.issuer)
        .ok_or(AppError::Authentication)?
        .model_policy(&session.tenant, &session.subject)?;
    let (id, models) = service.register(session, input.into_inner()).await?;
    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .json(json!({"id":id,"models":models,"default_model":policy.default_model,"allow_selection":policy.allow_selection})))
}
pub async fn turn(
    request: HttpRequest,
    service: web::Data<Arc<GatewayService>>,
    input: web::Json<Turn>,
) -> Result<HttpResponse, AppError> {
    let session = session(&request, &service).await?;
    let receiver = Arc::clone(service.get_ref())
        .start_turn(session, input.into_inner())
        .await?;
    Ok(HttpResponse::Ok()
        .insert_header(("Content-Type", "text/event-stream"))
        .insert_header(("Cache-Control", "no-store"))
        .insert_header(("X-Accel-Buffering", "no"))
        .streaming(ReceiverStream::new(receiver)))
}
pub async fn revoke(
    request: HttpRequest,
    service: web::Data<Arc<GatewayService>>,
) -> Result<HttpResponse, AppError> {
    let session = session(&request, &service).await?;
    let pool = service.pool.clone();
    tokio::task::spawn_blocking(move || pool.revoke(&session))
        .await
        .map_err(|_| AppError::Internal)??;
    Ok(HttpResponse::NoContent().finish())
}
pub async fn models(
    request: HttpRequest,
    service: web::Data<Arc<GatewayService>>,
) -> Result<HttpResponse, AppError> {
    let session = session(&request, &service).await?;
    let policy = service
        .config
        .products
        .iter()
        .find(|p| p.issuer == session.issuer)
        .ok_or(AppError::Authentication)?
        .model_policy(&session.tenant, &session.subject)?;
    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .json(policy))
}
pub fn routes(config: &mut web::ServiceConfig) {
    config
        .route("/openapi.yaml", web::get().to(specification))
        .route("/health/live", web::get().to(live))
        .route("/health/ready", web::get().to(ready))
        .route("/v1/sessions/exchange", web::post().to(exchange))
        .route("/v1/sessions/revoke", web::post().to(revoke))
        .route("/v1/models", web::get().to(models))
        .route("/v1/assistants/init", web::post().to(init))
        .route("/v1/turns", web::post().to(turn));
}

pub async fn live() -> HttpResponse {
    HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .json(json!({"status":"alive"}))
}

pub async fn ready(service: web::Data<Arc<GatewayService>>) -> Result<HttpResponse, AppError> {
    service.check_ready().await?;
    Ok(HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .json(json!({"status":"ready"})))
}

/// Public protocol documentation; contains no deployment secrets or tenant policy.
pub async fn specification() -> HttpResponse {
    HttpResponse::Ok()
        .content_type("application/yaml")
        .body(include_str!("../openapi.yaml"))
}
