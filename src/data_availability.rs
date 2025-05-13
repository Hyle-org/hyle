//! Minimal block storage layer for data availability.

mod blocks_fjall;
mod blocks_memory;

// Pick one of the two implementations
use blocks_fjall::Blocks;
//use blocks_memory::Blocks;

use hyle_modules::{
    bus::SharedMessageBus,
    log_error, module_bus_client, module_handle_messages,
    modules::Module,
    utils::da_codec::{
        DataAvailabilityClient, DataAvailabilityEvent, DataAvailabilityRequest,
        DataAvailabilityServer,
    },
};
use hyle_net::tcp::TcpEvent;

use crate::{
    bus::BusClientSender,
    consensus::ConsensusCommand,
    genesis::GenesisEvent,
    model::*,
    p2p::network::{OutboundMessage, PeerEvent},
    utils::conf::SharedConf,
};
use anyhow::{Context, Error, Result};
use core::str;
use std::collections::BTreeSet;
use tracing::{debug, error, info, trace, warn};

pub mod codec;

module_bus_client! {
#[derive(Debug)]
struct DABusClient {
    sender(OutboundMessage),
    sender(DataEvent),
    sender(ConsensusCommand),
    receiver(MempoolBlockEvent),
    receiver(MempoolStatusEvent),
    receiver(GenesisEvent),
    receiver(PeerEvent),
}
}

type DaTcpServer =
    hyle_net::tcp::tcp_server::TcpServer<DataAvailabilityRequest, DataAvailabilityEvent>;

#[derive(Debug)]
pub struct DataAvailability {
    config: SharedConf,
    bus: DABusClient,
    pub blocks: Blocks,

    buffered_signed_blocks: BTreeSet<SignedBlock>,

    need_catchup: bool,
    catchup_task: Option<tokio::task::JoinHandle<()>>,
    catchup_height: Option<BlockHeight>,
}

impl Module for DataAvailability {
    type Context = SharedRunContext;

