//! Embedded administration with separate credentials and no browser-side provider secrets.
pub mod model;
mod playground;
pub mod repository;
mod view;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use actix_web::{
    HttpRequest, HttpResponse,
    cookie::{Cookie, SameSite},
    web,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use subtle::ConstantTimeEq;
use crate::{error::AppError, service::GatewayService};
use model::EditPolicy;

const COOKIE: &str = "__Host-asystant_admin";
const LOGIN_COOKIE: &str = "__Host-asystant_login";
const SESSION_LIFETIME: Duration = Duration::from_secs(3600);

struct AdminSession {
    csrf: String,
    expires: Instant,
}
struct LoginAttempt {
    expires: Instant,
}
struct State {
    last_test: Option<Instant>,
    sessions: HashMap<String, AdminSession>,
    attempts: Vec<LoginAttempt>,
}
/// Memory-only sessions intentionally expire on restart; policy alone is durable.
pub struct AdminState {
    secret_hash: Option<[u8; 32]>,
    state: Mutex<State>,
}
impl AdminState {
    pub fn new(secret: Option<&str>) -> Result<Self, AppError> {
        if secret.is_some_and(|s| s.len() < 32 || s.len() > 1024) {
            return Err(AppError::Invalid);
        }
        Ok(Self {
            secret_hash: secret.map(hash),
            state: Mutex::new(State {
                last_test: None,
                sessions: HashMap::new(),
                attempts: Vec::new(),
            }),
        })
    }
    fn authenticate(&self, request: &HttpRequest) -> Result<String, AppError> {
        let value = request.cookie(COOKIE).ok_or(AppError::Authentication)?;
        let mut state = self.state.lock().map_err(|_| AppError::Internal)?;
        state.sessions.retain(|_, s| s.expires > Instant::now());
        state
            .sessions
            .get(&GatewayService::hash(value.value()))
            .map(|s| s.csrf.clone())
            .ok_or(AppError::Authentication)
    }
}
fn hash(value: &str) -> [u8; 32] {
    Sha256::digest(value.as_bytes()).into()
}
fn constant_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    bool::from(a.ct_eq(b))
}
fn random() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}
fn cookie(name: &'static str, value: String, seconds: i64) -> Cookie<'static> {
    Cookie::build(name, value)
        .path("/")
        .http_only(true)
        .secure(true)
        .same_site(SameSite::Strict)
        .max_age(actix_web::cookie::time::Duration::seconds(seconds))
        .finish()
}
fn html(body: String) -> HttpResponse {
    HttpResponse::Ok().insert_header(("Cache-Control","no-store"))
        .insert_header(("Content-Security-Policy","default-src 'none'; style-src 'self'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'"))
        .content_type("text/html; charset=utf-8").body(body)
}
fn redirect() -> HttpResponse {
    HttpResponse::SeeOther()
        .insert_header(("Location", "/admin"))
        .insert_header(("Cache-Control", "no-store"))
        .finish()
}
#[derive(Deserialize)]
struct Login {
    token: String,
    csrf: String,
}
#[derive(Deserialize)]
struct Csrf {
    csrf: String,
}
async fn index(
    request: HttpRequest,
    admin: web::Data<AdminState>,
    service: web::Data<Arc<GatewayService>>,
) -> Result<HttpResponse, AppError> {
    if admin.secret_hash.is_none() {
        return Ok(HttpResponse::NotFound().finish());
    }
    match admin.authenticate(&request) {
        Ok(csrf) => Ok(html(view::dashboard(
            &service.effective_config().await?,
            &csrf,
        ))),
        Err(AppError::Authentication) => {
            let csrf = random();
            let mut response = html(view::login(&csrf));
            response
                .add_cookie(&cookie(LOGIN_COOKIE, csrf, 600))
                .map_err(|_| AppError::Internal)?;
            Ok(response)
        }
        Err(e) => Err(e),
    }
}
async fn login(
    request: HttpRequest,
    admin: web::Data<AdminState>,
    form: web::Form<Login>,
) -> Result<HttpResponse, AppError> {
    let expected = admin.secret_hash.ok_or(AppError::Authentication)?;
    let csrf = request
        .cookie(LOGIN_COOKIE)
        .ok_or(AppError::Authentication)?;
    if csrf.value().len() != 64 || !constant_equal(csrf.value().as_bytes(), form.csrf.as_bytes()) {
        return Err(AppError::Authentication);
    }
    let mut state = admin.state.lock().map_err(|_| AppError::Internal)?;
    state.attempts.retain(|a| a.expires > Instant::now());
    // A global cap remains reliable behind reverse proxies, without trusting spoofable IP headers.
    if state.attempts.len() >= 5 {
        return Err(AppError::Limited);
    }
    if !constant_equal(&expected, &hash(&form.token)) {
        state.attempts.push(LoginAttempt {
            expires: Instant::now() + Duration::from_secs(900),
        });
        return Err(AppError::Authentication);
    }
    state.sessions.retain(|_, s| s.expires > Instant::now());
    if state.sessions.len() >= 128 {
        return Err(AppError::Limited);
    }
    let token = random();
    state.sessions.insert(
        GatewayService::hash(&token),
        AdminSession {
            csrf: random(),
            expires: Instant::now() + SESSION_LIFETIME,
        },
    );
    let mut response = redirect();
    response
        .add_cookie(&cookie(COOKIE, token, 3600))
        .map_err(|_| AppError::Internal)?;
    response
        .add_cookie(&cookie(LOGIN_COOKIE, String::new(), 0))
        .map_err(|_| AppError::Internal)?;
    Ok(response)
}
fn check_csrf(admin: &AdminState, request: &HttpRequest, csrf: &str) -> Result<(), AppError> {
    if !constant_equal(admin.authenticate(request)?.as_bytes(), csrf.as_bytes()) {
        return Err(AppError::Authentication);
    }
    Ok(())
}
async fn save(
    request: HttpRequest,
    admin: web::Data<AdminState>,
    service: web::Data<Arc<GatewayService>>,
    form: web::Form<EditPolicy>,
) -> Result<HttpResponse, AppError> {
    check_csrf(&admin, &request, &form.csrf)?;
    let pool = service.pool.clone();
    let config = service.config.clone();
    let edit = form.into_inner();
    tokio::task::spawn_blocking(move || repository::update_policy(&pool, config, &edit))
        .await
        .map_err(|_| AppError::Internal)??;
    service.refresh_origins().await?;
    Ok(redirect())
}
async fn logout(
    request: HttpRequest,
    admin: web::Data<AdminState>,
    form: web::Form<Csrf>,
) -> Result<HttpResponse, AppError> {
    check_csrf(&admin, &request, &form.csrf)?;
    if let Some(token) = request.cookie(COOKIE) {
        admin
            .state
            .lock()
            .map_err(|_| AppError::Internal)?
            .sessions
            .remove(&GatewayService::hash(token.value()));
    }
    let mut response = redirect();
    response
        .add_cookie(&cookie(COOKIE, String::new(), 0))
        .map_err(|_| AppError::Internal)?;
    Ok(response)
}
#[derive(Deserialize)]
struct TestModel {
    csrf: String,
    issuer: String,
}
async fn test_model(
    request: HttpRequest,
    admin: web::Data<AdminState>,
    service: web::Data<Arc<GatewayService>>,
    form: web::Form<TestModel>,
) -> Result<HttpResponse, AppError> {
    check_csrf(&admin, &request, &form.csrf)?;
    {
        let mut state = admin.state.lock().map_err(|_| AppError::Internal)?;
        if state
            .last_test
            .is_some_and(|last| last.elapsed() < Duration::from_secs(10))
        {
            return Err(AppError::Limited);
        }
        state.last_test = Some(Instant::now());
    }
    let result = playground::run(Arc::clone(service.get_ref()), &form.issuer).await;
    let message = match result {
        Ok(text) => text,
        Err(error) => format!(
            "Test failed: {error}. Review gateway/provider configuration and budget limits."
        ),
    };
    Ok(html(view::test_result(&message)))
}
async fn styles() -> HttpResponse {
    HttpResponse::Ok()
        .insert_header(("Cache-Control", "no-store"))
        .content_type("text/css")
        .body(include_str!("style.css"))
}
pub fn routes(config: &mut web::ServiceConfig) {
    config.service(
        web::scope("/admin")
            .app_data(web::FormConfig::default().limit(8192))
            .route("", web::get().to(index))
            .route("/", web::get().to(index))
            .route("/style.css", web::get().to(styles))
            .route("/login", web::post().to(login))
            .route("/logout", web::post().to(logout))
            .route("/policy", web::post().to(save))
            .route("/test", web::post().to(test_model)),
    );
}
