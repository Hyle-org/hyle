//! Handles all consensus logic up to block commitment.

use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    collections::{HashMap, HashSet},
    default::Default,
    fs,
    io::Write,
    path::Path,
    time::Duration,
};
use tokio::time::sleep;
use tracing::{debug, info, warn};

use crate::{
    bus::{bus_client, command_response::Query, BusMessage, SharedMessageBus},
    handle_messages,
    mempool::{MempoolCommand, MempoolEvent, MempoolResponse},
    model::{
        get_current_timestamp, Block, BlockHash, BlockHeight, Hashable, SharedRunContext,
        Transaction,
    },
    p2p::network::{OutboundMessage, Signature, Signed, SignedWithId, SignedWithKey},
    utils::{
        conf::SharedConf,
        crypto::{BlstCrypto, SharedBlstCrypto},
        logger::LogMe,
        modules::Module,
    },
    validator_registry::{
        ValidatorId, ValidatorPublicKey, ValidatorRegistry, ValidatorRegistryNetMessage,
    },
};

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub enum ConsensusNetMessage {
    StartNewSlot,
    Prepare(ConsensusProposal),
    PrepareVote(ConsensusProposalHash),
    Confirm(ConsensusProposalHash, QuorumCertificate),
    ConfirmAck(ConsensusProposalHash),
    Commit(ConsensusProposalHash, QuorumCertificate),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    SaveOnDisk,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusEvent {
    CommitBlock { block: Block },
}

impl BusMessage for ConsensusCommand {}
impl BusMessage for ConsensusEvent {}
impl BusMessage for ConsensusNetMessage {}

// TODO: move struct to model.rs ?
#[derive(Serialize, Deserialize, Encode, Decode, Default)]
pub struct BFTRoundState {
    consensus_proposal: ConsensusProposal,
    slot: Slot,
    view: View,
    prepare_votes: HashSet<Signed<ConsensusNetMessage, ValidatorPublicKey>>,
    prepare_quorum_certificate: QuorumCertificate,
    confirm_ack: HashSet<Signed<ConsensusNetMessage, ValidatorPublicKey>>,
    commit_quorum_certificates: HashMap<Slot, (ConsensusProposalHash, QuorumCertificate)>,
}

#[derive(Serialize, Deserialize, Encode, Decode, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct QuorumCertificate {
    signature: Signature,
    validators: Vec<ValidatorPublicKey>,
}

impl Hashable<QuorumCertificateHash> for QuorumCertificate {
    fn hash(&self) -> QuorumCertificateHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{:?}", self.signature);
        _ = write!(hasher, "{:?}", self.validators);
        return QuorumCertificateHash(hasher.finalize().as_slice().to_owned());
    }
}

pub type Slot = u64;
pub type View = u64;

// TODO: move struct to model.rs ?
#[derive(Debug, Clone, Default, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ConsensusProposal {
    slot: Slot,
    view: u64,
    previous_consensus_proposal_hash: ConsensusProposalHash,
    previous_commit_quorum_certificate: QuorumCertificate,
    block: Block, // FIXME: Block ou cut ?
}

impl Hashable<ConsensusProposalHash> for ConsensusProposal {
    fn hash(&self) -> ConsensusProposalHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.slot);
        _ = write!(hasher, "{}", self.view);
        _ = write!(hasher, "{:?}", self.previous_consensus_proposal_hash);
        _ = write!(hasher, "{:?}", self.previous_commit_quorum_certificate);
        _ = write!(hasher, "{}", self.block);
        return ConsensusProposalHash(hasher.finalize().as_slice().to_owned());
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Default)]
pub struct ConsensusProposalHash(Vec<u8>);
#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Default)]
pub struct QuorumCertificateHash(Vec<u8>);

bus_client! {
struct ConsensusBusClient {
    sender(OutboundMessage),
    sender(ConsensusEvent),
    sender(ConsensusCommand),
    sender(Query<MempoolCommand, MempoolResponse>),
    receiver(ValidatorRegistryNetMessage),
    receiver(ConsensusCommand),
    receiver(MempoolEvent),
    receiver(SignedWithId<ConsensusNetMessage>),
}
}

#[derive(Serialize, Deserialize, Encode, Decode)]
pub struct Consensus {
    // TODO: should be removed
    is_next_leader: bool,
    validators: ValidatorRegistry,
    bft_round_state: BFTRoundState,
    // FIXME: pub is here for testing
    pub blocks: Vec<Block>,
    pending_batches: Vec<Vec<Transaction>>,
}

