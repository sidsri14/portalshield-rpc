# PortalShield RPC 🛡️⚡

> **High-Reliability Health, Policy, and Observability Gateway for Portaldot JSON-RPC Nodes**  
> *Built for Portaldot Hacker House 2026 (DoraHacks) — Track: Node Operations & Tooling*

[![CI](https://github.com/sidsri14/portalshield-rpc/actions/workflows/ci.yml/badge.svg)](https://github.com/sidsri14/portalshield-rpc/actions/workflows/ci.yml)
[![Rust](https://img.shields.io/badge/rust-1.80%2B-orange.svg)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE)
[![Track](https://img.shields.io/badge/DoraHacks-Node%20Operations%20%26%20Tooling-magenta.svg)](https://dorahacks.io/hackathon/portaldot-hacker-house-2026)

---

## 1. Overview & Problem Statement

Portaldot builders and validator operators depend on resilient JSON-RPC connectivity. However, in production environments:
1. **Silent Node Stall & Block Lag:** Stale or syncing RPC nodes can silently desynchronize DApps and indexers without throwing standard transport errors.
2. **Unrestricted Administrative Exposure:** Publicly exposed or shared RPC endpoints risk catastrophic administrative key injections (`author_insertKey`, `author_rotateKeys`) or peer tampering (`system_addReservedPeer`).
3. **Hard Failures on Network Drops:** When a primary RPC node restarts or stalls, standard DApp frontends fail immediately instead of cleanly failing over to backup fullnodes.

**PortalShield RPC** solves this with a lightweight, high-throughput Rust/Axum gateway positioned between clients and Portaldot nodes. It continuously probes upstream nodes, calculates consensus block lag, strictly enforces administrative security policies, performs safe pre-response failovers, and streams Prometheus metrics alongside an interactive operator HUD.

---

## 2. Architecture

```text
       [ DApps / Indexers / Polkadot-JS / Subxt ]
                           │
                           ▼ (JSON-RPC 2.0 / HTTP POST)
            ┌──────────────────────────────┐
            │       PORTALSHIELD RPC       │
            │  ┌────────────────────────┐  │
            │  │  Policy Engine (Gates) │  │ ──► Rejects author_insertKey / rotateKeys
            │  └───────────┬────────────┘  │     (Fail-Closed with Code -32601)
            │              │               │
            │  ┌───────────▼────────────┐  │
            │  │  Proxy / Failover Router│  │ ──► Safe 1-retry pre-response fallback
            │  └───────────┬────────────┘  │
            │              │               │
            │  ┌───────────▼────────────┐  │
            │  │ Health Probe Background│  │ ──► system_health / chain_getHeader / system_chain
            │  └────────────────────────┘  │     Calculates block lag & peer health
            └───────┬──────────────┬───────┘
                    │              │
           Healthy  │              │  Backup / Failover
                    ▼              ▼
           ┌────────────────┐ ┌────────────────┐
           │ Portaldot Node │ │ Portaldot Node │
           │   (Primary)    │ │   (Secondary)  │
           └────────────────┘ └────────────────┘
```

---

## 3. Core Features

- **Automated Health Probes:** Probes `system_health` (peers, syncing state), `chain_getHeader` (block height), and `system_chain` (chain verification) every 5 seconds.
- **Dynamic Block-Lag Computation:** Computes the delta between each node's best block height and the pool leader. Nodes lagging behind a configurable threshold (`max_block_lag`) are instantly flagged `DEGRADED`.
- **Fail-Closed Security Policies:** Blocks dangerous administrative calls by default:
  - `author_insertKey`, `author_rotateKeys`, `author_hasKey`, `author_hasSessionKeys`
  - `author_removeExtrinsic`
  - `system_addReservedPeer`, `system_removeReservedPeer`, `system_nodeRoles`
  - `offchain_localStorageSet`, `offchain_localStorageClear`
- **Safe Pre-Response Retries:** If the primary upstream node drops connection before sending response bytes, PortalShield reroutes the request to the secondary node and records a failover metric. Replays are never attempted after partial byte transmission.
- **Prometheus Telemetry (`/metrics`):** Emits standard metrics for request latency, method counters, node block height, block lag, peer count, failovers, and policy rejections.
- **Interactive Operator HUD (`/`):** Built-in dark mode dashboard with real-time node cards, consensus lag indicators, and an interactive RPC simulator for testing policy blocks and safe reads.

---

## 4. 5-Minute Quickstart

### Prerequisites
- Rust 1.80+ (`cargo`)
- Git

### Build & Run Locally

```bash
# Clone repository
git clone https://github.com/sidsri14/portalshield-rpc.git
cd portalshield-rpc

# Run automated test suite (18 unit + integration tests)
cargo test

# Launch PortalShield pointing to Portaldot nodes
cargo run --release -- \
  --listen 0.0.0.0:9944 \
  --upstreams http://127.0.0.1:9933,http://127.0.0.1:9934 \
  --expected-chain Portaldot
```

The gateway is now live:
- **Interactive Web Dashboard:** `http://localhost:9944/`
- **JSON-RPC Endpoint:** `http://localhost:9944/`
- **Gateway Health Check:** `http://localhost:9944/health`
- **Detailed Upstream Node Status:** `http://localhost:9944/status`
- **Prometheus Metrics:** `http://localhost:9944/metrics`

---

## 5. Live Verification Runbook

### 1. Test Safe Read Query (Allowed)
```bash
curl -s -X POST http://localhost:9944/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"chain_getHeader","params":[]}'
```
*Expected Result:* Clean JSON-RPC header response routed through the healthiest Portaldot node.

### 2. Test Policy Rejection (Administrative Injection Blocked)
```bash
curl -s -X POST http://localhost:9944/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":2,"method":"author_insertKey","params":["aura","//Alice","0x01"]}'
```
*Expected Result (HTTP 403 Forbidden / JSON-RPC Error):*
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "error": {
    "code": -32601,
    "message": "Method disallowed by security policy: Method 'author_insertKey' is forbidden by PortalShield administrative security policy",
    "data": {
      "gateway": "PortalShield-RPC/0.1.0",
      "policy_action": "BLOCKED"
    }
  }
}
```

### 3. Inspect Upstream Node States
```bash
curl -s http://localhost:9944/status | jq .
```

### 4. Scrape Prometheus Metrics
```bash
curl -s http://localhost:9944/metrics | grep portalshield
```

---

## 6. Docker Deployment

```bash
# Build and run with Docker Compose
docker compose up -d

# Check gateway logs
docker compose logs -f portalshield
```

---

## 7. Configuration Schema (`config.json`)

```json
{
  "listen_addr": "0.0.0.0:9944",
  "upstreams": [
    {
      "name": "portaldot-validator-primary",
      "url": "http://127.0.0.1:9933",
      "weight": 100,
      "priority": 1
    },
    {
      "name": "portaldot-fullnode-backup",
      "url": "http://127.0.0.1:9934",
      "weight": 80,
      "priority": 2
    }
  ],
  "policy": {
    "blocked_methods": [
      "author_insertKey",
      "author_rotateKeys",
      "author_hasKey",
      "system_addReservedPeer",
      "system_removeReservedPeer"
    ],
    "allowed_methods": [],
    "max_payload_bytes": 5242880,
    "rate_limit_rps": 1000,
    "fail_closed": true
  },
  "health": {
    "probe_interval_secs": 5,
    "max_block_lag": 10,
    "min_peers": 1,
    "expected_chain": "Portaldot",
    "timeout_ms": 3000
  }
}
```

---

## 8. Test Suite Coverage

```text
running 18 tests
test config::tests::test_default_config_validity ... ok
test config::tests::test_serde_json_roundtrip ... ok
test error::tests::test_error_jsonrpc_serialization ... ok
test error::tests::test_no_upstream_error_code ... ok
test test_node_state_initialization ... ok
test test_node_status_availability ... ok
test test_probe_node_against_mock_portaldot ... ok
test test_metrics_export_format ... ok
test test_safe_methods_allowed ... ok
test test_forbidden_methods_blocked_by_default ... ok
test test_inspect_valid_single_rpc ... ok
test test_inspect_blocked_single_rpc ... ok
test test_inspect_batch_rpc ... ok
test test_oversized_payload_rejected ... ok
test test_malformed_json_rejected ... ok
test test_proxy_forwarding_success ... ok
test test_proxy_policy_blocks_before_upstream ... ok
test test_proxy_failover_to_backup_upstream ... ok

test result: ok. 18 passed; 0 failed; finished in 0.54s
```

---

## 9. License

Dual-licensed under either:
- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)
