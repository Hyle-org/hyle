use anyhow::{Context, Result};
use clap::Parser;
use hyle::{
    bus::SharedMessageBus,
    consensus::Consensus,
    mempool::Mempool,
    node_state::NodeState,
    p2p::{self, network::MempoolMessage},
    rest,
    utils::conf::{self, SharedConf},
};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{debug, error, info, warn};

fn start_consensus(bus: SharedMessageBus, config: SharedConf) {
    tokio::spawn(async move {
        Consensus::load_from_disk()
            .unwrap_or_else(|_| {
                warn!("Failed to load consensus state from disk, using a default one");
                Consensus::default()
            })
            .start(bus, config)
            .await
    });
}

fn start_node_state(bus: SharedMessageBus, config: SharedConf) {
    tokio::spawn(async move {
        let mut consensus = NodeState::default();
        consensus.start(bus, config).await
    });
}

fn start_peer(bus: SharedMessageBus, config: SharedConf, mempool: UnboundedSender<MempoolMessage>) {
    tokio::spawn(async move {
        if let Err(e) = p2p::p2p_server(bus, config, mempool).await {
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

    let mut mp = Mempool::new(SharedMessageBus::new_handle(&bus));
    let (mempool_message_sender, mempool_message_receiver) =
        tokio::sync::mpsc::unbounded_channel::<MempoolMessage>();

    tokio::spawn(async move {
        mp.start(mempool_message_receiver).await;
    });

    start_node_state(SharedMessageBus::new_handle(&bus), Arc::clone(&config));
    start_consensus(SharedMessageBus::new_handle(&bus), Arc::clone(&config));
    start_peer(
        SharedMessageBus::new_handle(&bus),
        Arc::clone(&config),
        mempool_message_sender,
    );

    // Start REST server
    rest::rest_server(config)
        .await
        .context("Starting REST server")
}