    async fn build(bus: SharedMessageBus, ctx: Self::Context) -> Result<Self> {
        let bus = DABusClient::new_from_bus(bus.new_handle()).await;

        Ok(DataAvailability {
            config: ctx.config.clone(),
            bus,
            blocks: Blocks::new(&ctx.config.data_directory.join("data_availability.db"))?,
            buffered_signed_blocks: BTreeSet::new(),
            need_catchup: false,
            catchup_task: None,
            catchup_height: None,
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        self.start()
    }
}

impl DataAvailability {
    pub async fn start(&mut self) -> Result<()> {
        info!(
            "📡  Starting DataAvailability module, listening for stream requests on port {}",
            self.config.da_server_port
        );

        let mut server: DaTcpServer = DataAvailabilityServer::start_with_opts(
            self.config.da_server_port,
            Some(self.config.da_max_frame_length),
            "DAServer",
        )
        .await?;

        let (catchup_block_sender, mut catchup_block_receiver) =
            tokio::sync::mpsc::channel::<SignedBlock>(100);

        let (catchup_sender, mut catchup_receiver) = tokio::sync::mpsc::channel(100);

        module_handle_messages! {
            on_bus self.bus,
            listen<MempoolBlockEvent> evt => {
                _ = log_error!(self.handle_mempool_event(evt, &mut server).await, "Handling Mempool Event");
            }

            listen<MempoolStatusEvent> evt => {
                self.handle_mempool_status_event(evt, &mut server).await;
            }

            listen<GenesisEvent> cmd => {
                if let GenesisEvent::GenesisBlock(signed_block) = cmd {
                    debug!("🌱  Genesis block received with validators {:?}", signed_block.consensus_proposal.staking_actions.clone());
                    let _= log_error!(self.handle_signed_block(signed_block, &mut server).await, "Handling GenesisBlock Event");
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
            Some(streamed_block) = catchup_block_receiver.recv() => {
                let height = streamed_block.height().0;

                let _ = log_error!(self.handle_signed_block(streamed_block, &mut server).await, "Handling streamed block {}", height);

                // Stop streaming after reaching a height communicated by Mempool
                if let Some(until_height) = self.catchup_height.as_ref() {
                    if until_height.0 <= height {
                        if let Some(t) = self.catchup_task.take() {
                            t.abort();
                            info!("Stopped streaming since received height {} and until {}", height, until_height.0);
                            self.need_catchup = false;
                        } else {
                            info!("Did not stop streaming (received height {} and until {}) since no catchup task was running", height, until_height.0);
                        }
                    }
                }
            }

            Some(tcp_event) = server.listen_next() => {
                if let TcpEvent::Message { dest, data } = tcp_event {
                    _ = self.start_streaming_to_peer(data.0, catchup_sender.clone(), &dest).await;
                }
            }


            // Send one block to a peer as part of "catchup",
            // once we have sent all blocks the peer is presumably synchronised.
            Some((mut block_hashes, peer_ip)) = catchup_receiver.recv() => {
                let hash = block_hashes.pop();

                if let Some(hash) = hash {
                    debug!("📡  Sending block {} to peer {}", &hash, &peer_ip);
                    if let Ok(Some(signed_block)) = self.blocks.get(&hash)
                    {
                        // Errors will be handled when sending new blocks, ignore here.
                        if server
                            .send(peer_ip.clone(), DataAvailabilityEvent::SignedBlock(signed_block))
                            .await.is_ok() {
                            let _ = catchup_sender.send((block_hashes, peer_ip)).await;
                        }
                    }
                }
            }

        };

        Ok(())
    }

    async fn handle_mempool_event(
        &mut self,
        evt: MempoolBlockEvent,
        tcp_server: &mut DaTcpServer,
    ) -> Result<()> {
        match evt {
            MempoolBlockEvent::BuiltSignedBlock(signed_block) => {
                self.handle_signed_block(signed_block, tcp_server).await?;
            }
            MempoolBlockEvent::StartedBuildingBlocks(height) => {
                self.catchup_height = Some(height - 1);
                if let Some(handle) = self.catchup_task.as_ref() {
                    if self
                        .blocks
                        .last()
                        .map(|b| b.height())
                        .unwrap_or(BlockHeight(0))
                        .0
                        >= height.0
                    {
                        info!("🏁 Stopped streaming blocks until height {}.", height);
                        handle.abort();
                        self.need_catchup = false;
                    }
                }
            }
        }

        Ok(())
    }
    async fn handle_mempool_status_event(
        &mut self,
        evt: MempoolStatusEvent,
        tcp_server: &mut DaTcpServer,
    ) {
        tcp_server
            .broadcast(DataAvailabilityEvent::MempoolStatusEvent(evt))
            .await;
    }

    async fn handle_signed_block(
        &mut self,
        block: SignedBlock,
        tcp_server: &mut DaTcpServer,
    ) -> Result<()> {
        let hash = block.hashed();
        // if new block is already handled, ignore it
        if self.blocks.contains(&hash) {
            warn!(
                "Block {} {} already exists !",
                block.height(),
                block.hashed()
            );
            return Ok(());
        }
        // if new block is not the next block in the chain, buffer
        if !self.blocks.is_empty() {
            if !self.blocks.contains(block.parent_hash()) {
                debug!(
                    "Parent block '{}' not found for block hash='{}' height {}",
                    block.parent_hash(),
                    block.hashed(),
                    block.height()
                );
                debug!("Buffering block {}", block.hashed());
                self.buffered_signed_blocks.insert(block);
                return Ok(());
            }
        // if genesis block is missing, buffer
        } else if block.height() != BlockHeight(0) {
            trace!(
                "Received block with height {} but genesis block is missing",
                block.height()
            );
            trace!("Buffering block {}", block.hashed());
            self.buffered_signed_blocks.insert(block);
            return Ok(());
        }

        // store block
        self.add_processed_block(block.clone(), tcp_server).await;
        self.pop_buffer(hash, tcp_server).await;
        self.blocks.persist().context("Persisting blocks")?;
        Ok(())
    }

    async fn pop_buffer(
        &mut self,
        mut last_block_hash: ConsensusProposalHash,
        tcp_server: &mut DaTcpServer,
    ) {
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
            #[allow(
                clippy::unwrap_used,
                reason = "Must exist as checked in the while above"
            )]
            let first_buffered = self.buffered_signed_blocks.pop_first().unwrap();
            last_block_hash = first_buffered.hashed();
            self.add_processed_block(first_buffered.clone(), tcp_server)
                .await;
        }
    }

    async fn add_processed_block(&mut self, block: SignedBlock, tcp_server: &mut DaTcpServer) {
        // TODO: if we don't have streaming peers, we could just pass the block here
        // and avoid a clone + drop cost (which can be substantial for large blocks).
        if let Err(e) = self.blocks.put(block.clone()) {
            error!("storing block: {}", e);
            return;
        }
        trace!("Block {} {}: {:#?}", block.height(), block.hashed(), block);

        if block.height().0 % 10 == 0 || block.has_txs() {
            info!(
                "new block #{} 0x{} with {} txs",
                block.height(),
                block.hashed(),
                block.count_txs(),
            );
        }
        debug!(
            "new block #{} 0x{} with {} transactions: {}",
            block.height(),
            block.hashed(),
            block.count_txs(),
            block
                .iter_txs_with_id()
                .map(|(_, tx_id, tx)| {
                    let variant: &'static str = (&tx.transaction_data).into();
                    format!("\n - 0x{} {}", tx_id.1, variant)
                })
                .collect::<Vec<_>>()
                .join("")
        );

        // TODO: use retain once async closures are supported ?
        //
        tcp_server
            .broadcast(DataAvailabilityEvent::SignedBlock(block.clone()))
            .await;

        // Send the block to NodeState for processing
        _ = log_error!(
            self.bus.send(DataEvent::OrderedSignedBlock(block)),
            "Sending OrderedSignedBlock"
        );
    }

    async fn start_streaming_to_peer(
        &mut self,
        start_height: BlockHeight,
        catchup_sender: tokio::sync::mpsc::Sender<(Vec<ConsensusProposalHash>, String)>,
        peer_ip: &str,
    ) -> Result<()> {
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
            .filter_map(|item| item.ok())
            .collect();
        processed_block_hashes.reverse();

        catchup_sender
            .send((processed_block_hashes, peer_ip.to_string()))
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
        let mut client = DataAvailabilityClient::connect("block_catcher".to_string(), ip)
            .await
            .context("Error occured setting up the DA listener")?;
        client.send(DataAvailabilityRequest(start)).await?;
        self.catchup_task = Some(tokio::spawn(async move {
            loop {
                match client.recv().await {
                    None => {
                        break;
                    }
                    Some(DataAvailabilityEvent::SignedBlock(block)) => {
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
                    Some(_) => {}
                }
            }
        }));
        Ok(())
    }
}

#[cfg(test)]
pub mod tests {
    #![allow(clippy::indexing_slicing)]

    use crate::data_availability::codec::DataAvailabilityRequest;
    use crate::node_state::NodeState;
    use crate::{
        bus::BusClientSender,
        consensus::CommittedConsensusProposal,
        model::*,
        node_state::module::{NodeStateBusClient, NodeStateEvent},
        utils::{conf::Conf, integration_test::find_available_port},
    };
    use hyle_modules::log_error;
    use hyle_modules::utils::da_codec::{DataAvailabilityClient, DataAvailabilityServer};

    use super::codec::DataAvailabilityEvent;
    use super::Blocks;
    use super::{module_bus_client, DaTcpServer};
    use anyhow::Result;
    use staking::state::Staking;

    /// For use in integration tests
    pub struct DataAvailabilityTestCtx {
        pub node_state_bus: NodeStateBusClient,
        pub da: super::DataAvailability,
        pub node_state: NodeState,
    }

    impl DataAvailabilityTestCtx {
        pub async fn new(shared_bus: crate::bus::SharedMessageBus) -> Self {
            let path = tempfile::tempdir().unwrap().keep();
            let tmpdir = path;
            let blocks = Blocks::new(&tmpdir).unwrap();

            let bus = super::DABusClient::new_from_bus(shared_bus.new_handle()).await;
            let node_state_bus = NodeStateBusClient::new_from_bus(shared_bus).await;

            let mut config: Conf = Conf::new(None, None, None).unwrap();

            let node_state = NodeState::create(config.id.clone(), "data_availability");

            config.da_server_port = find_available_port().await;
            config.da_public_address = format!("127.0.0.1:{}", config.da_server_port);
            let da = super::DataAvailability {
                config: config.into(),
                bus,
                blocks,
                buffered_signed_blocks: Default::default(),
                need_catchup: false,
                catchup_task: None,
                catchup_height: None,
            };

            DataAvailabilityTestCtx {
                node_state_bus,
                da,
                node_state,
            }
        }

        pub async fn handle_signed_block(
            &mut self,
            block: SignedBlock,
            tcp_server: &mut DaTcpServer,
        ) {
            _ = log_error!(
                self.da.handle_signed_block(block.clone(), tcp_server).await,
                "Handling Signed Block"
            );
            // TODO: we use this in autobahn_testing, but it'd be cleaner to separate it.
            let Ok(full_block) = self.node_state.handle_signed_block(&block) else {
                tracing::warn!("Error while handling signed block {}", block.hashed());
                return;
            };
            _ = log_error!(
                self.node_state_bus
                    .send(NodeStateEvent::NewBlock(Box::new(full_block))),
                "Sending NodeState event"
            );
        }
    }

    #[test_log::test]
    fn test_blocks() -> Result<()> {
        let tmpdir = tempfile::tempdir().unwrap().keep();
        let mut blocks = Blocks::new(&tmpdir).unwrap();
        let block = SignedBlock::default();
        blocks.put(block.clone())?;
        assert!(blocks.last().unwrap().height() == block.height());
        let last = blocks.get(&block.hashed())?;
        assert!(last.is_some());
        assert!(last.unwrap().height() == BlockHeight(0));
        Ok(())
    }

    #[tokio::test]
    async fn test_pop_buffer_large() {
        let tmpdir = tempfile::tempdir().unwrap().keep();
        let blocks = Blocks::new(&tmpdir).unwrap();

        let mut server = DataAvailabilityServer::start(7898, "DaServer")
            .await
            .unwrap();

        let bus = super::DABusClient::new_from_bus(crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        ))
        .await;
        let mut da = super::DataAvailability {
            config: Default::default(),
            bus,
            blocks,
            buffered_signed_blocks: Default::default(),
            need_catchup: false,
            catchup_task: None,
            catchup_height: None,
        };
        let mut block = SignedBlock::default();
        let mut blocks = vec![];
        for i in 1..10000 {
            blocks.push(block.clone());
            block.consensus_proposal.parent_hash = block.hashed();
            block.consensus_proposal.slot = i;
        }
        blocks.reverse();
        for block in blocks {
            da.handle_signed_block(block, &mut server).await.unwrap();
        }
    }

    module_bus_client! {
    #[derive(Debug)]
    struct TestBusClient {
        sender(MempoolBlockEvent),
    }
    }

    #[test_log::test(tokio::test)]
    async fn test_da_streaming() {
        let tmpdir = tempfile::tempdir().unwrap().keep();
        let blocks = Blocks::new(&tmpdir).unwrap();

        let global_bus = crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        );
        let bus = super::DABusClient::new_from_bus(global_bus.new_handle()).await;
        let mut block_sender = TestBusClient::new_from_bus(global_bus).await;

        let mut config: Conf = Conf::new(None, None, None).unwrap();
        config.da_server_port = find_available_port().await;
        config.da_public_address = format!("127.0.0.1:{}", config.da_server_port);
        let mut da = super::DataAvailability {
            config: config.clone().into(),
            bus,
            blocks,
            buffered_signed_blocks: Default::default(),
            need_catchup: false,
            catchup_task: None,
            catchup_height: None,
        };

        let mut block = SignedBlock::default();
        let mut blocks = vec![];
        for i in 1..15 {
            blocks.push(block.clone());
            block.consensus_proposal.parent_hash = block.hashed();
            block.consensus_proposal.slot = i;
        }
        blocks.reverse();

        // Start Da and its client
        tokio::spawn(async move {
            da.start().await.unwrap();
        });

        let mut client =
            DataAvailabilityClient::connect("client_id", config.da_public_address.clone())
                .await
                .unwrap();

        client
            .send(DataAvailabilityRequest(BlockHeight(0)))
            .await
            .unwrap();

        // Feed Da with blocks, should stream them to the client
        for block in blocks {
            block_sender
                .send(MempoolBlockEvent::BuiltSignedBlock(block))
                .unwrap();
        }

        // wait until it's up
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut heights_received = vec![];
        while let Some(event) = client.recv().await {
            if let DataAvailabilityEvent::SignedBlock(block) = event {
                heights_received.push(block.height().0);
            }
            if heights_received.len() == 14 {
                break;
            }
        }
        assert_eq!(heights_received, (0..14).collect::<Vec<u64>>());

        client.close().await.unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut ccp = CommittedConsensusProposal {
            staking: Staking::default(),
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature {
                signature: crate::model::Signature("signature".into()),
                validators: vec![],
            },
        };

        for i in 14..18 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hashed();
            ccp.consensus_proposal.slot = i;
            block_sender
                .send(MempoolBlockEvent::BuiltSignedBlock(SignedBlock {
                    data_proposals: vec![(LaneId::default(), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
                .unwrap();
        }

        // End of the first stream

        let mut client =
            DataAvailabilityClient::connect("client_id", config.da_public_address.clone())
                .await
                .unwrap();

        client
            .send(DataAvailabilityRequest(BlockHeight(0)))
            .await
            .unwrap();

        let mut heights_received = vec![];
        while let Some(event) = client.recv().await {
            if let DataAvailabilityEvent::SignedBlock(block) = event {
                heights_received.push(block.height().0);
            }
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
        let mut server = DataAvailabilityServer::start(7890, "DaServer")
            .await
            .unwrap();

        let receiver_global_bus = crate::bus::SharedMessageBus::new(
            crate::bus::metrics::BusMetrics::global("global".to_string()),
        );
        let mut da_receiver = DataAvailabilityTestCtx::new(receiver_global_bus).await;

        // Push some blocks to the sender
        let mut block = SignedBlock::default();
        let mut blocks = vec![];
        for i in 1..11 {
            blocks.push(block.clone());
            block.consensus_proposal.parent_hash = block.hashed();
            block.consensus_proposal.slot = i;
        }
        blocks.reverse();
        for block in blocks {
            da_sender.handle_signed_block(block, &mut server).await;
        }

        let da_sender_address = da_sender.da.config.da_public_address.clone();

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
        while let Some(streamed_block) = rx.recv().await {
            da_receiver
                .handle_signed_block(streamed_block.clone(), &mut server)
                .await;
            received_blocks.push(streamed_block);
            if received_blocks.len() == 10 {
                break;
            }
        }
        assert_eq!(received_blocks.len(), 10);
        assert_eq!(received_blocks[0].height(), BlockHeight(0));
        assert_eq!(received_blocks[9].height(), BlockHeight(9));

        // Add a few blocks (via bus to avoid mutex)
        let mut ccp = CommittedConsensusProposal {
            staking: Staking::default(),
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature::default(),
        };

        for i in 10..15 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hashed();
            ccp.consensus_proposal.slot = i;
            block_sender
                .send(MempoolBlockEvent::BuiltSignedBlock(SignedBlock {
                    data_proposals: vec![(LaneId::default(), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
                .unwrap();
        }

        // We should still be subscribed
        while let Some(streamed_block) = rx.recv().await {
            da_receiver
                .handle_signed_block(streamed_block.clone(), &mut server)
                .await;
            received_blocks.push(streamed_block);
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
            staking: Staking::default(),
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature::default(),
        };

        for i in 15..20 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hashed();
            ccp.consensus_proposal.slot = i;
            block_sender
                .send(MempoolBlockEvent::BuiltSignedBlock(SignedBlock {
                    data_proposals: vec![(LaneId::default(), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
                .unwrap();
        }

        // Resubscribe - we should only receive the new ones.
        da_receiver
            .da
            .ask_for_catchup_blocks(da_sender_address.clone(), tx)
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
