use anyhow::{Context, Result};
use axum_otel_metrics::HttpMetricsLayerBuilder;
use clap::Parser;
use hyle::{
    bus::SharedMessageBus,
    consensus::Consensus,
    history::History,
    mempool::Mempool,
    model::RunContext,
    node_state::NodeState,
    p2p::{P2P},
    rest::{RestApi, RestApiRunContext},
    tools::mock_workflow::MockWorkflowHandler,
    utils::{
        conf::{self},
        crypto::BlstCrypto,
        modules::{Module, ModulesHandler},
    },
};
use std::{path::Path, sync::Arc};
use tracing::{debug, error, info, level_filters::LevelFilter};
use tracing_subscriber::{prelude::*, EnvFilter};

//async fn start_consensus(
//    mut consensus: Consensus,
//    bus: SharedMessageBus,
//    config: SharedConf,
//    crypto: BlstCrypto,
//) -> Result<()> {
//    consensus.start(bus, config, crypto).await
//}
//
//async fn start_history(
//    mut history: History,
//    bus: SharedMessageBus,
//    config: SharedConf,
//) -> Result<()> {
//    history.start(config, bus).await
//}
//
//async fn start_node_state(mut node_state: NodeState, config: SharedConf) -> Result<()> {
//    node_state.start(config).await
//}
//
//async fn start_mempool(mut mempool: Mempool) -> Result<()> {
//    mempool.start().await
//}
//
//async fn start_p2p(bus: SharedMessageBus, config: SharedConf, crypto: BlstCrypto) -> Result<()> {
//    p2p::p2p_server(config, bus, crypto).await
//}
//
//async fn start_mock_workflow(mut mock_workflow: MockWorkflowHandler) -> Result<()> {
//    mock_workflow.start().await
//}
//
//async fn start_rest_server(
//    config: SharedConf,
//    bus: SharedMessageBus,
//    metrics_layer: HttpMetricsLayer,
//    history: History,
//) -> Result<()> {
//    rest::rest_server(config, bus, metrics_layer, history).await
//}

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
        if !var.contains("sled") {
            filter = filter.add_directive("sled=info".parse()?);
        }
        if !var.contains("tower_http") {
            // API request/response debug tracing
            filter = filter.add_directive("tower_http::trace=debug".parse()?);
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

    // Init global metrics meter we expose as an endpoint
    let metrics_layer = HttpMetricsLayerBuilder::new()
        .with_service_name(config.id.to_string().clone())
        .build();

    let bus = SharedMessageBus::new();
    let crypto = Arc::new(BlstCrypto::new(config.id.clone())); // TODO load sk from disk instead of random

    let data_directory = Path::new(
        args.data_directory
            .as_deref()
            .unwrap_or(config.data_directory.as_deref().unwrap_or("data")),
    );

    std::fs::create_dir_all(data_directory).context("creating data directory")?;

    let data_directory = data_directory.to_path_buf();

    let ctx = Arc::new(RunContext {
        bus,
        config,
        crypto,
        data_directory,
    });

    let history = History::build(&ctx)?;

    let rest_api_ctx = RestApiRunContext {
        ctx: ctx.clone(),
        metrics_layer,
        history: history.share(),
    };

    let mut handler = ModulesHandler::default();
    handler.build_module::<Mempool>(ctx.clone())?;
    handler.build_module::<NodeState>(ctx.clone())?;
    handler.build_module::<Consensus>(ctx.clone())?;
    handler.build_module::<P2P>(ctx.clone())?;
    handler.build_module::<MockWorkflowHandler>(ctx.clone())?;
    handler.build_module::<RestApi>(rest_api_ctx)?;

    handler.add_module(history, ctx.clone())?;

    if let Err(e) = handler.start_modules().await {
        error!("Error in module handler: {}", e)
    }

    Ok(())
}
