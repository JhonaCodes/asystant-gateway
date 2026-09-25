use std::sync::Arc;
use actix_web::{App, http::StatusCode, test, web};
use asystant_gateway::{
    admin::{self, AdminState, model::EditPolicy, repository::update_policy},
    config::{Config, ModelConfig, Product, Provider},
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
        manifest: &asystant_gateway::model::Manifest,
        _: &[asystant_gateway::model::Message],
        _: &str,
        _: &tokio::sync::mpsc::Sender<Result<web::Bytes, actix_web::Error>>,
    ) -> Result<(asystant_gateway::model::Message, i64), asystant_gateway::error::AppError> {
        assert!(manifest.tools.is_empty());
        Ok((
            asystant_gateway::model::Message {
                role: "assistant".into(),
                content: "Connection successful.".into(),
                calls: vec![],
                call_id: String::new(),
            },
            5,
        ))
    }
}
fn config() -> Config {
    Config {
        products: vec![Product {
            issuer: "test-product".into(),
            secret: "product-secret-never-expose-this-12345".into(),
            daily_tenant_micros: 10000000,
            daily_user_micros: 1000000,
            models: vec!["model-a".into(), "model-b".into()],
            default_model: None,
            client_models: vec![],
            budget_overrides: vec![],
        }],
        models: ["model-a", "model-b"]
            .iter()
            .map(|id| ModelConfig {
                id: (*id).into(),
                model: (*id).into(),
                provider: Provider::Openrouter,
                key_env: "PROVIDER_SECRET_ENV".into(),
                wire_api: Default::default(),
                input_micros_per_million: 1000000,
                output_micros_per_million: 1000000,
                max_input_tokens: 32768,
                max_output_tokens: 2048,
            })
            .collect(),
        origins: vec![],
    }
}
fn service(pool: PoolConfig) -> Arc<GatewayService> {
    Arc::new(GatewayService {
        inference_slots: Arc::new(tokio::sync::Semaphore::new(1)),
        config: config(),
        pool,
        provider: Arc::new(LocalProvider),
    })
}
fn csrf(body: &str) -> String {
    body.split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
#[actix_rt::test]
async fn admin_authentication_csrf_redaction_logout_and_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("admin.db");
    let svc = service(PoolConfig::new(path.to_str().unwrap()).unwrap());
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(svc.clone()))
            .app_data(web::Data::new(
                AdminState::new(Some("admin-token-random-at-least-32-bytes-long")).unwrap(),
            ))
            .configure(admin::routes),
    )
    .await;
    let unauthorized = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/policy")
            .set_form([("csrf", "invalid"), ("kind", "model")])
            .to_request(),
    )
    .await;
    assert_eq!(unauthorized.status(), StatusCode::UNAUTHORIZED);
    let login = test::call_service(&app, test::TestRequest::get().uri("/admin").to_request()).await;
    let login_cookie = login.response().cookies().next().unwrap().into_owned();
    assert_eq!(login_cookie.secure(), Some(true));
    assert_eq!(login_cookie.http_only(), Some(true));
    let login_body = String::from_utf8(test::read_body(login).await.to_vec()).unwrap();
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/login")
            .cookie(login_cookie)
            .set_form([
                ("csrf", csrf(&login_body)),
                ("token", "admin-token-random-at-least-32-bytes-long".into()),
            ])
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    let session = response
        .response()
        .cookies()
        .find(|c| c.name() == "__Host-asystant_admin")
        .unwrap()
        .into_owned();
    let dashboard = test::call_service(
        &app,
        test::TestRequest::get()
            .uri("/admin")
            .cookie(session.clone())
            .to_request(),
    )
    .await;
    let body = String::from_utf8(test::read_body(dashboard).await.to_vec()).unwrap();
    assert!(body.contains("Model limits"));
    assert!(!body.contains("product-secret"));
    assert!(!body.contains("PROVIDER_SECRET_ENV"));
    let denied = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/policy")
            .cookie(session.clone())
            .set_form([("csrf", "invalid"), ("kind", "model")])
            .to_request(),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    let saved = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/policy")
            .insert_header(("Origin", "https://ai.example.com"))
            .cookie(session.clone())
            .set_form([
                ("csrf", csrf(&body)),
                ("kind", "client".into()),
                ("issuer", "test-product".into()),
                ("tenant", "company-123".into()),
                ("model", "model-b".into()),
                ("daily_tenant_micros", "5000000".into()),
            ])
            .to_request(),
    )
    .await;
    assert_eq!(saved.status(), StatusCode::SEE_OTHER);
    let restarted = service(PoolConfig::new(path.to_str().unwrap()).unwrap());
    let effective = restarted.effective_config().await.unwrap();
    assert_eq!(
        effective.products[0]
            .model_policy("company-123", "user")
            .unwrap()
            .default_model,
        "model-b"
    );
    assert_eq!(
        effective.products[0].limits("company-123", "user"),
        (5000000, 1000000)
    );
    assert_eq!(effective.products[0].secret, config().products[0].secret);
    let tested = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/test")
            .cookie(session.clone())
            .set_form([("csrf", csrf(&body)), ("issuer", "test-product".into())])
            .to_request(),
    )
    .await;
    assert_eq!(tested.status(), StatusCode::OK);
    let tested_body = String::from_utf8(test::read_body(tested).await.to_vec()).unwrap();
    assert!(tested_body.contains("Connection successful."));
    let throttled = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/test")
            .cookie(session.clone())
            .set_form([("csrf", csrf(&body)), ("issuer", "test-product".into())])
            .to_request(),
    )
    .await;
    assert_eq!(throttled.status(), StatusCode::TOO_MANY_REQUESTS);
    let logged_out = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/logout")
            .cookie(session.clone())
            .set_form([("csrf", csrf(&body))])
            .to_request(),
    )
    .await;
    assert_eq!(logged_out.status(), StatusCode::SEE_OTHER);
    let denied = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/policy")
            .cookie(session)
            .set_form([("csrf", csrf(&body)), ("kind", "model".into())])
            .to_request(),
    )
    .await;
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
}
#[actix_rt::test]
async fn brute_force_lock_and_disabled_panel() {
    assert!(AdminState::new(Some("short")).is_err());
    let dir = tempfile::tempdir().unwrap();
    let svc = service(PoolConfig::new(dir.path().join("admin.db").to_str().unwrap()).unwrap());
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(svc.clone()))
            .app_data(web::Data::new(
                AdminState::new(Some("admin-token-random-at-least-32-bytes-long")).unwrap(),
            ))
            .configure(admin::routes),
    )
    .await;
    let response =
        test::call_service(&app, test::TestRequest::get().uri("/admin").to_request()).await;
    let cookie = response.response().cookies().next().unwrap().into_owned();
    let body = String::from_utf8(test::read_body(response).await.to_vec()).unwrap();
    for _ in 0..5 {
        let response = test::call_service(
            &app,
            test::TestRequest::post()
                .uri("/admin/login")
                .cookie(cookie.clone())
                .set_form([("csrf", csrf(&body)), ("token", "wrong".into())])
                .to_request(),
        )
        .await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    let response = test::call_service(
        &app,
        test::TestRequest::post()
            .uri("/admin/login")
            .cookie(cookie)
            .set_form([
                ("csrf", csrf(&body)),
                ("token", "admin-token-random-at-least-32-bytes-long".into()),
            ])
            .to_request(),
    )
    .await;
    assert_eq!(response.status(), StatusCode::TOO_MANY_REQUESTS);
    let disabled = test::init_service(
        App::new()
            .app_data(web::Data::new(svc))
            .app_data(web::Data::new(AdminState::new(None).unwrap()))
            .configure(admin::routes),
    )
    .await;
    assert_eq!(
        test::call_service(
            &disabled,
            test::TestRequest::get().uri("/admin").to_request()
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}
#[actix_rt::test]
async fn invalid_policy_is_atomic_and_live_limits_change() {
    let dir = tempfile::tempdir().unwrap();
    let pool = PoolConfig::new(dir.path().join("admin.db").to_str().unwrap()).unwrap();
    let svc = service(pool.clone());
    let mut config = config();
    let mut other = config.products[0].clone();
    other.issuer = "other-product".into();
    config.products.push(other);
    let mut edit = EditPolicy {
        csrf: String::new(),
        kind: "model".into(),
        issuer: String::new(),
        tenant: String::new(),
        model: "model-a".into(),
        daily_tenant_micros: 0,
        daily_user_micros: 0,
        max_input_tokens: 65536,
        max_output_tokens: 1024,
    };
    update_policy(&pool, config.clone(), &edit).unwrap();
    assert_eq!(
        svc.effective_config().await.unwrap().models[0].max_input_tokens,
        65536
    );
    edit.max_input_tokens = 1000001;
    assert!(update_policy(&pool, config, &edit).is_err());
    assert_eq!(
        svc.effective_config().await.unwrap().models[0].max_input_tokens,
        65536
    );
}

#[actix_rt::test]
async fn persisted_limits_are_enforced_on_existing_registrations() {
    let dir = tempfile::tempdir().unwrap();
    let pool = PoolConfig::new(dir.path().join("admin.db").to_str().unwrap()).unwrap();
    let svc = service(pool.clone());
    let session = asystant_gateway::model::Session {
        token_hash: "test".into(),
        identity: "stable-test".into(),
        sid: "sid".into(),
        issuer: "test-product".into(),
        tenant: "tenant".into(),
        subject: "user".into(),
        expires_at: chrono::Utc::now() + chrono::Duration::minutes(5),
    };
    let (id, _) = svc
        .register(
            session.clone(),
            asystant_gateway::model::Manifest {
                tools: vec![],
                prompts: vec![],
                models: vec![],
            },
        )
        .await
        .unwrap();
    let turn = asystant_gateway::model::Turn {
        registration_id: id,
        request_id: "unique".into(),
        model: "model-a".into(),
        messages: vec![asystant_gateway::model::Message {
            role: "user".into(),
            content: "hello".into(),
            calls: vec![],
            call_id: String::new(),
        }],
    };
    let mut edit = EditPolicy {
        csrf: String::new(),
        kind: "model".into(),
        issuer: String::new(),
        tenant: String::new(),
        model: "model-a".into(),
        daily_tenant_micros: 0,
        daily_user_micros: 0,
        max_input_tokens: 1,
        max_output_tokens: 1024,
    };
    update_policy(&pool, config(), &edit).unwrap();
    assert!(matches!(
        svc.clone().start_turn(session.clone(), turn.clone()).await,
        Err(asystant_gateway::error::AppError::InputLimit { .. })
    ));
    edit.max_input_tokens = 65536;
    update_policy(&pool, config(), &edit).unwrap();
    let mut stream = svc
        .clone()
        .start_turn(session.clone(), turn.clone())
        .await
        .unwrap();
    assert!(stream.recv().await.unwrap().is_ok());
    edit.kind = "client".into();
    edit.issuer = "test-product".into();
    edit.tenant = "tenant".into();
    edit.model = "model-b".into();
    edit.daily_tenant_micros = 5000000;
    update_policy(&pool, config(), &edit).unwrap();
    assert!(matches!(
        svc.start_turn(session, turn).await,
        Err(asystant_gateway::error::AppError::Invalid)
    ));
}
