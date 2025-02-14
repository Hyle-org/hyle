use std::{
    ops::{Deref, DerefMut},
    sync::Arc,
};

use anyhow::{bail, Error, Result};
use futures::{SinkExt, StreamExt};
use hyle_model::Hashable;
use tokio::net::TcpStream;
use tokio_util::codec::Framed;
use tracing::{debug, info, warn};

use crate::{
    bus::BusClientSender,
    data_availability::codec::{
        DataAvailabilityClientCodec, DataAvailabilityServerEvent, DataAvailabilityServerRequest,
    },
    model::{BlockHeight, CommonRunContext},
    module_handle_messages,
    node_state::{module::NodeStateEvent, NodeState},
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

/// Implementation of the bit that actually listens to the data availability stream
pub struct RawDAListener {
    da_stream: Framed<TcpStream, DataAvailabilityClientCodec>,
}

impl Deref for RawDAListener {
    type Target = Framed<TcpStream, DataAvailabilityClientCodec>;
    fn deref(&self) -> &Self::Target {
        &self.da_stream
    }
}
impl DerefMut for RawDAListener {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.da_stream
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
            frame = self.listener.next() => {
                if let Some(Ok(streamed_signed_block)) = frame {
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
                block.consensus_proposal.hash()
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
        let da_stream = Self::connect_to(target, height).await?;
        Ok(RawDAListener { da_stream })
    }

    async fn ping(&mut self) -> Result<()> {
        self.da_stream
            .send(DataAvailabilityServerRequest::Ping)
            .await
    }

    async fn connect_to(
        target: &str,
        height: BlockHeight,
    ) -> Result<Framed<TcpStream, DataAvailabilityClientCodec>> {
        info!(
            "Connecting to node for data availability stream on {}",
            &target
        );
        let timeout = std::time::Duration::from_secs(10);
        let start = std::time::Instant::now();

        let stream = loop {
            debug!("Trying to connect to {}", target);
            match TcpStream::connect(&target).await {
                Ok(stream) => break stream,
                Err(e) => {
                    if start.elapsed() >= timeout {
                        bail!("Failed to connect to {}: {}. Timeout reached.", target, e);
                    }
                    warn!(
                        "Failed to connect to {}: {}. Retrying in 1 second...",
                        target, e
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        };
        let addr = stream.local_addr()?;
        let mut da_stream = Framed::new(stream, DataAvailabilityClientCodec::default());
        info!(
            "Connected to data stream to {} on {}. Starting stream from height {}",
            &target, addr, height
        );
        // Send the start height
        da_stream
            .send(DataAvailabilityServerRequest::BlockHeight(height))
            .await?;
        Ok(da_stream)
    }
}
