//! Minimal block storage layer for data availability.

mod blocks;

use crate::{
    bus::{bus_client, command_response::Query, BusMessage, SharedMessageBus},
    handle_messages,
    mempool::MempoolEvent,
    model::{
        get_current_timestamp, Block, BlockHash, BlockHeight, Hashable, SharedRunContext,
        Transaction, ValidatorPublicKey,
    },
    p2p::network::{NetMessage, OutboundMessage, PeerEvent},
    utils::{conf::SharedConf, logger::LogMe, modules::Module},
};
use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use blocks::Blocks;
use bytes::Bytes;
use core::str;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::{BTreeSet, HashSet};
use tokio::{
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, info, warn};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataNetMessage {
    QueryBlock {
        respond_to: ValidatorPublicKey,
        hash: BlockHash,
    },
    QueryLastBlock {
        respond_to: ValidatorPublicKey,
    },
    QueryBlockResponse {
        block: Block,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataEvent {
    NewBlock(Block),
}

impl BusMessage for DataNetMessage {}
impl BusMessage for DataEvent {}

impl From<DataNetMessage> for NetMessage {
    fn from(msg: DataNetMessage) -> Self {
        NetMessage::DataMessage(msg)
    }
}

#[derive(Clone)]
pub struct QueryBlockHeight {}

bus_client! {
#[derive(Debug)]
struct DABusClient {
    sender(OutboundMessage),
    sender(DataEvent),
    receiver(DataNetMessage),
    receiver(PeerEvent),
    receiver(Query<QueryBlockHeight , BlockHeight>),
    receiver(MempoolEvent),
}
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
    config: SharedConf,
    bus: DABusClient,
    pub blocks: Blocks,

    buffered_blocks: BTreeSet<Block>,
    self_pubkey: ValidatorPublicKey,
    asked_last_block: bool,

    // Peers subscribed to block streaming
    stream_peer_metadata: HashMap<String, BlockStreamPeer>,
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

        let buffered_blocks = BTreeSet::new();
        let self_pubkey = ctx.node.crypto.validator_pubkey().clone();

        Ok(DataAvailability {
            config: ctx.common.config.clone(),
            bus,
            blocks: Blocks::new(&db)?,
            buffered_blocks,
            self_pubkey,
            asked_last_block: false,
            stream_peer_metadata: HashMap::new(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DataAvailability {
    pub async fn start(&mut self) -> Result<()> {
        let stream_request_receiver = TcpListener::bind(&self.config.da_address).await?;

        let mut pending_stream_requests = JoinSet::new();

        // TODO: this is a soft cap on the number of peers we can stream to.
        let (ping_sender, mut ping_receiver) = tokio::sync::mpsc::channel(100);
        let (catchup_sender, mut catchup_receiver) = tokio::sync::mpsc::channel(100);

        handle_messages! {
            on_bus self.bus,
            listen<MempoolEvent> cmd => {
                if let MempoolEvent::CommitBlock(txs, new_bonded_validators) = cmd {
                    self.handle_commit_block_event(txs, new_bonded_validators).await;
               }
            }

            listen<DataNetMessage> msg => {
                _ = self.handle_data_message(msg).await
                    .log_error("NodeState: Error while handling data message");
            }
            listen<PeerEvent> msg => {
                match msg {
                    PeerEvent::NewPeer { pubkey, .. } => {
                        if !self.asked_last_block {
                            info!("📡  Asking for last block from new peer");
                            self.query_last_block(pubkey);
                            self.asked_last_block = true;
                        }
                    }
                }
            }
            command_response<QueryBlockHeight, BlockHeight> _ => {
                Ok(self.blocks.last().map(|block| block.height).unwrap_or(BlockHeight(0)))
            }

            // Handle new TCP connections to stream data to peers
            // We spawn an async task that waits for the start height as the first message.
            Ok((stream, addr)) = stream_request_receiver.accept() => {
                // This handler is defined inline so I don't have to give a type to pending_stream_requests
                pending_stream_requests.spawn(async move {
                    let (sender, mut receiver) = Framed::new(stream, LengthDelimitedCodec::new()).split();
                    // Read the start height from the peer.
                    match receiver.next().await {
                        Some(Ok(data)) => {
                            let (start_height, _) =
                                bincode::decode_from_slice(&data, bincode::config::standard())
                                    .map_err(|_| anyhow::anyhow!("Could not decode start height"))?;
                            Ok((start_height, sender, receiver, addr.to_string()))
                        }
                        _ => Err(anyhow::anyhow!("no start height")),
                    }
                });
            }

            // Actually connect to a peer and start streaming data.
            Some(Ok(cmd)) = pending_stream_requests.join_next() => {
                match cmd {
                    Ok((start_height, sender, receiver, peer_ip)) => {
                        if let Err(e) = self.start_streaming_to_peer(start_height, ping_sender.clone(), catchup_sender.clone(), sender, receiver, &peer_ip).await {
                            error!("Error while starting stream to peer {}: {:?}", &peer_ip, e)
                        }
                    }
                    Err(e) => {
                        error!("Error while handling stream request: {:?}", e);
                    }
                }
            }

            // Send one block to a peer as part of "catchup",
            // once we have sent all blocks the peer is presumably synchronised.
            Some((mut block_hashes, peer_ip)) = catchup_receiver.recv() => {
                let hash = block_hashes
                .iter()
                .next();
                if let Some(hash) = hash {
                    if let Some(Ok(Some(block))) = block_hashes.take(&hash.clone()).map(|hash| self.blocks.get(hash))
                    {
                        let bytes: bytes::Bytes =
                            bincode::encode_to_vec(block, bincode::config::standard())?.into();
                        if self.stream_peer_metadata
                            .get_mut(&peer_ip)
                            .context("peer not found")?
                            .sender
                            .send(bytes)
                            .await.is_ok() {
                            let _ = catchup_sender.send((block_hashes, peer_ip)).await;
                        }
                    }
                }
            }

            Some(peer_id) = ping_receiver.recv() => {
                if let Some(peer) = self.stream_peer_metadata.get_mut(&peer_id) {
                    peer.last_ping = get_current_timestamp();
                }
            }
        }
    }

    async fn handle_data_message(&mut self, msg: DataNetMessage) -> Result<()> {
        match msg {
            DataNetMessage::QueryBlock { respond_to, hash } => {
                self.blocks.get(hash).map(|block| {
                    if let Some(block) = block {
                        _ = self.bus.send(OutboundMessage::send(
                            respond_to,
                            DataNetMessage::QueryBlockResponse { block },
                        ));
                    }
                })?;
            }
            DataNetMessage::QueryBlockResponse { block } => {
                debug!(
                    block_hash = %block.hash(),
                    block_height = %block.height,
                    "⬇️  Received block data");
                self.handle_block(block).await;
            }
            DataNetMessage::QueryLastBlock { respond_to } => {
                if let Some(block) = self.blocks.last() {
                    _ = self.bus.send(OutboundMessage::send(
                        respond_to,
                        DataNetMessage::QueryBlockResponse {
                            block: block.clone(),
                        },
                    ));
                }
            }
        }
        Ok(())
    }

    async fn handle_commit_block_event(
        &mut self,
        txs: Vec<Transaction>,
        new_bonded_validators: Vec<ValidatorPublicKey>,
    ) {
        info!("🔒  Cut committed");
        let last_block = self.blocks.last();
        let parent_hash = last_block
            .as_ref()
            .map(|b| b.hash())
            .unwrap_or(BlockHash::new(
                "46696174206c757820657420666163746120657374206c7578",
            ));
        let next_height = last_block.map(|b| b.height.0 + 1).unwrap_or(0);

        self.handle_block(Block {
            parent_hash,
            height: BlockHeight(next_height),
            timestamp: get_current_timestamp(),
            new_bonded_validators,
            txs,
        })
        .await;
    }

    async fn handle_block(&mut self, block: Block) {
        // if new block is not the next block in the chain, buffer
        if self.blocks.last().is_some() {
            if self
                .blocks
                .get(block.parent_hash.clone())
                .unwrap_or(None)
                .is_none()
            {
                debug!(
                    "Parent block '{}' not found for block hash='{}' height {}",
                    block.parent_hash,
                    block.hash(),
                    block.height
                );
                self.query_block(block.parent_hash.clone());
                self.buffered_blocks.insert(block);
                return;
            }
        // if genesis block is missing, buffer
        } else if block.height != BlockHeight(0) {
            debug!(
                "Received block with height {} but genesis block is missing",
                block.height
            );
            self.query_block(block.parent_hash.clone());
            self.buffered_blocks.insert(block);
            return;
        }

        info!(
            "new block {} with {} txs, last hash = {:?}",
            block.height,
            block.txs.len(),
            self.blocks.last_block_hash()
        );
        // store block
        let block_hash = block.hash();
        self.add_block(block.clone());
        self.pop_buffer(block_hash.clone());

        // Stream block to all peers
        // TODO: use retain once async closures are supported ?
        let mut to_remove = Vec::new();
        for (peer_id, peer) in self.stream_peer_metadata.iter_mut() {
            let last_ping = peer.last_ping;
            if last_ping + 60 * 5 < get_current_timestamp() {
                info!("peer {} timed out", &peer_id);
                peer.keepalive_abort.abort();
                to_remove.push(peer_id.clone());
            } else {
                info!("streaming block {} to peer {}", block_hash, &peer_id);
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

    fn pop_buffer(&mut self, mut last_block_hash: BlockHash) {
        while let Some(first_buffered) = self.buffered_blocks.first() {
            if first_buffered.parent_hash != last_block_hash {
                break;
            }

            let first_buffered = self.buffered_blocks.pop_first().unwrap();
            let first_buffered_hash = first_buffered.hash();

            self.add_block(first_buffered);
            last_block_hash = first_buffered_hash;
        }
    }

    fn add_block(&mut self, block: Block) {
        if let Err(e) = self.blocks.put(block.clone()) {
            error!("storing block: {}", e);
        } else {
            _ = self
                .bus
                .send(DataEvent::NewBlock(block))
                .log_error("Error sending DataEvent");
        }
    }

    fn query_block(&mut self, hash: BlockHash) {
        _ = self
            .bus
            .send(OutboundMessage::broadcast(DataNetMessage::QueryBlock {
                respond_to: self.self_pubkey.clone(),
                hash,
            }));
    }

    fn query_last_block(&mut self, peer: ValidatorPublicKey) {
        _ = self.bus.send(OutboundMessage::send(
            peer,
            DataNetMessage::QueryLastBlock {
                respond_to: self.self_pubkey.clone(),
            },
        ));
    }

    async fn start_streaming_to_peer(
        &mut self,
        start_height: u64,
        ping_sender: tokio::sync::mpsc::Sender<String>,
        catchup_sender: tokio::sync::mpsc::Sender<(HashSet<BlockHash>, String)>,
        sender: SplitSink<Framed<TcpStream, LengthDelimitedCodec>, Bytes>,
        mut receiver: SplitStream<Framed<TcpStream, LengthDelimitedCodec>>,
        peer_ip: &String,
    ) -> Result<()> {
        // Start a task to process pings from the peer.
        // We do the processing in the main select! loop to keep things synchronous.
        // This makes it easier to store data in the same struct without mutexing.
        let peer_ip_keepalive = peer_ip.to_string();
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
                last_ping: get_current_timestamp(),
                sender,
                keepalive_abort,
            },
        );

        // Finally, stream past blocks as required.
        // We'll create a copy of the range so we don't stream everything.
        // We will safely stream everything as any new block will be sent
        // because we registered in the struct beforehand.
        // Like pings, this just sends a message processed in the main select! loop.
        let block_hashes: HashSet<BlockHash> = self
            .blocks
            .range(
                blocks::BlocksOrdKey(BlockHeight(start_height)),
                blocks::BlocksOrdKey(
                    self.blocks
                        .last()
                        .map_or(BlockHeight(start_height), |block| block.height),
                ),
            )
            .filter_map(|block| {
                block
                    .map(|b| b.value().map_or(BlockHash::new(""), |i| i.hash()))
                    .ok()
            })
            .collect();

        catchup_sender.send((block_hashes, peer_ip.clone())).await?;

        Ok(())
    }
}

//#[cfg(test)]
//mod tests {
//    use crate::model::{
//        Blob, BlobTransaction, Block, BlockHash, BlockHeight, Hashable, Transaction,
//        TransactionData,
//    };
//
//    use super::blocks::Blocks;
//    use anyhow::Result;
//    use hyle_contract_sdk::BlobData;
//
//    #[test]
//    fn test_blocks() -> Result<()> {
//        let tmpdir = tempfile::Builder::new().prefix("history-tests").tempdir()?;
//        let db = sled::open(tmpdir.path().join("history"))?;
//        let mut blocks = Blocks::new(&db)?;
//        let block = Block {
//            parent_hash: BlockHash::new("0123456789abcdef"),
//            height: BlockHeight(1),
//            timestamp: 42,
//            new_bonded_validators: vec![],
//            txs: vec![Transaction {
//                version: 1,
//                transaction_data: TransactionData::Blob(BlobTransaction {
//                    fees: Fees::default_test(),
//                    identity: "tx_id".into(),
//                    blobs: vec![Blob {
//                        contract_name: "c1".into(),
//                        data: BlobData(vec![4, 5, 6]),
//                    }],
//                }),
//            }],
//        };
//        blocks.put(block.clone())?;
//        assert!(blocks.last().unwrap().height == block.height);
//        let last = blocks.get(block.hash())?;
//        assert!(last.is_some());
//        assert!(last.unwrap().height == BlockHeight(1));
//        Ok(())
//    }
//}
