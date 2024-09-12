use anyhow::{Context, Result};
use clap::Parser;
use hyle::{
    bus::SharedMessageBus,
    consensus::Consensus,
    mempool::Mempool,
    node_state::NodeState,
    p2p, rest,
    utils::conf::{self, SharedConf},
};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

fn start_consensus(bus: SharedMessageBus, net_bus: SharedMessageBus, config: SharedConf) {
    tokio::spawn(async move {
        Consensus::load_from_disk()
            .unwrap_or_else(|_| {
                warn!("Failed to load consensus state from disk, using a default one");
                Consensus::default()
            })
            .start(bus, net_bus, config)
            .await
    });
}

fn start_node_state(bus: SharedMessageBus, config: SharedConf) {
    tokio::spawn(async move {
        let mut consensus = NodeState::default();
        consensus.start(bus, config).await
    });
}

fn start_mempool(bus: SharedMessageBus, net_bus: SharedMessageBus) {
    tokio::spawn(async move {
        let mut mempool = Mempool::new(bus, net_bus);
        mempool.start().await
    });
}

fn start_peer(config: SharedConf, mempool: SharedMessageBus, consensus: SharedMessageBus) {
    tokio::spawn(async move {
        if let Err(e) = p2p::p2p_server(config, mempool, consensus).await {
            error!("RPC server failed: {:?}", e);
        }
    });
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub client: Option<bool>,

    #[arg(long, default_value = "master.ron")]
    pub config_file: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let config = conf::Conf::new_shared(args.config_file)?;
    info!("Starting node with config: {:?}", &config);

    debug!("server mode");

    let bus = SharedMessageBus::new();
    let mempool_net_bus = SharedMessageBus::new();
    let consensus_net_bus = SharedMessageBus::new();

    start_mempool(
        SharedMessageBus::new_handle(&bus),
        SharedMessageBus::new_handle(&mempool_net_bus),
    );
    start_node_state(SharedMessageBus::new_handle(&bus), Arc::clone(&config));
    start_consensus(
        SharedMessageBus::new_handle(&bus),
        SharedMessageBus::new_handle(&consensus_net_bus),
        Arc::clone(&config),
    );
    start_peer(
        Arc::clone(&config),
        SharedMessageBus::new_handle(&mempool_net_bus),
        SharedMessageBus::new_handle(&consensus_net_bus),
    );

    // Start REST server
    rest::rest_server(config)
        .await
        .context("Starting REST server")
}
