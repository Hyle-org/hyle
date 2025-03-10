use std::sync::Arc;

use anyhow::Result;
use hyle_model::Hashed;
use tracing::{debug, info};

use crate::{
    bus::BusClientSender,
    data_availability::codec::{
        codec_data_availability, DataAvailabilityEvent, DataAvailabilityRequest,
    },
    log_error,
    model::{BlockHeight, CommonRunContext},
    module_handle_messages,
    node_state::{metrics::NodeStateMetrics, module::NodeStateEvent, NodeState, NodeStateStore},
    utils::{
        conf::SharedConf,
        modules::{module_bus_client, Module},
    },
};

module_bus_client! {
#[derive(Debug)]
struct DAListenerBusClient {
    sender(NodeStateEvent),
}
}

/// Module that listens to the data availability stream and sends the blocks to the bus
pub struct DAListener {
    config: SharedConf,
    bus: DAListenerBusClient,
    node_state: NodeState,
    start_block: BlockHeight,
}

pub struct DAListenerCtx {
    pub common: Arc<CommonRunContext>,
    pub start_block: Option<BlockHeight>,
}

impl Module for DAListener {
    type Context = DAListenerCtx;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let node_state_store = Self::load_from_disk_or_default::<NodeStateStore>(
            ctx.common
                .config
                .data_directory
                .join("da_listener_node_state.bin")
                .as_path(),
        );

        let node_state = NodeState {
            store: node_state_store,
            metrics: NodeStateMetrics::global(ctx.common.config.id.clone(), "da_listener"),
        };

        let start_block = ctx.start_block.unwrap_or(node_state.current_height);

        let bus = DAListenerBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        for name in node_state.contracts.keys() {
            info!("📝 Loaded contract state for {}", name);
        }

        Ok(DAListener {
            config: ctx.common.config.clone(),
            start_block,
            bus,
            node_state,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DAListener {
    async fn start_client(
        &self,
        block_height: BlockHeight,
    ) -> Result<codec_data_availability::Client> {
        let mut client = codec_data_availability::connect(
            "raw_da_listener".to_string(),
            self.config.da_address.to_string(),
        )
        .await?;

        client.send(DataAvailabilityRequest(block_height)).await?;

        Ok(client)
    }
    pub async fn start(&mut self) -> Result<()> {
        let mut client = self.start_client(self.start_block).await?;

        module_handle_messages! {
            on_bus self.bus,
            frame = client.recv() => {
                if let Some(streamed_signed_block) = frame {
                    log_error!(self.processing_next_frame(streamed_signed_block).await, "Consuming da stream")?;
                    client.ping().await?;
                } else {
                    client = self.start_client(self.node_state.current_height + 1).await?;
                }
            }
        };
        let _ = log_error!(
            Self::save_on_disk::<NodeStateStore>(
                self.config
                    .data_directory
                    .join("da_listener_node_state.bin")
                    .as_path(),
                &self.node_state,
            ),
            "Saving node state"
        );

        Ok(())
    }

    async fn processing_next_frame(&mut self, event: DataAvailabilityEvent) -> Result<()> {
        if let DataAvailabilityEvent::SignedBlock(block) = event {
            debug!(
                "📦 Received block: {} {}",
                block.consensus_proposal.slot,
                block.consensus_proposal.hashed()
            );
            let block = self.node_state.handle_signed_block(&block);
            debug!("📦 Handled block outputs: {:?}", block);

            self.bus.send(NodeStateEvent::NewBlock(Box::new(block)))?;
        }

        Ok(())
    }
}
