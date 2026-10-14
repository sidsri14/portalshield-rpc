use axum::{routing::post, Json, Router};
use portalshield_rpc::{
    config::UpstreamConfig,
    health::{HealthMonitor, NodeState, NodeStatus},
};
use reqwest::Client;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::net::TcpListener;

#[test]
fn test_node_state_initialization() {
    let cfg = UpstreamConfig {
        name: "test-node".to_string(),
        url: "http://127.0.0.1:9933".to_string(),
        weight: 100,
        priority: 1,
    };
    let node = NodeState::new(&cfg);

    assert_eq!(node.name, "test-node");
    assert_eq!(node.url, "http://127.0.0.1:9933");
    assert!(node.is_healthy());
    assert!(node.is_available());
    assert_eq!(node.block_height, 0);
    assert_eq!(node.block_lag, 0);
}

#[test]
fn test_node_status_availability() {
    let mut node = NodeState::new(&UpstreamConfig {
        name: "node1".to_string(),
        url: "http://localhost:8000".to_string(),
        weight: 10,
        priority: 1,
    });

    node.status = NodeStatus::Healthy;
    assert!(node.is_healthy());
    assert!(node.is_available());

    node.status = NodeStatus::Degraded {
        reason: "Lagging".to_string(),
    };
    assert!(!node.is_healthy());
    assert!(node.is_available()); // Still available as fallback

    node.status = NodeStatus::Down {
        reason: "Connection refused".to_string(),
    };
    assert!(!node.is_healthy());
    assert!(!node.is_available());
}

async fn mock_portaldot_handler(Json(payload): Json<Value>) -> Json<Value> {
    let method = payload.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let id = payload.get("id").cloned().unwrap_or(json!(1));

    match method {
        "system_health" => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "isSyncing": false,
                "peers": 8,
                "shouldHavePeers": true
            }
        })),
        "chain_getHeader" => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "number": "0x12c0", // decimal 4800
                "parentHash": "0xabc123"
            }
        })),
        "system_chain" => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": "Portaldot"
        })),
        _ => Json(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": "ok"
        })),
    }
}

#[tokio::test]
async fn test_probe_node_against_mock_portaldot() {
    let app = Router::new().route("/", post(mock_portaldot_handler));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    let client = Client::builder()
        .timeout(Duration::from_secs(2))
        .build()
        .unwrap();

    let url = format!("http://{}", addr);

    // 1. Probe matching expected chain
    let res = HealthMonitor::probe_node(&client, &url, Some("Portaldot")).await;
    assert!(res.is_ok());
    let (block, peers, syncing, chain, _latency) = res.unwrap();
    assert_eq!(block, 4800);
    assert_eq!(peers, 8);
    assert!(!syncing);
    assert_eq!(chain, "Portaldot");

    // 2. Probe with mismatching expected chain should fail
    let mismatch_res = HealthMonitor::probe_node(&client, &url, Some("Ethereum")).await;
    assert!(mismatch_res.is_err());
    assert!(mismatch_res.unwrap_err().contains("Chain mismatch"));
}
