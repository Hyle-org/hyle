//! Minimal block storage layer for data availability.

mod api;
pub mod codec;
pub mod node_state;

mod blocks_fjall;
mod blocks_memory;

// Pick one of the two implementations
use blocks_fjall::Blocks;
//use blocks_memory::Blocks;

use codec::{DataAvailabilityServerCodec, DataAvailabilityServerRequest};

use crate::{
    bus::{command_response::Query, BusClientSender, BusMessage},
    consensus::{
        CommittedConsensusProposal, ConsensusCommand, ConsensusEvent, ConsensusProposalHash,
    },
    genesis::GenesisEvent,
    indexer::da_listener::RawDAListener,
    mempool::{MempoolCommand, MempoolEvent},
    model::{
        data_availability::Contract,
        get_current_timestamp,
        mempool::{Cut, DataProposal},
        Block, BlockHeight, ContractName, Hashable, SharedRunContext, SignedBlock,
        ValidatorPublicKey,
    },
    module_handle_messages,
    p2p::network::{OutboundMessage, PeerEvent},
    utils::{
        conf::SharedConf,
        logger::LogMe,
        modules::{module_bus_client, Module},
    },
};
use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use core::str;
use futures::{
    stream::{SplitSink, SplitStream},
    SinkExt, StreamExt,
};
use node_state::NodeState;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::collections::{BTreeSet, VecDeque};
use tokio::{
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};
use tokio_util::codec::Framed;
use tracing::{debug, error, info, trace, warn};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataEvent {
    NewBlock(Box<Block>),
}

impl BusMessage for DataEvent {}

#[derive(Clone)]
pub struct QueryBlockHeight {}

module_bus_client! {
#[derive(Debug)]
struct DABusClient {
    sender(OutboundMessage),
    sender(DataEvent),
    sender(ConsensusCommand),
    sender(MempoolCommand),
    receiver(Query<ContractName, Contract>),
    receiver(Query<QueryBlockHeight , BlockHeight>),
    receiver(ConsensusEvent),
    receiver(MempoolEvent),
    receiver(GenesisEvent),
    receiver(PeerEvent),
}
}

/// A peer we are streaming blocks to
#[derive(Debug)]
struct BlockStreamPeer {
    /// Last timestamp we received a ping from the peer.
    last_ping: u64,
    /// Sender to stream blocks to the peer
    sender: SplitSink<Framed<TcpStream, DataAvailabilityServerCodec>, SignedBlock>,
    /// Handle to abort the receiving side of the stream
    keepalive_abort: JoinHandle<()>,
}

type PendingDataProposals = Vec<(ValidatorPublicKey, Vec<DataProposal>)>;

#[derive(Debug)]
pub struct DataAvailability {
    config: SharedConf,
    bus: DABusClient,
    pub blocks: Blocks,

    buffered_signed_blocks: BTreeSet<SignedBlock>,
    pending_cps: VecDeque<CommittedConsensusProposal>,
    pending_data_proposals: Vec<(Cut, PendingDataProposals)>,

    // Peers subscribed to block streaming
    stream_peer_metadata: HashMap<String, BlockStreamPeer>,

    need_catchup: bool,
    catchup_task: Option<tokio::task::JoinHandle<()>>,

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
            blocks: Blocks::new(
                &ctx.common
                    .config
                    .data_directory
                    .join("data_availability.db"),
            )?,
            buffered_signed_blocks: BTreeSet::new(),
            pending_cps: VecDeque::new(),
            pending_data_proposals: vec![],
            stream_peer_metadata: HashMap::new(),
            node_state,
            need_catchup: false,
            catchup_task: None,
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

        let (catchup_block_sender, mut catchup_block_receiver) = tokio::sync::mpsc::channel(100);

        // TODO: this is a soft cap on the number of peers we can stream to.
        let (ping_sender, mut ping_receiver) = tokio::sync::mpsc::channel(100);
        let (catchup_sender, mut catchup_receiver) = tokio::sync::mpsc::channel(100);

