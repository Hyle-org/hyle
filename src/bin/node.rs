use anyhow::{Context, Result};
use clap::Parser;
use hyle::{
    bus::SharedMessageBus,
    consensus::Consensus,
    indexer::Indexer,
    mempool::Mempool,
    node_state::NodeState,
    p2p, rest,
    utils::conf::{self, SharedConf},
};
use std::sync::Arc;
use tracing::{debug, error, info, warn};

fn start_consensus(bus: SharedMessageBus, config: SharedConf) {
    let _ = tokio::task::Builder::new()
        .name("Consensus")
        .spawn(async move {
            Consensus::load_from_disk()
                .unwrap_or_else(|_| {
                    warn!("Failed to load consensus state from disk, using a default one");
                    Consensus::default()
                })
                .start(bus, config)
                .await
        });
}

fn start_indexer(mut idxr: Indexer, bus: SharedMessageBus, config: SharedConf) {
    let _ = tokio::task::Builder::new()
        .name("Indexer")
        .spawn(async move {
            idxr.start(config, bus).await;
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

fn start_mempool(bus: SharedMessageBus) {
    let _ = tokio::task::Builder::new()
        .name("Mempool")
        .spawn(async move {
            let mut mempool = Mempool::new(bus);
            mempool.start().await
        });
}

fn start_p2p(bus: SharedMessageBus, config: SharedConf) {
    let _ = tokio::task::Builder::new().name("p2p").spawn(async move {
        if let Err(e) = p2p::p2p_server(config, bus).await {
            error!("RPC server failed: {:?}", e);
        }
    });
}

fn start_mock_workflow(bus: SharedMessageBus) {
    let _ = tokio::task::Builder::new()
        .name("MockWorkflow")
        .spawn(async move {
            let mut mock_workflow = hyle::tools::mock_workflow::MockWorkflowHandler::new(bus);
            mock_workflow.start().await;
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

    // The console subscriber also enables stdout logging by default, configured via RUST_LOG.
    // If there is no RUST_LOG set, default to info
    if std::env::var("RUST_LOG").is_err() {
        std::env::set_var("RUST_LOG", "info");
    }
    console_subscriber::init();

    let config = conf::Conf::new_shared(args.config_file).context("reading config file")?;
    info!("Starting node with config: {:?}", &config);

    debug!("server mode");

    let bus = SharedMessageBus::new();

    start_mempool(SharedMessageBus::new_handle(&bus));

    let idxr = Indexer::new();
    start_indexer(idxr.share(), bus.new_handle(), Arc::clone(&config));
    start_node_state(bus.new_handle(), Arc::clone(&config));
    start_consensus(bus.new_handle(), Arc::clone(&config));
    start_p2p(bus.new_handle(), Arc::clone(&config));

    start_mock_workflow(bus.new_handle());

    // Start REST server
    rest::rest_server(config, bus.new_handle(), idxr)
        .await
        .context("Starting REST server")
}
