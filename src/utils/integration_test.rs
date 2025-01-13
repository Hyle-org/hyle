#![cfg(test)]

use std::any::TypeId;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use anyhow::{bail, Context, Result};
use axum::Router;
use tracing::info;

use crate::bus::metrics::BusMetrics;
use crate::bus::{bus_client, BusClientReceiver, SharedMessageBus};
use crate::consensus::Consensus;
use crate::data_availability::{DataAvailability, DataEvent};
use crate::genesis::Genesis;
use crate::handle_messages;
use crate::indexer::Indexer;
use crate::mempool::Mempool;
use crate::model::{CommonRunContext, NodeRunContext, SharedRunContext};
use crate::node_state::module::NodeStateModule;
use crate::p2p::P2P;
use crate::single_node_consensus::SingleNodeConsensus;
use crate::utils::conf::Conf;
use crate::utils::crypto::BlstCrypto;
use crate::utils::modules::signal::ShutdownModule;
use crate::utils::modules::ModulesHandler;

use super::modules::{module_bus_client, Module};

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
        // Have to wait forever as the module handler doesn't like exiting modules
        // TODO: fix this?
        handle_messages! {
            on_bus self.bus,
            listen<ShutdownModule> shutdown_event => {
                if shutdown_event.module == std::any::type_name::<Self>() {
                    info!("MockModule received shutdown event");
                    break;
                }
            }
            else => {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await
            }
        }
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
        let crypto = BlstCrypto::new("test".to_owned());
        let conf = Conf::new(
            None,
            tmpdir.path().to_str().map(|s| s.to_owned()),
            Some(false),
        )
        .expect("conf ok");

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

        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        let node_task = Some(tokio::spawn(async move {
            tokio::select! {
                res = node_modules.start_modules() => {
                    res
                }
                Ok(_) = rx => {
                    info!("Node shutdown requested");
                    let _ = node_modules.shutdown_modules(std::time::Duration::from_secs(2)).await;
                    Ok(())
                }
            }
        }));

        // Micro-wait to start things off
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;

        Ok(NodeIntegrationCtx {
            tmpdir: self.tmpdir,
            conf,
            bus: self.bus.new_handle(),
            crypto: self.crypto,
            node_task,
            shutdown_tx: Some(tx),
            bus_client: IntegrationBusClient::new_from_bus(self.bus).await,
        })
    }
}

bus_client! {
struct IntegrationBusClient {
    receiver(DataEvent),
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
        reg_ctx: &<T as Module>::Context,
        mocks: &mut HashMap<TypeId, MockBuilder>,
    ) -> Result<()>
    where
        T: Module + 'static + Send,
        <T as Module>::Context: Send + Clone,
    {
        if let Some(mock) = mocks.remove(&TypeId::of::<T>()) {
            mock(handler, ctx).await
        } else {
            handler.build_module::<T>(reg_ctx.clone()).await
        }
    }

    async fn start_node(
        config: Arc<Conf>,
        bus: SharedMessageBus,
        crypto: BlstCrypto,
        mut mocks: HashMap<TypeId, MockBuilder>,
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

        Self::build_module::<Mempool>(&mut handler, &ctx, &ctx, &mut mocks).await?;

        Self::build_module::<Genesis>(&mut handler, &ctx, &ctx, &mut mocks).await?;

        if config.single_node.unwrap_or(false) {
            Self::build_module::<SingleNodeConsensus>(&mut handler, &ctx, &ctx, &mut mocks).await?;
        } else {
            Self::build_module::<Consensus>(&mut handler, &ctx, &ctx, &mut mocks).await?;
        }

        if run_indexer {
            Self::build_module::<Indexer>(&mut handler, &ctx, &ctx.common, &mut mocks).await?;
        }

        Self::build_module::<DataAvailability>(&mut handler, &ctx, &ctx, &mut mocks).await?;
        Self::build_module::<NodeStateModule>(&mut handler, &ctx.common, &ctx, &mut mocks).await?;

        Self::build_module::<P2P>(&mut handler, &ctx, &ctx, &mut mocks).await?;

        // Ensure we didn't pass a Mock we didn't use
        if !mocks.is_empty() {
            bail!("Mock didn't get used: {:?}", mocks.keys());
        }

        Ok(handler)
    }

    pub async fn wait_for_processed_genesis(&mut self) -> Result<()> {
        let _: DataEvent = self.bus_client.recv().await?;
        Ok(())
    }
}
