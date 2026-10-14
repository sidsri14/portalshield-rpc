use futures_util::{SinkExt, StreamExt};
use reqwest::{Client, StatusCode};
use std::time::Duration;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message};

#[derive(Debug)]
pub struct RpcResponse {
    pub status: Option<StatusCode>,
    pub body: Vec<u8>,
}

impl RpcResponse {
    pub fn is_success(&self) -> bool {
        self.status.is_none_or(|status| status.is_success())
    }
}

#[derive(Clone)]
pub struct RpcClient {
    http: Client,
    timeout: Duration,
}

impl RpcClient {
    pub fn new(timeout_duration: Duration) -> Self {
        let http = Client::builder()
            .timeout(timeout_duration)
            .build()
            .expect("Failed to create reqwest HTTP client");

        Self {
            http,
            timeout: timeout_duration,
        }
    }

    pub async fn call(&self, url: &str, payload: &[u8]) -> Result<RpcResponse, String> {
        if url.starts_with("http://") || url.starts_with("https://") {
            return self.call_http(url, payload).await;
        }
        if url.starts_with("ws://") || url.starts_with("wss://") {
            return self.call_websocket(url, payload).await;
        }

        Err("Unsupported upstream URL scheme; use http(s) or ws(s)".to_string())
    }

    async fn call_http(&self, url: &str, payload: &[u8]) -> Result<RpcResponse, String> {
        let response = self
            .http
            .post(url)
            .header("Content-Type", "application/json")
            .body(payload.to_vec())
            .send()
            .await
            .map_err(|error| format!("HTTP transport error: {}", error))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|error| format!("HTTP response body error: {}", error))?
            .to_vec();

        Ok(RpcResponse {
            status: Some(status),
            body,
        })
    }

    async fn call_websocket(&self, url: &str, payload: &[u8]) -> Result<RpcResponse, String> {
        let request_text = std::str::from_utf8(payload)
            .map_err(|error| format!("WebSocket request is not UTF-8 JSON: {}", error))?;
        let (mut socket, _) = timeout(self.timeout, connect_async(url))
            .await
            .map_err(|_| "WebSocket connection timed out".to_string())?
            .map_err(|error| format!("WebSocket connection error: {}", error))?;

        timeout(
            self.timeout,
            socket.send(Message::Text(request_text.to_string())),
        )
        .await
        .map_err(|_| "WebSocket send timed out".to_string())?
        .map_err(|error| format!("WebSocket send error: {}", error))?;

        loop {
            let message = timeout(self.timeout, socket.next())
                .await
                .map_err(|_| "WebSocket response timed out".to_string())?
                .ok_or_else(|| "WebSocket closed before returning a response".to_string())?
                .map_err(|error| format!("WebSocket receive error: {}", error))?;

            match message {
                Message::Text(text) => {
                    return Ok(RpcResponse {
                        status: None,
                        body: text.as_bytes().to_vec(),
                    });
                }
                Message::Binary(bytes) => {
                    return Ok(RpcResponse {
                        status: None,
                        body: bytes.to_vec(),
                    });
                }
                Message::Ping(payload) => {
                    socket
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|error| format!("WebSocket pong error: {}", error))?;
                }
                Message::Close(frame) => {
                    return Err(format!(
                        "WebSocket closed before returning a response: {:?}",
                        frame
                    ));
                }
                Message::Pong(_) | Message::Frame(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    #[tokio::test]
    async fn websocket_transport_returns_jsonrpc_response() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            let request = socket.next().await.unwrap().unwrap();
            let request_text = request.into_text().unwrap();
            let request_json: Value = serde_json::from_str(&request_text).unwrap();
            let response = json!({
                "jsonrpc": "2.0",
                "id": request_json["id"],
                "result": "Portaldot"
            });
            socket
                .send(Message::Text(response.to_string()))
                .await
                .unwrap();
        });

        let client = RpcClient::new(Duration::from_secs(2));
        let request = serde_json::to_vec(&json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "system_chain",
            "params": []
        }))
        .unwrap();
        let response = client
            .call(&format!("ws://{}", address), &request)
            .await
            .unwrap();
        let response_json: Value = serde_json::from_slice(&response.body).unwrap();

        assert!(response.is_success());
        assert_eq!(response.status, None);
        assert_eq!(response_json["id"], 7);
        assert_eq!(response_json["result"], "Portaldot");
    }

    #[tokio::test]
    async fn rejects_unsupported_upstream_scheme() {
        let client = RpcClient::new(Duration::from_secs(1));
        let error = client.call("ftp://example.com", b"{}").await.unwrap_err();
        assert!(error.contains("Unsupported upstream URL scheme"));
    }
}
