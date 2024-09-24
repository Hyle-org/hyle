//! Handles all consensus logic up to block commitment.

use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    default::Default,
    fs,
    path::Path,
    time::Duration,
};
use tokio::{sync::broadcast::Sender, time::sleep};
use tracing::{info, warn};

use crate::{
    bus::{command_response::CmdRespClient, BusMessage, SharedMessageBus},
    handle_messages,
    mempool::{MempoolCommand, MempoolResponse},
    model::{get_current_timestamp, Block, BlockHash, Hashable, Transaction},
    p2p::network::{OutboundMessage, SignedWithId},
    utils::{conf::SharedConf, crypto::BlstCrypto, logger::LogMe},
    validator_registry::{ValidatorId, ValidatorRegistry, ValidatorRegistryNetMessage},
};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode)]
pub enum ConsensusNetMessage {
    StartNewSlot,
    Prepare(ConsensusProposal),
    PrepareVote(bool), // FIXME: set correct type
    Confirm(u64),      // FIXME: set correct type (*Prepare* Quorum Certificate)
    ConfirmAck,
    Commit(u64), // FIXME: set correct type (*Commit* Quorum Certificate)
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    SaveOnDisk,
    GenerateNewBlock,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusEvent {
    CommitBlock { batch_id: String, block: Block },
}

impl BusMessage for ConsensusCommand {}
impl BusMessage for ConsensusEvent {}
impl BusMessage for ConsensusNetMessage {}

// TODO: move struct to model.rs ?
#[derive(Serialize, Deserialize, Encode, Decode, Default)]
pub struct BFTRoundState {
    is_leader: bool,
    slot: u64,
    view: u64,
    prepare_quorum_certificate: u64,
    prep_votes: HashMap<ValidatorId, bool>, // FIXME: set correct type (here it's validator_id:vote)
    commit_quorum_certificate: HashMap<u64, u64>, // FIXME: set correct type (here it's slot:quorum certificate)
    confirm_ack: HashSet<ValidatorId>, // FIXME: set correct type (here it's validator_id)
    block: Option<Block>,              // FIXME: Block ou cut ?
}

impl BFTRoundState {
    fn finish_round(
        &mut self,
        consensus_command_sender: &Sender<ConsensusCommand>,
    ) -> Result<(), Error> {
        self.slot += 1;
        self.view = 0;
        self.prep_votes = HashMap::default();
        self.confirm_ack = HashSet::default();

        // Applies and add new block to its NodeState
        _ = consensus_command_sender
            .send(ConsensusCommand::GenerateNewBlock)
            .context("Cannot send message over channel")?;
        _ = consensus_command_sender
            .send(ConsensusCommand::SaveOnDisk)
            .context("Cannot send message over channel")?;
        Ok(())
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
    validators: ValidatorRegistry,
    bft_round_state: BFTRoundState,
    pub blocks: Vec<Block>,
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
        let last_block = self.blocks.last();

        let parent_hash = last_block
            .map(|b| b.hash())
            .unwrap_or(BlockHash::new("000"));
        let parent_height = last_block.map(|b| b.height).unwrap_or_default();

        let mut all_txs = vec![];

        // prepare accumulated txs to feed the block
        // a previous batch can be there because of a previous block that failed to commit
        for (batch_id, txs) in self.tx_batches.iter() {
            all_txs.extend(txs.clone());
            self.current_block_batches.push(batch_id.clone());
        }

        // Start Consensus with following block
        let block = Block {
            parent_hash,
            height: parent_height + 1,
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

        info!("New block {}", block.height);
        block
    }

    pub fn save_on_disk(&self) -> Result<()> {
        let mut writer = fs::File::create("data.bin").log_error("Create Ctx file")?;
        bincode::encode_into_std_write(self, &mut writer, bincode::config::standard())
            .log_error("Serializing Ctx chain")?;
        info!("Saved blockchain on disk with {} blocks", self.blocks.len());

        Ok(())
    }

    pub fn load_from_disk(data_directory: &Path) -> Result<Self> {
        let mut reader =
            fs::File::open(data_directory.join("data.bin")).log_warn("Loading data from disk")?;
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

    fn verify_prepare_quorum_certificate(&self, _prepare_quorum_certificate: u64) -> bool {
        // TODO
        true
    }

    fn verify_commit_quorum_certificate(&self, _commit_quorum_certificate: u64) -> bool {
        // TODO
        true
    }

    fn is_leader(&self) -> bool {
        // TODO
        true
    }

    fn get_next_leader(&self) -> ValidatorId {
        // TODO
        ValidatorId("node-1".to_owned())
    }

    fn leader_id(&self) -> ValidatorId {
        // TODO
        ValidatorId("node-1".to_owned())
    }

    fn handle_net_message(
        &mut self,
        msg: SignedWithId<ConsensusNetMessage>,
        crypto: &BlstCrypto,
        outbound_sender: &Sender<OutboundMessage>,
        consensus_command_sender: &Sender<ConsensusCommand>,
    ) -> Result<(), Error> {
        if !self.validators.check_signed(&msg)? {
            bail!("Invalid signature for message {:?}", msg);
        }

        match msg.msg {
            ConsensusNetMessage::StartNewSlot => {
                // Message received by leader.

                // Verifies that previous slot has a valid *Commit* Quorum Certificate

                // Verifies that to-be-built block is large enough (?)

                // Updates its bft state
                self.bft_round_state.is_leader = true;

                // Creates new block

                // Creates ConsensusProposal
                // let previous_commit_quorum_certificate = match self
                //     .bft_round_state
                //     .commit_quorum_certificate
                //     .get(&self.bft_round_state.slot)
                // {
                //     Some(v) => v,
                //     None => {
                //         bail!("Can't start new slot: unknown previous commit quorum certificate")
                //     }
                // };
                let consensus_proposal = ConsensusProposal {
                    slot: self.bft_round_state.slot,
                    view: self.bft_round_state.view,
                    previous_commit_quorum_certificate: 1,
                    block: self.new_block(),
                };

                // Broadcasts Prepare message to all replicas
                _ = outbound_sender
                    .send(OutboundMessage::broadcast(Self::sign_net_message(
                        crypto,
                        ConsensusNetMessage::Prepare(consensus_proposal),
                    )?))
                    .context("Failed to broadcast ConsensusNetMessage::Prepare msg on the bus")?;

                Ok(())
            }
            ConsensusNetMessage::Prepare(consensus_proposal) => {
                // Message received by replica.

                // Validate message comes from the correct leader
                if !msg.validators.contains(&self.leader_id()) {
                    // fail
                }
                // Validate consensus_proposal slot, view, previous_qc and proposed block
                let vote = self.verify_consensus_proposal(consensus_proposal);
                // Responds PrepareVote message to leader with replica's vote on this proposal
                _ = outbound_sender
                    .send(OutboundMessage::send(
                        self.leader_id(),
                        Self::sign_net_message(crypto, ConsensusNetMessage::PrepareVote(vote))?,
                    ))
                    .context("Failed to send ConsensusNetMessage::Confirm msg on the bus")?;
                Ok(())
            }
            ConsensusNetMessage::PrepareVote(vote) => {
                // Message received by leader.

                // Save vote
                msg.validators.iter().for_each(|v| {
                    self.bft_round_state.prep_votes.insert(v.clone(), vote);
                });
                // Get matching vote count
                let validated_votes = self
                    .bft_round_state
                    .prep_votes
                    .iter()
                    .fold(0, |acc, (_, vote)| acc + if *vote { 1 } else { 0 });

                // Waits for at least 𝑛−𝑓 = 2𝑓+1 matching PrepareVote messages
                let f: usize = self.validators.get_validators_count() / 3;
                if validated_votes == 2 * f + 1 {
                    // Aggregates them into a *Prepare* Quorum Certificate
                    let prepare_quorum_certificate = rand::thread_rng().gen(); // FIXME

                    // if fast-path ... TODO
                    // else send Confirm message to replicas
                    _ = outbound_sender
                        .send(OutboundMessage::broadcast(Self::sign_net_message(
                            crypto,
                            ConsensusNetMessage::Confirm(prepare_quorum_certificate),
                        )?))
                        .context(
                            "Failed to broadcast ConsensusNetMessage::Confirm msg on the bus",
                        )?;
                }
                Ok(())
            }
            ConsensusNetMessage::Confirm(prepare_quorum_certificate) => {
                // Message received by replica.

                // Verifies and save the *Prepare* Quorum Certificate
                if !self.verify_prepare_quorum_certificate(prepare_quorum_certificate) {
                    // Fail
                }
                // Buffers the *Prepare* Quorum Cerficiate
                self.bft_round_state.prepare_quorum_certificate = prepare_quorum_certificate;
                // Responds ConfirmAck to leader
                _ = outbound_sender
                    .send(OutboundMessage::send(
                        self.leader_id(),
                        Self::sign_net_message(crypto, ConsensusNetMessage::ConfirmAck)?,
                    ))
                    .context("Failed to send ConsensusNetMessage::ConfirmAck msg on the bus")?;
                Ok(())
            }
            ConsensusNetMessage::ConfirmAck => {
                // Message received by leader.

                // Save ConfirmAck
                msg.validators.iter().for_each(|v| {
                    self.bft_round_state.confirm_ack.insert(v.clone());
                });

                let f: usize = self.validators.get_validators_count() / 3;
                if self.bft_round_state.confirm_ack.len() >= (2 * f + 1) {
                    // Aggregates 2𝑓 +1 matching ConfirmAck messages into a *Commit* Quorum Certificate
                    let commit_quorum_certificate = rand::thread_rng().gen(); // FIXME

                    // Send Commit message of this certificate to all replicas
                    _ = outbound_sender
                        .send(OutboundMessage::broadcast(Self::sign_net_message(
                            crypto,
                            ConsensusNetMessage::Commit(commit_quorum_certificate),
                        )?))
                        .context(
                            "Failed to broadcast ConsensusNetMessage::Confirm msg on the bus",
                        )?;

                    // Finishes the bft round
                    self.bft_round_state.is_leader = false;
                    self.bft_round_state
                        .finish_round(consensus_command_sender)?;
                }

                Ok(())
            }
            ConsensusNetMessage::Commit(commit_quorum_certificate) => {
                // Message received by replica.

                // Verifies and save the *Commit* Quorum Certificate
                self.verify_commit_quorum_certificate(commit_quorum_certificate);
                self.bft_round_state
                    .commit_quorum_certificate
                    .insert(self.bft_round_state.slot, commit_quorum_certificate);

                // Finishes the bft round
                self.bft_round_state
                    .finish_round(consensus_command_sender)?;

                // Sends message to next leader to start new slot
                if self.is_leader() {
                    // Send Prepare message to all replicas
                    _ = outbound_sender
                        .send(OutboundMessage::send(
                            self.get_next_leader(),
                            Self::sign_net_message(crypto, ConsensusNetMessage::StartNewSlot)?,
                        ))
                        .context(
                            "Failed to send ConsensusNetMessage::SartNewSlot msg on the bus",
                        )?;
                }
                Ok(())
            }
        }
    }

    async fn handle_command(
        &mut self,
        msg: ConsensusCommand,
        bus: &SharedMessageBus,
        _crypto: &BlstCrypto,
        consensus_event_sender: &Sender<ConsensusEvent>,
        _outbound_sender: &Sender<OutboundMessage>,
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

    pub async fn start(
        &mut self,
        bus: SharedMessageBus,
        config: SharedConf,
        crypto: BlstCrypto,
    ) -> Result<()> {
        // let interval = config.storage.interval;
        // let node_is_alone = config.peers.is_empty();
        // FIXME: should be removed
        let temp_outbound_sender = bus.sender::<OutboundMessage>().await;
        let outbound_sender = bus.sender::<OutboundMessage>().await;
        let consensus_event_sender = bus.sender::<ConsensusEvent>().await;
        let consensus_command_sender = bus.sender::<ConsensusCommand>().await;

        // FIXME: pour le moment node-1 sera le validateur à chaque slot
        if config.id == ValidatorId("node-1".to_owned()) {
            self.bft_round_state.is_leader = true;
        }
        // FIXME: node-2 sera le validateur qui déclanchera le démarrage du premier slot
        if config.id == ValidatorId("node-2".to_owned()) {
            let next_leader = self.get_next_leader();
            if let Ok(signed_message) =
                Self::sign_net_message(&crypto, ConsensusNetMessage::StartNewSlot)
            {
                tokio::spawn(async move {
                    // FIXME: on attend qlq secondes pour laisser le temps au bus de setup l'écoute
                    sleep(Duration::from_secs(2)).await;
                    _ = temp_outbound_sender
                        .send(OutboundMessage::send(next_leader, signed_message))
                        .log_error(
                            "Failed to send ConsensusNetMessage::StartNewSlot msg on the bus",
                        );
                });
            }
        }

        handle_messages! {
            on_bus bus,
            listen<ConsensusCommand> cmd => {
                match self.handle_command(cmd, &bus, &crypto, &consensus_event_sender, &outbound_sender).await{
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling consensus command: {:#}", e),
                }
            }
            listen<SignedWithId<ConsensusNetMessage>> cmd => {
                // FIXME: remove info message
                info!("[{}] Received consensus message: {:?}", config.id, cmd);
                sleep(Duration::from_secs(1)).await;
                match self.handle_net_message(cmd, &crypto, &outbound_sender, &consensus_command_sender) {
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling consensus net message: {:#}", e),
                }
            }
            listen<ValidatorRegistryNetMessage> cmd => {
                self.validators.handle_net_message(cmd);
            }
        }
    }

    fn sign_net_message(
        crypto: &BlstCrypto,
        msg: ConsensusNetMessage,
    ) -> Result<SignedWithId<ConsensusNetMessage>> {
        crypto.sign(msg)
    }
}

impl Default for Consensus {
    fn default() -> Self {
        Self {
            blocks: vec![Block::default()],
            validators: ValidatorRegistry::default(),
            batch_id: 0,
            tx_batches: HashMap::new(),
            current_block_batches: vec![],
            bft_round_state: BFTRoundState::default(),
        }
    }
}
