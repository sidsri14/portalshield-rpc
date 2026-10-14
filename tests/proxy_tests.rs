use axum::{http::StatusCode, response::IntoResponse, routing::post, Json, Router};
use portalshield_rpc::{
    config::{PolicyConfig, UpstreamConfig},
    error::GatewayError,
    health::{NodeState, SharedNodePool},
    policy::PolicyEngine,
    proxy::ProxyEngine,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

async fn mock_rpc_server(Json(payload): Json<Value>) -> Json<Value> {
    let method = payload.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = payload.get("id").cloned().unwrap_or(json!(1));

    match method {
        "chain_getBlock" => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "block": {
                    "header": { "number": "0x100" }
                }
            }
        })),
        _ => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": "generic_ok"
        })),
    }
}

#[tokio::test]
async fn test_proxy_forwarding_success() {
    let app = Router::new().route("/", post(mock_rpc_server));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let node_cfg = UpstreamConfig {
        name: "test-primary".to_string(),
        url: format!("http://{}", addr),
        weight: 100,
        priority: 1,
    };

    let pool: SharedNodePool = Arc::new(RwLock::new(vec![NodeState::new(&node_cfg)]));
    let policy = Arc::new(PolicyEngine::new(PolicyConfig::default()));
    let proxy = ProxyEngine::new(pool, policy, Duration::from_secs(2));

    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "chain_getBlock",
        "params": []
    });

    let req_bytes = serde_json::to_vec(&req).unwrap();
    let res = proxy.process_request(&req_bytes).await;

    assert!(res.is_ok(), "Expected proxy request to succeed");
    let resp_bytes = res.unwrap();
    let resp_val: Value = serde_json::from_slice(&resp_bytes).unwrap();

    assert_eq!(resp_val.get("id"), Some(&json!(1)));
    assert!(resp_val.get("result").is_some());
}

#[tokio::test]
async fn test_proxy_policy_blocks_before_upstream() {
    // Upstream doesn't even exist
    let node_cfg = UpstreamConfig {
        name: "test-primary".to_string(),
        url: "http://127.0.0.1:9".to_string(),
        weight: 100,
        priority: 1,
    };

    let pool: SharedNodePool = Arc::new(RwLock::new(vec![NodeState::new(&node_cfg)]));
    let policy = Arc::new(PolicyEngine::new(PolicyConfig::default()));
    let proxy = ProxyEngine::new(pool, policy, Duration::from_secs(2));

    let forbidden_req = json!({
        "jsonrpc": "2.0",
        "id": 123,
        "method": "author_insertKey",
        "params": ["babe", "//Alice", "0x00"]
    });

    let req_bytes = serde_json::to_vec(&forbidden_req).unwrap();
    let res = proxy.process_request(&req_bytes).await;

    assert!(res.is_err());
    match res.unwrap_err() {
        GatewayError::PolicyViolation(msg) => {
            assert!(msg.contains("author_insertKey"));
        }
        other => panic!("Expected PolicyViolation, got: {:?}", other),
    }
}

#[tokio::test]
async fn test_proxy_failover_to_backup_upstream() {
    // Spawn healthy backup server
    let app = Router::new().route("/", post(mock_rpc_server));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backup_addr: SocketAddr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Primary points to dead port (e.g. 127.0.0.1:9)
    let primary_cfg = UpstreamConfig {
        name: "dead-primary".to_string(),
        url: "http://127.0.0.1:9".to_string(),
        weight: 100,
        priority: 1,
    };

    let backup_cfg = UpstreamConfig {
        name: "healthy-backup".to_string(),
        url: format!("http://{}", backup_addr),
        weight: 80,
        priority: 2,
    };

    let pool: SharedNodePool = Arc::new(RwLock::new(vec![
        NodeState::new(&primary_cfg),
        NodeState::new(&backup_cfg),
    ]));

    let policy = Arc::new(PolicyEngine::new(PolicyConfig::default()));
    let proxy = ProxyEngine::new(pool, policy, Duration::from_millis(500));

    let req = json!({
        "jsonrpc": "2.0",
        "id": 777,
        "method": "chain_getBlock",
        "params": []
    });

    let req_bytes = serde_json::to_vec(&req).unwrap();
    let res = proxy.process_request(&req_bytes).await;

    // Must succeed via failover to backup!
    assert!(res.is_ok(), "Expected failover to backup to succeed");
    let resp_bytes = res.unwrap();
    let resp_val: Value = serde_json::from_slice(&resp_bytes).unwrap();
    assert_eq!(resp_val.get("id"), Some(&json!(777)));
}

