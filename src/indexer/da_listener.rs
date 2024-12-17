use std::sync::Arc;

use anyhow::{bail, Context, Error, Result};
use bytes::BytesMut;
use futures::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, info, warn};

use crate::{
    bus::BusClientSender,
    data_availability::DataEvent,
    model::{Block, BlockHeight, CommonRunContext},
    module_handle_messages,
    utils::{
        logger::LogMe,
        modules::{module_bus_client, Module},
    },
};

module_bus_client! {
#[derive(Debug)]
struct DAListenerBusClient {
    sender(DataEvent),
}
}

/// Module that listens to the data availability stream and sends the blocks to the bus
pub struct DAListener {
    da_stream: Framed<TcpStream, LengthDelimitedCodec>,
    bus: DAListenerBusClient,
}

pub struct DAListenerCtx {
    pub common: Arc<CommonRunContext>,
    pub start_block: BlockHeight,
}

impl Module for DAListener {
    type Context = DAListenerCtx;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let da_stream = connect_to(&ctx.common.config.da_address, ctx.start_block).await?;
        let bus = DAListenerBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        Ok(DAListener { da_stream, bus })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DAListener {
    pub async fn start(&mut self) -> Result<(), Error> {
        module_handle_messages! {
            on_bus self.bus,
            frame = self.da_stream.next() => {
                if let Some(Ok(bytes)) = frame {
                    _ = self.processing_next_frame(bytes).await.log_error("Consuming da stream");
                } else if frame.is_none() {
                    bail!("DA stream closed");
                } else if let Some(Err(e)) = frame {
                    bail!("Error while reading DA stream: {}", e);
                }
            }
        }
        Ok(())
    }

    async fn processing_next_frame(&mut self, bytes: BytesMut) -> Result<()> {
        // FIXME: Should be signed blocks
        let block: Block = bincode::decode_from_slice(&bytes, bincode::config::standard())
            .log_error("Decoding block")?
            .0;
        if let Err(e) = self.handle_processed_block(block) {
            error!("Error while handling block: {:#}", e);
        }
        SinkExt::<bytes::Bytes>::send(&mut self.da_stream, "ok".into())
            .await
            .context("Sending ok answer on da stream")
    }

    fn handle_processed_block(&mut self, block: Block) -> Result<()> {
        info!("Received block in DA listener");
        self.bus
            .send(DataEvent::NewBlock(Box::new(block.clone())))?;
        Ok(())
    }
}

pub async fn connect_to(
    target: &str,
    height: BlockHeight,
) -> Result<Framed<TcpStream, LengthDelimitedCodec>> {
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
    let mut da_stream = Framed::new(stream, LengthDelimitedCodec::new());
    info!(
        "Connected to data stream to {} on {}. Starting stream from height {}",
        &target, addr, height
    );
    // Send the start height
    let height = bincode::encode_to_vec(height.0, bincode::config::standard())?;
    da_stream.send(height.into()).await?;
    Ok(da_stream)
}
