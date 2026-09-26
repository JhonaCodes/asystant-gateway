use std::sync::{Arc, Barrier};
use chrono::{Duration, Utc};
use jsonwebtoken::{EncodingKey, Header, encode};
use uuid::Uuid;
use asystant_api::{
    config::{Config, ModelConfig, Product, Provider},
    model::{Session, TicketClaims},
    provider::ProviderClient,
    repository::{GatewayRepository, PoolConfig},
    service::GatewayService,
};

#[actix_rt::test]
async fn durable_budgets_replay_rotation_and_revocation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("gateway.db");
    let url = path.to_str().unwrap();
    let pool = PoolConfig::new(url).unwrap();
    let identity = Uuid::new_v4().to_string();
    let secret = "test-only-secret-with-at-least-32-bytes";
    let config = Config {
        products: vec![Product {
            issuer: identity.clone(),
            secret: secret.into(),
            budget_overrides: vec![],
            default_model: None,
            client_models: vec![],
            daily_tenant_micros: 100,
            daily_user_micros: 100,
            models: vec!["test".into()],
        }],
        models: vec![ModelConfig {
            id: "test".into(),
            provider: Provider::Openrouter,
            model: "test".into(),
            key_env: "UNUSED".into(),
            wire_api: Default::default(),
            input_micros_per_million: 1,
            output_micros_per_million: 1,
            max_input_tokens: 10,
            max_output_tokens: 10,
        }],
        origins: vec![],
    };
    let service = GatewayService {
        inference_slots: Arc::new(tokio::sync::Semaphore::new(32)),
        config,
        pool: pool.clone(),
        provider: Arc::new(ProviderClient::new().unwrap()),
        origins: asystant_api::origins::AllowedOrigins::default(),
    };
    let now = Utc::now().timestamp();
    let mut claims = TicketClaims {
        iss: identity.clone(),
        aud: "asystant-api".into(),
        sub: "user".into(),
        tenant: "tenant".into(),
        sid: "session".into(),
        jti: Uuid::new_v4().to_string(),
        iat: now,
        exp: now + 60,
        session_exp: now + 3600,
    };
    let ticket = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();
    let first = service.exchange(&ticket).await.unwrap();
    assert!(service.exchange(&ticket).await.is_err());
    let session = service.authenticate(&first.token).await.unwrap();
    let barrier = Arc::new(Barrier::new(8));
    let mut workers = vec![];
    for _ in 0..8 {
        let pool = pool.clone();
        let session = session.clone();
        let barrier = barrier.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            pool.reserve(&Uuid::new_v4().to_string(), &session, 30, 100, 100)
                .is_ok()
        }));
    }
    assert_eq!(
        workers
            .into_iter()
            .filter(|w| w.thread().id() != std::thread::current().id())
            .map(|w| usize::from(w.join().unwrap()))
            .sum::<usize>(),
        3
    );
    claims.jti = Uuid::new_v4().to_string();
    let ticket = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();
    let second = service.exchange(&ticket).await.unwrap();
    let rotated = service.authenticate(&second.token).await.unwrap();
    assert!(
        pool.reserve(&Uuid::new_v4().to_string(), &rotated, 11, 100, 100)
            .is_err()
    );
    let last = Uuid::new_v4().to_string();
    pool.reserve(&last, &rotated, 10, 100, 100).unwrap();
    assert!(pool.reserve(&last, &rotated, 10, 100, 100).is_err());
    pool.settle(&last, 0).unwrap();
    assert!(pool.settle(&last, 0).is_err());
    pool.reserve(&Uuid::new_v4().to_string(), &rotated, 10, 100, 100)
        .unwrap();
    pool.revoke(&session).unwrap();
    assert!(service.authenticate(&first.token).await.is_err());
    assert!(service.authenticate(&second.token).await.is_err());
    claims.jti = Uuid::new_v4().to_string();
    let ticket = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )
    .unwrap();
    assert!(service.exchange(&ticket).await.is_err());
    // A fresh login keeps the same daily user budget.
    let different_session = Session {
        sid: "other".into(),
        identity: "other".into(),
        expires_at: Utc::now() + Duration::minutes(10),
        ..session
    };
    assert!(
        pool.reserve(&Uuid::new_v4().to_string(), &different_session, 1, 100, 100)
            .is_err()
    );
    let reopened = PoolConfig::new(url).unwrap();
    assert!(
        reopened
            .reserve(&Uuid::new_v4().to_string(), &different_session, 1, 100, 100)
            .is_err()
    );
}
