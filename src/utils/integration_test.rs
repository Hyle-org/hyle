#![cfg(test)]

use std::any::TypeId;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use axum::Router;
use client_sdk::rest_client::NodeApiHttpClient;
use hyle_model::api::NodeInfo;
use hyle_model::TxHash;
use tracing::info;

use crate::bus::metrics::BusMetrics;
use crate::bus::{bus_client, BusClientReceiver, SharedMessageBus};
use crate::consensus::Consensus;
use crate::data_availability::DataAvailability;
use crate::genesis::{Genesis, GenesisEvent};
use crate::indexer::Indexer;
use crate::mempool::Mempool;
use crate::model::{CommonRunContext, NodeRunContext, SharedRunContext};
use crate::module_handle_messages;
use crate::node_state::module::{NodeStateEvent, NodeStateModule};
use crate::p2p::P2P;
use crate::rest::{RestApi, RestApiRunContext};
use crate::single_node_consensus::SingleNodeConsensus;
use crate::tcp_server::TcpServer;
use crate::utils::conf::Conf;
use crate::utils::crypto::BlstCrypto;
use crate::utils::modules::ModulesHandler;

use super::modules::{module_bus_client, Module};

// Assume that we can reuse the OS-provided port.
pub async fn find_available_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    addr.port()
}

type MockBuilder = Box<
    dyn for<'a> FnOnce(
        &'a mut ModulesHandler,
        &'a SharedRunContext,
    ) -> Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>>,
>;

module_bus_client! {
struct MockModuleBusClient {
}
}
// Generic as ModulesHandler uses the type ID, so we need different mock types for different modules.
struct MockModule<T> {
    bus: MockModuleBusClient,
    _t: std::marker::PhantomData<T>,
}
impl<T> MockModule<T> {
    async fn new(bus: SharedMessageBus) -> Result<Self> {
        Ok(Self {
            bus: MockModuleBusClient::new_from_bus(bus).await,
            _t: std::marker::PhantomData,
        })
    }
    async fn start(&mut self) -> Result<()> {
        module_handle_messages! {
            on_bus self.bus,
        };
        Ok(())
    }
}
impl<T: Send> Module for MockModule<T> {
    type Context = SharedRunContext;
    fn build(ctx: Self::Context) -> impl futures::Future<Output = Result<Self>> + Send {
        MockModule::new(ctx.common.bus.new_handle())
    }
    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

pub struct NodeIntegrationCtxBuilder {
    tmpdir: tempfile::TempDir,
    pub conf: Conf,
    pub bus: SharedMessageBus,
    pub crypto: BlstCrypto,
    mocks: HashMap<TypeId, MockBuilder>,
}

impl NodeIntegrationCtxBuilder {
    pub async fn new() -> Self {
        let tmpdir = tempfile::tempdir().unwrap();
        let bus = SharedMessageBus::new(BusMetrics::global("default".to_string()));
        let crypto = BlstCrypto::new("test").unwrap();
        let mut conf = Conf::new(
            None,
            tmpdir.path().to_str().map(|s| s.to_owned()),
            Some(false),
        )
        .expect("conf ok");
        conf.host = format!("localhost:{}", find_available_port().await);
        conf.da_address = format!("localhost:{}", find_available_port().await);
        conf.tcp_server_address = Some(format!("localhost:{}", find_available_port().await));
        conf.rest = format!("localhost:{}", find_available_port().await);

        Self {
            tmpdir,
            conf,
            bus,
            crypto,
            mocks: HashMap::new(),
        }
    }

    pub fn with_mock<Original: 'static, Mock>(mut self) -> Self
    where
        Mock: Module<Context = SharedRunContext> + 'static + Send,
        <Mock as Module>::Context: Send,
    {
        self.mocks.insert(
            TypeId::of::<Original>(),
            Box::new(move |handler, ctx| Box::pin(handler.build_module::<Mock>(ctx.clone()))),
        );
        self
    }

    pub fn skip<T: Send + 'static>(self) -> Self {
        self.with_mock::<T, MockModule<T>>()
    }

    pub async fn build(self) -> Result<NodeIntegrationCtx> {
        let conf = Arc::new(self.conf);
        let mut node_modules = NodeIntegrationCtx::start_node(
            conf.clone(),
            self.bus.new_handle(),
            self.crypto.clone(),
            self.mocks,
        )
        .await?;

        let bus_client = IntegrationBusClient::new_from_bus(self.bus.new_handle()).await;

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let node_task = Some(tokio::spawn(async move {
            tokio::select! {
                res = node_modules.start_modules() => {
                    res
                }
                Ok(_) = rx => {
                    info!("Node shutdown requested");
                    let _ = node_modules.shutdown_next_module().await;
                    Ok(())
                }
            }
        }));

        // Micro-wait to start things off
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;

        Ok(NodeIntegrationCtx {
            tmpdir: self.tmpdir,
            conf,
            bus: self.bus,
            crypto: self.crypto,
            node_task,
            shutdown_tx: Some(tx),
            bus_client,
        })
    }
}

bus_client! {
struct IntegrationBusClient {
    receiver(GenesisEvent),
    receiver(NodeStateEvent),
}
}

pub struct NodeIntegrationCtx {
    #[allow(dead_code)]
    tmpdir: tempfile::TempDir,
    #[allow(dead_code)]
    pub conf: Arc<Conf>,
    pub bus: SharedMessageBus,
    pub crypto: BlstCrypto,
    node_task: Option<tokio::task::JoinHandle<Result<()>>>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    bus_client: IntegrationBusClient,
}

