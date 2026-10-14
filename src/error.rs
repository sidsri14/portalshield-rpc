use serde_json::{json, Value};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum GatewayError {
    #[error("Method disallowed by security policy: {0}")]
    PolicyViolation(String),

    #[error("Payload exceeds maximum size limit ({0} bytes > {1} bytes)")]
    PayloadTooLarge(usize, usize),

    #[error("Rate limit exceeded")]
    RateLimitExceeded,

    #[error("No healthy upstream Portaldot nodes available")]
    NoHealthyUpstream,

    #[error("Upstream node error: {0}")]
    UpstreamError(String),

    #[error("Invalid JSON-RPC request: {0}")]
    InvalidRequest(String),

    #[error("Internal gateway error: {0}")]
    Internal(String),
}

impl GatewayError {
    pub fn rpc_code(&self) -> i32 {
        match self {
            GatewayError::PolicyViolation(_) => -32601, // Method not found / blocked
            GatewayError::PayloadTooLarge(_, _) => -32000,
            GatewayError::RateLimitExceeded => -32005,
            GatewayError::NoHealthyUpstream => -32002, // Resource unavailable
            GatewayError::UpstreamError(_) => -32003,
            GatewayError::InvalidRequest(_) => -32600, // Invalid Request
            GatewayError::Internal(_) => -32603,       // Internal error
        }
    }

    pub fn to_jsonrpc_response(&self, id: Option<Value>) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": id.unwrap_or(Value::Null),
            "error": {
                "code": self.rpc_code(),
                "message": self.to_string(),
                "data": {
                    "gateway": "PortalShield-RPC/0.1.0",
                    "policy_action": match self {
                        GatewayError::PolicyViolation(_) => "BLOCKED",
                        GatewayError::RateLimitExceeded => "RATE_LIMITED",
                        GatewayError::NoHealthyUpstream => "ALL_UPSTREAMS_DOWN",
                        _ => "REJECTED"
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_jsonrpc_serialization() {
        let err = GatewayError::PolicyViolation("author_insertKey is forbidden".to_string());
        let res = err.to_jsonrpc_response(Some(json!(101)));

        assert_eq!(res["jsonrpc"], "2.0");
        assert_eq!(res["id"], 101);
        assert_eq!(res["error"]["code"], -32601);
        assert_eq!(res["error"]["data"]["policy_action"], "BLOCKED");
    }

    #[test]
    fn test_no_upstream_error_code() {
        let err = GatewayError::NoHealthyUpstream;
        let res = err.to_jsonrpc_response(None);

        assert_eq!(res["jsonrpc"], "2.0");
        assert_eq!(res["id"], Value::Null);
        assert_eq!(res["error"]["code"], -32002);
        assert_eq!(res["error"]["data"]["policy_action"], "ALL_UPSTREAMS_DOWN");
    }
}

