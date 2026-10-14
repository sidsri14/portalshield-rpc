use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    Router,
};
use portalshield_rpc::{
    config::PolicyConfig,
    health::SharedNodePool,
    policy::PolicyEngine,
    proxy::ProxyEngine,
    rate_limit::RateLimiter,
    server::{create_router, AppState},
};
use std::{net::SocketAddr, sync::Arc, time::Duration, time::Instant};
use tokio::sync::RwLock;
use tower::ServiceExt;

fn test_app(rate_limit_rps: u32, max_payload_bytes: usize) -> Router {
    let nodes: SharedNodePool = Arc::new(RwLock::new(Vec::new()));
    let policy = Arc::new(PolicyEngine::new(PolicyConfig::default()));
    let proxy = Arc::new(ProxyEngine::new(
        nodes.clone(),
        policy,
        Duration::from_millis(100),
    ));
    let state = Arc::new(AppState {
        proxy,
        nodes,
        rate_limiter: Arc::new(RateLimiter::new(rate_limit_rps)),
        start_time: Instant::now(),
    });

    create_router(state, max_payload_bytes)
}

fn rpc_request(body: &'static str) -> Request<Body> {
    let mut request = Request::post("/").body(Body::from(body)).unwrap();
    request
        .extensions_mut()
        .insert(ConnectInfo(SocketAddr::from(([192, 0, 2, 10], 50000))));
    request
}

#[tokio::test]
async fn rate_limit_returns_http_429() {
    let app = test_app(1, 1024);
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"system_health","params":[]}"#;

    let first = app.clone().oneshot(rpc_request(body)).await.unwrap();
    assert_eq!(first.status(), StatusCode::SERVICE_UNAVAILABLE);

    let second = app.oneshot(rpc_request(body)).await.unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn oversized_body_is_rejected_before_handler() {
    let app = test_app(100, 32);
    let response = app
        .oneshot(rpc_request(
            r#"{"jsonrpc":"2.0","id":1,"method":"system_health","params":["too-large"]}"#,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn rate_limit_runs_before_body_extraction() {
    let app = test_app(1, 32);
    let small = r#"{"jsonrpc":"2.0"}"#;
    let oversized = r#"{"jsonrpc":"2.0","id":1,"method":"system_health","params":["too-large"]}"#;

    let first = app.clone().oneshot(rpc_request(small)).await.unwrap();
    assert_eq!(first.status(), StatusCode::BAD_REQUEST);

    let second = app.oneshot(rpc_request(oversized)).await.unwrap();
    assert_eq!(second.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn health_is_unavailable_when_no_upstream_is_available() {
    let app = test_app(100, 1024);
    let response = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

#[tokio::test]
async fn responses_include_security_headers() {
    let app = test_app(100, 1024);
    let response = app
        .oneshot(Request::get("/health").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(
        response.headers().get("x-content-type-options").unwrap(),
        "nosniff"
    );
    assert_eq!(
        response.headers().get("referrer-policy").unwrap(),
        "no-referrer"
    );
    assert!(response
        .headers()
        .get("content-security-policy")
        .unwrap()
        .to_str()
        .unwrap()
        .contains("frame-ancestors 'none'"));
}
