//! Minimal block storage layer for data availability.

pub mod codec;

mod blocks_fjall;
mod blocks_memory;

// Pick one of the two implementations
use blocks_fjall::Blocks;
//use blocks_memory::Blocks;

use crate::{
    bus::{BusClientSender, BusMessage},
    consensus::{ConsensusCommand, ConsensusEvent},
    genesis::GenesisEvent,
    indexer::da_listener::RawDAListener,
    mempool::MempoolEvent,
    model::*,
    module_handle_messages,
    p2p::network::PeerEvent,
    utils::{
        logger::LogMe,
        modules::{module_bus_client, Module},
    },
};
use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use core::str;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use tracing::{debug, error, info, trace, warn};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataEvent {
    OrderedSignedBlock(SignedBlock),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub struct DataAvailabilityStreamRequest(pub String, pub BlockHeight);

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, Eq, PartialEq)]
pub enum DataAvailabilityStreamEvent {
    Send { peer: String, block: SignedBlock },
    Broadcast { block: SignedBlock },
}

impl BusMessage for DataAvailabilityStreamRequest {}
impl BusMessage for DataAvailabilityStreamEvent {}
impl BusMessage for DataEvent {}

module_bus_client! {
#[derive(Debug)]
struct DABusClient {
    sender(ConsensusCommand),
    sender(DataAvailabilityStreamEvent),
    sender(DataEvent),
    receiver(DataAvailabilityStreamRequest),
    receiver(ConsensusEvent),
    receiver(MempoolEvent),
    receiver(GenesisEvent),
    receiver(PeerEvent),
}
}

#[derive(Debug)]
pub struct DataAvailability {
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
        let (catchup_block_sender, mut catchup_block_receiver) =
            tokio::sync::mpsc::channel::<SignedBlock>(100);

        // TODO: this is a soft cap on the number of peers we can stream to.
        let (catchup_sender, mut catchup_receiver) =
            tokio::sync::mpsc::channel::<(Vec<ConsensusProposalHash>, String)>(100);

