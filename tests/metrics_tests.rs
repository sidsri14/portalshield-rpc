use portalshield_rpc::metrics::{export_metrics, REQUESTS_TOTAL};

#[test]
fn test_metrics_export_format() {
    REQUESTS_TOTAL
        .with_label_values(&["system_health", "success"])
        .inc();

    let output = export_metrics().expect("Failed to export Prometheus metrics");

    assert!(output.contains("# HELP portalshield_requests_total"));
    assert!(output.contains("# TYPE portalshield_requests_total counter"));
    assert!(output.contains("portalshield_requests_total{"));
}
