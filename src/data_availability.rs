//! Minimal block storage layer for data availability.

pub mod codec;

mod blocks_fjall;
mod blocks_memory;

// Pick one of the two implementations
use blocks_fjall::Blocks;
//use blocks_memory::Blocks;

use codec::{codec_data_availability, DataAvailabilityEvent};

use crate::{
    bus::{BusClientSender, BusMessage},
    consensus::{ConsensusCommand, ConsensusEvent},
    genesis::GenesisEvent,
    indexer::da_listener::RawDAListener,
    log_error,
    mempool::{MempoolBlockEvent, MempoolStatusEvent},
    model::*,
    module_handle_messages,
    p2p::network::{OutboundMessage, PeerEvent},
    tcp::{TcpCommand, TcpEvent},
    utils::{
        conf::SharedConf,
        modules::{module_bus_client, Module},
    },
};
use anyhow::{bail, Context, Error, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use core::str;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tokio::sync::mpsc::Sender;
use tracing::{debug, error, info, trace, warn};

#[derive(Debug, Serialize, Deserialize, Clone, BorshSerialize, BorshDeserialize, Eq, PartialEq)]
pub enum DataEvent {
    OrderedSignedBlock(SignedBlock),
}

impl BusMessage for DataEvent {}

module_bus_client! {
#[derive(Debug)]
struct DABusClient {
    sender(OutboundMessage),
    sender(DataEvent),
    sender(ConsensusCommand),
    receiver(ConsensusEvent),
    receiver(MempoolBlockEvent),
    receiver(MempoolStatusEvent),
    receiver(GenesisEvent),
    receiver(PeerEvent),
}
}

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

    async fn build(ctx: Self::Context) -> Result<Self> {
        let bus = DABusClient::new_from_bus(ctx.common.bus.new_handle()).await;

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
            "📡  Starting DataAvailability module, listening for stream requests on {}",
            &self.config.da_address
        );

        let (pool_sender, mut pool_receiver) =
            codec_data_availability::create_server(self.config.da_address.clone())
                .run_in_background()
                .await?;

        let (catchup_block_sender, mut catchup_block_receiver) =
            tokio::sync::mpsc::channel::<SignedBlock>(100);

        let (catchup_sender, mut catchup_receiver) = tokio::sync::mpsc::channel(100);

        module_handle_messages! {
            on_bus self.bus,
            listen<MempoolBlockEvent> evt => {
                _ = log_error!(self.handle_mempool_event(evt, pool_sender.clone()).await, "Handling Mempool Event");
            }

            listen<MempoolStatusEvent> evt => {
                _ = log_error!(self.handle_mempool_status_event(evt, pool_sender.clone()).await, "Handling Mempool Event");
            }

            listen<GenesisEvent> cmd => {
                if let GenesisEvent::GenesisBlock(signed_block) = cmd {
                    debug!("🌱  Genesis block received with validators {:?}", signed_block.consensus_proposal.staking_actions.clone());
                    let _= log_error!(self.handle_signed_block(signed_block, pool_sender.clone()).await, "Handling GenesisBlock Event");
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

                let _ = log_error!(self.handle_signed_block(streamed_block, pool_sender.clone()).await, format!("Handling streamed block {height}"));

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

            Some(request) = pool_receiver.recv() => {
                info!("Received message from the connection pool");

                let TcpEvent{ dest, data } = request;
                _ = self.start_streaming_to_peer(data.0, catchup_sender.clone(), &dest).await;
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
                        if pool_sender
                            .send(TcpCommand::Send(peer_ip.clone(), Box::new(DataAvailabilityEvent::SignedBlock(signed_block))))
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
        pool_sender: Sender<TcpCommand<DataAvailabilityEvent>>,
    ) -> Result<()> {
        match evt {
            MempoolBlockEvent::BuiltSignedBlock(signed_block) => {
                self.handle_signed_block(signed_block, pool_sender).await?;
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
        pool_sender: Sender<TcpCommand<DataAvailabilityEvent>>,
    ) -> Result<()> {
        pool_sender
            .send(TcpCommand::Broadcast(Box::new(
                DataAvailabilityEvent::MempoolStatusEvent(evt),
            )))
            .await?;

        Ok(())
    }

    async fn handle_signed_block(
        &mut self,
        block: SignedBlock,
        pool_sender: Sender<TcpCommand<DataAvailabilityEvent>>,
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
        self.add_processed_block(block, pool_sender.clone()).await;
        self.pop_buffer(hash, pool_sender).await;
        self.blocks.persist().context("Persisting blocks")?;
        Ok(())
    }

    async fn pop_buffer(
        &mut self,
        mut last_block_hash: ConsensusProposalHash,
        pool_sender: Sender<TcpCommand<DataAvailabilityEvent>>,
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
            self.add_processed_block(first_buffered, pool_sender.clone())
                .await;
        }
    }

    async fn add_processed_block(
        &mut self,
        block: SignedBlock,
        pool_sender: Sender<TcpCommand<DataAvailabilityEvent>>,
    ) {
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
                .map(|(tx_id, tx)| {
                    let variant: &'static str = (&tx.transaction_data).into();
                    format!("\n - 0x{} {}", tx_id.1, variant)
                })
                .collect::<Vec<_>>()
                .join("")
        );

        // TODO: use retain once async closures are supported ?
        //
        _ = log_error!(
            pool_sender
                .send(TcpCommand::Broadcast(Box::new(
                    DataAvailabilityEvent::SignedBlock(block.clone()),
                )))
                .await,
            "Sending block to tcp connection pool"
        );

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
            .filter_map(|block| block.map(|b| b.hashed()).ok())
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
        let Ok(mut stream) = RawDAListener::new(&ip, start).await else {
            bail!("Error occured setting up the DA listener");
        };
        self.catchup_task = Some(tokio::spawn(async move {
            loop {
                match stream.recv().await {
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

    use crate::data_availability::codec::{DataAvailabilityEvent, DataAvailabilityRequest};
    use crate::model::ValidatorPublicKey;
    use crate::tcp::TcpCommand;
    use crate::{
        bus::BusClientSender,
        consensus::CommittedConsensusProposal,
        mempool::MempoolBlockEvent,
        model::*,
        node_state::{
            module::{NodeStateBusClient, NodeStateEvent},
            NodeState,
        },
        utils::{conf::Conf, integration_test::find_available_port},
    };
    use staking::state::Staking;
    use tokio::sync::mpsc::{channel, Sender};

    use super::codec::codec_data_availability;
    use super::module_bus_client;
    use super::Blocks;
    use anyhow::Result;

    /// For use in integration tests
    pub struct DataAvailabilityTestCtx {
        pub node_state_bus: NodeStateBusClient,
        pub da: super::DataAvailability,
        pub node_state: NodeState,
    }

    impl DataAvailabilityTestCtx {
        pub async fn new(shared_bus: crate::bus::SharedMessageBus) -> Self {
            let tmpdir = tempfile::tempdir().unwrap().into_path();
            let blocks = Blocks::new(&tmpdir).unwrap();

            let bus = super::DABusClient::new_from_bus(shared_bus.new_handle()).await;
            let node_state_bus = NodeStateBusClient::new_from_bus(shared_bus).await;

            let mut config: Conf = Conf::new(None, None, None).unwrap();
            config.da_address = format!("127.0.0.1:{}", find_available_port().await);
            let da = super::DataAvailability {
                config: config.into(),
                bus,
                blocks,
                buffered_signed_blocks: Default::default(),
                need_catchup: false,
                catchup_task: None,
                catchup_height: None,
            };

            let node_state = NodeState::default();

            DataAvailabilityTestCtx {
                node_state_bus,
                da,
                node_state,
            }
        }

        pub async fn handle_signed_block(
            &mut self,
            block: SignedBlock,
            sender: Sender<TcpCommand<DataAvailabilityEvent>>,
        ) {
            self.da
                .handle_signed_block(block.clone(), sender)
                .await
                .unwrap();
            let full_block = self.node_state.handle_signed_block(&block);
            self.node_state_bus
                .send(NodeStateEvent::NewBlock(Box::new(full_block)))
                .unwrap();
        }
    }

    #[test_log::test]
    fn test_blocks() -> Result<()> {
        let tmpdir = tempfile::tempdir().unwrap().into_path();
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
        let tmpdir = tempfile::tempdir().unwrap().into_path();
        let blocks = Blocks::new(&tmpdir).unwrap();

        let (sender, _) = channel(1);
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
            da.handle_signed_block(block, sender.clone()).await.unwrap();
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
            codec_data_availability::connect("client_id".to_string(), config.da_address.clone())
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
                    data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
                .unwrap();
        }

        // End of the first stream

        let mut client =
            codec_data_availability::connect("client_id".to_string(), config.da_address)
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
        let (sender, _) = channel(1);
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
            block.consensus_proposal.parent_hash = block.hashed();
            block.consensus_proposal.slot = i;
        }
        blocks.reverse();
        for block in blocks {
            da_sender.handle_signed_block(block, sender.clone()).await;
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
        while let Some(streamed_block) = rx.recv().await {
            da_receiver
                .handle_signed_block(streamed_block.clone(), sender.clone())
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
                    data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
                .unwrap();
        }

        // We should still be subscribed
        while let Some(streamed_block) = rx.recv().await {
            da_receiver
                .handle_signed_block(streamed_block.clone(), sender.clone())
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
                    data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
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