        module_handle_messages! {
            on_bus self.bus,
            listen<MempoolEvent> evt => {
                _ = self.handle_mempool_event(evt).await.log_error("Handling Mempool Event");
            }

            listen<GenesisEvent> cmd => {
                if let GenesisEvent::GenesisBlock(signed_block) = cmd {
                    debug!("🌱  Genesis block received with validators {:?}", signed_block.consensus_proposal.staking_actions.clone());
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

            listen<DataAvailabilityStreamRequest> request => {
                let _ = self.handle_da_server_request(request, catchup_sender.clone()).await.log_error("Handling DA request");
            }

            Some(streamed_block) = catchup_block_receiver.recv() => {
                let height = streamed_block.height().0;

                self.handle_signed_block(streamed_block).await;

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

            // Send one block to a peer as part of "catchup",
            // once we have sent all blocks the peer is presumably synchronised.
            Some((mut block_hashes, peer_ip)) = catchup_receiver.recv() => {
                let hash = block_hashes.pop();

                trace!("📡  Sending block {:?} to peer {}", &hash, &peer_ip);
                if let Some(hash) = hash {
                    if let Ok(Some(signed_block)) = self.blocks.get(&hash)
                    {
                        // Errors will be handled when sending new blocks, ignore here.
                        let _ = self.bus.send(DataAvailabilityStreamEvent::Send {
                            peer: peer_ip.clone(),
                            block: signed_block
                        });
                        let _ = catchup_sender.send((block_hashes, peer_ip)).await;
                    }
                }
            }

        };

        Ok(())
    }

    async fn handle_mempool_event(&mut self, evt: MempoolEvent) -> Result<()> {
        match evt {
            MempoolEvent::BuiltSignedBlock(signed_block) => {
                self.handle_signed_block(signed_block).await;
            }
            MempoolEvent::StartedBuildingBlocks(height) => {
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

    async fn handle_da_server_request(
        &mut self,
        DataAvailabilityStreamRequest(peer_addr, start_height): DataAvailabilityStreamRequest,
        catchup_sender: tokio::sync::mpsc::Sender<(Vec<ConsensusProposalHash>, String)>,
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
            .filter_map(|block| block.map(|b| b.hash()).ok())
            .collect();
        processed_block_hashes.reverse();

        catchup_sender
            .send((processed_block_hashes, peer_addr))
            .await
            .context("test")
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
            #[allow(
                clippy::unwrap_used,
                reason = "Must exist as checked in the while above"
            )]
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
            "new block {} {} with {} txs",
            block.height(),
            block.hash(),
            block.txs().len(),
        );
        debug!(
            "Transactions: {:#?}",
            block.txs().iter().map(|tx| tx.hash().0).collect::<Vec<_>>()
        );

        // Send the block to NodeState for processing
        _ = self
            .bus
            .send(DataAvailabilityStreamEvent::Broadcast {
                block: block.clone(),
            })
            .log_error("broadcasting block on tcp");

        _ = self
            .bus
            .send(DataEvent::OrderedSignedBlock(block))
            .log_error("Sending OrderedSignedBlock");
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
                    Some(Ok(streamed_block)) => {
                        info!(
                            "📦 Received block (height {}) from stream",
                            streamed_block.consensus_proposal.slot
                        );
                        // TODO: we should wait if the stream is full.
                        if let Err(e) = sender.send(streamed_block).await {
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
    #![allow(clippy::indexing_slicing)]

    use crate::bus::BusClientReceiver;
    use crate::model::ValidatorPublicKey;
    use crate::{
        bus::BusClientSender,
        consensus::CommittedConsensusProposal,
        mempool::MempoolEvent,
        model::*,
        node_state::{
            module::{NodeStateBusClient, NodeStateEvent},
            NodeState,
        },
    };
    use staking::state::Staking;

    use super::{module_bus_client, DataAvailabilityStreamEvent};
    use super::{Blocks, DataAvailabilityStreamRequest};
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

            let da = super::DataAvailability {
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

        pub async fn handle_signed_block(&mut self, block: SignedBlock) {
            self.da.handle_signed_block(block.clone()).await;
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
        sender(MempoolEvent),
        sender(DataAvailabilityStreamRequest),
        receiver(DataAvailabilityStreamEvent),
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
        let mut test_bus_client = TestBusClient::new_from_bus(global_bus).await;

        let mut da = super::DataAvailability {
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

        _ = test_bus_client.send(DataAvailabilityStreamRequest(
            "peer".to_string(),
            BlockHeight(0),
        ));

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let mut broadcasts = vec![];
        // Lets remove broadcast messages genenrated by handle_signed_block calls
        while let Ok(cmd) = test_bus_client.try_recv() {
            if let DataAvailabilityStreamEvent::Broadcast { block } = cmd {
                broadcasts.push(block.height().0);
                if broadcasts.len() == 14 {
                    break;
                }
            } else {
                panic!(
                    "message {:?} should match {}",
                    cmd,
                    stringify!(DataAvailabilityStreamEvent::Broadcast {})
                );
            }
        }

        assert_eq!(broadcasts, (0..14).collect::<Vec<u64>>());

        let mut heights_received = vec![];

        while let Ok(cmd) = test_bus_client.try_recv() {
            if let DataAvailabilityStreamEvent::Send { peer, block } = cmd {
                assert_eq!(peer, "peer");
                heights_received.push(block.height().0);
                if heights_received.len() == 14 {
                    break;
                }
            } else {
                panic!(
                    "message {:?} should match {}",
                    cmd,
                    stringify!(DataAvailabilityStreamEvent::Send {})
                );
            }
        }
        assert_eq!(heights_received, (0..14).collect::<Vec<u64>>());

        let mut ccp = CommittedConsensusProposal {
            staking: Staking::default(),
            consensus_proposal: ConsensusProposal::default(),
            certificate: AggregateSignature {
                signature: crate::model::Signature("signature".into()),
                validators: vec![],
            },
        };

        for i in 14..18 {
            ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
            ccp.consensus_proposal.slot = i;
            test_bus_client
                .send(MempoolEvent::BuiltSignedBlock(SignedBlock {
                    data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
                    certificate: ccp.certificate.clone(),
                    consensus_proposal: ccp.consensus_proposal.clone(),
                }))
                .unwrap();
        }

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        // End of the first stream

        while let Ok(cmd) = test_bus_client.try_recv() {
            if let DataAvailabilityStreamEvent::Broadcast { block } = cmd {
                heights_received.push(block.height().0);
                if heights_received.len() == 18 {
                    break;
                }
            } else {
                panic!(
                    "message {:?} should match {}",
                    cmd,
                    stringify!(DataAvailabilityStreamEvent::Broadcast {})
                );
            }
        }

        assert_eq!(heights_received, (0..18).collect::<Vec<u64>>());
    }
    // #[test_log::test(tokio::test)]
    // async fn test_da_catchup() {
    //     let sender_global_bus = crate::bus::SharedMessageBus::new(
    //         crate::bus::metrics::BusMetrics::global("global".to_string()),
    //     );
    //     let mut block_sender = TestBusClient::new_from_bus(sender_global_bus.new_handle()).await;
    //     let mut da_sender = DataAvailabilityTestCtx::new(sender_global_bus).await;

    //     let receiver_global_bus = crate::bus::SharedMessageBus::new(
    //         crate::bus::metrics::BusMetrics::global("global".to_string()),
    //     );
    //     let mut da_receiver = DataAvailabilityTestCtx::new(receiver_global_bus).await;

    //     // Push some blocks to the sender
    //     let mut block = SignedBlock::default();
    //     let mut blocks = vec![];
    //     for i in 1..11 {
    //         blocks.push(block.clone());
    //         block.consensus_proposal.parent_hash = block.hash();
    //         block.consensus_proposal.slot = i;
    //     }
    //     blocks.reverse();
    //     for block in blocks {
    //         da_sender.handle_signed_block(block).await;
    //     }

    //     tokio::spawn(async move {
    //         da_sender.da.start().await.unwrap();
    //     });

    //     // wait until it's up
    //     tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    //     // Setup done
    //     let (tx, mut rx) = tokio::sync::mpsc::channel(200);
    //     da_receiver
    //         .da
    //         .ask_for_catchup_blocks(da_sender_address.clone(), tx.clone())
    //         .await
    //         .expect("Error while asking for catchup blocks");

    //     let mut received_blocks = vec![];
    //     while let Some(streamed_block) = rx.recv().await {
    //         da_receiver
    //             .handle_signed_block(streamed_block.clone())
    //             .await;
    //         received_blocks.push(streamed_block);
    //         if received_blocks.len() == 10 {
    //             break;
    //         }
    //     }
    //     assert_eq!(received_blocks.len(), 10);
    //     assert_eq!(received_blocks[0].height(), BlockHeight(0));
    //     assert_eq!(received_blocks[9].height(), BlockHeight(9));

    //     // Add a few blocks (via bus to avoid mutex)
    //     let mut ccp = CommittedConsensusProposal {
    //         staking: Staking::default(),
    //         consensus_proposal: ConsensusProposal::default(),
    //         certificate: AggregateSignature::default(),
    //     };

    //     for i in 10..15 {
    //         ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
    //         ccp.consensus_proposal.slot = i;
    //         block_sender
    //             .send(MempoolEvent::BuiltSignedBlock(SignedBlock {
    //                 data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
    //                 certificate: ccp.certificate.clone(),
    //                 consensus_proposal: ccp.consensus_proposal.clone(),
    //             }))
    //             .unwrap();
    //     }

    //     // We should still be subscribed
    //     while let Some(streamed_block) = rx.recv().await {
    //         da_receiver
    //             .handle_signed_block(streamed_block.clone())
    //             .await;
    //         received_blocks.push(streamed_block);
    //         if received_blocks.len() == 15 {
    //             break;
    //         }
    //     }
    //     assert_eq!(received_blocks.len(), 15);
    //     assert_eq!(received_blocks[14].height(), BlockHeight(14));

    //     // Unsub
    //     // TODO: ideally via processing the correct message
    //     da_receiver.da.catchup_task.take().unwrap().abort();

    //     // Add a few blocks (via bus to avoid mutex)
    //     let mut ccp = CommittedConsensusProposal {
    //         staking: Staking::default(),
    //         consensus_proposal: ConsensusProposal::default(),
    //         certificate: AggregateSignature::default(),
    //     };

    //     for i in 15..20 {
    //         ccp.consensus_proposal.parent_hash = ccp.consensus_proposal.hash();
    //         ccp.consensus_proposal.slot = i;
    //         block_sender
    //             .send(MempoolEvent::BuiltSignedBlock(SignedBlock {
    //                 data_proposals: vec![(ValidatorPublicKey("".into()), vec![])],
    //                 certificate: ccp.certificate.clone(),
    //                 consensus_proposal: ccp.consensus_proposal.clone(),
    //             }))
    //             .unwrap();
    //     }

    //     // Resubscribe - we should only receive the new ones.
    //     da_receiver
    //         .da
    //         .ask_for_catchup_blocks(da_sender_address, tx)
    //         .await
    //         .expect("Error while asking for catchup blocks");

    //     let mut received_blocks = vec![];
    //     while let Some(block) = rx.recv().await {
    //         received_blocks.push(block);
    //         if received_blocks.len() == 5 {
    //             break;
    //         }
    //     }
    //     assert_eq!(received_blocks.len(), 5);
    //     assert_eq!(received_blocks[0].height(), BlockHeight(15));
    //     assert_eq!(received_blocks[4].height(), BlockHeight(19));
    // }
}
