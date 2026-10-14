use portalshield_rpc::{
    config::PolicyConfig,
    error::GatewayError,
    policy::{ParsedRpc, PolicyEngine},
};
use serde_json::json;

#[test]
fn test_safe_methods_allowed() {
    let policy = PolicyEngine::new(PolicyConfig::default());

    assert!(policy.check_method("system_health").is_ok());
    assert!(policy.check_method("chain_getHeader").is_ok());
    assert!(policy.check_method("system_chain").is_ok());
    assert!(policy.check_method("state_getStorage").is_ok());
    assert!(policy.check_method("author_submitExtrinsic").is_ok());
}

#[test]
fn test_forbidden_methods_blocked_by_default() {
    let policy = PolicyEngine::new(PolicyConfig::default());

    let forbidden = vec![
        "author_insertKey",
        "author_rotateKeys",
        "author_hasKey",
        "author_hasSessionKeys",
        "author_removeExtrinsic",
        "system_addReservedPeer",
        "system_removeReservedPeer",
        "system_nodeRoles",
        "offchain_localStorageSet",
    ];

    for method in forbidden {
        let res = policy.check_method(method);
        assert!(
            res.is_err(),
            "Expected method '{}' to be blocked by policy",
            method
        );
        match res.unwrap_err() {
            GatewayError::PolicyViolation(msg) => {
                assert!(msg.contains(method));
            }
            other => panic!("Unexpected error type: {:?}", other),
        }
    }
}

#[test]
fn test_inspect_valid_single_rpc() {
    let policy = PolicyEngine::new(PolicyConfig::default());
    let req = json!({
        "jsonrpc": "2.0",
        "id": 42,
        "method": "chain_getHeader",
        "params": []
    });

    let bytes = serde_json::to_vec(&req).unwrap();
    let parsed = policy.inspect_bytes(&bytes).unwrap();

    match parsed {
        ParsedRpc::Single { id, method, .. } => {
            assert_eq!(id, Some(json!(42)));
            assert_eq!(method, "chain_getHeader");
        }
        ParsedRpc::Batch(_) => panic!("Expected single request"),
    }
}

#[test]
fn test_inspect_blocked_single_rpc() {
    let policy = PolicyEngine::new(PolicyConfig::default());
    let req = json!({
        "jsonrpc": "2.0",
        "id": 99,
        "method": "author_insertKey",
        "params": ["aura", "//Alice", "0x01"]
    });

    let bytes = serde_json::to_vec(&req).unwrap();
    let res = policy.inspect_bytes(&bytes);

    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), GatewayError::PolicyViolation(_)));
}

#[test]
fn test_inspect_batch_rpc() {
    let policy = PolicyEngine::new(PolicyConfig::default());

    // Valid batch
    let valid_batch = json!([
        { "jsonrpc": "2.0", "id": 1, "method": "system_health", "params": [] },
        { "jsonrpc": "2.0", "id": 2, "method": "system_chain", "params": [] }
    ]);
    let bytes = serde_json::to_vec(&valid_batch).unwrap();
    let res = policy.inspect_bytes(&bytes);
    assert!(res.is_ok());

    // Tainted batch containing 1 forbidden method
    let tainted_batch = json!([
        { "jsonrpc": "2.0", "id": 1, "method": "system_health", "params": [] },
        { "jsonrpc": "2.0", "id": 2, "method": "author_insertKey", "params": [] }
    ]);
    let tainted_bytes = serde_json::to_vec(&tainted_batch).unwrap();
    let tainted_res = policy.inspect_bytes(&tainted_bytes);
    assert!(tainted_res.is_err());
    assert!(matches!(
        tainted_res.unwrap_err(),
        GatewayError::PolicyViolation(_)
    ));
}

#[test]
fn test_oversized_payload_rejected() {
    let mut config = PolicyConfig::default();
    config.max_payload_bytes = 100; // artificially small 100 bytes
    let policy = PolicyEngine::new(config);

    let large_str = "x".repeat(200);
    let req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "system_health",
        "params": [large_str]
    });
    let bytes = serde_json::to_vec(&req).unwrap();
    let res = policy.inspect_bytes(&bytes);

    assert!(res.is_err());
    assert!(matches!(
        res.unwrap_err(),
        GatewayError::PayloadTooLarge(_, _)
    ));
}

#[test]
fn test_malformed_json_rejected() {
    let policy = PolicyEngine::new(PolicyConfig::default());
    let malformed = b"{ jsonrpc: invalid }";

    let res = policy.inspect_bytes(malformed);
    assert!(res.is_err());
    assert!(matches!(res.unwrap_err(), GatewayError::InvalidRequest(_)));
}
