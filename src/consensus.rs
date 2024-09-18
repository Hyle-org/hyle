use anyhow::{Context, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    default::Default,
    fs,
    time::Duration,
};
use tokio::{select, sync::broadcast::Sender, time::sleep};
use tracing::info;

use crate::{
    bus::{command_response::CmdRespClient, SharedMessageBus},
    mempool::{MempoolCommand, MempoolResponse},
    model::{get_current_timestamp, Block, Hashable, Transaction},
    p2p::network::{ConsensusNetMessage, OutboundMessage},
    utils::{conf::SharedConf, logger::LogMe},
};

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    SaveOnDisk,
    GenerateNewBlock,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusEvent {
    CommitBlock { batch_id: String, block: Block },
}

// TODO: move struct to model.rs ?
#[derive(Serialize, Deserialize, Encode, Decode, Default)]
pub struct BFTRoundState {
    is_leader: bool,
    f: u64,
    slot: u64,
    view: u64,
    prepare_quorum_certificate: u64,
    prep_votes: HashMap<u64, bool>, // FIXME: set correct type (here it's replica_peer_id:vote)
    commit_quorum_certificate: HashMap<u64, u64>, // FIXME: set correct type (here it's slot:quorum certificate)
    confirm_ack: HashSet<u64>, // FIXME: set correct type (here it's replica_peer_id)
    block: Option<Block>,      // FIXME: Block ou cut ?
}

impl BFTRoundState {
    fn finish_round(&mut self) {
        self.slot += 1;
        self.view = 0;
        self.prep_votes = HashMap::default();
        self.confirm_ack = HashSet::default();
    }
}

// TODO: move struct to model.rs ?
#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode)]
pub struct ConsensusProposal {
    pub slot: u64,
    pub view: u64,
    pub previous_commit_quorum_certificate: u64, // FIXME. Set correct type
    pub block: Block,                            // FIXME: Block ou cut ?
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct Consensus {
    // For each replicas, leader keeps track of everu Prepare Vote
    bft_round_state: BFTRoundState,
    blocks: Vec<Block>,
    batch_id: u64,
    // Accumulated batches from mempool
    tx_batches: HashMap<String, Vec<Transaction>>,
    // Current proposed block
    current_block_batches: Vec<String>,
    // Once a block commits, we store it there (or not ?)
    // committed_block_batches: HashMap<String, HashMap<String, Vec<Transaction>>>,
}

impl Consensus {
    fn add_block(&mut self, block: Block) {
        self.blocks.push(block);
    }

    fn new_block(&mut self) -> Block {
        let last_block = self.blocks.last().unwrap();

        let mut all_txs = vec![];

        // prepare accumulated txs to feed the block
        // a previous batch can be there because of a previous block that failed to commit
        for (batch_id, txs) in self.tx_batches.iter() {
            all_txs.extend(txs.clone());
            self.current_block_batches.push(batch_id.clone());
        }

        // Start Consensus with following block
        let block = Block {
            parent_hash: last_block.hash(),
            height: last_block.height + 1,
            timestamp: get_current_timestamp(),
            txs: all_txs,
        };

        // Waiting for commit... TODO split this task

        // Commit block/if commit fails,
        // block won't be added, next block will try to add the txs
        self.add_block(block.clone());

        // Once commited we clean the state for the next block
        for cbb in self.current_block_batches.iter() {
            self.tx_batches.remove(cbb);
        }
        _ = self.current_block_batches.drain(0..);

        info!("New block {:?}", block);
        block
    }

    pub fn save_on_disk(&self) -> Result<()> {
        let mut writer = fs::File::create("data.bin").log_error("Create Ctx file")?;
        bincode::encode_into_std_write(self, &mut writer, bincode::config::standard())
            .log_error("Serializing Ctx chain")?;
        info!("Saved blockchain on disk with {} blocks", self.blocks.len());

        Ok(())
    }

    pub fn load_from_disk() -> Result<Self> {
        let mut reader = fs::File::open("data.bin").log_warn("Loading data from disk")?;
        let ctx: Consensus =
            bincode::decode_from_std_read(&mut reader, bincode::config::standard())
                .log_warn("Deserializing data from disk")?;
        info!("Loaded {} blocks from disk.", ctx.blocks.len());

        Ok(ctx)
    }

