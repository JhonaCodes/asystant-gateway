use std::sync::Arc;
use actix_web::{App, http::StatusCode, test, web};
use chrono::Utc;
use jsonwebtoken::{EncodingKey, Header, encode};
use serde_json::{Value, json};
use tokio::sync::mpsc::Sender;
use uuid::Uuid;
use asystant_gateway::{
    config::{Config, ModelConfig, Product, Provider},
    error::AppError,
    handler,
    model::{Manifest, Message, TicketClaims},
    provider::InferenceProvider,
    repository::PoolConfig,
    service::GatewayService,
};

struct LocalProvider;
#[async_trait::async_trait(?Send)]
impl InferenceProvider for LocalProvider {
    async fn complete(
        &self,
        _: &ModelConfig,
        manifest: &Manifest,
        messages: &[Message],
        _: &str,
        _: &Sender<Result<web::Bytes, actix_web::Error>>,
    ) -> Result<(Message, i64), AppError> {
        assert_eq!(manifest.tools.len(), 1);
        assert_eq!(messages[0].role, "user");
        Ok((
            Message {
                role: "assistant".into(),
                content: "Verified through gateway".into(),
                calls: vec![],
                call_id: String::new(),
            },
            5,
        ))
    }
}
#[actix_rt::test]
async fn http_exchange_init_infer_replay_and_revoke() {
    let directory = tempfile::tempdir().unwrap();
    let pool = PoolConfig::new(directory.path().join("gateway.db").to_str().unwrap()).unwrap();
    let issuer = Uuid::new_v4().to_string();
    let secret = "test-only-signing-secret-more-than-32-bytes";
    let config = Config {
        products: vec![Product {
            issuer: issuer.clone(),
            secret: secret.into(),
            daily_tenant_micros: 100000,
            daily_user_micros: 100000,
            models: vec!["test".into()],
            budget_overrides: vec![],
            default_model: None,
            client_models: vec![],
        }],
        models: vec![ModelConfig {
            id: "test".into(),
            provider: Provider::Openrouter,
            model: "test".into(),
            key_env: "UNUSED".into(),
            wire_api: Default::default(),
            input_micros_per_million: 1_000_000,
            output_micros_per_million: 1_000_000,
            max_input_tokens: 10000,
            max_output_tokens: 100,
        }],
        origins: vec![],
    };
    let service = Arc::new(GatewayService {
        inference_slots: Arc::new(tokio::sync::Semaphore::new(32)),
        config,
        pool,
        provider: Arc::new(LocalProvider),
        origins: asystant_gateway::origins::AllowedOrigins::default(),
    });
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(Arc::clone(&service)))
            .configure(handler::routes),
    )
    .await;
    for path in ["/health/live", "/health/ready"] {
        let response =
            test::call_service(&app, test::TestRequest::get().uri(path).to_request()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get("Cache-Control").unwrap(), "no-store");
    }
    let specification = test::call_service(
        &app,
        test::TestRequest::get().uri("/openapi.yaml").to_request(),
    )
    .await;
    assert_eq!(specification.status(), StatusCode::OK);
    let contract: Value = serde_json::from_slice(&test::read_body(specification).await).unwrap();
    assert_eq!(contract["openapi"], "3.1.0");
    assert!(contract["paths"].get("/v1/turns").is_some());
    assert!(
        serde_json::from_value::<asystant_gateway::model::ExchangeInput>(
            json!({"ticket":"example", "tenant":"forged"})
        )
        .is_err()
    );
    let unauthorized = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/assistants/init")
            .set_json(json!({"tools":[],"prompts":[],"models":["test"]}))
            .to_request(),
    )
    .await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let now = Utc::now().timestamp();
    let ticket = encode(
        &Header::default(),
        &TicketClaims {
            iss: issuer,
            aud: "asystant-gateway".into(),
            sub: "user".into(),
            tenant: "tenant".into(),
            sid: "session".into(),
            jti: Uuid::new_v4().to_string(),
            iat: now,
            exp: now + 60,
            session_exp: now + 120,
        },
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/sessions/exchange")
            .set_json(json!({"ticket":ticket}))
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers().get("Cache-Control").unwrap(), "no-store");
    let access: Value = test::read_body_json(response).await;
    let bearer = format!("Bearer {}", access["token"].as_str().unwrap());
    let models = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/v1/models")
            .insert_header(("Authorization", bearer.as_str()))
            .to_request(),
    )
    .await;
    assert_eq!(models.status(), StatusCode::OK);
    let policy: Value = test::read_body_json(models).await;
    assert_eq!(policy["default_model"], "test");

    let manifest = json!({"tools":[{"name":"local_write","description":"Write a draft","parameters":{"type":"object","properties":{}}}],"prompts":["Assist"],"models":["test"]});
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/assistants/init")
            .insert_header(("Authorization", bearer.as_str()))
            .set_json(manifest)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let registration: Value = test::read_body_json(response).await;
    assert_eq!(registration["default_model"], "test");
    assert_eq!(registration["allow_selection"], true);
    let turn = json!({"registration_id":registration["id"],"request_id":"turn-1","model":"test","messages":[{"role":"user","content":"Hello"}]});
    // Oversized turns report safe diagnostics without consuming the request ID.
    let mut oversized = turn.clone();
    oversized["messages"][0]["content"] = json!("x".repeat(10_000));
    let rejected = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/turns")
            .insert_header(("Authorization", bearer.as_str()))
            .set_json(oversized)
            .to_request(),
    )
    .await;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let reason: Value = test::read_body_json(rejected).await;
    assert_eq!(reason["code"], "input_limit_exceeded");
    assert_eq!(reason["max_input_tokens"], 10_000);
    assert!(reason["estimated_input_upper_bound"].as_u64().unwrap() > 10_000);
    let held = service
        .inference_slots
        .clone()
        .acquire_many_owned(32)
        .await
        .unwrap();
    let limited = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/turns")
            .insert_header(("Authorization", bearer.as_str()))
            .set_json(&turn)
            .to_request(),
    )
    .await;
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    drop(held);
    // The same request ID must still succeed: capacity rejection reserved no funds.
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/turns")
            .insert_header(("Authorization", bearer.as_str()))
            .set_json(&turn)
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::OK);
    let body = test::read_body(response).await;
    assert!(
        std::str::from_utf8(&body)
            .unwrap()
            .contains("Verified through gateway")
    );
    let duplicate = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/turns")
            .insert_header(("Authorization", bearer.as_str()))
            .set_json(&turn)
            .to_request(),
    )
    .await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let revoke = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/sessions/revoke")
            .insert_header(("Authorization", bearer.as_str()))
            .to_request(),
    )
    .await;
    assert_eq!(revoke.status(), StatusCode::NO_CONTENT);
    let revoked = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/v1/turns")
            .insert_header(("Authorization", bearer.as_str()))
            .set_json(&turn)
            .to_request(),
    )
    .await;
    assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
}