/// Implement a custom Drop that shutdowns modules and returns, synchronously.
/// Note that in tests, this requires a multi-threaded tokio runtime.
impl Drop for NodeIntegrationCtx {
    fn drop(&mut self) {
        info!("Shutting down node");
        let Some(node_task) = self.node_task.take() else {
            return;
        };
        let Some(shutdown_tx) = self.shutdown_tx.take() else {
            return;
        };
        let _ = shutdown_tx.send(());
        let start_time = std::time::Instant::now();
        loop {
            if node_task.is_finished() {
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
    async fn build_module<T>(
        handler: &mut ModulesHandler,
        ctx: &SharedRunContext,
        reg_ctx: <T as Module>::Context,
        mocks: &mut HashMap<TypeId, MockBuilder>,
    ) -> Result<()>
    where
        T: Module + 'static + Send,
        <T as Module>::Context: Send,
    {
        if let Some(mock) = mocks.remove(&TypeId::of::<T>()) {
            mock(handler, ctx).await
        } else {
            handler.build_module::<T>(reg_ctx).await
        }
    }

    async fn start_node(
        config: Arc<Conf>,
        bus: SharedMessageBus,
        crypto: BlstCrypto,
        mut mocks: HashMap<TypeId, MockBuilder>,
    ) -> Result<ModulesHandler> {
        let crypto = Arc::new(crypto);
        let pubkey = crypto.validator_pubkey().clone();
        info!("Starting node with config: {:?}", &config);

        std::fs::create_dir_all(&config.data_directory).context("creating data directory")?;

        let run_indexer = config.run_indexer;
        let run_tcp_server = config.run_tcp_server;

        let ctx = SharedRunContext {
            common: CommonRunContext {
                bus: bus.new_handle(),
                config: config.clone(),
                router: Mutex::new(Some(Router::new())),
                openapi: Default::default(),
            }
            .into(),
            node: NodeRunContext { crypto }.into(),
        };

        let mut handler = ModulesHandler::new(&bus).await;

        Self::build_module::<Mempool>(&mut handler, &ctx, ctx.clone(), &mut mocks).await?;

        Self::build_module::<Genesis>(&mut handler, &ctx, ctx.clone(), &mut mocks).await?;

        if config.single_node.unwrap_or(false) {
            Self::build_module::<SingleNodeConsensus>(&mut handler, &ctx, ctx.clone(), &mut mocks)
                .await?;
        } else {
            Self::build_module::<Consensus>(&mut handler, &ctx, ctx.clone(), &mut mocks).await?;
        }

        if run_indexer {
            Self::build_module::<Indexer>(&mut handler, &ctx, ctx.common.clone(), &mut mocks)
                .await?;
        }

        Self::build_module::<DataAvailability>(&mut handler, &ctx, ctx.clone(), &mut mocks).await?;
        Self::build_module::<NodeStateModule>(&mut handler, &ctx, ctx.common.clone(), &mut mocks)
            .await?;

        Self::build_module::<P2P>(&mut handler, &ctx, ctx.clone(), &mut mocks).await?;

        // Should come last so the other modules have nested their own routes.
        #[allow(clippy::expect_used, reason = "Fail on misconfiguration")]
        let router = ctx
            .common
            .router
            .lock()
            .expect("Context router should be available")
            .take()
            .expect("Context router should be available");

        // Not really intended to be mocked but you can (and probably should) skip it.
        Self::build_module::<RestApi>(
            &mut handler,
            &ctx,
            RestApiRunContext {
                rest_addr: ctx.common.config.rest.clone(),
                max_body_size: ctx.common.config.rest_max_body_size,
                info: NodeInfo {
                    id: config.id.clone(),
                    pubkey: Some(pubkey),
                    da_address: config.da_address.clone(),
                },
                bus: ctx.common.bus.new_handle(),
                metrics_layer: None,
                router: router.clone(),
                openapi: Default::default(),
            },
            &mut mocks,
        )
        .await?;

        if run_tcp_server {
            Self::build_module::<TcpServer>(&mut handler, &ctx, ctx.clone(), &mut mocks).await?;
        }

        // Ensure we didn't pass a Mock we didn't use
        if !mocks.is_empty() {
            bail!("Mock didn't get used: {:?}", mocks.keys());
        }

        Ok(handler)
    }

    pub async fn wait_for_rest_api(&self, api: &NodeApiHttpClient) -> Result<()> {
        loop {
            if api.get_node_info().await.is_ok() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        Ok(())
    }
    pub async fn wait_for_genesis_event(&mut self) -> Result<()> {
        let _: GenesisEvent = self.bus_client.recv().await?;
        Ok(())
    }
    pub async fn wait_for_processed_genesis(&mut self) -> Result<()> {
        let _: NodeStateEvent = self.bus_client.recv().await?;
        Ok(())
    }
    pub async fn wait_for_n_blocks(&mut self, n: u32) -> Result<()> {
        for _ in 0..n {
            let _: NodeStateEvent = self.bus_client.recv().await?;
        }
        Ok(())
    }
    pub async fn wait_for_settled_tx(&mut self, tx: TxHash) -> Result<()> {
        loop {
            let event: NodeStateEvent = self.bus_client.recv().await?;
            let NodeStateEvent::NewBlock(block) = event;
            if block.successful_txs.iter().any(|tx_hash| tx_hash == &tx) {
                break;
            }
        }
        Ok(())
    }
}
