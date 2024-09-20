use anyhow::{Context, Result};
use clap::Parser;
use hyle::{
    bus::SharedMessageBus,
    consensus::Consensus,
    history::History,
    mempool::Mempool,
    node_state::NodeState,
    p2p, rest,
    utils::{
        conf::{self, SharedConf},
        crypto::BlstCrypto,
    },
};
use std::{path::Path, sync::Arc};
use tracing::{debug, error, info, level_filters::LevelFilter, warn};
use tracing_subscriber::{prelude::*, EnvFilter};

fn start_consensus(bus: SharedMessageBus, config: SharedConf, crypto: BlstCrypto) {
    let _ = tokio::task::Builder::new()
        .name("Consensus")
        .spawn(async move {
            Consensus::load_from_disk()
                .unwrap_or_else(|_| {
                    warn!("Failed to load consensus state from disk, using a default one");
                    Consensus::default()
                })
                .start(bus, config, crypto)
                .await
        });
}

fn start_history(mut history: History, bus: SharedMessageBus, config: SharedConf) {
    let _ = tokio::task::Builder::new()
        .name("History")
        .spawn(async move {
            history.start(config, bus).await;
        });
}

fn start_node_state(bus: SharedMessageBus, config: SharedConf) {
    let _ = tokio::task::Builder::new()
        .name("NodeState")
        .spawn(async move {
            let mut node_state = NodeState::new(bus);
            node_state.start(config).await
        });
}

fn start_mempool(bus: SharedMessageBus, crypto: BlstCrypto) {
    let _ = tokio::task::Builder::new()
        .name("Mempool")
        .spawn(async move {
            let mut mempool = Mempool::new(bus, crypto);
            mempool.start().await
        });
}

fn start_p2p(bus: SharedMessageBus, config: SharedConf, crypto: BlstCrypto) {
    let _ = tokio::task::Builder::new().name("p2p").spawn(async move {
        if let Err(e) = p2p::p2p_server(config, bus, crypto).await {
            error!("RPC server failed: {:?}", e);
        }
    });
}

fn start_mock_workflow(bus: SharedMessageBus) {
    let _ = tokio::task::Builder::new()
        .name("MockWorkflow")
        .spawn(async move {
            let mut mock_workflow = hyle::tools::mock_workflow::MockWorkflowHandler::new(bus).await;
            mock_workflow.start().await;
        });
}

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub client: Option<bool>,

    #[arg(long, default_value =  None)]
    pub data_directory: Option<String>,

    #[arg(long, default_value = "master.ron")]
    pub config_file: String,
}

/// Setup tracing - stdout and tokio-console subscriber
/// stdout defaults to INFO & sled to INFO even if RUST_LOG is set to e.g. debug (unless it contains "sled")
fn setup_tracing() -> Result<()> {
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()?;

    if let Ok(var) = std::env::var("RUST_LOG") {
        if var.contains("sled") {
            filter = filter.add_directive("sled=info".parse()?);
        }
    }

    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(filter))
        .with(console_subscriber::spawn())
        .init();

    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    setup_tracing()?;

    let args = Args::parse();
    let config = conf::Conf::new_shared(args.config_file).context("reading config file")?;
    info!("Starting node with config: {:?}", &config);

    debug!("server mode");

    let bus = SharedMessageBus::new();
    let crypto = BlstCrypto::new(config.id.clone()); // TODO load sk from disk instead of random

    start_mempool(SharedMessageBus::new_handle(&bus), crypto.clone());

    let data_directory = Path::new(
        args.data_directory
            .as_deref()
            .unwrap_or(config.data_directory.as_deref().unwrap_or("data")),
    );
    std::fs::create_dir_all(data_directory).context("creating data directory")?;

    let history = History::new(
        data_directory
            .join("history.db")
            .to_str()
            .context("invalid data directory")?,
    )?;
    start_history(history.share(), bus.new_handle(), Arc::clone(&config));

    start_node_state(bus.new_handle(), Arc::clone(&config));
    start_consensus(bus.new_handle(), Arc::clone(&config), crypto.clone());
    start_p2p(bus.new_handle(), Arc::clone(&config), crypto);

    start_mock_workflow(bus.new_handle());

    // Start REST server
    rest::rest_server(config, bus.new_handle(), history)
        .await
        .context("Starting REST server")
}
