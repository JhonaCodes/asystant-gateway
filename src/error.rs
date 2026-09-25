use actix_web::{HttpResponse, ResponseError, http::StatusCode};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("authentication required")]
    Authentication,
    #[error("invalid request")]
    Invalid,
    #[error("budget exhausted")]
    Budget,
    #[error("duplicate request or ticket")]
    Conflict,
    #[error("request limit reached")]
    Limited,
    #[error("provider unavailable")]
    Provider,
    #[error("service unavailable")]
    Internal,
}
impl ResponseError for AppError {
    fn status_code(&self) -> StatusCode {
        match self {
            Self::Authentication => StatusCode::UNAUTHORIZED,
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::Budget => StatusCode::PAYMENT_REQUIRED,
            Self::Conflict => StatusCode::CONFLICT,
            Self::Limited => StatusCode::TOO_MANY_REQUESTS,
            Self::Provider => StatusCode::BAD_GATEWAY,
            Self::Internal => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
    fn error_response(&self) -> HttpResponse {
        let mut response = HttpResponse::build(self.status_code());
        response.insert_header(("Cache-Control", "no-store"));
        if matches!(self, Self::Limited) {
            response.insert_header(("Retry-After", "60"));
        }
        response.json(json!({"error":self.to_string()}))
    }
}
impl From<diesel::result::Error> for AppError {
    fn from(error: diesel::result::Error) -> Self {
        match error {
            diesel::result::Error::DatabaseError(
                diesel::result::DatabaseErrorKind::UniqueViolation,
                _,
            ) => Self::Conflict,
            _ => Self::Internal,
        }
    }
}
