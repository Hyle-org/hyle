use anyhow::{Context, Result};
use clap::Parser;
use hyle::mempool::Mempool;
use hyle::model::Transaction;
use hyle::p2p::network::MempoolMessage;
use tokio::sync::mpsc::{self, Receiver, Sender, UnboundedSender};
use tracing::warn;
use tracing::{error, info};

use hyle::consensus::{Consensus, ConsensusCommand};
use hyle::p2p;
use hyle::rest;
use hyle::utils::conf::{self, Conf};

fn start_consensus(
    tx: Sender<ConsensusCommand>,
    rx: Receiver<ConsensusCommand>,
    sender: UnboundedSender<MempoolMessage>,
    config: Conf,
) {
    tokio::spawn(async move {
        let mut consensus = Consensus::load_from_disk().unwrap_or_else(|_| {
            warn!("Failed to load consensus state from disk, using a default one");
            Consensus::default()
        });

        consensus.start(tx, rx, sender, &config).await
    });
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub client: Option<bool>,

    #[arg(short, long)]
    pub id: usize,

    #[arg(long, default_value = "master.ron")]
    pub config_file: String,

    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub no_rest_server: bool,
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

    info!("server mode");

    let mut mp = Mempool::new();
    let (mempool_tx, mempool_rx) = tokio::sync::mpsc::unbounded_channel::<MempoolMessage>();
    tokio::spawn(async move {
        _ = mp.start(mempool_rx).await;
    });

    let (consensus_tx, consensus_rx) = mpsc::channel::<ConsensusCommand>(100);

    start_consensus(
        consensus_tx.clone(),
        consensus_rx,
        mempool_tx.clone(),
        config.clone(),
    );

    tokio::spawn(async move {
        if let Err(e) = p2p::p2p_server(&rpc_addr, &config, consensus_tx, mempool_tx).await {
            error!("RPC server failed: {:?}", e);
        }
    });
    // Start REST server
    rest::rest_server(&rest_addr)
        .await
        .context("Starting REST server")?;
    Ok(())
}
