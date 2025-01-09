#![cfg(test)]

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::Router;
use tracing::info;

use crate::bus::metrics::BusMetrics;
use crate::bus::SharedMessageBus;
use crate::consensus::Consensus;
use crate::data_availability::DataAvailability;
use crate::indexer::Indexer;
use crate::mempool::Mempool;
use crate::model::{CommonRunContext, NodeRunContext, SharedRunContext};
use crate::p2p::P2P;
use crate::single_node_consensus::SingleNodeConsensus;
use crate::utils::conf::Conf;
use crate::utils::crypto::BlstCrypto;
use crate::utils::logger::LogMe;
use crate::utils::modules::ModulesHandler;

pub struct NodeIntegrationCtxBuilder {
    pub conf: Conf,
    pub bus: SharedMessageBus,
    pub crypto: BlstCrypto,
}

impl NodeIntegrationCtxBuilder {
    pub async fn new() -> Self {
        let tmpdir = tempfile::tempdir().unwrap().into_path();
        let bus = SharedMessageBus::new(BusMetrics::global("default".to_string()));
        let crypto = BlstCrypto::new("test".to_owned());
        let conf =
            Conf::new(None, tmpdir.to_str().map(|s| s.to_owned()), Some(false)).expect("conf ok");

        Self { conf, bus, crypto }
    }

    pub async fn build(self) -> Result<NodeIntegrationCtx> {
        let conf = Arc::new(self.conf);
        let mut node_modules = NodeIntegrationCtx::start_node(
            conf.clone(),
            self.bus.new_handle(),
            self.crypto.clone(),
        )
        .await?;

        // Just a little bit of tomfoolery
        let nm = unsafe { NodeIntegrationCtx::lifetime_transmute(&mut node_modules) };
        let starter = nm.start_modules();
        let node_task = Some(tokio::spawn(starter));

        // Wait a little bit for the node to start
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        Ok(NodeIntegrationCtx {
            conf,
            bus: self.bus,
            crypto: self.crypto,
            node_modules,
            node_task,
        })
    }
}

pub struct NodeIntegrationCtx {
    #[allow(dead_code)]
    pub conf: Arc<Conf>,
    pub bus: SharedMessageBus,
    pub crypto: BlstCrypto,
    pub node_modules: ModulesHandler,
    node_task: Option<tokio::task::JoinHandle<Result<()>>>,
}

/// Implement a custom Drop that shutdowns modules and returns, synchronously.
/// Note that in tests, this requires a multi-threaded tokio runtime.
impl Drop for NodeIntegrationCtx {
    fn drop(&mut self) {
        let this = unsafe { Self::lifetime_transmute(self) };
        let shutdown_task = tokio::spawn(async move {
            let _ = this
                .node_modules
                .shutdown_modules(std::time::Duration::from_secs(1))
                .await
                .log_error("Should shutdown modules");
            if let Some(task) = this.node_task.take() {
                let _ = task
                    .await
                    .expect("Should finish node task")
                    .log_error("Node task failed");
            };
        });
        let start_time = std::time::Instant::now();
        loop {
            if shutdown_task.is_finished() {
                break;
            }
            if start_time.elapsed().as_secs() > 5 {
                panic!("Node shutdown took too long - you probably need a multi-threaded tokio runtime");
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

impl NodeIntegrationCtx {
    async fn start_node(
        config: Arc<Conf>,
        bus: SharedMessageBus,
        crypto: BlstCrypto,
    ) -> Result<ModulesHandler> {
        let crypto = Arc::new(crypto);
        info!("Starting node with config: {:?}", &config);

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

        let mut handler = ModulesHandler::new(&bus).await;
        handler.build_module::<Mempool>(ctx.clone()).await?;

        if config.single_node.unwrap_or(false) {
            handler
                .build_module::<SingleNodeConsensus>(ctx.clone())
                .await?;
        } else {
            handler.build_module::<Consensus>(ctx.clone()).await?;
        }

        if run_indexer {
            handler.build_module::<Indexer>(ctx.common.clone()).await?;
        }
        handler
            .build_module::<DataAvailability>(ctx.clone())
            .await?;

        handler.build_module::<P2P>(ctx.clone()).await?;

        Ok(handler)
    }

    // Used to bypass lifetime restrictions when sending stuff in tokio tasks that we know is safe to send.
    unsafe fn lifetime_transmute<'b, T>(t: &'_ mut T) -> &'b mut T {
        std::mem::transmute(t)
    }
}
