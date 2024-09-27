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
    p2p::P2P,
    rest::{RestApi, RestApiRunContext},
    tools::mock_workflow::MockWorkflowHandler,
    utils::{
        conf,
        crypto::BlstCrypto,
        modules::{Module, ModulesHandler},
    },
    validator_registry::ValidatorRegistry,
};
use std::sync::Arc;
use tracing::{debug, error, info, level_filters::LevelFilter};
use tracing_subscriber::{prelude::*, EnvFilter};

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
    let config = conf::Conf::new_shared(args.config_file, args.data_directory)
        .context("reading config file")?;
    info!("Starting node with config: {:?}", &config);

    debug!("server mode");

    // Init global metrics meter we expose as an endpoint
    let metrics_layer = HttpMetricsLayerBuilder::new()
        .with_service_name(config.id.to_string().clone())
        .build();

    let bus = SharedMessageBus::new();
    let crypto = Arc::new(BlstCrypto::new(config.id.clone())); // TODO load sk from disk instead of random

    std::fs::create_dir_all(&config.data_directory).context("creating data directory")?;

    let validator_registry = ValidatorRegistry::new();

    let ctx = Arc::new(RunContext {
        bus,
        config,
        crypto,
        validator_registry,
    });

    let history = History::build(&ctx).await?;

    let rest_api_ctx = RestApiRunContext {
        ctx: ctx.clone(),
        metrics_layer,
        history: history.share(),
    };

    let mut handler = ModulesHandler::default();
    handler.build_module::<Mempool>(ctx.clone()).await?;
    handler.build_module::<NodeState>(ctx.clone()).await?;
    handler.build_module::<Consensus>(ctx.clone()).await?;
    handler.build_module::<P2P>(ctx.clone()).await?;
    handler
        .build_module::<MockWorkflowHandler>(ctx.clone())
        .await?;
    handler.build_module::<RestApi>(rest_api_ctx).await?;

    handler.add_module(history, ctx.clone())?;

    if let Err(e) = handler.start_modules().await {
        error!("Error in module handler: {}", e)
    }

    Ok(())
}
