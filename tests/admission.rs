use std::net::{IpAddr, Ipv4Addr};
use actix_web::{App, http::StatusCode, middleware::from_fn, test, web};
use asystant_api::{
    admission::{self, Admission},
    error::AppError,
};

#[test]
async fn limits_exchange_attempts_and_separates_peers() {
    let guard = Admission::default();
    let first = IpAddr::V4(Ipv4Addr::LOCALHOST);
    for _ in 0..10 {
        assert!(guard.check(first, true).is_ok());
    }
    assert!(matches!(guard.check(first, true), Err(AppError::Limited)));
    assert!(guard.check(first, false).is_ok());
    assert!(
        guard
            .check(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)), true)
            .is_ok()
    );
}

#[actix_web::test]
async fn forwarded_headers_cannot_bypass_peer_limit() {
    let app = test::init_service(
        App::new()
            .app_data(web::Data::new(Admission::default()))
            .wrap(from_fn(admission::enforce))
            .route("/v1/sessions/exchange", web::post().to(|| async { "ok" })),
    )
    .await;
    for index in 0..11 {
        let result = test::try_call_service(
            &app,
            test::TestRequest::post()
                .uri("/v1/sessions/exchange")
                .peer_addr(([127, 0, 0, 1], 12345).into())
                .insert_header(("X-Forwarded-For", format!("192.0.2.{index}")))
                .to_request(),
        )
        .await;
        if index < 10 {
            assert_eq!(result.unwrap().status(), StatusCode::OK);
        } else {
            let error = match result {
                Err(error) => error,
                Ok(_) => panic!("rate limit bypassed"),
            };
            assert_eq!(
                error.as_response_error().status_code(),
                StatusCode::TOO_MANY_REQUESTS
            );
            assert_eq!(
                error.error_response().headers().get("Retry-After").unwrap(),
                "60"
            );
        }
    }
}