impl Module for Consensus {
    fn name() -> &'static str {
        "Consensus"
    }

    type Context = SharedRunContext;

    async fn build(ctx: &Self::Context) -> Result<Self> {
        Ok(
            Consensus::load_from_disk(&ctx.config.data_directory).unwrap_or_else(|_| {
                warn!("Failed to load consensus state from disk, using a default one");
                Consensus::new(ctx.validator_registry.share())
            }),
        )
    }

    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        self.start(ctx.bus.new_handle(), ctx.config.clone(), ctx.crypto.clone())
    }
}

impl Consensus {
    fn new(validators: ValidatorRegistry) -> Self {
        Self {
            is_next_leader: false,
            blocks: vec![Block::default()],
            validators,
            bft_round_state: BFTRoundState::default(),
            pending_batches: vec![],
        }
    }

    fn create_consensus_proposal(
        &mut self,
        previous_consensus_proposal_hash: ConsensusProposalHash,
        previous_commit_quorum_certificate: QuorumCertificate,
    ) -> Result<(), Error> {
        let last_block = self.blocks.last();

        let parent_hash = last_block
            .map(|b| b.hash())
            .unwrap_or(BlockHash::new("000"));
        let parent_height = last_block.map(|b| b.height).unwrap_or_default();

        // Create block to-be-proposed
        let mut txs = vec![];
        if !self.pending_batches.is_empty() {
            txs = self.pending_batches.remove(0);
        }
        let block = Block {
            parent_hash,
            height: parent_height + 1,
            timestamp: get_current_timestamp(),
            txs,
        };

        // Start Consensus with following block
        self.bft_round_state.consensus_proposal = ConsensusProposal {
            slot: self.bft_round_state.slot,
            view: self.bft_round_state.view,
            previous_consensus_proposal_hash,
            previous_commit_quorum_certificate,
            block,
        };
        Ok(())
    }

    fn create_genesis_consensus_proposal(&mut self) {
        let first_block = Block {
            parent_hash: BlockHash {
                inner: [
                    70, 105, 97, 116, 32, 108, 117, 120, 32, 101, 116, 32, 102, 97, 99, 116, 97,
                    32, 101, 115, 116, 32, 108, 117, 120,
                ]
                .to_vec(),
            },
            height: BlockHeight(0),
            timestamp: get_current_timestamp(),
            txs: vec![],
        };

        // Start Consensus with following block
        self.bft_round_state.consensus_proposal = ConsensusProposal {
            slot: self.bft_round_state.slot,
            view: self.bft_round_state.view,
            previous_consensus_proposal_hash: ConsensusProposalHash(vec![]),
            previous_commit_quorum_certificate: QuorumCertificate::default(),
            block: first_block,
        };
    }

