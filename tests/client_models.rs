use std::sync::Arc;
use asystant_gateway::{
    config::{Config, ModelConfig, Product},
    model::{Session, Manifest},
    repository::PoolConfig,
    provider::ProviderClient,
    service::GatewayService,
};
use chrono::{Utc, Duration};
use serde_json::json;
use uuid::Uuid;

#[actix_rt::test]
async fn client_assignment_overrides_requested_model_and_is_scoped_to_tenant() {
    let product:Product=serde_json::from_value(json!({"issuer":"policy-test","secret":"test-secret-with-more-than-32-characters","daily_tenant_micros":100000,"daily_user_micros":100000,"models":["small","large"],"default_model":"small","client_models":[{"tenant":"school-a","models":["large"],"default_model":"large","allow_selection":false}]})).unwrap();
    let models= ["small","large"].iter().map(|id|serde_json::from_value::<ModelConfig>(json!({"id":id,"provider":"openrouter","model":id,"key_env":"UNUSED","input_micros_per_million":100,"output_micros_per_million":100,"max_input_tokens":10000,"max_output_tokens":100})).unwrap()).collect();
    let config = Config {
        products: vec![product],
        models,
        origins: vec![],
    };
    config.validate().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let pool = PoolConfig::new(directory.path().join("gateway.db").to_str().unwrap()).unwrap();
    let service = GatewayService {
        inference_slots: Arc::new(tokio::sync::Semaphore::new(32)),
        config,
        pool,
        provider: Arc::new(ProviderClient::new().unwrap()),
    };
    let session = Session {
        token_hash: "unused".into(),
        identity: Uuid::new_v4().to_string(),
        sid: "session".into(),
        issuer: "policy-test".into(),
        tenant: "school-a".into(),
        subject: "user".into(),
        expires_at: Utc::now() + Duration::minutes(10),
    };
    let (registration, allowed) = service
        .register(
            session.clone(),
            Manifest {
                tools: vec![],
                prompts: vec![],
                models: vec!["small".into(), "large".into()],
            },
        )
        .await
        .unwrap();
    assert_eq!(allowed, vec!["large"]);
    let service = Arc::new(service);
    let result = service
        .start_turn(
            session,
            asystant_gateway::model::Turn {
                registration_id: registration,
                request_id: "forged-model".into(),
                model: "small".into(),
                messages: vec![asystant_gateway::model::Message {
                    role: "user".into(),
                    content: "Hi".into(),
                    calls: vec![],
                    call_id: String::new(),
                }],
            },
        )
        .await;
    assert!(matches!(
        result,
        Err(asystant_gateway::error::AppError::Invalid)
    ));
}

#[test]
fn tenant_and_user_policies_have_explicit_precedence_and_never_leak() {
    let p:Product=serde_json::from_value(json!({"issuer":"p","secret":"test-secret-with-more-than-32-characters","daily_tenant_micros":100,"daily_user_micros":100,"models":["small","large"],"default_model":"small","client_models":[{"tenant":"a","models":["large"],"default_model":"large","allow_selection":false},{"tenant":"a","subject":"limited","models":["small"],"default_model":"small","allow_selection":false}]})).unwrap();
    let a = p.model_policy("a", "teacher").unwrap();
    assert_eq!(a.default_model, "large");
    assert!(!a.permits("small"));
    assert_eq!(
        p.model_policy("a", "limited").unwrap().default_model,
        "small"
    );
    let b = p.model_policy("b", "teacher").unwrap();
    assert_eq!(b.default_model, "small");
    assert!(b.permits("large"));
}
