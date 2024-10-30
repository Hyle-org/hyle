use anyhow::{Context, Result};
use axum::Router;
use axum_otel_metrics::HttpMetricsLayerBuilder;
use clap::Parser;
use hyle::{
    bus::{metrics::BusMetrics, SharedMessageBus},
    consensus::Consensus,
    data_availability::DataAvailability,
    indexer::Indexer,
    mempool::Mempool,
    model::{CommonRunContext, NodeRunContext, SharedRunContext},
    p2p::P2P,
    rest::{NodeInfo, RestApi, RestApiRunContext},
    tools::mock_workflow::MockWorkflowHandler,
    utils::{
        conf,
        crypto::BlstCrypto,
        logger::{setup_tracing, TracingMode},
        modules::{Module, ModulesHandler},
    },
};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub client: Option<bool>,

    #[arg(long, default_value =  None)]
    pub data_directory: Option<String>,

    #[arg(long)]
    pub run_indexer: Option<bool>,

    #[arg(long, default_value = "config.ron")]
    pub config_file: Option<String>,
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

    let args = Args::parse();
    let config = Arc::new(
        conf::Conf::new(args.config_file, args.data_directory, args.run_indexer)
            .context("reading config file")?,
    );

    setup_tracing(
        match config.log_format.as_str() {
            "json" => TracingMode::Json,
            "node" => TracingMode::NodeName,
            _ => TracingMode::Full,
        },
        config.id.clone(),
    )?;

    info!("Starting node with config: {:?}", &config);

    debug!("server mode");

    // Init global metrics meter we expose as an endpoint
    let metrics_layer = HttpMetricsLayerBuilder::new()
        .with_service_name(config.id.clone())
        .build();

    let bus = SharedMessageBus::new(BusMetrics::global(config.id.clone()));
    let crypto = Arc::new(BlstCrypto::new(config.id.clone()));
    let pubkey = Some(crypto.validator_pubkey().clone());

    std::fs::create_dir_all(&config.data_directory).context("creating data directory")?;

    let run_indexer = config.run_indexer;

    let ctx = SharedRunContext {
        common: CommonRunContext {
            bus: bus.new_handle(),
            config: config.clone(),
            router: Mutex::new(Some(Router::new())),
        }
        .into(),
        node: NodeRunContext { crypto }.into(),
    };

    let mut handler = ModulesHandler::default();
    handler.build_module::<Mempool>(ctx.clone()).await?;
    handler.build_module::<Consensus>(ctx.clone()).await?;
    handler.build_module::<P2P>(ctx.clone()).await?;
    handler
        .build_module::<MockWorkflowHandler>(ctx.clone())
        .await?;

    if run_indexer {
        let indexer = Indexer::build(ctx.common.clone()).await?;
        handler.add_module(indexer)?;
    }
    handler
        .build_module::<DataAvailability>(ctx.clone())
        .await?;

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
            info: NodeInfo {
                id: config.id.clone(),
                pubkey,
                da_address: config.da_address.clone(),
            },
            bus: ctx.common.bus.new_handle(),
            metrics_layer,
            router: router.clone(),
        })
        .await?;

    let (running_modules, abort) = handler.start_modules()?;

    #[cfg(unix)]
    {
        use tokio::signal::unix;
        let mut terminate = unix::signal(unix::SignalKind::interrupt())?;
        tokio::select! {
            Err(e) = running_modules => {
                error!("Error running modules: {:?}", e);
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down");
                abort();
            }
            _ = terminate.recv() =>  {
                info!("SIGTERM received, shutting down");
                abort();
            }
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            Err(e) = running_modules => {
                error!("Error running modules: {:?}", e);
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down");
                abort();
            }
        }
    }

    Ok(())
}
