use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpstreamConfig {
    pub name: String,
    pub url: String,
    #[serde(default = "default_weight")]
    pub weight: u32,
    #[serde(default = "default_priority")]
    pub priority: u32,
}

fn default_weight() -> u32 {
    100
}

fn default_priority() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyConfig {
    #[serde(default = "default_blocked_methods")]
    pub blocked_methods: HashSet<String>,
    #[serde(default)]
    pub allowed_methods: HashSet<String>, // if empty, all non-blocked methods are allowed
    #[serde(default = "default_max_payload_bytes")]
    pub max_payload_bytes: usize,
    #[serde(default = "default_rate_limit_rps")]
    pub rate_limit_rps: u32,
    #[serde(default = "default_fail_closed")]
    pub fail_closed: bool,
}

fn default_blocked_methods() -> HashSet<String> {
    let mut s = HashSet::new();
    // Dangerous node administration and key injection calls in Substrate/Portaldot
    s.insert("author_insertKey".to_string());
    s.insert("author_rotateKeys".to_string());
    s.insert("author_hasKey".to_string());
    s.insert("author_hasSessionKeys".to_string());
    s.insert("author_removeExtrinsic".to_string());
    s.insert("system_addReservedPeer".to_string());
    s.insert("system_removeReservedPeer".to_string());
    s.insert("system_nodeRoles".to_string());
    s.insert("offchain_localStorageSet".to_string());
    s.insert("offchain_localStorageClear".to_string());
    s
}

fn default_max_payload_bytes() -> usize {
    5 * 1024 * 1024 // 5 MB
}

fn default_rate_limit_rps() -> u32 {
    1000 // requests per second per IP
}

fn default_fail_closed() -> bool {
    true
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            blocked_methods: default_blocked_methods(),
            allowed_methods: HashSet::new(),
            max_payload_bytes: default_max_payload_bytes(),
            rate_limit_rps: default_rate_limit_rps(),
            fail_closed: default_fail_closed(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthConfig {
    #[serde(default = "default_probe_interval_secs")]
    pub probe_interval_secs: u64,
    #[serde(default = "default_max_block_lag")]
    pub max_block_lag: u64,
    #[serde(default = "default_min_peers")]
    pub min_peers: u32,
    #[serde(default)]
    pub expected_chain: Option<String>,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_probe_interval_secs() -> u64 {
    5
}

fn default_max_block_lag() -> u64 {
    10 // flag node degraded if > 10 blocks behind best node
}

fn default_min_peers() -> u32 {
    1 // min peers to not be considered isolated
}

fn default_timeout_ms() -> u64 {
    3000 // 3s health probe timeout
}

impl Default for HealthConfig {
    fn default() -> Self {
        Self {
            probe_interval_secs: default_probe_interval_secs(),
            max_block_lag: default_max_block_lag(),
            min_peers: default_min_peers(),
            expected_chain: None,
            timeout_ms: default_timeout_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "default_listen_addr")]
    pub listen_addr: String,
    pub upstreams: Vec<UpstreamConfig>,
    #[serde(default)]
    pub policy: PolicyConfig,
    #[serde(default)]
    pub health: HealthConfig,
}

fn default_listen_addr() -> String {
    "0.0.0.0:9944".to_string()
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            listen_addr: default_listen_addr(),
            upstreams: vec![
                UpstreamConfig {
                    name: "portaldot-primary".to_string(),
                    url: "http://127.0.0.1:9933".to_string(),
                    weight: 100,
                    priority: 1,
                },
                UpstreamConfig {
                    name: "portaldot-backup".to_string(),
                    url: "http://127.0.0.1:9934".to_string(),
                    weight: 80,
                    priority: 2,
                },
            ],
            policy: PolicyConfig::default(),
            health: HealthConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_validity() {
        let cfg = GatewayConfig::default();
        assert_eq!(cfg.listen_addr, "0.0.0.0:9944");
        assert_eq!(cfg.upstreams.len(), 2);
        assert!(cfg.policy.blocked_methods.contains("author_insertKey"));
        assert!(cfg.policy.fail_closed);
        assert_eq!(cfg.health.probe_interval_secs, 5);
    }

    #[test]
    fn test_serde_json_roundtrip() {
        let cfg = GatewayConfig::default();
        let serialized = serde_json::to_string(&cfg).expect("serialize failed");
        let deserialized: GatewayConfig =
            serde_json::from_str(&serialized).expect("deserialize failed");

        assert_eq!(cfg.listen_addr, deserialized.listen_addr);
        assert_eq!(cfg.upstreams.len(), deserialized.upstreams.len());
    }
}

