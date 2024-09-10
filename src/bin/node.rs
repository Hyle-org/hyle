use anyhow::{Context, Result};
use clap::Parser;
use hyle::mempool::{Mempool, MempoolCommand, MempoolResponse};
use hyle::p2p::network::MempoolMessage;
use tokio::sync::mpsc::{self, Receiver, Sender, UnboundedReceiver, UnboundedSender};
use tracing::{debug, warn};
use tracing::{error, info};

use hyle::consensus::{Consensus, ConsensusCommand};
use hyle::p2p;
use hyle::rest;
use hyle::utils::conf::{self, Conf};

fn start_consensus(
    consensus_command_sender: Sender<ConsensusCommand>,
    consensus_command_receiver: Receiver<ConsensusCommand>,
    mempool_command_sender: UnboundedSender<MempoolCommand>,
    mempool_response_receiver: UnboundedReceiver<MempoolResponse>,
    config: Conf,
) {
    tokio::spawn(async move {
        let mut consensus = Consensus::load_from_disk().unwrap_or_else(|_| {
            warn!("Failed to load consensus state from disk, using a default one");
            Consensus::default()
        });

        consensus
            .start(
                consensus_command_sender,
                consensus_command_receiver,
                mempool_command_sender,
                mempool_response_receiver,
                &config,
            )
            .await
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

    let mut mp = Mempool::new();
    let (mempool_message_sender, mempool_message_receiver) =
        tokio::sync::mpsc::unbounded_channel::<MempoolMessage>();
    let (mempool_command_sender, mempool_command_receiver) =
        tokio::sync::mpsc::unbounded_channel::<MempoolCommand>();
    let (mempool_response_sender, mempool_response_receiver) =
        tokio::sync::mpsc::unbounded_channel::<MempoolResponse>();
    tokio::spawn(async move {
        _ = mp
            .start(
                mempool_message_receiver,
                mempool_command_receiver,
                mempool_response_sender,
            )
            .await;
    });

    let (consensus_command_sender, consensus_command_receiver) =
        mpsc::channel::<ConsensusCommand>(100);

    start_consensus(
        consensus_command_sender.clone(),
        consensus_command_receiver,
        mempool_command_sender.clone(),
        mempool_response_receiver,
        config.clone(),
    );

    tokio::spawn(async move {
        if let Err(e) = p2p::p2p_server(
            &rpc_addr,
            &config,
            consensus_command_sender,
            mempool_message_sender,
        )
        .await
        {
            error!("RPC server failed: {:?}", e);
        }
    });
    // Start REST server
    rest::rest_server(&rest_addr)
        .await
        .context("Starting REST server")?;
    Ok(())
}
