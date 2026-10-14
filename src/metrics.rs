use once_cell::sync::Lazy;
use prometheus::{
    register_counter_vec, register_gauge_vec, register_histogram_vec, CounterVec, Encoder,
    GaugeVec, HistogramVec, TextEncoder,
};

pub static REQUESTS_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!(
        "portalshield_requests_total",
        "Total number of JSON-RPC requests processed by PortalShield",
        &["method", "status"]
    )
    .expect("metric registration failed")
});

pub static REQUEST_DURATION_SECONDS: Lazy<HistogramVec> = Lazy::new(|| {
    register_histogram_vec!(
        "portalshield_request_duration_seconds",
        "Duration of JSON-RPC requests in seconds",
        &["method", "upstream"],
        vec![0.001, 0.005, 0.01, 0.025, 0.05, 0.1, 0.25, 0.5, 1.0, 2.5, 5.0]
    )
    .expect("metric registration failed")
});

pub static UPSTREAM_BLOCK_HEIGHT: Lazy<GaugeVec> = Lazy::new(|| {
    register_gauge_vec!(
        "portalshield_upstream_block_height",
        "Latest observed block height per upstream node",
        &["upstream"]
    )
    .expect("metric registration failed")
});

pub static UPSTREAM_BLOCK_LAG: Lazy<GaugeVec> = Lazy::new(|| {
    register_gauge_vec!(
        "portalshield_upstream_block_lag",
        "Block lag compared to highest observed block height in the pool",
        &["upstream"]
    )
    .expect("metric registration failed")
});

pub static UPSTREAM_HEALTHY: Lazy<GaugeVec> = Lazy::new(|| {
    register_gauge_vec!(
        "portalshield_upstream_healthy",
        "Health state of upstream node (1 = healthy, 0 = degraded/down)",
        &["upstream"]
    )
    .expect("metric registration failed")
});

pub static UPSTREAM_PEERS: Lazy<GaugeVec> = Lazy::new(|| {
    register_gauge_vec!(
        "portalshield_upstream_peers",
        "Connected peer count per upstream node",
        &["upstream"]
    )
    .expect("metric registration failed")
});

pub static UPSTREAM_PROBE_LATENCY_MS: Lazy<GaugeVec> = Lazy::new(|| {
    register_gauge_vec!(
        "portalshield_upstream_probe_latency_ms",
        "Health check round-trip latency in milliseconds",
        &["upstream"]
    )
    .expect("metric registration failed")
});

pub static FAILOVERS_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!(
        "portalshield_failovers_total",
        "Number of automatic failovers executed between upstreams",
        &["from_node", "to_node"]
    )
    .expect("metric registration failed")
});

pub static POLICY_REJECTIONS_TOTAL: Lazy<CounterVec> = Lazy::new(|| {
    register_counter_vec!(
        "portalshield_policy_rejections_total",
        "Number of requests rejected by PortalShield security policy",
        &["method", "reason"]
    )
    .expect("metric registration failed")
});

pub fn export_metrics() -> Result<String, String> {
    let encoder = TextEncoder::new();
    let metric_families = prometheus::gather();
    let mut buffer = Vec::new();
    encoder
        .encode(&metric_families, &mut buffer)
        .map_err(|e| format!("Failed to encode metrics: {}", e))?;
    String::from_utf8(buffer).map_err(|e| format!("Invalid UTF-8 metric string: {}", e))
}