    fn verify_consensus_proposal(&self, _consensus_proposal: ConsensusProposal) -> bool {
        // TODO
        true
    }
    fn verify_commit_quorum_certificate(&self, _commit_quorum_certificate: u64) -> bool {
        // TODO
        true
    }

    fn is_next_leader(&self) -> bool {
        // TODO
        true
    }

    fn leader_id(&self) -> u64 {
        // TODO
        1
    }

    fn handle_net_message(
        &mut self,
        msg: ConsensusNetMessage,
        outbound_sender: Sender<OutboundMessage>,
    ) -> Result<()> {
        match msg {
            ConsensusNetMessage::Prepare(consensus_proposal) => {
                // Message received by replica.

                // Validate consensus_proposal slot, view, previous_qc and proposed block
                let vote = self.verify_consensus_proposal(consensus_proposal);
                // Responds PrepareVote message to leader with replica's vote on this proposal
                _ = outbound_sender
                    .send(OutboundMessage::send(
                        self.leader_id(),
                        ConsensusNetMessage::PrepareVote(vote),
                    ))
                    .context("Failed to send ConsensusNetMessage::Confirm msg on the bus")?;
                Ok(())
            }
            ConsensusNetMessage::PrepareVote(vote) => {
                // Message received by leader.

                let peer_id = 1; // FIXME: we need the peer_id of who sent the message

                // Save vote
                self.bft_round_state.prep_votes.insert(peer_id, vote);
                // Get matching vote count
                let validated_votes = self
                    .bft_round_state
                    .prep_votes
                    .iter()
                    .fold(0, |acc, (_, vote)| acc + if *vote { 1 } else { 0 });

                // Waits for at least 𝑛−𝑓 = 2𝑓 +1 matching PrepareVote messages
                if validated_votes == 2 * self.bft_round_state.f + 1 {
                    // Aggregates them into a *Prepare* Quorum Certificate
                    let prepare_quorum_certificate = 1; // FIXME

                    // if fast-path ... TODO
                    // else send Confirm message to replicas
                    _ = outbound_sender
                        .send(OutboundMessage::broadcast(ConsensusNetMessage::Confirm(
                            prepare_quorum_certificate,
                        )))
                        .context("Failed to send ConsensusNetMessage::Confirm msg on the bus")?;
                }
                Ok(())
            }
            ConsensusNetMessage::Confirm(prepare_quorum_certificate) => {
                // Message received by replica.

                // Buffers the *Prepare* Quorum Cerficiate
                self.bft_round_state.prepare_quorum_certificate = prepare_quorum_certificate;
                // Responds ConfirmAck to leader
                _ = outbound_sender
                    .send(OutboundMessage::send(
                        self.leader_id(),
                        ConsensusNetMessage::ConfirmAck,
                    ))
                    .context("Failed to send ConsensusNetMessage::ConfirmAck msg on the bus")?;
                Ok(())
            }
            ConsensusNetMessage::ConfirmAck => {
                // Message received by leader.

                let peer_id = 1; // FIXME: we need the peer_id of who sent the message

                // Save ConfirmAck
                self.bft_round_state.confirm_ack.insert(peer_id);

                if self.bft_round_state.confirm_ack.len()
                    > (2 * self.bft_round_state.f + 1).try_into().unwrap()
                {
                    // Aggregates 2𝑓 +1 matching ConfirmAck messages into a *Commit* Quorum Certificate
                    let commit_quorum_certificate = 1; // FIXME

                    // Send Commit message of this certificate to all replicas
                    _ = outbound_sender
                        .send(OutboundMessage::broadcast(ConsensusNetMessage::Commit(
                            commit_quorum_certificate,
                        )))
                        .context("Failed to send ConsensusNetMessage::Confirm msg on the bus")?;
                }
                // Update its bft roun state
                self.bft_round_state.is_leader = false;
                self.bft_round_state.finish_round();

                Ok(())
            }
            ConsensusNetMessage::Commit(commit_quorum_certificate) => {
                // Message received by replica.

                // Verifies and save the *Commit* Quorum Certificate
                self.verify_commit_quorum_certificate(commit_quorum_certificate);
                self.bft_round_state
                    .commit_quorum_certificate
                    .insert(self.bft_round_state.slot, commit_quorum_certificate);

                // Applies and add new block to its NodeState
                // TODO

                // Updates its bft round state
                self.bft_round_state.finish_round();

                if !self.is_next_leader() {
                    return Ok(());
                }
                // Starts new slot
                self.bft_round_state.is_leader = true;

                // Creates ConsensusProposal
                let consensus_proposal = ConsensusProposal {
                    slot: self.bft_round_state.slot,
                    view: self.bft_round_state.view,
                    previous_commit_quorum_certificate: commit_quorum_certificate,
                    block: Block::default(), // FIXME
                };
                // Send Prepare message to all replicas
                _ = outbound_sender
                    .send(OutboundMessage::broadcast(ConsensusNetMessage::Prepare(
                        consensus_proposal,
                    )))
                    .context("Failed to send ConsensusNetMessage::Prepare msg on the bus")?;

                Ok(())
            }
        }
    }

