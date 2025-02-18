use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

use anyhow::{bail, Error, Result};
use futures::StreamExt;
use hyle_model::Hashed;
use tracing::{debug, info};

use crate::{
    bus::BusClientSender,
    data_availability::codec::{
        DataAvailabilityClientCodec, DataAvailabilityServerEvent, DataAvailabilityServerRequest,
    },
    model::{BlockHeight, CommonRunContext},
    module_handle_messages,
    node_state::{module::NodeStateEvent, NodeState},
    tcp::{TcpClient, TcpMessage},
    utils::{
        conf::SharedConf,
        logger::LogMe,
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
    listener: RawDAListener,
}

type DaClient = TcpClient<
    DataAvailabilityClientCodec,
    DataAvailabilityServerRequest,
    DataAvailabilityServerEvent,
>;

/// Implementation of the bit that actually listens to the data availability stream
pub struct RawDAListener {
    client: DaClient,
    // da_stcream: Framed<TcpStream, DataAvailabilityClientCodec>,
}

impl Deref for RawDAListener {
    type Target = DaClient;
    fn deref(&self) -> &Self::Target {
        &self.client
    }
}
impl DerefMut for RawDAListener {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.client
    }
}

pub struct DAListenerCtx {
    pub common: Arc<CommonRunContext>,
    pub start_block: Option<BlockHeight>,
}

impl Module for DAListener {
    type Context = DAListenerCtx;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let node_state = Self::load_from_disk_or_default::<NodeState>(
            ctx.common
                .config
                .data_directory
                .join("da_listener_node_state.bin")
                .as_path(),
        );

        let start_block = ctx.start_block.unwrap_or(node_state.current_height);

        let listener = RawDAListener::new(&ctx.common.config.da_address, start_block).await?;
        let bus = DAListenerBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        for name in node_state.contracts.keys() {
            info!("📝 Loaded contract state for {}", name);
        }

        Ok(DAListener {
            config: ctx.common.config.clone(),
            listener,
            bus,
            node_state,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DAListener {
    pub async fn start(&mut self) -> Result<(), Error> {
        module_handle_messages! {
            on_bus self.bus,
            frame = self.listener.receiver.next() => {
                if let Some(Ok(TcpMessage::Data(streamed_signed_block))) = frame {
                    _ = self.processing_next_frame(streamed_signed_block).await.log_error("Consuming da stream");
                } else if frame.is_none() {
                    bail!("DA stream closed");
                } else if let Some(Err(e)) = frame {
                    bail!("Error while reading DA stream: {}", e);
                }
            }
        };
        let _ = Self::save_on_disk::<NodeState>(
            self.config
                .data_directory
                .join("da_listener_node_state.bin")
                .as_path(),
            &self.node_state,
        )
        .log_error("Saving node state");

        Ok(())
    }

    async fn processing_next_frame(&mut self, event: DataAvailabilityServerEvent) -> Result<()> {
        if let DataAvailabilityServerEvent::SignedBlock(block) = event {
            debug!(
                "📦 Received block: {} {}",
                block.consensus_proposal.slot,
                block.consensus_proposal.hashed()
            );
            let block = self.node_state.handle_signed_block(&block);
            debug!("📦 Handled block outputs: {:?}", block);

            self.bus.send(NodeStateEvent::NewBlock(Box::new(block)))?;
        }

        self.listener.ping().await?;

        Ok(())
    }
}

impl RawDAListener {
    pub async fn new(target: &str, height: BlockHeight) -> Result<Self> {
        let mut client =
            TcpClient::connect("raw_da_listener".to_string(), &target.to_string()).await?;
        client.send(DataAvailabilityServerRequest(height)).await?;
        Ok(RawDAListener { client })
    }
}
