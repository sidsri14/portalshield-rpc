use crate::error::GatewayError;
use crate::health::{NodeState, SharedNodePool};
use crate::metrics::{export_metrics, POLICY_REJECTIONS_TOTAL};
use crate::proxy::ProxyEngine;
use crate::rate_limit::RateLimiter;
use axum::{
    body::{Body, Bytes},
    extract::{ConnectInfo, DefaultBodyLimit, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Request, StatusCode},
    middleware::{self, Next},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Instant;
use tower_http::{set_header::SetResponseHeaderLayer, trace::TraceLayer};
use tracing::warn;

pub struct AppState {
    pub proxy: Arc<ProxyEngine>,
    pub nodes: SharedNodePool,
    pub rate_limiter: Arc<RateLimiter>,
    pub start_time: Instant,
}

pub fn create_router(state: Arc<AppState>, max_payload_bytes: usize) -> Router {
    let rpc_route =
        Router::new()
            .route("/", post(handle_rpc))
            .route_layer(middleware::from_fn_with_state(
                state.clone(),
                enforce_rate_limit,
            ));

    Router::new()
        .route("/", get(handle_dashboard))
        .route("/health", get(handle_health))
        .route("/status", get(handle_status))
        .route("/metrics", get(handle_metrics))
        .route("/dashboard", get(handle_dashboard))
        .merge(rpc_route)
        .layer(DefaultBodyLimit::max(max_payload_bytes))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("content-security-policy"),
            HeaderValue::from_static(
                "default-src 'self'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
            ),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("x-content-type-options"),
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("referrer-policy"),
            HeaderValue::from_static("no-referrer"),
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

async fn enforce_rate_limit(
    State(state): State<Arc<AppState>>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if state.rate_limiter.allow(peer_addr.ip()).await {
        return next.run(request).await;
    }

    POLICY_REJECTIONS_TOTAL
        .with_label_values(&["unknown", "rate_limit"])
        .inc();
    let error = GatewayError::RateLimitExceeded;
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::CONTENT_TYPE, "application/json")],
        error.to_jsonrpc_response(None).to_string(),
    )
        .into_response()
}

async fn handle_rpc(
    State(state): State<Arc<AppState>>,
    _headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Attempt to extract an ID from the request to format nice JSON-RPC error if needed
    let maybe_id = serde_json::from_slice::<Value>(&body)
        .ok()
        .and_then(|v| v.get("id").cloned());

    match state.proxy.process_request(&body).await {
        Ok(resp_bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/json")],
            resp_bytes,
        )
            .into_response(),
        Err(err) => {
            warn!(error = %err, "PortalShield rejected or failed RPC request");
            let err_json = err.to_jsonrpc_response(maybe_id);
            let status_code = match err {
                GatewayError::PolicyViolation(_) => StatusCode::FORBIDDEN,
                GatewayError::PayloadTooLarge(_, _) => StatusCode::PAYLOAD_TOO_LARGE,
                GatewayError::RateLimitExceeded => StatusCode::TOO_MANY_REQUESTS,
                GatewayError::NoHealthyUpstream => StatusCode::SERVICE_UNAVAILABLE,
                GatewayError::UpstreamError(_) => StatusCode::BAD_GATEWAY,
                GatewayError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
                GatewayError::InvalidRequest(_) => StatusCode::BAD_REQUEST,
            };
            (
                status_code,
                [(header::CONTENT_TYPE, "application/json")],
                err_json.to_string(),
            )
                .into_response()
        }
    }
}

async fn handle_health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pool = state.nodes.read().await;
    let total = pool.len();
    let healthy = pool.iter().filter(|n| n.is_healthy()).count();
    let available = pool.iter().filter(|n| n.is_available()).count();

    let status = if healthy > 0 {
        "healthy"
    } else if available > 0 {
        "degraded"
    } else {
        "unhealthy"
    };

    let payload = json!({
        "status": status,
        "healthy_nodes": healthy,
        "available_nodes": available,
        "total_nodes": total,
        "uptime_seconds": state.start_time.elapsed().as_secs(),
        "version": env!("CARGO_PKG_VERSION")
    });

    let http_status = if available > 0 {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    (http_status, Json(payload))
}

