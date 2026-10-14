# PortalShield RPC

PortalShield is a Rust/Axum policy, health, and observability gateway for
Portaldot JSON-RPC nodes. Clients send JSON-RPC 2.0 over HTTP; the gateway can
reach upstream nodes over HTTP(S) or WebSocket (`ws://` / `wss://`).

Built for Portaldot Hacker House 2026, Node Operations & Tooling track.

[![CI](https://github.com/sidsri14/portalshield-rpc/actions/workflows/ci.yml/badge.svg)](https://github.com/sidsri14/portalshield-rpc/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)

## Why it exists

Node operators and backend services need more than a raw RPC URL:

- a stale or syncing node can return data while silently falling behind;
- shared RPC endpoints should not expose node-administration methods;
- retrying an extrinsic submission can duplicate a state-changing action;
- status pages must not leak tokens embedded in upstream URLs;
- an unavailable gateway must fail health checks, not return a misleading 200.

PortalShield handles those boundaries explicitly and exposes the results as
Prometheus metrics and a small same-origin operator dashboard.

## Architecture

```text
HTTP JSON-RPC client
        |
        v
+-------------------------------+
| PortalShield RPC              |
| - request size limit          |
| - per-peer fixed-window limit |
| - method policy               |
| - health-aware routing        |
| - bounded read-only retry     |
| - Prometheus metrics          |
+-------------------------------+
        |
        +---- HTTP(S) upstream
        |
        +---- WebSocket upstream
```

Health probes call `system_health`, `chain_getHeader`, and `system_chain`.
Candidates are ordered by health, priority, block lag, configured weight, and
observed probe latency.

## Safety model

- Dangerous administration and local-storage RPC methods are blocked by
  default, including `author_insertKey`, `author_rotateKeys`, reserved-peer
  changes, and off-chain local-storage writes.
- Optional allowlisting can restrict the gateway to a known method set.
- Only conservative read-only namespaces are eligible for one fallback retry.
  `author_submitExtrinsic` and mixed batches containing it are never replayed.
- A retry occurs only after a transport failure or HTTP 408, 429, 500, 502,
  503, or 504. Ordinary 4xx responses do not trigger failover.
- Request bodies are rejected at the Axum layer before handler extraction when
  they exceed `max_payload_bytes`.
- Rate limiting uses the TCP peer address. `rate_limit_rps: 0` disables it.
- `/status` omits upstream URLs so credentials and query tokens are not exposed.
- `/health` returns HTTP 503 when no upstream is available.
- The server does not enable permissive cross-origin access.

## Portaldot mainnet quickstart

Requirements: Rust 1.88+ and Git.

```bash
git clone https://github.com/sidsri14/portalshield-rpc.git
cd portalshield-rpc
cargo test --locked

cargo run --release -- \
  --listen 127.0.0.1:9944 \
  --upstreams wss://mainnet.portaldot.io \
  --expected-chain "Portaldot Mainnet"
```

In another terminal:

```bash
curl -s http://127.0.0.1:9944/health

curl -s -X POST http://127.0.0.1:9944/ \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"system_chain","params":[]}'

curl -s http://127.0.0.1:9944/status
curl -s http://127.0.0.1:9944/metrics | grep portalshield
```

The dashboard is available at `http://127.0.0.1:9944/dashboard`.

## Docker

`config.example.json` is configured for the documented Portaldot mainnet
WebSocket endpoint.

```bash
docker compose up --build -d
docker compose logs -f portalshield
curl -f http://127.0.0.1:9944/health
```

## Configuration

```json
{
  "listen_addr": "0.0.0.0:9944",
  "upstreams": [
    {
      "name": "portaldot-mainnet",
      "url": "wss://mainnet.portaldot.io",
      "weight": 100,
      "priority": 1
    }
  ],
  "policy": {
    "blocked_methods": [
      "author_insertKey",
      "author_rotateKeys",
      "system_addReservedPeer",
      "system_removeReservedPeer"
    ],
    "allowed_methods": [],
    "max_payload_bytes": 5242880,
    "rate_limit_rps": 1000
  },
  "health": {
    "probe_interval_secs": 5,
    "max_block_lag": 10,
    "min_peers": 1,
    "expected_chain": "Portaldot Mainnet",
    "timeout_ms": 3000
  }
}
```

When PortalShield is placed behind a reverse proxy, the TCP peer will be that
proxy. The gateway deliberately does not trust forwarded-IP headers by default;
apply per-user limits at the trusted edge or keep a conservative gateway-wide
limit for that peer.

## Verification

The current suite contains 32 unit and integration tests covering policy
inspection, retry safety, HTTP and WebSocket transport, failover status rules,
rate limiting, body limits, health semantics, metrics, and URL redaction.

```bash
cargo fmt --all -- --check
cargo clippy --locked --all-targets --all-features -- -D warnings
cargo test --locked --all-targets
```

A point-in-time mainnet smoke-test record is stored at
[`docs/evidence/portaldot-mainnet-smoke-2026-10-14.json`](docs/evidence/portaldot-mainnet-smoke-2026-10-14.json).
It records a real `system_chain` request proxied through PortalShield to
`wss://mainnet.portaldot.io`. Block height, peer count, and latency are a
snapshot, not performance claims.

## Current boundaries

- Client ingress is HTTP JSON-RPC. WebSocket subscriptions are not proxied.
- WebSocket upstream calls use a bounded request-response connection; there is
  no persistent upstream connection pool yet.
- The fixed-window limiter is intentionally simple and in-memory.
- PortalShield has not undergone an independent production security audit.

## License

Dual-licensed under MIT or Apache-2.0.
