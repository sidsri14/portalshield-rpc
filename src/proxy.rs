use crate::error::GatewayError;
use crate::health::{NodeState, SharedNodePool};
use crate::metrics::{FAILOVERS_TOTAL, REQUESTS_TOTAL, REQUEST_DURATION_SECONDS};
use crate::policy::{ParsedRpc, PolicyEngine};
use crate::transport::RpcClient;
use reqwest::StatusCode;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::{debug, info, warn};

#[derive(Clone)]
pub struct ProxyEngine {
    nodes: SharedNodePool,
    policy: Arc<PolicyEngine>,
    client: RpcClient,
}

impl ProxyEngine {
    pub fn new(nodes: SharedNodePool, policy: Arc<PolicyEngine>, timeout: Duration) -> Self {
        let client = RpcClient::new(timeout);

        Self {
            nodes,
            policy,
            client,
        }
    }

    /// Selects upstreams ordered by fitness (Healthy > Degraded, priority, block lag, latency)
    pub async fn get_ordered_candidates(&self) -> Vec<NodeState> {
        let pool = self.nodes.read().await;
        let mut available: Vec<NodeState> =
            pool.iter().filter(|n| n.is_available()).cloned().collect();

        available.sort_by(|a, b| {
            // 1. Healthy over Degraded
            let a_healthy = if a.is_healthy() { 0 } else { 1 };
            let b_healthy = if b.is_healthy() { 0 } else { 1 };
            if a_healthy != b_healthy {
                return a_healthy.cmp(&b_healthy);
            }

            // 2. Priority (lower is preferred)
            if a.priority != b.priority {
                return a.priority.cmp(&b.priority);
            }

            // 3. Block lag (lower is preferred)
            if a.block_lag != b.block_lag {
                return a.block_lag.cmp(&b.block_lag);
            }

            // 4. Configured weight (higher is preferred)
            if a.weight != b.weight {
                return b.weight.cmp(&a.weight);
            }

            // 5. Latency
            a.latency_ms.cmp(&b.latency_ms)
        });

        available
    }

    /// Forward a JSON-RPC request with pre-response single failover retry
    pub async fn process_request(&self, raw_bytes: &[u8]) -> Result<Vec<u8>, GatewayError> {
        // 1. Enforce policy and extract request metadata
        let parsed = self.policy.inspect_bytes(raw_bytes)?;

        let primary_method = match &parsed {
            ParsedRpc::Single { method, .. } => method.clone(),
            ParsedRpc::Batch(items) => {
                if let Some(first) = items.first() {
                    match first {
                        ParsedRpc::Single { method, .. } => format!("batch:{}", method),
                        _ => "batch".to_string(),
                    }
                } else {
                    "batch".to_string()
                }
            }
        };

        // 2. Get ordered candidate upstreams
        let candidates = self.get_ordered_candidates().await;
        if candidates.is_empty() {
            REQUESTS_TOTAL
                .with_label_values(&[&primary_method, "no_upstream"])
                .inc();
            return Err(GatewayError::NoHealthyUpstream);
        }

        let total_candidates = candidates.len();
        let retry_safe = parsed.is_retry_safe();
        let mut last_error = None;

        // Try primary candidate, and permit one bounded failover retry if primary fails pre-response
        for (attempt_idx, candidate) in candidates.iter().enumerate() {
            if attempt_idx > 0 && !retry_safe {
                break;
            }
            if attempt_idx > 1 {
                // Rule: at most 1 failover retry before giving up
                break;
            }

            let start = Instant::now();
            debug!(
                method = %primary_method,
                target_node = %candidate.name,
                attempt = attempt_idx + 1,
                "Forwarding JSON-RPC request to Portaldot upstream"
            );

            let res = self.client.call(&candidate.url, raw_bytes).await;

            match res {
                Ok(resp) => {
                    if resp.is_success() {
                        let elapsed = start.elapsed().as_secs_f64();
                        REQUEST_DURATION_SECONDS
                            .with_label_values(&[&primary_method, &candidate.name])
                            .observe(elapsed);
                        REQUESTS_TOTAL
                            .with_label_values(&[&primary_method, "success"])
                            .inc();

                        return Ok(resp.body);
                    }

                    let Some(status) = resp.status else {
                        last_error = Some(
                            "Unsuccessful upstream response did not include an HTTP status"
                                .to_string(),
                        );
                        break;
                    };
                    if is_retryable_status(status) {
                        let err_msg = format!("Upstream returned HTTP status {}", status);
                        warn!(
                            node = %candidate.name,
                            status = %status,
                            "Upstream returned HTTP failure before valid JSON-RPC payload"
                        );
                        last_error = Some(err_msg);
                    } else {
                        let err_msg =
                            format!("Upstream returned non-retryable HTTP status {}", status);
                        warn!(
                            node = %candidate.name,
                            status = %status,
                            "Upstream returned non-retryable HTTP failure"
                        );
                        last_error = Some(err_msg);
                        break;
                    }
                }
                Err(error) => {
                    let err_msg = format!("Transport failure to {}: {}", candidate.name, error);
                    warn!(
                        node = %candidate.name,
                        error = %error,
                        "Upstream transport error"
                    );
                    last_error = Some(err_msg);
                }
            }

            // If we have another candidate available, record failover and continue loop
            if attempt_idx == 0 && retry_safe && total_candidates > 1 {
                let fallback_name = &candidates[1].name;
                FAILOVERS_TOTAL
                    .with_label_values(&[&candidate.name, fallback_name])
                    .inc();
                info!(
                    failed_node = %candidate.name,
                    fallback_node = %fallback_name,
                    "Triggering safe pre-response failover to secondary Portaldot upstream"
                );
            }
        }

        REQUESTS_TOTAL
            .with_label_values(&[&primary_method, "upstream_error"])
            .inc();

        Err(GatewayError::UpstreamError(last_error.unwrap_or_else(
            || "All candidate upstreams failed pre-response".to_string(),
        )))
    }
}

fn is_retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT
            | StatusCode::TOO_MANY_REQUESTS
            | StatusCode::INTERNAL_SERVER_ERROR
            | StatusCode::BAD_GATEWAY
            | StatusCode::SERVICE_UNAVAILABLE
            | StatusCode::GATEWAY_TIMEOUT
    )
}
