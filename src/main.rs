use std::{env, sync::Arc};
use actix_cors::Cors;
use actix_web::{
    App, HttpServer, web,
    middleware::{DefaultHeaders, from_fn},
};
use asystant_gateway::{
    admission::{self, Admission},
    config::Config,
    handler,
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
    });
    let admission = web::Data::new(Admission::default());
    HttpServer::new(move || {
        let mut cors = Cors::default()
            .allowed_methods(vec!["GET", "POST"])
            .allowed_headers(vec!["Authorization", "Content-Type"])
            .max_age(600);
        for origin in &config.origins {
            cors = cors.allowed_origin(origin)
        }
        App::new()
            .app_data(admission.clone())
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
            .wrap(cors)
            .app_data(
                web::JsonConfig::default()
                    .limit(262144)
                    .error_handler(|_, _| asystant_gateway::error::AppError::Invalid.into()),
            )
            .app_data(web::Data::new(Arc::clone(&service)))
            .configure(handler::routes)
    })
    .shutdown_timeout(130)
    .bind(env::var("ASYSTANT_BIND").unwrap_or_else(|_| "127.0.0.1:8787".into()))?
    .run()
    .await?;
    Ok(())
}
