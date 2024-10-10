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
use bytes::Bytes;
use core::str;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use reqwest::StatusCode;
use std::{
    io::{Cursor, Write},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::TcpStream,
    sync::Mutex,
    task::{AbortHandle, JoinSet},
    time::sleep,
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{error, info};

pub fn u64_to_str(u: u64, buf: &mut [u8]) -> &str {
    let mut cursor = Cursor::new(&mut buf[..]);
    _ = write!(cursor, "{}", u);
    let len = cursor.position() as usize;
    str::from_utf8(&buf[..len]).unwrap()
}

bus_client! {
#[derive(Debug)]
struct DABusClient {
    receiver(ConsensusEvent),
}
}

#[derive(Debug, serde::Deserialize)]
struct StreamRequest {
    start_height: u64,
    peer_ip: String,
}

type FramedByLength = Framed<TcpStream, LengthDelimitedCodec>;
type BlockSink = SplitSink<FramedByLength, Bytes>;

/// A peer we are streaming blocks to
#[derive(Debug)]
struct BlockStreamPeer {
    /// Last timestamp we received a ping from the peer.
    last_ping: Arc<AtomicU64>,
    /// Sending side of the stream
    sender: Arc<Mutex<BlockSink>>,
    /// Handle to abort the receiving side of the stream
    receiver_abort: AbortHandle,
}

#[derive(Debug)]
pub struct DataAvailability {
    bus: DABusClient,
    pub blocks: Blocks,

    // To avoid mutexing DA we use a channel to process API requests
    stream_request_receiver: tokio::sync::mpsc::Receiver<StreamRequest>,
    // List of peers subscribed to block streaming
    stream_peer_metadata: Vec<BlockStreamPeer>,
    // List of tasks streaming past blocks to peers.
    // These stop on their own once we've reached the head of the chain.
    stream_catchup_tasks: JoinSet<(BlockHeight, Arc<Mutex<BlockSink>>)>,
    // Tasks that process pings from peers to keep track of their liveness
    peer_keepalives: JoinSet<(Arc<AtomicU64>, SplitStream<FramedByLength>)>,
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
                axum::routing::get(Self::api_stream_request),
            )
            .with_state(tx)
            .merge(ctx_router.take().expect("Router should be available"));
        let _ = ctx_router.insert(router);

        Ok(DataAvailability {
            bus,
            blocks: Blocks::new(&db)?,
            stream_request_receiver: rx,
            stream_peer_metadata: Vec::new(),
            stream_catchup_tasks: JoinSet::new(),
            peer_keepalives: JoinSet::new(),
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

            cmd = self.stream_request_receiver.recv() => {
                if let Some(cmd) = cmd {
                    self.handle_stream_request(cmd.start_height, &cmd.peer_ip).await?;
                }
            }

            Some(Ok((timestamp, mut receiver))) = self.peer_keepalives.join_next() => {
                timestamp.store(SystemTime::now().duration_since(UNIX_EPOCH).expect("time went backwards").as_secs(), Ordering::Relaxed);
                // Spawn a new task waiting for a message, which we'll process here.
                // This allows us to remain logically synchronous while still processing messages asynchronously.
                self.peer_keepalives.spawn(async move {
                    sleep(Duration::from_secs(60)).await;
                    receiver.next().await;
                    (timestamp, receiver)
                });
            }

            Some(Ok((block_height, sender))) = self.stream_catchup_tasks.join_next() => {
                if let Ok(Some(item)) = self.blocks.get(block_height) {
                    info!("streaming block {}", block_height);
                    let bytes: bytes::Bytes = bincode::encode_to_vec(item, bincode::config::standard())?.into();
                    sender.lock().await.send(bytes).await?;
                    self.stream_catchup_tasks
                    .spawn(async move { (block_height + 1, sender) });
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
        if let Err(e) = self.blocks.put(block.clone()) {
            error!("storing block: {}", e);
        } else {
            // stream block to peers
            let mut to_remove = Vec::new();
            for (i, peer) in self.stream_peer_metadata.iter().enumerate() {
                let last_ping = peer.last_ping.load(Ordering::Relaxed);
                if last_ping + 60 * 5
                    < SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("time went backwards")
                        .as_secs()
                {
                    info!("peer {} timed out", i);
                    peer.receiver_abort.abort();
                    to_remove.push(i);
                } else {
                    info!("streaming block {} to peer {}", block.height, i);
                    match bincode::encode_to_vec(block.clone(), bincode::config::standard()) {
                        Ok(bytes) => {
                            if let Err(e) = peer.sender.lock().await.send(bytes.into()).await {
                                error!("sending block to peer: {}", e);
                                to_remove.push(i);
                            }
                        }
                        Err(e) => error!("encoding block: {}", e),
                    }
                }
            }
            for i in to_remove {
                self.stream_peer_metadata.remove(i);
            }
        }
    }

    /// API endpoint to request a stream of blocks from a peer.
    /// The DA module will start at the requested block
    /// or its oldest stored block if the requested block is not present.
    /// New blocks will be pushed as soon as they are received.
    /// Blocks may arrive out of order and duplicates are possible.
    async fn api_stream_request(
        Query(filters): Query<StreamRequest>,
        State(state): State<tokio::sync::mpsc::Sender<StreamRequest>>,
    ) -> Result<(), StatusCode> {
        if let Err(err) = state.send(filters).await {
            error!("Error passing stream request to DA: {}", err);
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        }
        Ok(())
    }

    /// Process a stream request from a peer.
    async fn handle_stream_request(&mut self, start_height: u64, peer_ip: &str) -> Result<()> {
        let stream = TcpStream::connect(peer_ip).await?;
        info!("DA stream setup on {} to {}", stream.local_addr()?, peer_ip);
        let (sender, mut receiver) = Framed::new(stream, LengthDelimitedCodec::new()).split();
        let sender = Arc::new(Mutex::new(sender));
        // First: figure out if we have past blocks to send them.
        // If so, spin futures - we'll process them as part of the main select! loop.
        let mut range = self.blocks.range(
            blocks::BlocksKey(BlockHeight(start_height)),
            blocks::BlocksKey(self.blocks.last().height),
        );
        if let Some(Ok(start)) = range.next() {
            if let Ok(block) = start.value() {
                let sender = sender.clone();
                self.stream_catchup_tasks
                    .spawn(async move { (block.height, sender) });
            }
        }
        // Then start a task to process pings from the peer.
        // Likewise, we actually do the processing in the main select! loop.
        let timestamp = Arc::new(AtomicU64::new(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("time went backwards")
                .as_secs(),
        ));
        let cloned_timestamp = timestamp.clone();
        let t = self.peer_keepalives.spawn(async move {
            receiver.next().await;
            (cloned_timestamp, receiver)
        });

        // Then store data so we can send new blocks as they come.
        self.stream_peer_metadata.push(BlockStreamPeer {
            last_ping: timestamp,
            sender,
            receiver_abort: t,
        });
        Ok(())
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
