use anyhow::{Context, Result};
use axum::Router;
use axum_otel_metrics::HttpMetricsLayerBuilder;
use clap::Parser;
use hyle::{
    bus::{metrics::BusMetrics, SharedMessageBus},
    consensus::Consensus,
    indexer::Indexer,
    mempool::Mempool,
    model::{CommonRunContext, NodeRunContext, SharedRunContext},
    node_state::NodeState,
    p2p::P2P,
    rest::{RestApi, RestApiRunContext},
    tools::mock_workflow::MockWorkflowHandler,
    utils::{
        conf::{self},
        crypto::BlstCrypto,
        modules::{Module, ModulesHandler},
    },
    validator_registry::ValidatorRegistry,
};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, level_filters::LevelFilter};
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub client: Option<bool>,

    #[arg(long, default_value =  None)]
    pub data_directory: Option<String>,

    #[arg(long, action = clap::ArgAction::SetTrue)]
    pub run_indexer: Option<bool>,

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
        if !var.contains("risc0_zkvm") {
            filter = filter.add_directive("risc0_zkvm=info".parse()?);
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

#[cfg(feature = "dhat")]
#[global_allocator]
/// Use dhat to profile memory usage
static ALLOC: dhat::Alloc = dhat::Alloc;

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(feature = "dhat")]
    let _profiler = {
        info!("Running with dhat memory profiler");
        dhat::Profiler::new_heap()
    };

    setup_tracing()?;

    let args = Args::parse();
    let config = conf::Conf::new_shared(args.config_file, args.data_directory, args.run_indexer)
        .context("reading config file")?;
    info!("Starting node with config: {:?}", &config);

    debug!("server mode");

    // Init global metrics meter we expose as an endpoint
    let metrics_layer = HttpMetricsLayerBuilder::new()
        .with_service_name(config.id.to_string().clone())
        .build();

    let bus = SharedMessageBus::new(BusMetrics::global(config.id.0.clone()));
    let crypto = Arc::new(BlstCrypto::new(config.id.clone())); // TODO load sk from disk instead of random

    std::fs::create_dir_all(&config.data_directory).context("creating data directory")?;

    let validator_registry = ValidatorRegistry::new(crypto.as_validator());

    let run_indexer = config.run_indexer;

    let ctx = SharedRunContext {
        common: CommonRunContext {
            bus: bus.new_handle(),
            config: config.clone(),
            router: Mutex::new(Some(Router::new())),
        }
        .into(),
        node: NodeRunContext {
            crypto,
            validator_registry,
        }
        .into(),
    };

    let mut handler = ModulesHandler::default();
    handler.build_module::<Mempool>(ctx.clone()).await?;
    handler.build_module::<NodeState>(ctx.clone()).await?;
    handler.build_module::<Consensus>(ctx.clone()).await?;
    handler.build_module::<P2P>(ctx.clone()).await?;
    handler
        .build_module::<MockWorkflowHandler>(ctx.clone())
        .await?;

    if run_indexer {
        let indexer = Indexer::build(ctx.common.clone()).await?;
        handler.add_module(indexer)?;
    }

    // Should come last so the other modules have nested their own routes.
    let router = ctx
        .common
        .router
        .lock()
        .expect("Context router should be available")
        .take()
        .expect("Context router should be available");
    handler
        .build_module::<RestApi>(RestApiRunContext {
            rest_addr: ctx.common.config.rest.clone(),
            bus: ctx.common.bus.new_handle(),
            metrics_layer,
            router: router.clone(),
        })
        .await?;

    let (running_modules, abort) = handler.start_modules()?;

    tokio::select! {
        Err(e) = running_modules => {
            error!("Error running modules: {:?}", e);
        }
        _ = tokio::signal::ctrl_c() => {
            info!("Ctrl-C received, shutting down");
            abort();
        }
    }

    Ok(())
}
