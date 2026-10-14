use crate::config::PolicyConfig;
use crate::error::GatewayError;
use crate::metrics::POLICY_REJECTIONS_TOTAL;
use serde_json::Value;

#[derive(Debug, Clone)]
pub struct PolicyEngine {
    config: PolicyConfig,
}

#[derive(Debug, Clone)]
pub enum ParsedRpc {
    Single {
        id: Option<Value>,
        method: String,
        raw: Value,
    },
    Batch(Vec<ParsedRpc>),
}

impl PolicyEngine {
    pub fn new(config: PolicyConfig) -> Self {
        Self { config }
    }

    pub fn check_method(&self, method: &str) -> Result<(), GatewayError> {
        // 1. Check explicit blocklist
        if self.config.blocked_methods.contains(method) {
            POLICY_REJECTIONS_TOTAL
                .with_label_values(&[method, "blacklisted"])
                .inc();
            return Err(GatewayError::PolicyViolation(format!(
                "Method '{}' is forbidden by PortalShield administrative security policy",
                method
            )));
        }

        // 2. If allowlist is populated, enforce membership
        if !self.config.allowed_methods.is_empty()
            && !self.config.allowed_methods.contains(method)
        {
            POLICY_REJECTIONS_TOTAL
                .with_label_values(&[method, "not_whitelisted"])
                .inc();
            return Err(GatewayError::PolicyViolation(format!(
                "Method '{}' is not present in the allowed method whitelist",
                method
            )));
        }

        Ok(())
    }

    pub fn inspect_bytes(&self, bytes: &[u8]) -> Result<ParsedRpc, GatewayError> {
        // Payload size gate
        if bytes.len() > self.config.max_payload_bytes {
            POLICY_REJECTIONS_TOTAL
                .with_label_values(&["unknown", "payload_too_large"])
                .inc();
            return Err(GatewayError::PayloadTooLarge(
                bytes.len(),
                self.config.max_payload_bytes,
            ));
        }

        let val: Value = serde_json::from_slice(bytes)
            .map_err(|e| GatewayError::InvalidRequest(format!("Invalid JSON: {}", e)))?;

        self.inspect_value(val)
    }

    pub fn inspect_value(&self, val: Value) -> Result<ParsedRpc, GatewayError> {
        match val {
            Value::Object(map) => {
                let method = map
                    .get("method")
                    .and_then(|m| m.as_str())
                    .ok_or_else(|| {
                        GatewayError::InvalidRequest("Missing or invalid 'method' field".to_string())
                    })?;

                self.check_method(method)?;

                let id = map.get("id").cloned();
                Ok(ParsedRpc::Single {
                    id,
                    method: method.to_string(),
                    raw: Value::Object(map),
                })
            }
            Value::Array(arr) => {
                if arr.is_empty() {
                    return Err(GatewayError::InvalidRequest(
                        "Batch request cannot be empty".to_string(),
                    ));
                }

                let mut parsed_items = Vec::with_capacity(arr.len());
                for item in arr {
                    match self.inspect_value(item) {
                        Ok(p) => parsed_items.push(p),
                        Err(e) => return Err(e),
                    }
                }
                Ok(ParsedRpc::Batch(parsed_items))
            }
            _ => Err(GatewayError::InvalidRequest(
                "JSON-RPC payload must be an Object or Array".to_string(),
            )),
        }
    }

    pub fn is_method_blocked(&self, method: &str) -> bool {
        self.check_method(method).is_err()
    }
}
