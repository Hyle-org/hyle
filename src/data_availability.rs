//! Minimal block storage layer for data availability.

mod blocks;

use crate::{
    bus::{bus_client, SharedMessageBus},
    consensus::ConsensusEvent,
    handle_messages,
    model::{Block, BlockHeight, SharedRunContext},
    utils::modules::Module,
};
use anyhow::{Context, Result};
use axum::{
    extract::{Query, State},
    Router,
};
use blocks::Blocks;
use core::str;
use futures::{SinkExt, StreamExt};
use reqwest::StatusCode;
use std::io::{Cursor, Write};
use tokio::{net::TcpStream, task::JoinSet};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{error, info};

pub fn u64_to_str(u: u64, buf: &mut [u8]) -> &str {
    let mut cursor = Cursor::new(&mut buf[..]);
    _ = write!(cursor, "{}", u);
    let len = cursor.position() as usize;
    str::from_utf8(&buf[..len]).unwrap()
}

#[derive(Debug, serde::Deserialize)]
struct StreamRequest {
    start_height: u64,
    peer_ip: String,
}

async fn api_stream_request(
    Query(filters): Query<StreamRequest>,
    State(state): State<tokio::sync::mpsc::Sender<StreamRequest>>,
) -> Result<(), StatusCode> {
    if let Err(err) = state.send(filters).await {
        error!("Error sending stream request: {}", err);
        return Err(StatusCode::INTERNAL_SERVER_ERROR);
    }
    info!("stream request sent");
    Ok(())
}

bus_client! {
#[derive(Debug)]
struct DABusClient {
    receiver(ConsensusEvent),
}
}

#[derive(Debug)]
pub struct DataAvailability {
    bus: DABusClient,
    pub blocks: Blocks,
    request_receiver: tokio::sync::mpsc::Receiver<StreamRequest>,
    streams: JoinSet<Option<(BlockHeight, Framed<TcpStream, LengthDelimitedCodec>)>>,
}

impl Module for DataAvailability {
    fn name() -> &'static str {
        "DataAvailability"
    }

    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = DABusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        let db = sled::Config::new()
            .use_compression(true)
            .compression_factor(15)
            .path(
                ctx.common
                    .config
                    .data_directory
                    .join("data_availability.db"),
            )
            .open()
            .context("opening the database")?;

        let (tx, rx) = tokio::sync::mpsc::channel(100);

        let mut ctx_router = ctx
            .common
            .router
            .lock()
            .expect("Context router should be available");
        let router = Router::new()
            .route(
                "/v1/da/stream_request",
                axum::routing::get(api_stream_request),
            )
            .with_state(tx)
            .merge(ctx_router.take().expect("Router should be available"));
        let _ = ctx_router.insert(router);

        Ok(DataAvailability {
            bus,
            blocks: Blocks::new(&db)?,
            request_receiver: rx,
            streams: JoinSet::new(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DataAvailability {
    pub async fn start(&mut self) -> Result<()> {
        handle_messages! {
            on_bus self.bus,
            listen<ConsensusEvent> cmd => {
                self.handle_consensus_event(cmd).await;
            }

            cmd = self.request_receiver.recv() => {
                if let Some(cmd) = cmd {
                    self.handle_stream_request(cmd.start_height, &cmd.peer_ip).await?;
                }
            }

            Some(cmd) = self.streams.join_next() => {
                if let Ok(Some((height, mut framed))) = cmd {
                    let item = self.blocks.get(height);
                    if let Ok(Some(item)) = item {
                        info!("streaming block {}", height);
                        let bytes = bincode::encode_to_vec(item, bincode::config::standard())
                        .expect("Could not serialize Block");
                        framed.send(bytes.into()).await?;
                        // Spawn the next task
                        self.stream_block(height.0 + 1, framed);
                    } else {
                    // TODO: Else... For now we'll wait for a new ping
                    // Maybe we should just sleep for a bit and try again?
                    info!("block {} not found", height.0);
                    }
                }
            }
        }
    }

    async fn handle_consensus_event(&mut self, event: ConsensusEvent) {
        match event {
            ConsensusEvent::CommitBlock { block, .. } => self.handle_block(block).await,
        }
    }

    async fn handle_block(&mut self, block: Block) {
        info!("new block {} with {} txs", block.height, block.txs.len());
        // store block
        if let Err(e) = self.blocks.put(block) {
            error!("storing block: {}", e);
        }
    }

    async fn handle_stream_request(&mut self, start_height: u64, peer_ip: &str) -> Result<()> {
        let stream = TcpStream::connect(peer_ip).await?;
        info!("DA streaming listener setup on {}", stream.local_addr()?);
        let framed = Framed::new(stream, LengthDelimitedCodec::new());
        self.stream_block(start_height, framed);

        Ok(())
    }

    fn stream_block(&mut self, height: u64, mut framed: Framed<TcpStream, LengthDelimitedCodec>) {
        self.streams
            .spawn(async move { framed.next().await.map(|_| (BlockHeight(height), framed)) });
    }
}

#[cfg(test)]
mod tests {
    use crate::model::{
        Blob, BlobData, BlobTransaction, Block, BlockHash, BlockHeight, ContractName, Identity,
        Transaction, TransactionData,
    };

    use super::blocks::Blocks;
    use anyhow::Result;

    #[test]
    fn test_blocks() -> Result<()> {
        let tmpdir = tempdir::TempDir::new("history-tests")?;
        let db = sled::open(tmpdir.path().join("history"))?;
        let mut blocks = Blocks::new(&db)?;
        assert!(
            blocks.len() == 1,
            "blocks should contain genesis block after creation"
        );
        let block = Block {
            parent_hash: BlockHash {
                inner: vec![0, 1, 2, 3],
            },
            height: BlockHeight(1),
            timestamp: 42,
            txs: vec![Transaction {
                version: 1,
                transaction_data: TransactionData::Blob(BlobTransaction {
                    identity: Identity("tx_id".to_string()),
                    blobs: vec![Blob {
                        contract_name: ContractName("c1".to_string()),
                        data: BlobData(vec![4, 5, 6]),
                    }],
                }),
                inner: "tx".to_string(),
            }],
        };
        blocks.put(block.clone())?;
        assert!(blocks.last().height == block.height);
        let last = blocks.get(BlockHeight(1))?;
        assert!(last.is_some());
        assert!(last.unwrap().height == BlockHeight(1));
        Ok(())
    }
}