    fn add_block(&mut self, bus: &ConsensusBusClient) -> Result<(), Error> {
        // Send block to internal bus
        _ = bus
            .send(ConsensusEvent::CommitBlock {
                block: self.bft_round_state.consensus_proposal.block.clone(),
            })
            .context("Failed to send ConsensusEvent::CommitBlock msg on the bus")?;

        info!(
            "New block {}",
            self.bft_round_state.consensus_proposal.block.height
        );
        self.blocks
            .push(self.bft_round_state.consensus_proposal.block.clone());
        Ok(())
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

    fn finish_round(&mut self, bus: &ConsensusBusClient) -> Result<(), Error> {
        // Add and applies new block to its NodeState through ConsensusEvent
        self.add_block(bus)?;

        // Save added block
        _ = bus
            .send(ConsensusCommand::SaveOnDisk)
            .context("Cannot send message over channel")?;

        self.bft_round_state.slot += 1;
        self.bft_round_state.view = 0;
        self.bft_round_state.prepare_votes = HashSet::default();
        self.bft_round_state.confirm_ack = HashSet::default();

        Ok(())
    }

    fn verify_consensus_proposal_hash(
        &self,
        consensus_proposal_hash: &ConsensusProposalHash,
    ) -> bool {
        // Verify that the vote is for the correct proposal
        &self.bft_round_state.consensus_proposal.hash() == consensus_proposal_hash
    }

    fn is_next_leader(&self) -> bool {
        // TODO
        self.is_next_leader
    }

    fn leader_id(&self) -> ValidatorId {
        // TODO
        ValidatorId("node-1".to_owned())
    }

    fn start_new_slot(
        &mut self,
        bus: &ConsensusBusClient,
        crypto: &BlstCrypto,
    ) -> Result<(), Error> {
        // Message received by leader.

        info!("🚀 Starting new slot");
        // Verifies that previous slot received a *Commit* Quorum Certificate.
        match self
            .bft_round_state
            .commit_quorum_certificates
            .get(&(self.bft_round_state.slot.saturating_sub(1)))
        {
            Some((previous_consensus_proposal_hash, previous_commit_quorum_certificate)) => {
                // Creates ConsensusProposal
                self.create_consensus_proposal(
                    previous_consensus_proposal_hash.clone(),
                    previous_commit_quorum_certificate.clone(),
                )?;
            }
            None if self.bft_round_state.slot == 0 => {
                info!("#### Genesis block creation ####");
                self.create_genesis_consensus_proposal();
            }
            None => {
                bail!("Can't start new slot: previous commit quorum certificate not found")
            }
        };

        // Verifies that to-be-built block is large enough (?)

        // Updates its bft state
        // TODO: update view when rebellion
        // self.bft_round_state.view += 1;

        // Broadcasts Prepare message to all validators
        _ = bus
            .send(OutboundMessage::broadcast(Self::sign_net_message(
                crypto,
                ConsensusNetMessage::Prepare(self.bft_round_state.consensus_proposal.clone()),
            )?))
            .context("Failed to broadcast ConsensusNetMessage::Prepare msg on the bus")?;

        Ok(())
    }

    fn handle_net_message(
        &mut self,
        msg: SignedWithId<ConsensusNetMessage>,
        crypto: &BlstCrypto,
        bus: &ConsensusBusClient,
    ) -> Result<(), Error> {
        if !self.validators.check_signed(&msg)? {
            bail!("Invalid signature for message {:?}", msg);
        }

        match &msg.msg {
            ConsensusNetMessage::StartNewSlot => self.start_new_slot(bus, crypto),
            ConsensusNetMessage::Prepare(consensus_proposal) => {
                // Message received by follower.

                // Validate message comes from the correct leader
                if !msg.validators.contains(&self.leader_id()) {
                    bail!("Prepare consensus message does not come from current leader");
                    // TODO: slash ?
                }

                // Verify received previous *Commit* Quorum Certificate
                // If a previous *Commit* Quorum Certificate exists, verify hashes
                // If no previous *Commit* Quorum Certificate is found, verify this one and save it if legit
                match self
                    .bft_round_state
                    .commit_quorum_certificates
                    .get(&(self.bft_round_state.slot.saturating_sub(1)))
                {
                    Some((
                        previous_consensus_proposal_hash,
                        previous_commit_quorum_certificate,
                    )) => {
                        // FIXME: Ici on a besoin d'avoir une trace du consensus_proposal precedent...
                        // Quid des validators qui arrivent en cours de route ?
                        // On veut sûrement garder trace de quelques consensus_proposals précedents.
                        if previous_commit_quorum_certificate.hash()
                            != consensus_proposal.previous_commit_quorum_certificate.hash()
                        {
                            bail!("PrepareVote consensus proposal does not contain correct previous commit quorum certificate");
                        }
                        if previous_consensus_proposal_hash
                            != &consensus_proposal.previous_consensus_proposal_hash
                        {
                            bail!("PrepareVote previous consensus proposal hash does not match");
                        }
                        // Verify slot
                        if self.bft_round_state.slot != consensus_proposal.slot {
                            bail!(
                                "Consensus proposal slot is incorrect. Received {} and expected {}",
                                consensus_proposal.slot,
                                self.bft_round_state.slot
                            );
                        }
                        // Verify view
                        if self.bft_round_state.view != consensus_proposal.view {
                            bail!(
                                "Consensus proposal view is incorrect. Received {} and expected {}",
                                consensus_proposal.view,
                                self.bft_round_state.view
                            );
                        }
                        // Verify block
                    }
                    None if self.bft_round_state.slot == 0 => {
                        info!("#### Received genesis block proposal ####");
                    }
                    None => {
                        // Verify proposal hash is correct
                        if !self.verify_consensus_proposal_hash(
                            &consensus_proposal.previous_consensus_proposal_hash,
                        ) {
                            // TODO Retrieve that data on other validators
                            bail!("Previsou Commit Quorum certificate is about an unknown consensus proposal. Can't process any furter");
                        }

                        let previous_commit_quorum_certificate_with_message = SignedWithKey {
                            msg: ConsensusNetMessage::ConfirmAck(
                                consensus_proposal.previous_consensus_proposal_hash.clone(),
                            ),
                            signature: consensus_proposal
                                .previous_commit_quorum_certificate
                                .signature
                                .clone(),
                            validators: consensus_proposal
                                .previous_commit_quorum_certificate
                                .validators
                                .clone(),
                        };
                        // Verify valid signature
                        match BlstCrypto::verify(&previous_commit_quorum_certificate_with_message) {
                            Ok(res) => {
                                if !res {
                                    bail!("Previous Commit Quorum Certificate is invalid")
                                }
                                // Buffers the previous *Commit* Quorum Cerficiate
                                self.bft_round_state.commit_quorum_certificates.insert(
                                    self.bft_round_state.slot - 1,
                                    (
                                        consensus_proposal.previous_consensus_proposal_hash.clone(),
                                        consensus_proposal
                                            .previous_commit_quorum_certificate
                                            .clone(),
                                    ),
                                );
                            }
                            Err(err) => {
                                bail!(
                                    "Previous Commit Quorum Certificate verification failed: {}",
                                    err
                                )
                            }
                        }
                        // Properly finish the previous round
                        self.finish_round(bus)?;
                    }
                };

                // Update consensus_proposal
                self.bft_round_state.consensus_proposal = consensus_proposal.clone();

                // Responds PrepareVote message to leader with validator's vote on this proposal
                _ = bus
                    .send(OutboundMessage::send(
                        self.leader_id(),
                        Self::sign_net_message(
                            crypto,
                            ConsensusNetMessage::PrepareVote(consensus_proposal.hash()),
                        )?,
                    ))
                    .context("Failed to send ConsensusNetMessage::Confirm msg on the bus")?;
                Ok(())
            }
            ConsensusNetMessage::PrepareVote(consensus_proposal_hash) => {
                // Message received by leader.

                // Verify that the PrepareVote is for the correct proposal
                if !self.verify_consensus_proposal_hash(consensus_proposal_hash) {
                    bail!("PrepareVote has not received valid consensus proposal hash");
                }

                // Save vote message
                self.bft_round_state.prepare_votes.insert(
                    msg.with_pub_keys(self.validators.get_pub_keys_from_id(msg.validators.clone())),
                );

                // Get matching vote count
                let validated_votes = self
                    .bft_round_state
                    .prepare_votes
                    .iter()
                    .flat_map(|signed_message| signed_message.validators.clone())
                    .collect::<HashSet<ValidatorPublicKey>>();

                // Waits for at least n-f = 2f+1 matching PrepareVote messages
                let f: usize = self.validators.get_validators_count() / 3;
                // TODO: Only wait for 2 * f because leader himself if the +1
                if validated_votes.len() == 2 * f + 1 {
                    // Get all received signatures
                    let aggregates: &Vec<&Signed<ConsensusNetMessage, ValidatorPublicKey>> =
                        &self.bft_round_state.prepare_votes.iter().collect();

                    // Aggregates them into a *Prepare* Quorum Certificate
                    let prepvote_signed_aggregation = crypto.sign_aggregate(
                        ConsensusNetMessage::PrepareVote(
                            self.bft_round_state.consensus_proposal.hash(),
                        ),
                        aggregates,
                    )?;

                    // Buffers the *Prepare* Quorum Cerficiate
                    self.bft_round_state.prepare_quorum_certificate = QuorumCertificate {
                        signature: prepvote_signed_aggregation.signature.clone(),
                        validators: prepvote_signed_aggregation.validators.clone(),
                    };

                    // if fast-path ... TODO
                    // else send Confirm message to validators

                    // Broadcast the *Prepare* Quorum Certificate to all validators
                    _ = bus
                        .send(OutboundMessage::broadcast(Self::sign_net_message(
                            crypto,
                            ConsensusNetMessage::Confirm(
                                consensus_proposal_hash.clone(),
                                self.bft_round_state.prepare_quorum_certificate.clone(),
                            ),
                        )?))
                        .context(
                            "Failed to broadcast ConsensusNetMessage::Confirm msg on the bus",
                        )?;
                }
                // TODO(?): Update behaviour when having more ?
                // else if validated_votes > 2 * f + 1 {}
                Ok(())
            }
            ConsensusNetMessage::Confirm(consensus_proposal_hash, prepare_quorum_certificate) => {
                // Message received by follower.

                // Verify that the Confirm is for the correct proposal
                if !self.verify_consensus_proposal_hash(consensus_proposal_hash) {
                    bail!("Confirm has not received valid consensus proposal hash");
                }

                // Verifies the *Prepare* Quorum Certificate
                let prepare_quorum_certificate_with_message = SignedWithKey {
                    msg: ConsensusNetMessage::PrepareVote(consensus_proposal_hash.clone()),
                    signature: prepare_quorum_certificate.signature.clone(),
                    validators: prepare_quorum_certificate.validators.clone(),
                };
                match BlstCrypto::verify(&prepare_quorum_certificate_with_message) {
                    Ok(res) => {
                        if !res {
                            bail!("Prepare Quorum Certificate received is invalid")
                        }
                        // Verify enough validators signed
                        let f: usize = self.validators.get_validators_count() / 3;
                        if prepare_quorum_certificate.validators.len() < 2 * f + 1 {
                            bail!("Prepare Quorum Certificate does not contain enough validators signatures")
                        }
                        // Verify that validators that signed are legit
                        // TODO

                        // Buffers the *Prepare* Quorum Cerficiate
                        self.bft_round_state.prepare_quorum_certificate =
                            prepare_quorum_certificate.clone();
                    }
                    Err(err) => bail!("Prepare Quorum Certificate verification failed: {}", err),
                }

                // Responds ConfirmAck to leader
                _ = bus
                    .send(OutboundMessage::send(
                        self.leader_id(),
                        Self::sign_net_message(
                            crypto,
                            ConsensusNetMessage::ConfirmAck(consensus_proposal_hash.clone()),
                        )?,
                    ))
                    .context("Failed to send ConsensusNetMessage::ConfirmAck msg on the bus")?;
                Ok(())
            }
            ConsensusNetMessage::ConfirmAck(consensus_proposal_hash) => {
                // Message received by leader.

                // Verify that the ConfirmAck is for the correct proposal
                if !self.verify_consensus_proposal_hash(consensus_proposal_hash) {
                    bail!("ConfirmAck has not received valid consensus proposal hash");
                }

                // Save ConfirmAck. Ends if the message already has been processed
                if !self.bft_round_state.confirm_ack.insert(
                    msg.with_pub_keys(self.validators.get_pub_keys_from_id(msg.validators.clone())),
                ) {
                    info!("ConfirmAck has aleady been processed");
                    return Ok(());
                }

                // Get ConfirmAck count
                let confirmed_ack_validators = self
                    .bft_round_state
                    .confirm_ack
                    .iter()
                    .flat_map(|signed_message| signed_message.validators.clone())
                    .collect::<HashSet<ValidatorPublicKey>>();

                let f: usize = self.validators.get_validators_count() / 3;

                // TODO: Only waiting for 2 * f because leader himself if the +1
                if confirmed_ack_validators.len() == (2 * f + 1) {
                    // Get all signatures received and change ValidatorId for ValidatorPubKey
                    let aggregates: &Vec<&Signed<ConsensusNetMessage, ValidatorPublicKey>> =
                        &self.bft_round_state.confirm_ack.iter().collect();

                    // Aggregates them into a *Commit* Quorum Certificate
                    let commit_signed_aggregation = crypto.sign_aggregate(
                        ConsensusNetMessage::ConfirmAck(
                            self.bft_round_state.consensus_proposal.hash(),
                        ),
                        aggregates,
                    )?;

                    // Buffers the *Commit* Quorum Cerficiate
                    let commit_quorum_certificate = QuorumCertificate {
                        signature: commit_signed_aggregation.signature.clone(),
                        validators: commit_signed_aggregation.validators.clone(),
                    };
                    self.bft_round_state.commit_quorum_certificates.insert(
                        self.bft_round_state.slot,
                        (
                            consensus_proposal_hash.clone(),
                            commit_quorum_certificate.clone(),
                        ),
                    );

                    // Broadcast the *Commit* Quorum Certificate to all validators
                    _ = bus
                        .send(OutboundMessage::broadcast(Self::sign_net_message(
                            crypto,
                            ConsensusNetMessage::Commit(
                                consensus_proposal_hash.clone(),
                                commit_quorum_certificate,
                            ),
                        )?))
                        .context(
                            "Failed to broadcast ConsensusNetMessage::Confirm msg on the bus",
                        )?;

                    // Finishes the bft round
                    self.finish_round(bus)?;

                    // Start new slot
                    if self.is_next_leader() {
                        // Send Prepare message to all validators
                        self.start_new_slot(bus, crypto)?;
                    }
                }
                // TODO(?): Update behaviour when having more ?
                Ok(())
            }
            ConsensusNetMessage::Commit(consensus_proposal_hash, commit_quorum_certificate) => {
                // Message received by follower.

                // Verify that the vote is for the correct proposal
                if !self.verify_consensus_proposal_hash(consensus_proposal_hash) {
                    bail!("PrepareVote has not received valid consensus proposal hash");
                }

                // Verifies and save the *Commit* Quorum Certificate
                let commit_quorum_certificate_with_message = SignedWithKey {
                    msg: ConsensusNetMessage::ConfirmAck(consensus_proposal_hash.clone()),
                    signature: commit_quorum_certificate.signature.clone(),
                    validators: commit_quorum_certificate.validators.clone(),
                };

                match BlstCrypto::verify(&commit_quorum_certificate_with_message) {
                    Ok(res) => {
                        if !res {
                            bail!("Commit Quorum Certificate received is invalid")
                        }
                        // Verify enough validators signed
                        let f: usize = self.validators.get_validators_count() / 3;
                        if commit_quorum_certificate.validators.len() < 2 * f + 1 {
                            bail!("Commit Quorum Certificate does not contain enough validators signatures")
                        }
                        // Verify that validators that signed are legit
                        // TODO

                        // Buffers the *Commit* Quorum Cerficiate
                        self.bft_round_state.commit_quorum_certificates.insert(
                            self.bft_round_state.slot,
                            (
                                consensus_proposal_hash.clone(),
                                commit_quorum_certificate.clone(),
                            ),
                        );
                    }
                    Err(err) => bail!("Commit Quorum Certificate verification failed: {}", err),
                }

                // Finishes the bft round
                self.finish_round(bus)?;

                // Start new slot
                if self.is_next_leader() {
                    // Send Prepare message to all validators
                    self.start_new_slot(bus, crypto)?;
                }
                Ok(())
            }
        }
    }

    async fn handle_command(&mut self, msg: ConsensusCommand) -> Result<()> {
        match msg {
            ConsensusCommand::SaveOnDisk => {
                // FIXME: Might need to be removed as we have History module
                _ = self.save_on_disk();
                Ok(())
            }
        }
    }

    async fn handle_mempool_event(&mut self, msg: MempoolEvent) -> Result<()> {
        match msg {
            MempoolEvent::LatestBatch(batch) => {
                debug!("Received batch with txs: {:?}", batch);
                self.pending_batches.push(batch);

                Ok(())
            }
        }
    }

    pub async fn start(
        &mut self,
        shared_bus: SharedMessageBus,
        config: SharedConf,
        crypto: SharedBlstCrypto,
    ) -> Result<()> {
        let mut bus = ConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

        // FIXME: node-1 sera le leader
        if config.id == ValidatorId("node-1".to_owned()) {
            self.is_next_leader = true;
        }

        handle_messages! {
            on_bus bus,
            listen<ConsensusCommand> cmd => {
                match self.handle_command(cmd).await{
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling consensus command: {:#}", e),
                }
            }
            listen<MempoolEvent> cmd => {
                match self.handle_mempool_event(cmd).await{
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling mempool event: {:#}", e),
                }
            }
            listen<SignedWithId<ConsensusNetMessage>> cmd => {
                match self.handle_net_message(cmd, &crypto, &bus) {
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling consensus net message: {:#}", e),
                }
            }
            listen<ValidatorRegistryNetMessage> cmd => {
                self.validators.handle_net_message(cmd.clone());
                if self.validators.get_validators_count() == 1 && self.is_next_leader() {
                    // Start first slot
                    sleep(Duration::from_secs(2)).await;
                    _ = self.start_new_slot(&bus, &crypto);
                }
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
