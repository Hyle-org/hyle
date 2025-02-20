use anyhow::{bail, Context, Result};
use axum::Router;
use axum_otel_metrics::HttpMetricsLayerBuilder;
use clap::Parser;
use hydentity::Hydentity;
use hyle::{
    bus::{metrics::BusMetrics, SharedMessageBus},
    consensus::Consensus,
    data_availability::DataAvailability,
    genesis::Genesis,
    indexer::{
        contract_state_indexer::{ContractStateIndexer, ContractStateIndexerCtx},
        Indexer,
    },
    mempool::Mempool,
    model::{api::NodeInfo, CommonRunContext, NodeRunContext, SharedRunContext},
    node_state::module::NodeStateModule,
    p2p::P2P,
    rest::{ApiDoc, RestApi, RestApiRunContext},
    single_node_consensus::SingleNodeConsensus,
    tcp_server::TcpServer,
    tools::mock_workflow::MockWorkflowHandler,
    utils::{
        conf,
        crypto::BlstCrypto,
        logger::{setup_tracing, TracingMode},
        modules::ModulesHandler,
    },
};
use hyllar::Hyllar;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ImageExt},
};
use tracing::{error, info, warn};
use utoipa::OpenApi;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    #[arg(short, long, action = clap::ArgAction::SetTrue)]
    pub client: Option<bool>,

    #[arg(long, default_value =  None)]
    pub data_directory: Option<String>,

    #[arg(long)]
    pub run_indexer: Option<bool>,

    #[clap(long, action)]
    pub pg: bool,

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
    let mut config = conf::Conf::new(args.config_file, args.data_directory, args.run_indexer)
        .context("reading config file")?;

    let crypto = Arc::new(BlstCrypto::new(config.id.clone()).context("Could not create crypto")?);
    let pubkey = Some(crypto.validator_pubkey().clone());

    setup_tracing(
        match config.log_format.as_str() {
            "json" => TracingMode::Json,
            "node" => TracingMode::NodeName,
            _ => TracingMode::Full,
        },
        format!(
            "{}({})",
            config.id.clone(),
            pubkey.clone().unwrap_or_default()
        ),
    )?;

    let pg;
    if args.pg {
        if std::fs::metadata(&config.data_directory).is_ok() {
            bail!(
                "Data directory {} exists. --pg flag is given, please clean data dir first.",
                config.data_directory.display()
            );
        }
        info!("🐘 Starting postgres DB with default settings for the indexer");
        pg = Postgres::default()
            .with_tag("17-alpine")
            .with_cmd(["postgres", "-c", "log_statement=all"])
            .start()
            .await?;

        config.database_url = format!(
            "postgres://postgres:postgres@localhost:{}/postgres",
            pg.get_host_port_ipv4(5432).await?
        );
        config.run_indexer = true;
    }

    let config = Arc::new(config);

    info!("Starting node with config: {:?}", &config);

    // Init global metrics meter we expose as an endpoint
    let provider = opentelemetry_sdk::metrics::SdkMeterProvider::builder()
        .with_reader(
            opentelemetry_prometheus::exporter()
                .with_registry(prometheus::default_registry().clone())
                .build()
                .context("starting prometheus exporter")?,
        )
        .build();

    opentelemetry::global::set_meter_provider(provider.clone());

    let metrics_layer = HttpMetricsLayerBuilder::new()
        .with_provider(provider)
        .build();

    let bus = SharedMessageBus::new(BusMetrics::global(config.id.clone()));

    std::fs::create_dir_all(&config.data_directory).context("creating data directory")?;

    let run_indexer = config.run_indexer;
    let run_tcp_server = config.run_tcp_server;

    let ctx = SharedRunContext {
        common: CommonRunContext {
            bus: bus.new_handle(),
            config: config.clone(),
            router: Mutex::new(Some(Router::new())),
            openapi: Mutex::new(ApiDoc::openapi()),
        }
        .into(),
        node: NodeRunContext { crypto }.into(),
    };

    let mut handler = ModulesHandler::new(&bus).await;
    handler.build_module::<Mempool>(ctx.clone()).await?;

    handler.build_module::<Genesis>(ctx.clone()).await?;
    if config.single_node.unwrap_or(false) {
        handler
            .build_module::<SingleNodeConsensus>(ctx.clone())
            .await?;
    } else {
        handler.build_module::<Consensus>(ctx.clone()).await?;
    }
    handler
        .build_module::<MockWorkflowHandler>(ctx.clone())
        .await?;

    if run_indexer {
        handler.build_module::<Indexer>(ctx.common.clone()).await?;
        handler
            .build_module::<ContractStateIndexer<Hyllar>>(ContractStateIndexerCtx {
                contract_name: "hyllar".into(),
                common: ctx.common.clone(),
            })
            .await?;
        handler
            .build_module::<ContractStateIndexer<Hyllar>>(ContractStateIndexerCtx {
                contract_name: "hyllar2".into(),
                common: ctx.common.clone(),
            })
            .await?;
        handler
            .build_module::<ContractStateIndexer<Hydentity>>(ContractStateIndexerCtx {
                contract_name: "hydentity".into(),
                common: ctx.common.clone(),
            })
            .await?;
    }
    handler
        .build_module::<DataAvailability>(ctx.clone())
        .await?;
    handler
        .build_module::<NodeStateModule>(ctx.common.clone())
        .await?;

    handler.build_module::<P2P>(ctx.clone()).await?;

    // Should come last so the other modules have nested their own routes.
    #[allow(clippy::expect_used, reason = "Fail on misconfiguration")]
    let router = ctx
        .common
        .router
        .lock()
        .expect("Context router should be available")
        .take()
        .expect("Context router should be available");
    #[allow(clippy::expect_used, reason = "Fail on misconfiguration")]
    let openapi = ctx
        .common
        .openapi
        .lock()
        .expect("OpenAPI should be available")
        .clone();
    handler
        .build_module::<RestApi>(RestApiRunContext {
            rest_addr: ctx.common.config.rest.clone(),
            max_body_size: ctx.common.config.rest_max_body_size,
            info: NodeInfo {
                id: config.id.clone(),
                pubkey,
                da_address: config.da_address.clone(),
            },
            bus: ctx.common.bus.new_handle(),
            metrics_layer: Some(metrics_layer),
            router: router.clone(),
            openapi,
        })
        .await?;

    if run_tcp_server {
        handler.build_module::<TcpServer>(ctx.clone()).await?;
    }

    #[cfg(unix)]
    {
        use tokio::signal::unix;
        let mut interrupt = unix::signal(unix::SignalKind::interrupt())?;
        let mut terminate = unix::signal(unix::SignalKind::terminate())?;
        tokio::select! {
            Err(e) = handler.start_modules() => {
                error!("Error running modules: {:?}", e);
            }
            _ = interrupt.recv() =>  {
                info!("SIGINT received, shutting down");
            }
            _ = terminate.recv() =>  {
                info!("SIGTERM received, shutting down");
            }
        }
        _ = handler.shutdown_modules(Duration::from_secs(3)).await;
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            Err(e) = handler.start_modules() => {
                error!("Error running modules: {:?}", e);
            }
            _ = tokio::signal::ctrl_c() => {
                info!("Ctrl-C received, shutting down");
            }
        }
        _ = handler.shutdown_modules(Duration::from_secs(3)).await;
    }

    if args.pg {
        warn!("--pg option given. Postgres server will stop. Cleaning data dir");
        std::fs::remove_dir_all(&config.data_directory).context("removing data directory")?;
    }

    Ok(())
}
