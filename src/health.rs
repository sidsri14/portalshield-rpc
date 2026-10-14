use crate::config::{HealthConfig, UpstreamConfig};
use crate::metrics::{
    UPSTREAM_BLOCK_HEIGHT, UPSTREAM_BLOCK_LAG, UPSTREAM_HEALTHY, UPSTREAM_PEERS,
    UPSTREAM_PROBE_LATENCY_MS,
};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum NodeStatus {
    Healthy,
    Degraded { reason: String },
    Down { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeState {
    pub name: String,
    pub url: String,
    pub weight: u32,
    pub priority: u32,
    pub status: NodeStatus,
    pub block_height: u64,
    pub block_lag: u64,
    pub peers: u32,
    pub is_syncing: bool,
    pub latency_ms: u64,
    pub chain_name: String,
    pub last_probe: Option<DateTime<Utc>>,
    pub consecutive_errors: u32,
    pub consecutive_successes: u32,
}

impl NodeState {
    pub fn new(cfg: &UpstreamConfig) -> Self {
        Self {
            name: cfg.name.clone(),
            url: cfg.url.clone(),
            weight: cfg.weight,
            priority: cfg.priority,
            status: NodeStatus::Healthy, // optimistic until first probe
            block_height: 0,
            block_lag: 0,
            peers: 0,
            is_syncing: false,
            latency_ms: 0,
            chain_name: String::new(),
            last_probe: None,
            consecutive_errors: 0,
            consecutive_successes: 0,
        }
    }

    pub fn is_available(&self) -> bool {
        !matches!(self.status, NodeStatus::Down { .. })
    }

    pub fn is_healthy(&self) -> bool {
        matches!(self.status, NodeStatus::Healthy)
    }
}

pub type SharedNodePool = Arc<RwLock<Vec<NodeState>>>;

pub struct HealthMonitor {
    nodes: SharedNodePool,
    config: HealthConfig,
    client: Client,
}

impl HealthMonitor {
    pub fn new(nodes: SharedNodePool, config: HealthConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_millis(config.timeout_ms))
            .build()
            .expect("Failed to create reqwest HTTP client for health monitor");

        Self {
            nodes,
            config,
            client,
        }
    }

    pub async fn probe_node(
        client: &Client,
        url: &str,
        expected_chain: Option<&str>,
    ) -> Result<(u64, u32, bool, String, u64), String> {
        let start = Instant::now();

        // 1. Probe system_health
        let health_req = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "system_health",
            "params": []
        });

        let resp = client
            .post(url)
            .json(&health_req)
            .send()
            .await
            .map_err(|e| format!("HTTP request error: {}", e))?;

        let health_val: Value = resp
            .json()
            .await
            .map_err(|e| format!("Invalid JSON from system_health: {}", e))?;

        let (peers, is_syncing) = if let Some(res) = health_val.get("result") {
            let peers = res.get("peers").and_then(|p| p.as_u64()).unwrap_or(0) as u32;
            let syncing = res
                .get("isSyncing")
                .and_then(|s| s.as_bool())
                .unwrap_or(false);
            (peers, syncing)
        } else {
            (0, false)
        };

        // 2. Probe chain_getHeader to extract latest block height
        let header_req = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "chain_getHeader",
            "params": []
        });

        let header_resp = client
            .post(url)
            .json(&header_req)
            .send()
            .await
            .map_err(|e| format!("HTTP header request error: {}", e))?;

        let header_val: Value = header_resp
            .json()
            .await
            .map_err(|e| format!("Invalid JSON from chain_getHeader: {}", e))?;

        let block_height = if let Some(res) = header_val.get("result") {
            if let Some(num_val) = res.get("number") {
                if let Some(num_str) = num_val.as_str() {
                    // Parse hex block number e.g. "0x12a4"
                    let clean_hex = num_str.trim_start_matches("0x");
                    u64::from_str_radix(clean_hex, 16).unwrap_or(0)
                } else if let Some(n) = num_val.as_u64() {
                    n
                } else {
                    0
                }
            } else {
                0
            }
        } else {
            0
        };

        // 3. Probe system_chain
        let chain_req = json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "system_chain",
            "params": []
        });

        let chain_resp = client
            .post(url)
            .json(&chain_req)
            .send()
            .await
            .map_err(|e| format!("HTTP chain request error: {}", e))?;

        let chain_val: Value = chain_resp
            .json()
            .await
            .map_err(|e| format!("Invalid JSON from system_chain: {}", e))?;

        let chain_name = chain_val
            .get("result")
            .and_then(|c| c.as_str())
            .unwrap_or("Unknown")
            .to_string();

        let latency_ms = start.elapsed().as_millis() as u64;

        if let Some(exp) = expected_chain {
            if !chain_name.eq_ignore_ascii_case(exp) {
                return Err(format!(
                    "Chain mismatch: expected '{}', got '{}'",
                    exp, chain_name
                ));
            }
        }

        Ok((block_height, peers, is_syncing, chain_name, latency_ms))
    }

    pub async fn run_probe_cycle(&self) {
        let node_count = { self.nodes.read().await.len() };
        let mut results = Vec::with_capacity(node_count);

        for i in 0..node_count {
            let (url, name) = {
                let pool = self.nodes.read().await;
                (pool[i].url.clone(), pool[i].name.clone())
            };

            let res = Self::probe_node(
                &self.client,
                &url,
                self.config.expected_chain.as_deref(),
            )
            .await;

            results.push((i, name, url, res));
        }

        // Calculate pool highest block height
        let mut highest_block = 0u64;
        for (_, _, _, res) in &results {
            if let Ok((block, _, _, _, _)) = res {
                if *block > highest_block {
                    highest_block = *block;
                }
            }
        }

        let mut pool = self.nodes.write().await;
        for (i, name, url, res) in results {
            let node = &mut pool[i];
            node.last_probe = Some(Utc::now());

            match res {
                Ok((block, peers, syncing, chain_name, latency)) => {
                    node.block_height = block;
                    node.peers = peers;
                    node.is_syncing = syncing;
                    node.chain_name = chain_name;
                    node.latency_ms = latency;
                    node.consecutive_successes += 1;
                    node.consecutive_errors = 0;

                    let block_lag = highest_block.saturating_sub(block);
                    node.block_lag = block_lag;

                    // Evaluate status
                    if syncing {
                        node.status = NodeStatus::Degraded {
                            reason: "Node is currently syncing".to_string(),
                        };
                    } else if peers < self.config.min_peers {
                        node.status = NodeStatus::Degraded {
                            reason: format!(
                                "Peer count {} below required minimum {}",
                                peers, self.config.min_peers
                            ),
                        };
                    } else if block_lag > self.config.max_block_lag {
                        node.status = NodeStatus::Degraded {
                            reason: format!(
                                "Block lag {} blocks exceeds threshold {}",
                                block_lag, self.config.max_block_lag
                            ),
                        };
                    } else {
                        node.status = NodeStatus::Healthy;
                    }

                    // Update Prometheus metrics
                    UPSTREAM_BLOCK_HEIGHT
                        .with_label_values(&[&name])
                        .set(block as f64);
                    UPSTREAM_BLOCK_LAG
                        .with_label_values(&[&name])
                        .set(block_lag as f64);
                    UPSTREAM_PEERS.with_label_values(&[&name]).set(peers as f64);
                    UPSTREAM_PROBE_LATENCY_MS
                        .with_label_values(&[&name])
                        .set(latency as f64);
                    UPSTREAM_HEALTHY
                        .with_label_values(&[&name])
                        .set(if node.is_healthy() { 1.0 } else { 0.0 });

                    debug!(
                        target: "portalshield::health",
                        node = %name,
                        status = ?node.status,
                        block = block,
                        peers = peers,
                        latency_ms = latency,
                        "Probed Portaldot upstream"
                    );
                }
                Err(err) => {
                    node.consecutive_errors += 1;
                    node.consecutive_successes = 0;
                    node.status = NodeStatus::Down {
                        reason: err.clone(),
                    };

                    UPSTREAM_HEALTHY.with_label_values(&[&name]).set(0.0);

                    warn!(
                        target: "portalshield::health",
                        node = %name,
                        url = %url,
                        error = %err,
                        "Portaldot upstream probe failed; node marked DOWN"
                    );
                }
            }
        }
    }

    pub fn start_background_loop(self: Arc<Self>) -> tokio::task::JoinHandle<()> {
        let interval_duration = Duration::from_secs(self.config.probe_interval_secs);
        tokio::spawn(async move {
            info!(
                interval_secs = self.config.probe_interval_secs,
                "Started Portaldot RPC upstream health monitor loop"
            );
            loop {
                self.run_probe_cycle().await;
                tokio::time::sleep(interval_duration).await;
            }
        })
    }
}