#[tokio::test]
async fn test_proxy_does_not_replay_write_like_request() {
    let app = Router::new().route("/", post(mock_rpc_server));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backup_addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let pool: SharedNodePool = Arc::new(RwLock::new(vec![
        NodeState::new(&UpstreamConfig {
            name: "dead-primary".to_string(),
            url: "http://127.0.0.1:9".to_string(),
            weight: 100,
            priority: 1,
        }),
        NodeState::new(&UpstreamConfig {
            name: "healthy-backup".to_string(),
            url: format!("http://{}", backup_addr),
            weight: 80,
            priority: 2,
        }),
    ]));
    let proxy = ProxyEngine::new(
        pool,
        Arc::new(PolicyEngine::new(PolicyConfig::default())),
        Duration::from_millis(500),
    );
    let request = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "author_submitExtrinsic",
        "params": ["0x1234"]
    }))
    .unwrap();

    assert!(matches!(
        proxy.process_request(&request).await,
        Err(GatewayError::UpstreamError(_))
    ));
}

#[tokio::test]
async fn test_proxy_does_not_fail_over_on_non_retryable_http_status() {
    let failing = Router::new().route(
        "/",
        post(|| async { (StatusCode::BAD_REQUEST, "invalid request").into_response() }),
    );
    let failing_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let failing_addr = failing_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(failing_listener, failing).await.unwrap() });

    let backup = Router::new().route("/", post(mock_rpc_server));
    let backup_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backup_addr = backup_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(backup_listener, backup).await.unwrap() });

    let pool: SharedNodePool = Arc::new(RwLock::new(vec![
        NodeState::new(&UpstreamConfig {
            name: "bad-request-primary".to_string(),
            url: format!("http://{}", failing_addr),
            weight: 100,
            priority: 1,
        }),
        NodeState::new(&UpstreamConfig {
            name: "healthy-backup".to_string(),
            url: format!("http://{}", backup_addr),
            weight: 80,
            priority: 2,
        }),
    ]));
    let proxy = ProxyEngine::new(
        pool,
        Arc::new(PolicyEngine::new(PolicyConfig::default())),
        Duration::from_secs(1),
    );
    let request = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "chain_getHeader",
        "params": []
    }))
    .unwrap();

    let error = proxy.process_request(&request).await.unwrap_err();
    assert!(matches!(error, GatewayError::UpstreamError(message) if message.contains("400")));
}

#[tokio::test]
async fn test_proxy_fails_over_on_retryable_http_status() {
    let failing = Router::new().route(
        "/",
        post(|| async { (StatusCode::SERVICE_UNAVAILABLE, "try later").into_response() }),
    );
    let failing_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let failing_addr = failing_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(failing_listener, failing).await.unwrap() });

    let backup = Router::new().route("/", post(mock_rpc_server));
    let backup_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let backup_addr = backup_listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(backup_listener, backup).await.unwrap() });

    let pool: SharedNodePool = Arc::new(RwLock::new(vec![
        NodeState::new(&UpstreamConfig {
            name: "unavailable-primary".to_string(),
            url: format!("http://{}", failing_addr),
            weight: 100,
            priority: 1,
        }),
        NodeState::new(&UpstreamConfig {
            name: "healthy-backup".to_string(),
            url: format!("http://{}", backup_addr),
            weight: 80,
            priority: 2,
        }),
    ]));
    let proxy = ProxyEngine::new(
        pool,
        Arc::new(PolicyEngine::new(PolicyConfig::default())),
        Duration::from_secs(1),
    );
    let request = serde_json::to_vec(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "chain_getHeader",
        "params": []
    }))
    .unwrap();

    assert!(proxy.process_request(&request).await.is_ok());
}
