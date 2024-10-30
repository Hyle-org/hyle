use anyhow::{Context, Result};
use axum::Router;
use axum_otel_metrics::HttpMetricsLayerBuilder;
use clap::Parser;
use hyle::{
    bus::{metrics::BusMetrics, SharedMessageBus},
    indexer::Indexer,
    model::CommonRunContext,
    rest::{RestApi, RestApiRunContext},
    utils::{
        conf,
        logger::{setup_tracing, TracingMode},
        modules::{Module, ModulesHandler},
    },
};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
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
        conf::Conf::new(args.config_file, None, Some(true)).context("reading config file")?,
    );

    setup_tracing(
        match config.log_format.as_str() {
            "json" => TracingMode::Json,
            "node" => TracingMode::NodeName,
            _ => TracingMode::Full,
        },
        config.id.clone(),
    )?;

    info!("Starting indexer with config: {:?}", &config);

    debug!("server mode");

    // Init global metrics meter we expose as an endpoint
    let metrics_layer = HttpMetricsLayerBuilder::new()
        .with_service_name(config.id.clone())
        .build();

    let bus = SharedMessageBus::new(BusMetrics::global(config.id.clone()));

    std::fs::create_dir_all(&config.data_directory).context("creating data directory")?;

    let mut handler = ModulesHandler::default();

    let ctx = Arc::new(CommonRunContext {
        bus: bus.new_handle(),
        config: config.clone(),
        router: Mutex::new(Some(Router::new())),
    });

    if config.run_indexer {
        let mut indexer = Indexer::build(ctx.clone()).await?;
        indexer.connect_to(&config.da_address).await?;
        handler.add_module(indexer)?;
    }

    // Should come last so the other modules have nested their own routes.
    let router = ctx
        .router
        .lock()
        .expect("Context router should be available")
        .take()
        .expect("Context router should be available");
    handler
        .build_module::<RestApi>(RestApiRunContext {
            rest_addr: ctx.config.rest.clone(),
            bus: ctx.bus.new_handle(),
            metrics_layer,
            router: router.clone(),
            info: hyle::rest::NodeInfo {
                id: ctx.config.id.clone(),
                da_address: ctx.config.da_address.clone(),
                pubkey: None,
            },
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