async fn handle_status(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let pool = state.nodes.read().await;
    let nodes_list: Vec<PublicNodeState> = pool.iter().map(PublicNodeState::from).collect();
    (StatusCode::OK, Json(nodes_list))
}

#[derive(Serialize)]
struct PublicNodeState {
    name: String,
    weight: u32,
    priority: u32,
    status: crate::health::NodeStatus,
    block_height: u64,
    block_lag: u64,
    peers: u32,
    is_syncing: bool,
    latency_ms: u64,
    chain_name: String,
    last_probe: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<&NodeState> for PublicNodeState {
    fn from(node: &NodeState) -> Self {
        Self {
            name: node.name.clone(),
            weight: node.weight,
            priority: node.priority,
            status: node.status.clone(),
            block_height: node.block_height,
            block_lag: node.block_lag,
            peers: node.peers,
            is_syncing: node.is_syncing,
            latency_ms: node.latency_ms,
            chain_name: node.chain_name.clone(),
            last_probe: node.last_probe,
        }
    }
}

async fn handle_metrics() -> Response {
    match export_metrics() {
        Ok(metrics_text) => (
            StatusCode::OK,
            [(
                header::CONTENT_TYPE,
                "text/plain; version=0.0.4; charset=utf-8",
            )],
            metrics_text,
        )
            .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            [(header::CONTENT_TYPE, "text/plain")],
            format!("Error gathering metrics: {}", err),
        )
            .into_response(),
    }
}

