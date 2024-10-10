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
use futures::{stream::SplitSink, SinkExt, StreamExt};
use reqwest::StatusCode;
use std::{
    collections::HashMap,
    io::{Cursor, Write},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{net::TcpStream, task::JoinHandle};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{error, info, warn};

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

/// A peer we are streaming blocks to
#[derive(Debug)]
struct BlockStreamPeer {
    /// Last timestamp we received a ping from the peer.
    last_ping: u64,
    /// Sender to stream blocks to the peer
    sender: SplitSink<Framed<TcpStream, LengthDelimitedCodec>, Bytes>,
    /// Handle to abort the receiving side of the stream
    keepalive_abort: JoinHandle<()>,
}

#[derive(Debug)]
pub struct DataAvailability {
    bus: DABusClient,
    pub blocks: Blocks,

    // To avoid mutexing DA we use a channel to process API requests
    stream_request_receiver: tokio::sync::mpsc::Receiver<StreamRequest>,

    // Peers subscribed to block streaming
    stream_peer_metadata: HashMap<String, BlockStreamPeer>,

    ping_sender: tokio::sync::mpsc::Sender<String>,
    ping_receiver: tokio::sync::mpsc::Receiver<String>,
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

        let (stream_request_sender, stream_request_receiver) = tokio::sync::mpsc::channel(100);

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
            .with_state(stream_request_sender)
            .merge(ctx_router.take().expect("Router should be available"));
        let _ = ctx_router.insert(router);

        // TODO: this is a soft cap on the number of peers we can stream to.
        let (ping_sender, ping_receiver) = tokio::sync::mpsc::channel(100);

        Ok(DataAvailability {
            bus,
            blocks: Blocks::new(&db)?,
            stream_request_receiver,
            ping_sender,
            ping_receiver,
            stream_peer_metadata: HashMap::new(),
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

            Some(peer_id) = self.ping_receiver.recv() => {
                if let Some(peer) = self.stream_peer_metadata.get_mut(&peer_id) {
                    peer.last_ping = SystemTime::now().duration_since(UNIX_EPOCH).expect("time went backwards").as_secs();
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
            // Stream block to all peers
            // TODO: use retain once async closures are supported ?
            let mut to_remove = Vec::new();
            for (peer_id, peer) in self.stream_peer_metadata.iter_mut() {
                let last_ping = peer.last_ping;
                if last_ping + 60 * 5
                    < SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .expect("time went backwards")
                        .as_secs()
                {
                    info!("peer {} timed out", &peer_id);
                    peer.keepalive_abort.abort();
                    to_remove.push(peer_id.clone());
                } else {
                    info!("streaming block {} to peer {}", block.height, &peer_id);
                    match bincode::encode_to_vec(block.clone(), bincode::config::standard()) {
                        Ok(bytes) => {
                            if let Err(e) = peer.sender.send(bytes.into()).await {
                                warn!("failed to send block to peer {}: {}", &peer_id, e);
                                // TODO: retry?
                                to_remove.push(peer_id.clone());
                            }
                        }
                        Err(e) => error!("encoding block: {}", e),
                    }
                }
            }
            for peer_id in to_remove {
                self.stream_peer_metadata.remove(&peer_id);
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

        // Start a task to process pings from the peer.
        // We do the processing in the main select! loop to keep things synchronous.
        let peer_ip_keepalive = peer_ip.to_string();
        let ping_sender = self.ping_sender.clone();
        let keepalive_abort = tokio::spawn(async move {
            loop {
                receiver.next().await;
                let _ = ping_sender.send(peer_ip_keepalive.clone()).await;
            }
        });

        // Then store data so we can send new blocks as they come.
        self.stream_peer_metadata.insert(
            peer_ip.to_string(),
            BlockStreamPeer {
                last_ping: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .expect("time went backwards")
                    .as_secs(),
                sender,
                keepalive_abort,
            },
        );

        // First: figure out if we have past blocks to send them.
        // If so, spin futures - we'll process them as part of the main select! loop.
        let mut range = self.blocks.range(
            blocks::BlocksKey(BlockHeight(start_height)),
            blocks::BlocksKey(self.blocks.last().height),
        );
        if let Some(Ok(start)) = range.next() {
            if let Ok(block) = start.value() {
                let mut block_height = block.height;
                while let Ok(Some(block)) = self.blocks.get(block_height) {
                    let bytes: bytes::Bytes =
                        bincode::encode_to_vec(block, bincode::config::standard())?.into();
                    self.stream_peer_metadata
                        .get_mut(peer_ip)
                        .unwrap()
                        .sender
                        .send(bytes)
                        .await?;
                    block_height.0 += 1;
                }
            }
        }

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