        module_handle_messages! {
            on_bus self.bus,
            command_response<QueryBlockHeight, BlockHeight> _ => {
                Ok(self.blocks.last().map(|block| block.height()).unwrap_or(BlockHeight(0)))
            }
            command_response<ContractName, Contract> cmd => {
                self.node_state.contracts.get(cmd).cloned().context("Contract not found")
            }
            listen<ConsensusEvent> ConsensusEvent::CommitConsensusProposal( consensus_proposal )  => {
                _ = self.handle_commit_consensus_proposal(consensus_proposal)
                    .await.log_error("Handling Committed Consensus Proposal");
                if let Some(handle) = self.catchup_task.take() {
                    info!("🏁 Stopped streaming blocks.");
                    handle.abort();
                    self.need_catchup = false;
                }
            }
            listen<MempoolEvent> evt => {
                _ = self.handle_mempool_event(evt).await.log_error("Handling Mempool Event");
            }

            listen<GenesisEvent> cmd => {
                if let GenesisEvent::GenesisBlock(signed_block) = cmd {
                    debug!("🌱  Genesis block received with validators {:?}", signed_block.consensus_proposal.new_validators_to_bond.clone());
                    self.handle_signed_block(signed_block).await;
                } else {
                    // TODO: I think this is technically a data race with p2p ?
                    self.need_catchup = true;
                    // This also triggers when restarting from serialized state, which seems fine.
                }
            }
            listen<PeerEvent> msg => {
                if !self.need_catchup || self.catchup_task.is_some() {
                    continue;
                }
                match msg {
                    PeerEvent::NewPeer { da_address, .. } => {
                        self.ask_for_catchup_blocks(da_address, catchup_block_sender.clone()).await?;
                    }
                }
            }
            Some(block) = catchup_block_receiver.recv() => {
                self.handle_signed_block(block).await;
            }

            // Handle new TCP connections to stream data to peers
            // We spawn an async task that waits for the start height as the first message.
            Ok((stream, addr)) = stream_request_receiver.accept() => {
                // This handler is defined inline so I don't have to give a type to pending_stream_requests
                pending_stream_requests.spawn(async move {
                    let (sender, mut receiver) = Framed::new(stream, DataAvailabilityServerCodec::default()).split();
                    // Read the start height from the peer.
                    match receiver.next().await {
                        Some(Ok(data)) => {
                            if let DataAvailabilityServerRequest::BlockHeight(start_height) = data {
                                Ok((start_height, sender, receiver, addr.to_string()))
                            } else {
                                Err(anyhow::anyhow!("Got a ping instead of a block height"))
                            }
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

            // Send one block to a peer as part of "catchup",
            // once we have sent all blocks the peer is presumably synchronised.
            Some((mut block_hashes, peer_ip)) = catchup_receiver.recv() => {
                let hash = block_hashes.pop();

                trace!("📡  Sending block {:?} to peer {}", &hash, &peer_ip);
                if let Some(hash) = hash {
                    if let Ok(Some(signed_block)) = self.blocks.get(&hash)
                    {
                        // Errors will be handled when sending new blocks, ignore here.
                        if self.stream_peer_metadata
                            .get_mut(&peer_ip)
                            .context("peer not found")?
                            .sender
                            .send(signed_block)
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

        let _ = Self::save_on_disk::<NodeState>(
            self.config
                .data_directory
                .join("da_node_state.bin")
                .as_path(),
            &self.node_state,
        )
        .log_error("Saving node state");

        Ok(())
    }

    async fn handle_mempool_event(&mut self, evt: MempoolEvent) -> Result<()> {
        match evt {
            MempoolEvent::DataProposals(cut, data_proposals) => {
                self.pending_data_proposals.push((cut, data_proposals));

                while let Some(oldest_cut_to_process) = self.pending_cps.pop_front() {
                    if let Some(dps) = self
                        .pending_data_proposals
                        .iter()
                        .position(|(cut, _)| cut == &oldest_cut_to_process.consensus_proposal.cut)
                    {
                        let (_, dps) = self.pending_data_proposals.remove(dps);

                        self.handle_commit_block_event(dps, oldest_cut_to_process)
                            .await;
                    } else {
                        self.pending_cps.push_front(oldest_cut_to_process);
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    async fn handle_commit_consensus_proposal(
        &mut self,
        commit_consensus_proposal: CommittedConsensusProposal,
    ) -> Result<()> {
        info!(
            "Handling Committed Consensus Proposal {:?}",
            &commit_consensus_proposal
        );

        // FIXME: Make sure the block we get is the previous one wrt the committedConsensusProposal
        let to = commit_consensus_proposal.consensus_proposal.cut.clone();
        let from = self.blocks.last().map(|b| b.consensus_proposal.cut);

        self.pending_cps.push_back(commit_consensus_proposal);
        self.bus
            .send(MempoolCommand::FetchDataProposals { from, to })
            .context("Handling commit consensus proposal")?;

        Ok(())
    }

    async fn handle_commit_block_event(
        &mut self,
        data_proposals: Vec<(ValidatorPublicKey, Vec<DataProposal>)>,
        CommittedConsensusProposal {
            validators: _,
            certificate,
            consensus_proposal,
        }: CommittedConsensusProposal,
    ) {
        info!("🔒  Cut committed");
        let signed_block = SignedBlock {
            data_proposals,
            certificate,
            consensus_proposal,
        };

        self.handle_signed_block(signed_block).await;
    }

    async fn handle_signed_block(&mut self, block: SignedBlock) {
        let hash = block.hash();
        // if new block is already handled, ignore it
        if self.blocks.contains(&hash) {
            warn!("Block {} {} already exists !", block.height(), block.hash());
            return;
        }
        // if new block is not the next block in the chain, buffer
        if !self.blocks.is_empty() {
            if !self.blocks.contains(block.parent_hash()) {
                debug!(
                    "Parent block '{}' not found for block hash='{}' height {}",
                    block.parent_hash(),
                    block.hash(),
                    block.height()
                );
                debug!("Buffering block {}", block.hash());
                self.buffered_signed_blocks.insert(block);
                return;
            }
        // if genesis block is missing, buffer
        } else if block.height() != BlockHeight(0) {
            trace!(
                "Received block with height {} but genesis block is missing",
                block.height()
            );
            trace!("Buffering block {}", block.hash());
            self.buffered_signed_blocks.insert(block);
            return;
        }

        // store block
        self.add_processed_block(block).await;
        self.pop_buffer(hash).await;
        _ = self.blocks.persist().log_error("Persisting blocks");
    }

    async fn pop_buffer(&mut self, mut last_block_hash: ConsensusProposalHash) {
        // Iterative loop to avoid stack overflows
        while let Some(first_buffered) = self.buffered_signed_blocks.first() {
            if first_buffered.parent_hash() != &last_block_hash {
                error!(
                    "Buffered block parent hash {} does not match last block hash {}",
                    first_buffered.parent_hash(),
                    last_block_hash
                );
                break;
            }

            let first_buffered = self.buffered_signed_blocks.pop_first().unwrap();
            last_block_hash = first_buffered.hash();
            self.add_processed_block(first_buffered).await;
        }
    }

    async fn add_processed_block(&mut self, block: SignedBlock) {
        // TODO: if we don't have streaming peers, we could just pass the block here
        // and avoid a clone + drop cost (which can be substantial for large blocks).
        if let Err(e) = self.blocks.put(block.clone()) {
            error!("storing block: {}", e);
            return;
        }
        trace!("Block {} {}: {:#?}", block.height(), block.hash(), block);

        info!(
            "new block {} {} with {} txs, last hash = {}",
            block.height(),
            block.hash(),
            block.txs().len(),
            self.blocks.last_block_hash().unwrap_or_default()
        );
        debug!(
            "Transactions: {:#?}",
            block.txs().iter().map(|tx| tx.hash().0).collect::<Vec<_>>()
        );

        // Send the block

        let node_state_block = self.node_state.handle_signed_block(&block);
        _ = self
            .bus
            .send(DataEvent::NewBlock(Box::new(node_state_block)))
            .log_error("Sending DataEvent while processing SignedBlock");

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
                info!("streaming block {} to peer {}", block.hash(), &peer_id);
                match peer.sender.send(block.clone()).await {
                    Ok(_) => {}
                    Err(e) => {
                        debug!(
                            "Couldn't send new block to peer {}, stopping streaming  : {:?}",
                            &peer_id, e
                        );
                        peer.keepalive_abort.abort();
                        to_remove.push(peer_id.clone());
                    }
                }
            }
        }
        for peer_id in to_remove {
            self.stream_peer_metadata.remove(&peer_id);
        }
    }

    async fn start_streaming_to_peer(
        &mut self,
        start_height: BlockHeight,
        ping_sender: tokio::sync::mpsc::Sender<String>,
        catchup_sender: tokio::sync::mpsc::Sender<(Vec<ConsensusProposalHash>, String)>,
        sender: SplitSink<Framed<TcpStream, DataAvailabilityServerCodec>, SignedBlock>,
        mut receiver: SplitStream<Framed<TcpStream, DataAvailabilityServerCodec>>,
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

        // Finally, stream past blocks as required.
        // We'll create a copy of the range so we don't stream everything.
        // We will safely stream everything as any new block will be sent
        // because we registered in the struct beforehand.
        // Like pings, this just sends a message processed in the main select! loop.
        let mut processed_block_hashes: Vec<_> = self
            .blocks
            .range(
                start_height,
                self.blocks
                    .last()
                    .map_or(start_height, |block| block.height())
                    + 1,
            )
            .filter_map(|block| block.map(|b| b.hash()).ok())
            .collect();
        processed_block_hashes.reverse();

        catchup_sender
            .send((processed_block_hashes, peer_ip.clone()))
            .await?;

        Ok(())
    }

    async fn ask_for_catchup_blocks(
        &mut self,
        ip: String,
        sender: tokio::sync::mpsc::Sender<SignedBlock>,
    ) -> Result<(), Error> {
        info!("📡 Streaming data from {ip}");
        let start = self
            .blocks
            .last()
            .map(|block| block.height() + 1)
            .unwrap_or(BlockHeight(0));
        let Ok(mut stream) = RawDAListener::new(&ip, start).await else {
            bail!("Error occured setting up the DA listener");
        };
        self.catchup_task = Some(tokio::spawn(async move {
            loop {
                match stream.next().await {
                    None => {
                        warn!("End of stream");
                        break;
                    }
                    Some(Err(e)) => {
                        warn!("Error while streaming data from peer: {:#}", e);
                        break;
                    }
                    Some(Ok(block)) => {
                        info!(
                            "📦 Received block (height {}) from stream",
                            block.consensus_proposal.slot
                        );
                        // TODO: we should wait if the stream is full.
                        if let Err(e) = sender.send(block).await {
                            tracing::error!("Error while sending block over channel: {:#}", e);
                            break;
                        }
                    }
                }
            }
        }));
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    use std::collections::VecDeque;

    use crate::{
        bus::BusClientSender,
        consensus::{CommittedConsensusProposal, ConsensusEvent, ConsensusProposal},
        mempool::{MempoolCommand, MempoolEvent},
        model::{BlockHeight, Hashable, SignedBlock},
        utils::{conf::Conf, crypto::AggregateSignature},
    };
    use futures::{SinkExt, StreamExt};
    use staking::model::ValidatorPublicKey;
    use tokio::io::AsyncWriteExt;
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    use super::module_bus_client;
    use super::Blocks;
    use anyhow::Result;

    /// For use in integration tests
    pub struct DataAvailabilityTestCtx {
        pub da: super::DataAvailability,
    }

    impl DataAvailabilityTestCtx {
        pub async fn new(shared_bus: crate::bus::SharedMessageBus) -> Self {
            let tmpdir = tempfile::tempdir().unwrap().into_path();
            let blocks = Blocks::new(&tmpdir).unwrap();

            let bus = super::DABusClient::new_from_bus(shared_bus).await;

            let mut config: Conf = Conf::new(None, None, None).unwrap();
            config.da_address = format!("127.0.0.1:{}", find_available_port().await);
            let da = super::DataAvailability {
                config: config.into(),
                bus,
                blocks,
                pending_data_proposals: vec![],
                pending_cps: VecDeque::new(),
                buffered_signed_blocks: Default::default(),
                stream_peer_metadata: Default::default(),
                node_state: Default::default(),
                need_catchup: false,
                catchup_task: None,
            };

            DataAvailabilityTestCtx { da }
        }

        pub async fn handle_signed_block(&mut self, block: SignedBlock) {
            self.da.handle_signed_block(block).await;
        }
    }

    // Assume that we can reuse the OS-provided port.
    async fn find_available_port() -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        addr.port()
    }

    #[test_log::test]
    fn test_blocks() -> Result<()> {
        let tmpdir = tempfile::tempdir().unwrap().into_path();
        let mut blocks = Blocks::new(&tmpdir).unwrap();
        let block = SignedBlock::default();
        blocks.put(block.clone())?;
        assert!(blocks.last().unwrap().height() == block.height());
        let last = blocks.get(&block.hash())?;
        assert!(last.is_some());
        assert!(last.unwrap().height() == BlockHeight(0));
        Ok(())
    }

    #[tokio::test]
    async fn test_pop_buffer_large() {
        let tmpdir = tempfile::tempdir().unwrap().into_path();
        let blocks = Blocks::new(&tmpdir).unwrap();

        let bus = super::DABusClient::new_from_bus(crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        ))
        .await;
        let mut da = super::DataAvailability {
            config: Default::default(),
            bus,
            blocks,
            pending_data_proposals: vec![],
            pending_cps: VecDeque::new(),
            buffered_signed_blocks: Default::default(),
            stream_peer_metadata: Default::default(),
            node_state: Default::default(),
            need_catchup: false,
            catchup_task: None,
        };
        let mut block = SignedBlock::default();
        let mut blocks = vec![];
        for i in 1..10000 {
            blocks.push(block.clone());
            block.consensus_proposal.parent_hash = block.hash();
            block.consensus_proposal.slot = i;
        }
        blocks.reverse();
        for block in blocks {
            da.handle_signed_block(block).await;
        }
    }

    module_bus_client! {
    #[derive(Debug)]
    struct TestBusClient {
        sender(ConsensusEvent),
        sender(MempoolEvent),
        receiver(MempoolCommand),
    }
    }

    #[test_log::test(tokio::test)]
    async fn test_da_streaming() {
        let tmpdir = tempfile::tempdir().unwrap().into_path();
        let blocks = Blocks::new(&tmpdir).unwrap();

        let global_bus = crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        );
        let bus = super::DABusClient::new_from_bus(global_bus.new_handle()).await;
        let mut block_sender = TestBusClient::new_from_bus(global_bus).await;

        let mut config: Conf = Conf::new(None, None, None).unwrap();
        config.da_address = format!("127.0.0.1:{}", find_available_port().await);
        let mut da = super::DataAvailability {
            config: config.clone().into(),
            bus,
            blocks,
            pending_data_proposals: vec![],
            pending_cps: VecDeque::new(),
            buffered_signed_blocks: Default::default(),
            stream_peer_metadata: Default::default(),
            node_state: Default::default(),
            need_catchup: false,
            catchup_task: None,
        };

        let mut block = SignedBlock::default();
        let mut blocks = vec![];
        for i in 1..15 {
            blocks.push(block.clone());
            block.consensus_proposal.parent_hash = block.hash();
            block.consensus_proposal.slot = i;
        }
        blocks.reverse();
        for block in blocks {
            da.handle_signed_block(block).await;
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
            let block: SignedBlock =
                bincode::decode_from_slice(&bytes, bincode::config::standard())
                    .unwrap()
                    .0;
            heights_received.push(block.height().0);
            if heights_received.len() == 14 {
                break;
            }
        }
        assert_eq!(heights_received, (0..14).collect::<Vec<u64>>());

        da_stream.close().await.unwrap();

        let mut ccp = CommittedConsensusProposal {
            validators: vec![],
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature {
                signature: crate::utils::crypto::Signature("signature".into()),
                validators: vec![],
            },
        };

        for i in 14..18 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
            ccp.consensus_proposal.slot = i;
            block_sender
                .send(ConsensusEvent::CommitConsensusProposal(ccp.clone()))
                .unwrap();
            block_sender
                .send(MempoolEvent::DataProposals(
                    ccp.clone().consensus_proposal.cut,
                    vec![(ValidatorPublicKey("".into()), vec![])],
                ))
                .unwrap();
        }

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
            let block: SignedBlock =
                bincode::decode_from_slice(&bytes, bincode::config::standard())
                    .unwrap()
                    .0;
            dbg!(&block);
            heights_received.push(block.height().0);
            if heights_received.len() == 18 {
                break;
            }
        }

        assert_eq!(heights_received, (0..18).collect::<Vec<u64>>());
    }
    #[test_log::test(tokio::test)]
    async fn test_da_catchup() {
        let sender_global_bus = crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        );
        let mut block_sender = TestBusClient::new_from_bus(sender_global_bus.new_handle()).await;
        let mut da_sender = DataAvailabilityTestCtx::new(sender_global_bus).await;

        let receiver_global_bus = crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        );
        let mut da_receiver = DataAvailabilityTestCtx::new(receiver_global_bus).await;

        // Push some blocks to the sender
        let mut block = SignedBlock::default();
        let mut blocks = vec![];
        for i in 1..11 {
            blocks.push(block.clone());
            block.consensus_proposal.parent_hash = block.hash();
            block.consensus_proposal.slot = i;
        }
        blocks.reverse();
        for block in blocks {
            da_sender.handle_signed_block(block).await;
        }

        let da_sender_address = da_sender.da.config.da_address.clone();

        tokio::spawn(async move {
            da_sender.da.start().await.unwrap();
        });

        // wait until it's up
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Setup done
        let (tx, mut rx) = tokio::sync::mpsc::channel(200);
        da_receiver
            .da
            .ask_for_catchup_blocks(da_sender_address.clone(), tx.clone())
            .await
            .expect("Error while asking for catchup blocks");

        let mut received_blocks = vec![];
        while let Some(block) = rx.recv().await {
            da_receiver.handle_signed_block(block.clone()).await;
            received_blocks.push(block);
            if received_blocks.len() == 10 {
                break;
            }
        }
        assert_eq!(received_blocks.len(), 10);
        assert_eq!(received_blocks[0].height(), BlockHeight(0));
        assert_eq!(received_blocks[9].height(), BlockHeight(9));

        // Add a few blocks (via bus to avoid mutex)
        let mut ccp = CommittedConsensusProposal {
            validators: vec![],
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature::default(),
        };

        for i in 10..15 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
            ccp.consensus_proposal.slot = i;
            block_sender
                .send(ConsensusEvent::CommitConsensusProposal(ccp.clone()))
                .unwrap();
            block_sender
                .send(MempoolEvent::DataProposals(
                    ccp.clone().consensus_proposal.cut,
                    vec![(ValidatorPublicKey("".into()), vec![])],
                ))
                .unwrap();
        }

        // We should still be subscribed
        while let Some(block) = rx.recv().await {
            da_receiver.handle_signed_block(block.clone()).await;
            received_blocks.push(block);
            if received_blocks.len() == 15 {
                break;
            }
        }
        assert_eq!(received_blocks.len(), 15);
        assert_eq!(received_blocks[14].height(), BlockHeight(14));

        // Unsub
        // TODO: ideally via processing the correct message
        da_receiver.da.catchup_task.take().unwrap().abort();

        // Add a few blocks (via bus to avoid mutex)
        let mut ccp = CommittedConsensusProposal {
            validators: vec![],
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature::default(),
        };

        for i in 15..20 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
            ccp.consensus_proposal.slot = i;
            block_sender
                .send(ConsensusEvent::CommitConsensusProposal(ccp.clone()))
                .unwrap();
            block_sender
                .send(MempoolEvent::DataProposals(
                    ccp.clone().consensus_proposal.cut,
                    vec![(ValidatorPublicKey("".into()), vec![])],
                ))
                .unwrap();
        }

        // Resubscribe - we should only receive the new ones.
        da_receiver
            .da
            .ask_for_catchup_blocks(da_sender_address, tx)
            .await
            .expect("Error while asking for catchup blocks");

        let mut received_blocks = vec![];
        while let Some(block) = rx.recv().await {
            received_blocks.push(block);
            if received_blocks.len() == 5 {
                break;
            }
        }
        assert_eq!(received_blocks.len(), 5);
        assert_eq!(received_blocks[0].height(), BlockHeight(15));
        assert_eq!(received_blocks[4].height(), BlockHeight(19));
    }
}