    async fn handle_command(
        &mut self,
        msg: ConsensusCommand,
        bus: SharedMessageBus,
        consensus_event_sender: Sender<ConsensusEvent>,
    ) -> Result<()> {
        match msg {
            ConsensusCommand::GenerateNewBlock => {
                self.batch_id += 1;
                if let Some(res) = bus
                    .request(MempoolCommand::CreatePendingBatch {
                        id: self.batch_id.to_string(),
                    })
                    .await?
                {
                    match res {
                        MempoolResponse::PendingBatch { id, txs } => {
                            info!("Received pending batch {} with {} txs", &id, &txs.len());
                            self.tx_batches.insert(id.clone(), txs);
                            let block = self.new_block();
                            // send to internal bus
                            _ = consensus_event_sender
                                .send(ConsensusEvent::CommitBlock {
                                    batch_id: id,
                                    block: block.clone(),
                                })
                                .context(
                                    "Failed to send ConsensusEvent::CommitBlock msg on the bus",
                                )?;
                            // send to network
                            // _ = outbound_sender
                            //     .send(OutboundMessage::broadcast((ConsensusNetMessage::CommitBlock(block))).context("Failed to send ConsensusNetMessage::CommitBlock msg on the bus")?;
                        }
                    }
                }
                Ok(())
            }

            ConsensusCommand::SaveOnDisk => {
                _ = self.save_on_disk();
                Ok(())
            }
        }
    }

    pub async fn start(&mut self, bus: SharedMessageBus, config: SharedConf) -> Result<()> {
        let interval = config.storage.interval;
        let is_master = config.peers.is_empty();
        let outbound_sender = bus.sender::<OutboundMessage>().await;
        let consensus_event_sender = bus.sender::<ConsensusEvent>().await;
        let consensus_command_sender = bus.sender::<ConsensusCommand>().await;
        let mut consensus_command_receiver = bus.receiver::<ConsensusCommand>().await;
        let mut consensus_net_message_receiver = bus.receiver::<ConsensusNetMessage>().await;

        if is_master {
            info!(
                "No peers configured, starting as master generating blocks every {} seconds",
                interval
            );

            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(interval)).await;

                    _ = consensus_command_sender
                        .send(ConsensusCommand::GenerateNewBlock)
                        .log_error("Cannot send message over channel");
                    _ = consensus_command_sender
                        .send(ConsensusCommand::SaveOnDisk)
                        .log_error("Cannot send message over channel");
                }
            });
        }

        loop {
            let shared_bus = SharedMessageBus::new_handle(&bus);
            select! {
                Ok(msg) = consensus_command_receiver.recv() => {
                    _ = self.handle_command(msg, shared_bus,
                    consensus_event_sender.clone()).await;
                }
                Ok(msg) = consensus_net_message_receiver.recv() => {
                    let _ = self.handle_net_message(msg, outbound_sender.clone());
                }
            }
        }
    }
}

impl Default for Consensus {
    fn default() -> Self {
        Self {
            blocks: vec![Block::default()],
            batch_id: 0,
            tx_batches: HashMap::new(),
            current_block_batches: vec![],
            bft_round_state: BFTRoundState::default(),
        }
    }
}
