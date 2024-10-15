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
        modules::{Module, ModulesHandler},
    },
};
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info, level_filters::LevelFilter};
use tracing_subscriber::{prelude::*, EnvFilter};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(long, default_value = "config.ron")]
    pub config_file: Option<String>,
}

/// Setup tracing - stdout and tokio-console subscriber
/// stdout defaults to INFO & sled to INFO even if RUST_LOG is set to e.g. debug (unless it contains "sled")
fn setup_tracing() -> Result<()> {
    let mut filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env()?;

    let var = std::env::var("RUST_LOG").unwrap_or("".to_string());
    if !var.contains("sled") {
        filter = filter.add_directive("sled=info".parse()?);
    }
    if !var.contains("tower_http") {
        // API request/response debug tracing
        filter = filter.add_directive("tower_http::trace=debug".parse()?);
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
    let config = Arc::new(
        conf::Conf::new(args.config_file, None, Some(true)).context("reading config file")?,
    );

    info!("Starting node with config: {:?}", &config);

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
