use anyhow::{Context, Result};
use clap::Parser;
use hyle::bus::SharedMessageBus;
use hyle::mempool::Mempool;
use hyle::p2p::network::MempoolMessage;
use tracing::{debug, warn};
use tracing::{error, info};

use hyle::consensus::Consensus;
use hyle::p2p;
use hyle::rest;
use hyle::utils::conf::{self, Conf};

fn start_consensus(bus: SharedMessageBus, config: Conf) {
    tokio::spawn(async move {
        let mut consensus = Consensus::load_from_disk().unwrap_or_else(|_| {
            warn!("Failed to load consensus state from disk, using a default one");
            Consensus::default()
        });

        consensus.start(bus, &config).await
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

    let config = conf::Conf::new(args.config_file)?;
    info!("Starting node with config: {:?}", config);

    let rpc_addr = config.addr();
    let rest_addr = config.rest_addr().to_string();

    debug!("server mode");

    let bus = SharedMessageBus::new();

    let mut mp = Mempool::new(SharedMessageBus::new_handle(&bus));
    let (mempool_message_sender, mempool_message_receiver) =
        tokio::sync::mpsc::unbounded_channel::<MempoolMessage>();

    tokio::spawn(async move {
        _ = mp.start(mempool_message_receiver).await;
    });

    start_consensus(SharedMessageBus::new_handle(&bus), config.clone());

    tokio::spawn(async move {
        if let Err(e) = p2p::p2p_server(&rpc_addr, &config, mempool_message_sender).await {
            error!("RPC server failed: {:?}", e);
        }
    });
    // Start REST server
    rest::rest_server(&rest_addr)
        .await
        .context("Starting REST server")?;
    Ok(())
}
