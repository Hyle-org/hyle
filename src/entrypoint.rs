#![allow(clippy::expect_used, reason = "Fail on misconfiguration")]

use crate::{
    bus::{metrics::BusMetrics, SharedMessageBus},
    consensus::Consensus,
    data_availability::DataAvailability,
    genesis::Genesis,
    indexer::{
        contract_state_indexer::{ContractStateIndexer, ContractStateIndexerCtx},
        da_listener::{DAListener, DAListenerCtx},
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
    utils::{conf, crypto::SharedBlstCrypto, modules::ModulesHandler},
};
use anyhow::{bail, Context, Result};
use axum::Router;
use axum_otel_metrics::HttpMetricsLayerBuilder;
use hydentity::Hydentity;
use hyllar::Hyllar;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
use testcontainers_modules::{
    postgres::Postgres,
    testcontainers::{runners::AsyncRunner, ContainerAsync, ImageExt},
};
use tracing::{error, info};
use utoipa::OpenApi;

pub struct RunPg {
    data_dir: PathBuf,
    #[allow(dead_code)]
    pg: ContainerAsync<Postgres>,
}

impl RunPg {
    pub async fn new(config: &mut conf::Conf) -> Result<Self> {
        if std::fs::metadata(&config.data_directory).is_ok() {
            bail!(
                "Data directory {} exists. --pg flag is given, please clean data dir first.",
                config.data_directory.display()
            );
        }

        info!("🐘 Starting postgres DB with default settings for the indexer");
        let pg = Postgres::default()
            .with_tag("17-alpine")
            .with_cmd(["postgres", "-c", "log_statement=all"])
            .start()
            .await?;

        std::thread::sleep(std::time::Duration::from_secs(3));

        config.database_url = format!(
            "postgres://postgres:postgres@localhost:{}/postgres",
            pg.get_host_port_ipv4(5432).await?
        );
        config.run_indexer = true;

        Ok(Self {
            pg,
            data_dir: config.data_directory.clone(),
        })
    }
}

impl Drop for RunPg {
    fn drop(&mut self) {
        tracing::warn!("--pg option given. Postgres server will stop. Cleaning data dir");
        if let Err(e) = std::fs::remove_dir_all(&self.data_dir) {
            error!("Error cleaning data dir: {:?}", e);
        }
    }
}

pub async fn main_loop(config: conf::Conf, crypto: Option<SharedBlstCrypto>) -> Result<()> {
    let mut handler = common_main(config, crypto).await?;
    handler.exit_loop().await?;

    Ok(())
}

pub async fn main_process(config: conf::Conf, crypto: Option<SharedBlstCrypto>) -> Result<()> {
    let mut handler = common_main(config, crypto).await?;
    handler.exit_process().await?;

    Ok(())
}

async fn common_main(
    config: conf::Conf,
    crypto: Option<SharedBlstCrypto>,
) -> Result<ModulesHandler> {
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

    let common_run_ctx = Arc::new(CommonRunContext {
        bus: bus.new_handle(),
        config: config.clone(),
        router: Mutex::new(Some(Router::new())),
        openapi: Mutex::new(ApiDoc::openapi()),
    });

    let mut handler = ModulesHandler::new(&bus).await;

    if config.run_indexer {
        handler
            .build_module::<Indexer>(common_run_ctx.clone())
            .await?;
        handler
            .build_module::<ContractStateIndexer<Hyllar>>(ContractStateIndexerCtx {
                contract_name: "hyllar".into(),
                common: common_run_ctx.clone(),
            })
            .await?;
        handler
            .build_module::<ContractStateIndexer<Hyllar>>(ContractStateIndexerCtx {
                contract_name: "hyllar2".into(),
                common: common_run_ctx.clone(),
            })
            .await?;
        handler
            .build_module::<ContractStateIndexer<Hydentity>>(ContractStateIndexerCtx {
                contract_name: "hydentity".into(),
                common: common_run_ctx.clone(),
            })
            .await?;
    }

    if config.p2p.mode != conf::P2pMode::None {
        let ctx = SharedRunContext {
            common: common_run_ctx.clone(),
            node: NodeRunContext {
                crypto: crypto
                    .as_ref()
                    .expect("Crypto must be defined to run p2p")
                    .clone(),
            }
            .into(),
        };

        handler
            .build_module::<NodeStateModule>(common_run_ctx.clone())
            .await?;

        handler
            .build_module::<DataAvailability>(ctx.clone())
            .await?;

        handler.build_module::<Mempool>(ctx.clone()).await?;

        handler.build_module::<Genesis>(ctx.clone()).await?;

        if config.p2p.mode == conf::P2pMode::FullValidator {
            if config.consensus.solo {
                handler
                    .build_module::<SingleNodeConsensus>(ctx.clone())
                    .await?;
            } else {
                handler.build_module::<Consensus>(ctx.clone()).await?;
            }
        }

        handler
            .build_module::<MockWorkflowHandler>(ctx.clone())
            .await?;

        handler.build_module::<P2P>(ctx.clone()).await?;
    } else {
        handler
            .build_module::<DAListener>(DAListenerCtx {
                common: common_run_ctx.clone(),
                start_block: None,
            })
            .await?;
    }

    if config.run_rest_server {
        // Should come last so the other modules have nested their own routes.
        let router = common_run_ctx
            .router
            .lock()
            .expect("Context router should be available")
            .take()
            .expect("Context router should be available");
        let openapi = common_run_ctx
            .openapi
            .lock()
            .expect("OpenAPI should be available")
            .clone();

        handler
            .build_module::<RestApi>(RestApiRunContext {
                rest_addr: config.rest_address.clone(),
                max_body_size: config.rest_max_body_size,
                info: NodeInfo {
                    id: config.id.clone(),
                    pubkey: crypto.as_ref().map(|c| c.validator_pubkey()).cloned(),
                    da_address: config.da_address.clone(),
                },
                bus: common_run_ctx.bus.new_handle(),
                metrics_layer: Some(metrics_layer),
                router: router.clone(),
                openapi,
            })
            .await?;
    }

    if config.run_tcp_server {
        handler
            .build_module::<TcpServer>(common_run_ctx.clone())
            .await?;
    }

    _ = handler.start_modules().await;

    Ok(handler)
}
