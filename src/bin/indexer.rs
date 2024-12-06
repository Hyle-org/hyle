use anyhow::{Context, Result};
use axum::Router;
use axum_otel_metrics::HttpMetricsLayerBuilder;
use clap::Parser;
use hydentity::Hydentity;
use hyle::{
    bus::{metrics::BusMetrics, SharedMessageBus},
    indexer::{
        contract_state_indexer::{ContractStateIndexer, ContractStateIndexerCtx},
        da_listener::{DAListener, DAListenerCtx},
        Indexer,
    },
    model::{BlockHeight, CommonRunContext},
    rest::{RestApi, RestApiRunContext},
    utils::{
        conf,
        logger::{setup_tracing, TracingMode},
        modules::{Module, ModulesHandler},
    },
};
use hyllar::HyllarToken;
use std::sync::{Arc, Mutex};
use tracing::{error, info};

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

    handler
        .build_module::<ContractStateIndexer<HyllarToken>>(ContractStateIndexerCtx {
            contract_name: "hyllar".into(),
            common: ctx.clone(),
        })
        .await?;
    handler
        .build_module::<ContractStateIndexer<HyllarToken>>(ContractStateIndexerCtx {
            contract_name: "hyllar2".into(),
            common: ctx.clone(),
        })
        .await?;
    handler
        .build_module::<ContractStateIndexer<Hydentity>>(ContractStateIndexerCtx {
            contract_name: "hydentity".into(),
            common: ctx.clone(),
        })
        .await?;

    let indexer = Indexer::build(ctx.clone()).await?;
    //let last_block: Option<BlockHeight> = None;
    let last_block = indexer.get_last_block().await?;
    handler.add_module(indexer)?;

    handler
        .build_module::<DAListener>(DAListenerCtx {
            common: ctx.clone(),
            start_block: last_block.map(|b| b + 1).unwrap_or(BlockHeight(0)),
        })
        .await?;

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
