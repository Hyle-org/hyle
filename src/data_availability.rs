//! Minimal processed block storage layer for data availability.

mod api;
pub mod node_state;
mod processed_blocks;

use crate::{
    bus::{command_response::Query, BusClientSender, BusMessage},
    consensus::ConsensusCommand,
    genesis::GenesisEvent,
    handle_messages,
    mempool::MempoolEvent,
    model::{
        get_current_timestamp, BlockHeight, ContractName, Hashable, ProcessedBlock,
        ProcessedBlockHash, SharedRunContext, Transaction, ValidatorPublicKey,
    },
    module_handle_messages,
    p2p::network::{NetMessage, OutboundMessage, PeerEvent},
    utils::{
        conf::SharedConf,
        logger::LogMe,
        modules::{module_bus_client, Module},
    },
};
use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use bytes::Bytes;
use core::str;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use node_state::{model::Contract, NodeState};
use processed_blocks::ProcessedBlocks;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::collections::HashMap;
use tokio::{
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
use tokio_util::codec::{Framed, LengthDelimitedCodec};
use tracing::{debug, error, info, trace, warn};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataNetMessage {
    QueryProcessedBlock {
        respond_to: ValidatorPublicKey,
        hash: ProcessedBlockHash,
    },
    QueryLastProcessedBlock {
        respond_to: ValidatorPublicKey,
    },
    QueryProcessedBlockResponse {
        processed_block: Box<ProcessedBlock>,
    },
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataEvent {
    ProcessedBlock(Box<ProcessedBlock>),
    CatchupDone(BlockHeight),
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

module_bus_client! {
#[derive(Debug)]
struct DABusClient {
    sender(OutboundMessage),
    sender(DataEvent),
    sender(ConsensusCommand),
    receiver(Query<ContractName, Contract>),
    receiver(DataNetMessage),
    receiver(PeerEvent),
    receiver(Query<QueryBlockHeight , BlockHeight>),
    receiver(MempoolEvent),
    receiver(GenesisEvent),
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
    pub processed_blocks: ProcessedBlocks,

    buffered_processed_blocks: BTreeSet<ProcessedBlock>,
    self_pubkey: ValidatorPublicKey,
    asked_last_processed_block: Option<ValidatorPublicKey>,

    // Peers subscribed to processed block streaming
    stream_peer_metadata: HashMap<String, BlockStreamPeer>,

    node_state: NodeState,
}

impl Module for DataAvailability {
    type Context = SharedRunContext;

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = DABusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        let api = api::api(&ctx.common).await;
        if let Ok(mut guard) = ctx.common.router.lock() {
            if let Some(router) = guard.take() {
                guard.replace(router.nest("/v1/", api));
            }
        }

        #[cfg(not(test))]
        let db = sled::Config::new()
            .use_compression(true)
            .compression_factor(3)
            .path(
                ctx.common
                    .config
                    .data_directory
                    .join("data_availability.db"),
            )
            .open()
            .context("opening the database")?;

        #[cfg(test)]
        let db = sled::Config::new()
            .use_compression(false)
            .open()
            .context("opening the database")?;

        let buffered_blocks = BTreeSet::new();
        let self_pubkey = ctx.node.crypto.validator_pubkey().clone();

        let node_state = Self::load_from_disk_or_default::<NodeState>(
            ctx.common
                .config
                .data_directory
                .join("da_node_state.bin")
                .as_path(),
        );

        Ok(DataAvailability {
            config: ctx.common.config.clone(),
            bus,
            processed_blocks: ProcessedBlocks::new(&db)?,
            buffered_processed_blocks: buffered_blocks,
            self_pubkey,
            asked_last_processed_block: None,
            stream_peer_metadata: HashMap::new(),
            node_state,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DataAvailability {
    pub async fn start(&mut self) -> Result<()> {
        let stream_request_receiver = TcpListener::bind(&self.config.da_address).await?;
        info!(
            "📡  Starting DataAvailability module, listening for stream requests on {}",
            &self.config.da_address
        );

        let mut pending_stream_requests = JoinSet::new();

        // TODO: this is a soft cap on the number of peers we can stream to.
        let (ping_sender, mut ping_receiver) = tokio::sync::mpsc::channel(100);
        let (catchup_sender, mut catchup_receiver) = tokio::sync::mpsc::channel(100);

        module_handle_messages! {
            on_bus self.bus,
            command_response<ContractName, Contract> cmd => {
                self.node_state.contracts.get(cmd).cloned().context("Contract not found")
            }
            listen<MempoolEvent> cmd => {
                match cmd {
                    MempoolEvent::CommitCutWithTxs(txs, new_bounded_validators) => {
                        self.handle_commit_block_event(txs, new_bounded_validators).await;
                    }
                }
            }

            listen<GenesisEvent> cmd => {
                if let GenesisEvent::GenesisBlock { initial_validators, genesis_txs } = cmd {
                    debug!("🌱  Genesis processed block received");

                    let processed_block = self.node_state.handle_new_cut(
                        BlockHeight(0),
                        ProcessedBlockHash::new("0000000000000000"),
                        get_current_timestamp(),
                        initial_validators,
                        genesis_txs,
                    );
                    self.handle_processed_block(processed_block).await;
                }
            }

            listen<DataNetMessage> msg => {
                _ = self.handle_data_message(msg).await
                    .log_error("NodeState: Error while handling data message");
            }
            listen<PeerEvent> msg => {
                match msg {
                    PeerEvent::NewPeer { pubkey, .. } => {
                        if self.asked_last_processed_block.is_none() {
                            info!("📡  Asking for last processed block from new peer");
                            self.asked_last_processed_block = Some(pubkey);
                            self.query_last_block();
                        }
                    }
                }
            }
            command_response<QueryBlockHeight, BlockHeight> _ => {
                Ok(self.processed_blocks.last().map(|processed_block| processed_block.block_height).unwrap_or(BlockHeight(0)))
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
                                    .map_err(|e| anyhow::anyhow!("Could not decode start height: {:?}", e))?;
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
                        info!("📡 Started streaming to peer {}", &peer_ip);
                    }
                    Err(e) => {
                        error!("Error while handling stream request: {:?}", e);
                    }
                }
            }

            // Send one processed block to a peer as part of "catchup",
            // once we have sent all blocks the peer is presumably synchronised.
            Some((mut block_hashes, peer_ip)) = catchup_receiver.recv() => {
                let hash = block_hashes.pop();

                trace!("📡  Sending processed block {:?} to peer {}", &hash, &peer_ip);
                if let Some(hash) = hash {
                    if let Ok(Some(processed_block)) = self.processed_blocks.get(hash)
                    {
                        let bytes: bytes::Bytes =
                            bincode::encode_to_vec(processed_block, bincode::config::standard())?.into();
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

        Ok(())
    }

    async fn handle_data_message(&mut self, msg: DataNetMessage) -> Result<()> {
        match msg {
            DataNetMessage::QueryProcessedBlock { respond_to, hash } => {
                self.processed_blocks.get(hash).map(|processed_block| {
                    if let Some(processed_block) = processed_block {
                        _ = self.bus.send(OutboundMessage::send(
                            respond_to,
                            DataNetMessage::QueryProcessedBlockResponse {
                                processed_block: Box::new(processed_block),
                            },
                        ));
                    }
                })?;
            }
            DataNetMessage::QueryProcessedBlockResponse { processed_block } => {
                debug!(
                    block_hash = %processed_block.hash(),
                    block_height = %processed_block.block_height,
                    "⬇️  Received processed block data");
                self.handle_processed_block(*processed_block).await;
            }
            DataNetMessage::QueryLastProcessedBlock { respond_to } => {
                if let Some(processed_block) = self.processed_blocks.last() {
                    _ = self.bus.send(OutboundMessage::send(
                        respond_to,
                        DataNetMessage::QueryProcessedBlockResponse {
                            processed_block: Box::new(processed_block.clone()),
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
        new_bounded_validators: Vec<ValidatorPublicKey>,
    ) {
        info!("🔒  Cut committed");
        let last_processed_block = self.processed_blocks.last();
        let block_parent_hash =
            last_processed_block
                .as_ref()
                .map(|b| b.hash())
                .unwrap_or(ProcessedBlockHash::new(
                    "46696174206c757820657420666163746120657374206c7578",
                ));
        let next_height = last_processed_block
            .map(|b| b.block_height.0 + 1)
            .unwrap_or(0);

        let processed_block = self.node_state.handle_new_cut(
            BlockHeight(next_height),
            block_parent_hash,
            get_current_timestamp(),
            new_bounded_validators,
            txs,
        );

        self.handle_processed_block(processed_block).await;
    }

    async fn handle_processed_block(&mut self, processed_block: ProcessedBlock) {
        // if new processed_block is already handled, ignore it
        if self.processed_blocks.contains(&processed_block) {
            warn!("ProcessedBlock {:?} already exists !", processed_block);
            return;
        }
        // if new processed block is not the next processed block in the chain, buffer
        if self.processed_blocks.last().is_some() {
            if self
                .processed_blocks
                .get(processed_block.block_parent_hash.clone())
                .unwrap_or(None)
                .is_none()
            {
                debug!(
                    "Parent processed block '{}' not found for processed block hash='{}' height {}",
                    processed_block.block_parent_hash,
                    processed_block.hash(),
                    processed_block.block_height
                );
                self.query_block(processed_block.block_parent_hash.clone());
                debug!("Buffering processed block {}", processed_block.hash());
                self.buffered_processed_blocks.insert(processed_block);
                return;
            }
        // if genesis processed block is missing, buffer
        } else if processed_block.block_height != BlockHeight(0) {
            trace!(
                "Received processed block with height {} but genesis processed block is missing",
                processed_block.block_height
            );
            self.query_block(processed_block.block_parent_hash.clone());
            trace!("Buffering processed block {}", processed_block.hash());
            self.buffered_processed_blocks.insert(processed_block);
            return;
        }

        // store processed block
        self.add_processed_block(processed_block.clone()).await;
        self.pop_buffer(processed_block.hash()).await;
    }

    async fn pop_buffer(&mut self, mut last_block_hash: ProcessedBlockHash) {
        let got_buffered = !self.buffered_processed_blocks.is_empty();
        // Iterative loop to avoid stack overflows
        while let Some(first_buffered) = self.buffered_processed_blocks.first() {
            if first_buffered.block_parent_hash != last_block_hash {
                error!(
                    "Buffered processed block parent hash does not match last processed block hash"
                );
                break;
            }

            let first_buffered = self.buffered_processed_blocks.pop_first().unwrap();
            last_block_hash = first_buffered.hash();
            self.add_processed_block(first_buffered).await;
        }

        if got_buffered {
            info!(
                "📡 Asking for last processed block from peer in case new blocks were mined during catchup."
            );
            self.query_last_block();
        } else {
            let height = self
                .processed_blocks
                .last()
                .map_or(BlockHeight(0), |b| b.block_height);
            _ = self
                .bus
                .send(DataEvent::CatchupDone(height))
                .log_error("Error sending DataEvent");
        }
    }

    async fn add_processed_block(&mut self, processed_block: ProcessedBlock) {
        // Don't run this in tests, takes forever.
        if let Err(e) = self.processed_blocks.put(processed_block.clone()) {
            error!("storing processed block: {}", e);
            return;
        }

        info!(
            "new processed block {} with {} txs, last hash = {}",
            processed_block.block_height,
            processed_block.new_contract_txs.len()
                + processed_block.new_blob_txs.len()
                + processed_block.new_verified_proof_txs.len(),
            self.processed_blocks
                .last_block_hash()
                .unwrap_or(ProcessedBlockHash("".to_string()))
        );

        // Send the processed block
        if let Err(e) = self
            .bus
            .send(DataEvent::ProcessedBlock(Box::new(processed_block.clone())))
        {
            error!("Failed to send processed block consensus command: {:?}", e);
        }

        // Stream processed block to all peers
        // TODO: use retain once async closures are supported ?
        let mut to_remove = Vec::new();
        for (peer_id, peer) in self.stream_peer_metadata.iter_mut() {
            let last_ping = peer.last_ping;
            if last_ping + 60 * 5 < get_current_timestamp() {
                info!("peer {} timed out", &peer_id);
                peer.keepalive_abort.abort();
                to_remove.push(peer_id.clone());
            } else {
                info!(
                    "streaming processed block {} to peer {}",
                    processed_block.hash(),
                    &peer_id
                );
                match bincode::encode_to_vec(processed_block.clone(), bincode::config::standard()) {
                    Ok(bytes) => {
                        if let Err(e) = peer.sender.send(bytes.into()).await {
                            warn!("failed to send processed block to peer {}: {}", &peer_id, e);
                            // TODO: retry?
                            to_remove.push(peer_id.clone());
                        }
                    }
                    Err(e) => error!("encoding processed block: {}", e),
                }
            }
        }
        for peer_id in to_remove {
            self.stream_peer_metadata.remove(&peer_id);
        }
    }

    fn query_block(&mut self, hash: ProcessedBlockHash) {
        _ = self.bus.send(OutboundMessage::broadcast(
            DataNetMessage::QueryProcessedBlock {
                respond_to: self.self_pubkey.clone(),
                hash,
            },
        ));
    }

    fn query_last_block(&mut self) {
        if let Some(pubkey) = &self.asked_last_processed_block {
            _ = self.bus.send(OutboundMessage::send(
                pubkey.clone(),
                DataNetMessage::QueryLastProcessedBlock {
                    respond_to: self.self_pubkey.clone(),
                },
            ));
        }
    }

    async fn start_streaming_to_peer(
        &mut self,
        start_height: u64,
        ping_sender: tokio::sync::mpsc::Sender<String>,
        catchup_sender: tokio::sync::mpsc::Sender<(Vec<ProcessedBlockHash>, String)>,
        sender: SplitSink<Framed<TcpStream, LengthDelimitedCodec>, Bytes>,
        mut receiver: SplitStream<Framed<TcpStream, LengthDelimitedCodec>>,
        peer_ip: &String,
    ) -> Result<()> {
        // Start a task to process pings from the peer.
        // We do the processing in the main select! loop to keep things synchronous.
        // This makes it easier to store data in the same struct without mutexing.
        let peer_ip_keepalive = peer_ip.to_string();
        let keepalive_abort = tokio::task::Builder::new()
            .name("da-keep-alive-abort")
            .spawn(async move {
                loop {
                    receiver.next().await;
                    let _ = ping_sender.send(peer_ip_keepalive.clone()).await;
                }
            })?;

        // Then store data so we can send new blocks as they come.
        self.stream_peer_metadata.insert(
            peer_ip.to_string(),
            BlockStreamPeer {
                last_ping: get_current_timestamp(),
                sender,
                keepalive_abort,
            },
        );

        // Finally, stream past processed blocks as required.
        // We'll create a copy of the range so we don't stream everything.
        // We will safely stream everything as any new processed block will be sent
        // because we registered in the struct beforehand.
        // Like pings, this just sends a message processed in the main select! loop.
        let mut processed_block_hashes: Vec<ProcessedBlockHash> = self
            .processed_blocks
            .range(
                processed_blocks::BlocksOrdKey(BlockHeight(start_height)),
                processed_blocks::BlocksOrdKey(
                    self.processed_blocks
                        .last()
                        .map_or(BlockHeight(start_height), |processed_block| {
                            processed_block.block_height
                        })
                        + 1,
                ),
            )
            .filter_map(|processed_block| {
                processed_block
                    .map(|b| b.value().map_or(ProcessedBlockHash::new(""), |i| i.hash()))
                    .ok()
            })
            .collect();
        processed_block_hashes.reverse();

        catchup_sender
            .send((processed_block_hashes, peer_ip.clone()))
            .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        bus::BusClientSender,
        mempool::MempoolEvent,
        model::{BlockHeight, Hashable, ProcessedBlock},
        utils::conf::Conf,
    };
    use futures::{SinkExt, StreamExt};
    use tokio::io::AsyncWriteExt;
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    use super::{module_bus_client, processed_blocks::ProcessedBlocks};
    use anyhow::Result;

    #[test]
    fn test_blocks() -> Result<()> {
        let tmpdir = tempfile::Builder::new().prefix("history-tests").tempdir()?;
        let db = sled::open(tmpdir.path().join("history"))?;
        let mut processed_blocks = ProcessedBlocks::new(&db)?;
        let processed_block = ProcessedBlock::default();
        processed_blocks.put(processed_block.clone())?;
        assert!(processed_blocks.last().unwrap().block_height == processed_block.block_height);
        let last = processed_blocks.get(processed_block.hash())?;
        assert!(last.is_some());
        assert!(last.unwrap().block_height == BlockHeight(0));
        Ok(())
    }

    #[tokio::test]
    async fn test_pop_buffer_large() {
        let tmpdir = tempfile::Builder::new()
            .prefix("history-tests")
            .tempdir()
            .unwrap();
        let db = sled::open(tmpdir.path().join("history")).unwrap();
        let processed_blocks = ProcessedBlocks::new(&db).unwrap();

        let bus = super::DABusClient::new_from_bus(crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        ))
        .await;
        let mut da = super::DataAvailability {
            config: Default::default(),
            bus,
            processed_blocks,
            buffered_processed_blocks: Default::default(),
            self_pubkey: Default::default(),
            asked_last_processed_block: Default::default(),
            stream_peer_metadata: Default::default(),
            node_state: Default::default(),
        };
        let mut processed_block = ProcessedBlock::default();
        let mut processed_blocks = vec![];
        for i in 1..1000 {
            processed_blocks.push(processed_block.clone());
            processed_block.block_parent_hash = processed_block.hash();
            processed_block.block_height = BlockHeight(i);
        }
        processed_blocks.reverse();
        for processed_block in processed_blocks {
            da.handle_processed_block(processed_block).await;
        }
    }

    module_bus_client! {
    #[derive(Debug)]
    struct TestBusClient {
        sender(MempoolEvent),
    }
    }

    #[test_log::test(tokio::test)]
    async fn test_da_streaming() {
        let tmpdir = tempfile::Builder::new()
            .prefix("history-tests")
            .tempdir()
            .unwrap();
        let db = sled::open(tmpdir.path().join("history")).unwrap();
        let processed_blocks = ProcessedBlocks::new(&db).unwrap();

        let global_bus = crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        );
        let bus = super::DABusClient::new_from_bus(global_bus.new_handle()).await;
        let mut block_sender = TestBusClient::new_from_bus(global_bus).await;

        let config: Conf = Conf::new(None, None, None).unwrap();
        let mut da = super::DataAvailability {
            config: config.clone().into(),
            bus,
            processed_blocks,
            buffered_processed_blocks: Default::default(),
            self_pubkey: Default::default(),
            asked_last_processed_block: Default::default(),
            stream_peer_metadata: Default::default(),
            node_state: Default::default(),
        };

        let mut processed_block = ProcessedBlock::default();
        let mut processed_blocks = vec![];
        for i in 1..15 {
            processed_blocks.push(processed_block.clone());
            processed_block.block_parent_hash = processed_block.hash();
            processed_block.block_height = BlockHeight(i);
        }
        processed_blocks.reverse();
        for processed_block in processed_blocks {
            da.handle_processed_block(processed_block).await;
        }

        tokio::spawn(async move {
            da.start().await.unwrap();
        });

        // wait until it's up
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut stream = tokio::net::TcpStream::connect(config.da_address.clone())
            .await
            .unwrap();

        // TODO: figure out why writing doesn't work with da_stream.
        stream.write_u32(8).await.unwrap();
        stream.write_u64(0).await.unwrap();

        let mut da_stream = Framed::new(stream, LengthDelimitedCodec::new());

        let mut heights_received = vec![];
        while let Some(Ok(cmd)) = da_stream.next().await {
            let bytes = cmd;
            let processed_block: ProcessedBlock =
                bincode::decode_from_slice(&bytes, bincode::config::standard())
                    .unwrap()
                    .0;
            heights_received.push(processed_block.block_height.0);
            if heights_received.len() == 14 {
                break;
            }
        }
        assert_eq!(heights_received, (0..14).collect::<Vec<u64>>());

        da_stream.close().await.unwrap();

        block_sender
            .send(MempoolEvent::CommitCutWithTxs(vec![], vec![]))
            .unwrap();
        block_sender
            .send(MempoolEvent::CommitCutWithTxs(vec![], vec![]))
            .unwrap();
        block_sender
            .send(MempoolEvent::CommitCutWithTxs(vec![], vec![]))
            .unwrap();
        block_sender
            .send(MempoolEvent::CommitCutWithTxs(vec![], vec![]))
            .unwrap();

        // End of the first stream

        let mut stream = tokio::net::TcpStream::connect(config.da_address.clone())
            .await
            .unwrap();

        // TODO: figure out why writing doesn't work with da_stream.
        stream.write_u32(8).await.unwrap();
        stream.write_u64(0).await.unwrap();

        let mut da_stream = Framed::new(stream, LengthDelimitedCodec::new());

        let mut heights_received = vec![];
        while let Some(Ok(cmd)) = da_stream.next().await {
            let bytes = cmd;
            let processed_block: ProcessedBlock =
                bincode::decode_from_slice(&bytes, bincode::config::standard())
                    .unwrap()
                    .0;
            heights_received.push(processed_block.block_height.0);
            if heights_received.len() == 18 {
                break;
            }
        }

        assert_eq!(heights_received, (0..18).collect::<Vec<u64>>());
    }
}
