use clap::Parser;
use portalshield_rpc::{
    config::{GatewayConfig, HealthConfig, PolicyConfig, UpstreamConfig},
    health::{HealthMonitor, NodeState, SharedNodePool},
    policy::PolicyEngine,
    proxy::ProxyEngine,
    server::{create_router, AppState},
};
use std::{net::SocketAddr, sync::Arc, time::Duration, time::Instant};
use tokio::sync::RwLock;
use tracing::{info, Level};
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(
    name = "portalshield-rpc",
    author = "Siddharth Srivastava <sidsri1502@gmail.com>",
    version = env!("CARGO_PKG_VERSION"),
    about = "High-availability health, policy, and observability gateway for Portaldot JSON-RPC nodes"
)]
struct Args {
    #[arg(
        short,
        long,
        default_value = "0.0.0.0:9944",
        help = "Socket address to listen on"
    )]
    listen: String,

    #[arg(
        short,
        long,
        value_delimiter = ',',
        default_value = "http://127.0.0.1:9933,http://127.0.0.1:9934",
        help = "Comma-separated upstream Portaldot node URLs"
    )]
    upstreams: Vec<String>,

    #[arg(long, help = "Expected Portaldot chain name (e.g., 'Portaldot')")]
    expected_chain: Option<String>,

    #[arg(
        long,
        default_value = "5",
        help = "Health probe interval in seconds"
    )]
    probe_interval: u64,

    #[arg(
        long,
        default_value = "10",
        help = "Maximum block lag before flagging node as degraded"
    )]
    max_block_lag: u64,

    #[arg(
        short,
        long,
        help = "Optional path to JSON config file"
    )]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Initialize structured logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .with_target(false)
        .finish();
    tracing::subscriber::set_global_default(subscriber)
        .expect("setting default subscriber failed");

    let args = Args::parse();

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "Starting PortalShield RPC Gateway"
    );

    // 2. Build configuration
    let config = if let Some(cfg_path) = args.config {
        info!(path = %cfg_path, "Loading configuration from file");
        let content = std::fs::read_to_string(&cfg_path)?;
        serde_json::from_str::<GatewayConfig>(&content)?
    } else {
        let upstreams = args
            .upstreams
            .into_iter()
            .enumerate()
            .map(|(idx, url)| UpstreamConfig {
                name: format!("portaldot-node-{}", idx + 1),
                url,
                weight: 100,
                priority: (idx + 1) as u32,
            })
            .collect();

        GatewayConfig {
            listen_addr: args.listen,
            upstreams,
            policy: PolicyConfig::default(),
            health: HealthConfig {
                probe_interval_secs: args.probe_interval,
                max_block_lag: args.max_block_lag,
                min_peers: 1,
                expected_chain: args.expected_chain,
                timeout_ms: 3000,
            },
        }
    };

    info!(
        listen = %config.listen_addr,
        upstreams_count = config.upstreams.len(),
        "Configured gateway parameters"
    );

    // 3. Initialize node state pool
    let initial_states: Vec<NodeState> = config
        .upstreams
        .iter()
        .map(NodeState::new)
        .collect();
    let nodes: SharedNodePool = Arc::new(RwLock::new(initial_states));

    // 4. Initialize Policy & Proxy Engines
    let policy = Arc::new(PolicyEngine::new(config.policy.clone()));
    let proxy = Arc::new(ProxyEngine::new(
        nodes.clone(),
        policy.clone(),
        Duration::from_millis(config.health.timeout_ms),
    ));

    // 5. Start Health Monitor
    let health_monitor = Arc::new(HealthMonitor::new(nodes.clone(), config.health.clone()));
    
    // Run immediate probe cycle
    info!("Executing initial upstream probe cycle...");
    health_monitor.run_probe_cycle().await;
    
    // Spawn background health polling loop
    health_monitor.clone().start_background_loop();

    // 6. Assemble AppState and Router
    let app_state = Arc::new(AppState {
        proxy,
        nodes: nodes.clone(),
        start_time: Instant::now(),
    });

    let app = create_router(app_state);

    let addr: SocketAddr = config.listen_addr.parse()?;
    let listener = tokio::net::TcpListener::bind(addr).await?;
    info!(addr = %addr, "PortalShield RPC Gateway listening for incoming requests");
    info!("Dashboard available at http://{}/", addr);
    info!("Prometheus metrics available at http://{}/metrics", addr);
    info!("Node health available at http://{}/health", addr);

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("PortalShield RPC Gateway shutdown cleanly");
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install CTRL+C signal handler");
    info!("Shutdown signal received, draining active connections...");
}
