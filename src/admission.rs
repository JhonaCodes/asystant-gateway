use std::{
    collections::HashMap,
    net::IpAddr,
    sync::Mutex,
    time::{Duration, Instant},
};
use actix_web::{
    Error,
    body::MessageBody,
    dev::{ServiceRequest, ServiceResponse},
    middleware::Next,
    web,
};
use crate::error::AppError;

/// Process-local admission guard. A trusted ingress must also enforce shared limits.
#[derive(Default)]
pub struct Admission {
    windows: Mutex<HashMap<(IpAddr, bool), Window>>,
}
struct Window {
    start: Instant,
    count: u32,
}
impl Admission {
    pub fn check(&self, peer: IpAddr, exchange: bool) -> Result<(), AppError> {
        let now = Instant::now();
        let mut windows = self.windows.lock().map_err(|_| AppError::Internal)?;
        windows.retain(|_, window| now.duration_since(window.start) < Duration::from_secs(60));
        let key = (peer, exchange);
        if !windows.contains_key(&key) && windows.len() >= 10_000 {
            return Err(AppError::Limited);
        }
        let window = windows.entry(key).or_insert(Window {
            start: now,
            count: 0,
        });
        let limit = if exchange { 10 } else { 120 };
        if window.count >= limit {
            return Err(AppError::Limited);
        }
        window.count += 1;
        Ok(())
    }
}

pub async fn enforce(
    request: ServiceRequest,
    next: Next<impl MessageBody>,
) -> Result<ServiceResponse<impl MessageBody>, Error> {
    if request.path().starts_with("/v1/") {
        // Never trust caller-supplied Forwarded/X-Forwarded-For headers.
        let peer = request.peer_addr().ok_or(AppError::Invalid)?.ip();
        let admission = request
            .app_data::<web::Data<Admission>>()
            .ok_or(AppError::Internal)?;
        admission.check(peer, request.path() == "/v1/sessions/exchange")?;
    }
    next.call(request).await
}
