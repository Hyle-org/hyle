use anyhow::{Context, Result};
use clap::Parser;
use tokio::sync::mpsc::{self, Receiver, Sender};
use tracing::warn;
use tracing::{error, info};

use hyle::client;
use hyle::conf::{self, Conf};
use hyle::consensus::{Consensus, ConsensusCommand};
use hyle::server;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    client: Option<bool>,

    #[arg(long, default_value = "master.ron")]
    config_file: String,
}

fn start_consensus(tx: Sender<ConsensusCommand>, rx: Receiver<ConsensusCommand>, config: Conf) {
    tokio::spawn(async move {
        let mut consensus = Consensus::load_from_disk().unwrap_or_else(|_| {
            warn!("Failed to load consensus state from disk, using a default one");
            Consensus::default()
        });

        consensus.start(tx, rx, &config).await
    });
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

    if args.client.unwrap_or(false) {
        info!("client mode");
        client::client(&rpc_addr).await?;
    }

    info!("server mode");

    let (consensus_tx, rx) = mpsc::channel::<ConsensusCommand>(100);

    start_consensus(consensus_tx.clone(), rx, config.clone());

    tokio::spawn(async move {
        if let Err(e) = server::p2p_server(&rpc_addr, &config, consensus_tx).await {
            error!("RPC server failed: {:?}", e);
        }
    });

    // Start REST server
    server::rest_server(&rest_addr)
        .await
        .context("Starting REST server")?;

    Ok(())
}