async fn handle_dashboard(State(state): State<Arc<AppState>>) -> Html<String> {
    let pool = state.nodes.read().await;
    let healthy_count = pool.iter().filter(|n| n.is_healthy()).count();
    let total_count = pool.len();
    let uptime = state.start_time.elapsed().as_secs();

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>PortalShield RPC - Portaldot Node Gateway</title>
    <style>
        :root {{
            --bg: #090d16;
            --surface: #101626;
            --surface-hover: #172138;
            --border: #1e293b;
            --accent: #e6007a; /* Substrate / Polkadot / Portaldot Magenta */
            --accent-glow: rgba(230, 0, 122, 0.25);
            --cyan: #00f2fe;
            --green: #10b981;
            --yellow: #f59e0b;
            --red: #ef4444;
            --text-main: #f8fafc;
            --text-muted: #94a3b8;
        }}
        * {{ box-sizing: border-box; margin: 0; padding: 0; }}
        body {{
            background: var(--bg);
            color: var(--text-main);
            font-family: system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
            min-height: 100vh;
            padding: 2rem;
            background-image: 
                radial-gradient(circle at 15% 15%, rgba(230, 0, 122, 0.08) 0%, transparent 40%),
                radial-gradient(circle at 85% 85%, rgba(0, 242, 254, 0.06) 0%, transparent 40%);
        }}
        .header {{
            display: flex;
            align-items: center;
            justify-content: space-between;
            max-width: 1200px;
            margin: 0 auto 2.5rem;
            padding-bottom: 1.5rem;
            border-bottom: 1px solid var(--border);
        }}
        .brand {{
            display: flex;
            align-items: center;
            gap: 1rem;
        }}
        .logo {{
            width: 48px;
            height: 48px;
            border-radius: 12px;
            background: linear-gradient(135deg, var(--accent), var(--cyan));
            display: flex;
            align-items: center;
            justify-content: center;
            font-weight: 800;
            font-size: 1.5rem;
            color: white;
            box-shadow: 0 0 20px var(--accent-glow);
        }}
        .title h1 {{
            font-size: 1.6rem;
            font-weight: 700;
            letter-spacing: -0.02em;
        }}
        .title p {{
            font-size: 0.875rem;
            color: var(--text-muted);
        }}
        .badge {{
            display: inline-flex;
            align-items: center;
            gap: 0.5rem;
            padding: 0.4rem 0.8rem;
            border-radius: 9999px;
            font-size: 0.8rem;
            font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
            background: rgba(16, 185, 129, 0.15);
            color: var(--green);
            border: 1px solid rgba(16, 185, 129, 0.3);
        }}
        .container {{
            max-width: 1200px;
            margin: 0 auto;
            display: grid;
            grid-template-columns: 2fr 1.3fr;
            gap: 2rem;
        }}
        .card {{
            background: var(--surface);
            border: 1px solid var(--border);
            border-radius: 16px;
            padding: 1.75rem;
            box-shadow: 0 8px 24px rgba(0,0,0,0.4);
            margin-bottom: 2rem;
        }}
        .card-header {{
            display: flex;
            justify-content: space-between;
            align-items: center;
            margin-bottom: 1.25rem;
        }}
        .card-header h2 {{
            font-size: 1.2rem;
            font-weight: 600;
            display: flex;
            align-items: center;
            gap: 0.5rem;
        }}
        .stats-grid {{
            display: grid;
            grid-template-columns: repeat(3, 1fr);
            gap: 1rem;
            margin-bottom: 1.5rem;
        }}
        .stat-box {{
            background: rgba(255, 255, 255, 0.02);
            border: 1px solid var(--border);
            border-radius: 12px;
            padding: 1rem;
            text-align: center;
        }}
        .stat-val {{
            font-size: 1.8rem;
            font-weight: 700;
            font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
            color: var(--cyan);
        }}
        .stat-lbl {{
            font-size: 0.75rem;
            color: var(--text-muted);
            text-transform: uppercase;
            letter-spacing: 0.05em;
            margin-top: 0.25rem;
        }}
        .nodes-table {{
            width: 100%;
            border-collapse: collapse;
            font-size: 0.875rem;
        }}
        .nodes-table th {{
            text-align: left;
            padding: 0.75rem;
            color: var(--text-muted);
            border-bottom: 1px solid var(--border);
            font-size: 0.75rem;
            text-transform: uppercase;
            letter-spacing: 0.05em;
        }}
        .nodes-table td {{
            padding: 1rem 0.75rem;
            border-bottom: 1px solid rgba(255,255,255,0.03);
            font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
        }}
        .status-pill {{
            padding: 0.25rem 0.6rem;
            border-radius: 6px;
            font-size: 0.75rem;
            font-weight: 600;
            display: inline-block;
        }}
        .status-pill.healthy {{
            background: rgba(16, 185, 129, 0.15);
            color: var(--green);
            border: 1px solid rgba(16, 185, 129, 0.3);
        }}
        .status-pill.degraded {{
            background: rgba(245, 158, 11, 0.15);
            color: var(--yellow);
            border: 1px solid rgba(245, 158, 11, 0.3);
        }}
        .status-pill.down {{
            background: rgba(239, 68, 68, 0.15);
            color: var(--red);
            border: 1px solid rgba(239, 68, 68, 0.3);
        }}
        .simulator {{
            display: flex;
            flex-direction: column;
            gap: 1rem;
        }}
        .form-group {{
            display: flex;
            flex-direction: column;
            gap: 0.4rem;
        }}
        .form-group label {{
            font-size: 0.8rem;
            color: var(--text-muted);
            font-weight: 500;
        }}
        select, textarea, button {{
            background: #0d121f;
            border: 1px solid var(--border);
            border-radius: 8px;
            color: var(--text-main);
            padding: 0.75rem;
            font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
            font-size: 0.85rem;
        }}
        select:focus, textarea:focus {{
            outline: none;
            border-color: var(--accent);
            box-shadow: 0 0 10px var(--accent-glow);
        }}
        button {{
            background: linear-gradient(135deg, var(--accent), #ff007f);
            color: white;
            font-weight: 600;
            font-family: system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif;
            cursor: pointer;
            transition: all 0.2s;
            border: none;
            padding: 0.85rem;
        }}
        button:hover {{
            transform: translateY(-1px);
            box-shadow: 0 4px 15px var(--accent-glow);
        }}
        .output-box {{
            background: #050811;
            border: 1px solid var(--border);
            border-radius: 8px;
            padding: 1rem;
            font-family: ui-monospace, SFMono-Regular, Consolas, monospace;
            font-size: 0.8rem;
            min-height: 140px;
            max-height: 260px;
            overflow-y: auto;
            white-space: pre-wrap;
            color: #38bdf8;
        }}
        .policy-tag {{
            background: rgba(230, 0, 122, 0.15);
            color: var(--accent);
            border: 1px solid rgba(230, 0, 122, 0.3);
            padding: 0.2rem 0.5rem;
            border-radius: 4px;
            font-size: 0.7rem;
            margin-left: 0.5rem;
        }}
    </style>
</head>
<body>
    <div class="header">
        <div class="brand">
            <div class="logo">PS</div>
            <div class="title">
                <h1>PortalShield RPC Gateway</h1>
                <p>High-Availability Policy & Observability Layer for Portaldot Nodes</p>
            </div>
        </div>
        <div class="badge">
            <span style="width: 8px; height: 8px; border-radius: 50%; background: var(--green); display: inline-block;"></span>
            OPERATIONAL ({healthy_count}/{total_count} Upstreams)
        </div>
    </div>

    <div class="container">
        <!-- Left: Live Node Pool & Metrics -->
        <div>
            <div class="card">
                <div class="card-header">
                    <h2>Portaldot Node Pool</h2>
                    <span style="font-size: 0.8rem; color: var(--text-muted); font-family: ui-monospace, SFMono-Regular, Consolas, monospace">Uptime: {uptime}s</span>
                </div>
                <div class="stats-grid">
                    <div class="stat-box">
                        <div class="stat-val">{healthy_count}</div>
                        <div class="stat-lbl">Active Nodes</div>
                    </div>
                    <div class="stat-box">
                        <div class="stat-val" id="hud-head">Synced</div>
                        <div class="stat-lbl">Consensus State</div>
                    </div>
                    <div class="stat-box">
                        <div class="stat-val" id="hud-lag">0</div>
                        <div class="stat-lbl">Max Block Lag</div>
                    </div>
                </div>

                <table class="nodes-table">
                    <thead>
                        <tr>
                            <th>Node</th>
                            <th>Chain</th>
                            <th>Block Height</th>
                            <th>Lag</th>
                            <th>Status</th>
                        </tr>
                    </thead>
                    <tbody id="nodes-tbody">
                        <tr><td colspan="5" style="text-align: center; color: var(--text-muted)">Loading real-time upstream telemetry...</td></tr>
                    </tbody>
                </table>
            </div>

            <div class="card">
                <div class="card-header">
                    <h2>Active Security Policies <span class="policy-tag">FAIL-CLOSED</span></h2>
                    <a href="/metrics" target="_blank" rel="noopener" style="color: var(--cyan); text-decoration: none; font-size: 0.8rem; font-family: ui-monospace, SFMono-Regular, Consolas, monospace">Prometheus /metrics &rarr;</a>
                </div>
                <div style="font-size: 0.875rem; color: var(--text-muted); line-height: 1.6;">
                    <p style="margin-bottom: 0.5rem;"><strong>Dangerous Administration Disallowed:</strong> <code>author_insertKey</code>, <code>author_rotateKeys</code>, <code>author_hasKey</code>, <code>system_addReservedPeer</code>, <code>system_removeReservedPeer</code>, <code>system_nodeRoles</code></p>
                    <p style="margin-bottom: 0.5rem;"><strong>Bounded Auto-Failover:</strong> Retries read-only requests once after a transport failure, HTTP 429, or selected 5xx response.</p>
                    <p><strong>Periodic Health Probes:</strong> Runs <code>system_health</code>, <code>chain_getHeader</code>, and <code>system_chain</code>.</p>
                </div>
            </div>
        </div>

        <!-- Right: Interactive RPC Inspector & Policy Simulator -->
        <div>
            <div class="card">
                <div class="card-header">
                    <h2>Interactive RPC Inspector</h2>
                </div>
                <div class="simulator">
                    <div class="form-group">
                        <label>Preset RPC Payload:</label>
                        <select id="preset-selector" onchange="loadPreset()">
                            <option value="health">system_health (Safe Read)</option>
                            <option value="chain">system_chain (Safe Read)</option>
                            <option value="header">chain_getHeader (Block Height)</option>
                            <option value="forbidden">author_insertKey (Blocked Admin Policy)</option>
                            <option value="rotate">author_rotateKeys (Blocked Session Policy)</option>
                        </select>
                    </div>

                    <div class="form-group">
                        <label>JSON-RPC Request Body:</label>
                        <textarea id="rpc-payload" rows="6"></textarea>
                    </div>

                    <button onclick="sendRpc()" id="send-btn">Transmit to PortalShield Gateway</button>

                    <div class="form-group">
                        <label>Gateway Response:</label>
                        <div class="output-box" id="rpc-output">Select a preset or enter raw JSON-RPC above and hit Transmit...</div>
                    </div>
                </div>
            </div>
        </div>
    </div>

    <script>
        const PRESETS = {{
            health: JSON.stringify({{"jsonrpc":"2.0","id":1,"method":"system_health","params":[]}}, null, 2),
            chain: JSON.stringify({{"jsonrpc":"2.0","id":2,"method":"system_chain","params":[]}}, null, 2),
            header: JSON.stringify({{"jsonrpc":"2.0","id":3,"method":"chain_getHeader","params":[]}}, null, 2),
            forbidden: JSON.stringify({{"jsonrpc":"2.0","id":4,"method":"author_insertKey","params":["aura","//Alice","0x01"]}}, null, 2),
            rotate: JSON.stringify({{"jsonrpc":"2.0","id":5,"method":"author_rotateKeys","params":[]}}, null, 2)
        }};

        function loadPreset() {{
            const key = document.getElementById('preset-selector').value;
            document.getElementById('rpc-payload').value = PRESETS[key] || '';
        }}

        loadPreset();

        function escapeHtml(value) {{
            const replacements = {{
                '&': '&amp;',
                '<': '&lt;',
                '>': '&gt;',
                '"': '&quot;',
                "'": '&#039;'
            }};
            return String(value).replace(/[&<>"']/g, character => replacements[character]);
        }}

        async function fetchStatus() {{
            try {{
                const res = await fetch('/status');
                const nodes = await res.json();
                const tbody = document.getElementById('nodes-tbody');
                if (nodes.length === 0) {{
                    tbody.innerHTML = '<tr><td colspan="5" style="text-align:center">No nodes configured</td></tr>';
                    return;
                }}
                let maxLag = 0;
                let rows = '';
                for (const n of nodes) {{
                    if (n.block_lag > maxLag) maxLag = n.block_lag;
                    let statusClass = 'healthy';
                    let statusText = 'HEALTHY';
                    if (typeof n.status === 'object') {{
                        if (n.status.Degraded) {{
                            statusClass = 'degraded';
                            statusText = 'DEGRADED';
                        }} else if (n.status.Down) {{
                            statusClass = 'down';
                            statusText = 'DOWN';
                        }}
                    }}
                    rows += `<tr>
                        <td><strong>${{escapeHtml(n.name)}}</strong></td>
                        <td>${{escapeHtml(n.chain_name || '-')}}</td>
                        <td>#${{n.block_height}}</td>
                        <td>${{n.block_lag}} blks</td>
                        <td><span class="status-pill ${{statusClass}}">${{statusText}}</span></td>
                    </tr>`;
                }}
                tbody.innerHTML = rows;
                document.getElementById('hud-lag').innerText = maxLag;
            }} catch (e) {{
                console.error("Failed to load status:", e);
            }}
        }}

        async function sendRpc() {{
            const out = document.getElementById('rpc-output');
            const btn = document.getElementById('send-btn');
            const payload = document.getElementById('rpc-payload').value;
            btn.disabled = true;
            btn.innerText = "Transmitting...";
            out.innerText = "Processing request...";

            const t0 = performance.now();
            try {{
                const res = await fetch('/', {{
                    method: 'POST',
                    headers: {{ 'Content-Type': 'application/json' }},
                    body: payload
                }});
                const elapsed = (performance.now() - t0).toFixed(1);
                const data = await res.json();
                out.innerText = `// HTTP ${{res.status}} (${{elapsed}}ms)\n` + JSON.stringify(data, null, 2);
            }} catch (e) {{
                out.innerText = `// Connection error: ${{e.message}}`;
            }} finally {{
                btn.disabled = false;
                btn.innerText = "Transmit to PortalShield Gateway";
                fetchStatus();
            }}
        }}

        fetchStatus();
        setInterval(fetchStatus, 3000);
    </script>
</body>
</html>
"#,
        healthy_count = healthy_count,
        total_count = total_count,
        uptime = uptime
    );

    Html(html)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::UpstreamConfig;

    #[test]
    fn public_node_state_does_not_expose_upstream_url() {
        let node = NodeState::new(&UpstreamConfig {
            name: "primary".to_string(),
            url: "https://user:secret@example.com/rpc?token=sensitive".to_string(),
            weight: 100,
            priority: 1,
        });

        let serialized = serde_json::to_string(&PublicNodeState::from(&node)).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("sensitive"));
        assert!(!serialized.contains("url"));
    }
}
