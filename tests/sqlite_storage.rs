//! SQLite-specific durability, contention and atomic accounting contracts.
use std::sync::{Arc, Barrier};
use asystant_gateway::{
    error::AppError,
    model::{Manifest, Session},
    repository::{GatewayRepository, PoolConfig},
};
use chrono::{Duration, Utc};
use diesel::{Connection, RunQueryDsl, SqliteConnection, connection::SimpleConnection};

fn session() -> Session {
    Session {
        token_hash: "hash".into(),
        identity: "identity".into(),
        sid: "login".into(),
        issuer: "product".into(),
        tenant: "tenant".into(),
        subject: "user".into(),
        expires_at: Utc::now() + Duration::minutes(10),
    }
}

#[test]
fn failed_user_budget_rolls_back_tenant_and_request() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.db");
    let pool = PoolConfig::new(path.to_str().unwrap()).unwrap();
    assert!(matches!(
        pool.reserve("request", &session(), 50, 100, 40),
        Err(AppError::Budget)
    ));
    // Both the request ID and tenant balance must have rolled back.
    pool.reserve("request", &session(), 100, 100, 100).unwrap();
    drop(pool);
    let reopened = PoolConfig::new(path.to_str().unwrap()).unwrap();
    assert!(matches!(
        reopened.reserve("another", &session(), 1, 100, 100),
        Err(AppError::Budget)
    ));
    // Pending usage survives restart, and settlement remains exactly once.
    reopened.settle("request", 75).unwrap();
    assert!(matches!(
        reopened.settle("request", 0),
        Err(AppError::Conflict)
    ));
    reopened
        .reserve("remaining", &session(), 25, 100, 100)
        .unwrap();
}

#[test]
fn independent_pools_serialize_reservations_and_settlement() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.db");
    let first = PoolConfig::new(path.to_str().unwrap()).unwrap();
    let second = PoolConfig::connect(path.to_str().unwrap()).unwrap();
    let barrier = Arc::new(Barrier::new(12));
    let workers: Vec<_> = (0..12)
        .map(|i| {
            let pool = if i % 2 == 0 {
                first.clone()
            } else {
                second.clone()
            };
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                match pool.reserve(&i.to_string(), &session(), 10, 50, 50) {
                    Ok(()) => Some(i.to_string()),
                    Err(AppError::Budget) => None,
                    Err(error) => panic!("Unexpected contention error: {error}"),
                }
            })
        })
        .collect();
    let accepted: Vec<_> = workers
        .into_iter()
        .filter_map(|w| w.join().unwrap())
        .collect();
    assert_eq!(accepted.len(), 5);
    let id = accepted[0].clone();
    let other = first.clone();
    let other_id = id.clone();
    let worker = std::thread::spawn(move || other.settle(&other_id, 0));
    let local = second.settle(&id, 0);
    let remote = worker.join().unwrap();
    assert!(matches!(
        (&local, &remote),
        (Ok(()), Err(AppError::Conflict)) | (Err(AppError::Conflict), Ok(()))
    ));
    first.reserve("released", &session(), 10, 50, 50).unwrap();
    assert!(matches!(
        first.reserve("overflow", &session(), 1, 50, 50),
        Err(AppError::Budget)
    ));
}

#[test]
fn wal_allows_reads_during_write_and_readiness_requires_schema() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gateway.db");
    let pool = PoolConfig::connect(path.to_str().unwrap()).unwrap();
    assert!(pool.check_ready().is_err());
    pool.migrate().unwrap();
    pool.migrate().unwrap();
    pool.check_ready().unwrap();
    let manifest = Manifest {
        tools: vec![],
        prompts: vec!["English prompt".into()],
        models: vec!["test".into()],
    };
    pool.register("registration", &session(), &manifest)
        .unwrap();
    let mut conn = SqliteConnection::establish(path.to_str().unwrap()).unwrap();
    conn.batch_execute("BEGIN IMMEDIATE;").unwrap();
    assert_eq!(
        pool.manifest("registration", "identity").unwrap().prompts,
        manifest.prompts
    );
    conn.batch_execute("ROLLBACK;").unwrap();
    assert!(pool.manifest("registration", "foreign-identity").is_err());
    diesel::sql_query("UPDATE registrations SET expires_at = '2000-01-01 00:00:00+00:00'")
        .execute(&mut conn)
        .unwrap();
    assert!(pool.manifest("registration", "identity").is_err());
}

#[test]
fn refuses_ephemeral_and_remote_database_paths() {
    for path in [
        "",
        ":memory:",
        "file:db?mode=memory",
        "postgres://localhost/db",
    ] {
        assert!(PoolConfig::connect(path).is_err());
    }
}
