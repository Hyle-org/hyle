use anyhow::{Context, Result};
use clap::Parser;
use hyle::mempool::Mempool;
use hyle::model::Transaction;
use tokio::sync::mpsc::{self, Receiver, Sender, UnboundedSender};
use tracing::warn;
use tracing::{error, info};

use hyle::consensus::{Consensus, ConsensusCommand};
use hyle::p2p;
use hyle::rest;
use hyle::utils::cli;
use hyle::utils::conf::{self, Conf};

fn start_consensus(
    tx: Sender<ConsensusCommand>,
    rx: Receiver<ConsensusCommand>,
    sender: UnboundedSender<Transaction>,
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = cli::Args::parse();

    // install global collector configured based on RUST_LOG env var.
    tracing_subscriber::fmt::init();

    let config = conf::Conf::new(args.config_file)?;
    info!("Starting node with config: {:?}", config);

    let rpc_addr = config.addr();
    let rest_addr = config.rest_addr().to_string();

    info!("server mode");

    let mut mp = Mempool::new(args.id, config.mempool_peers.clone());
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel::<Transaction>();
    tokio::spawn(async move {
        _ = mp.start(receiver).await;
    });

    let (consensus_tx, rx) = mpsc::channel::<ConsensusCommand>(100);

    start_consensus(consensus_tx.clone(), rx, sender, config.clone());

    if args.no_rest_server {
        info!("not starting rest server");
        if let Err(e) = p2p::p2p_server(&rpc_addr, &config, consensus_tx).await {
            error!("RPC server failed: {:?}", e);
        }
    } else {
        tokio::spawn(async move {
            if let Err(e) = p2p::p2p_server(&rpc_addr, &config, consensus_tx).await {
                error!("RPC server failed: {:?}", e);
            }
        });
        // Start REST server
        rest::rest_server(&rest_addr)
            .await
            .context("Starting REST server")?;
    }
    Ok(())
}
