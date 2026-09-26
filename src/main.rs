use std::{env, sync::Arc};
use actix_web::{
    App, HttpServer, web,
    middleware::{DefaultHeaders, from_fn},
};
use asystant_gateway::{
    admission::{self, Admission},
    config::Config,
    handler,
    origins::{self, AllowedOrigins},
    provider::ProviderClient,
    repository::PoolConfig,
    service::GatewayService,
};

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    anyhow::ensure!(
        env::var_os("DATABASE_URL").is_none(),
        "DATABASE_URL is no longer supported; configure DATABASE_PATH and migrate existing data before switching storage"
    );
    let mode = env::args().nth(1);
    if let Some(mode) = mode.as_deref()
        && mode != "--migrate-only"
        && mode != "--serve"
    {
        anyhow::bail!("usage: asystant_gateway [--migrate-only | --serve]");
    }
    let database_path = env::var("DATABASE_PATH").unwrap_or_else(|_| "asystant.db".into());
    if mode.as_deref() == Some("--migrate-only") {
        PoolConfig::connect(&database_path)?.migrate()?;
        return Ok(());
    }
    let config = Config::from_env()?;
    let pool = if mode.as_deref() == Some("--serve") {
        PoolConfig::connect(&database_path)?
    } else {
        PoolConfig::new(&database_path)?
    };
    let service = Arc::new(GatewayService {
        inference_slots: Arc::new(tokio::sync::Semaphore::new(32)),
        config: config.clone(),
        pool,
        provider: Arc::new(ProviderClient::new()?),
        origins: AllowedOrigins::default(),
    });
    service.load_origins().await;
    let admin = web::Data::new(asystant_gateway::admin::AdminState::new(
        env::var("ASYSTANT_ADMIN_TOKEN").ok().as_deref(),
    )?);
    let admission = web::Data::new(Admission::default());
    HttpServer::new(move || {
        let cors = origins::cors(service.origins.clone());
        App::new()
            .app_data(admission.clone())
            .app_data(admin.clone())
            .wrap(from_fn(admission::enforce))
            .wrap(
                DefaultHeaders::new()
                    .add(("X-Content-Type-Options", "nosniff"))
                    .add(("Referrer-Policy", "no-referrer"))
                    .add((
                        "Content-Security-Policy",
                        "default-src 'none'; frame-ancestors 'none'",
                    )),
            )
            .app_data(
                web::JsonConfig::default()
                    .limit(262144)
                    .error_handler(|_, _| asystant_gateway::error::AppError::Invalid.into()),
            )
            .app_data(web::Data::new(Arc::clone(&service)))
            .service(web::scope("/v1").wrap(cors).configure(handler::api_routes))
            .configure(handler::public_routes)
            .configure(asystant_gateway::admin::routes)
    })
    .shutdown_timeout(130)
    .bind(env::var("ASYSTANT_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into()))?
    .run()
    .await?;
    Ok(())
}
