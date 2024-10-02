//! Handles all consensus logic up to block commitment.

use anyhow::{bail, Context, Error, Result};
use bincode::{Decode, Encode};
use metrics::ConsensusMetrics;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    cmp::max,
    collections::{HashMap, HashSet},
    default::Default,
    fmt::Display,
    io::Write,
    ops::{Deref, DerefMut},
    path::PathBuf,
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

pub mod metrics;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub enum ConsensusNetMessage {
    StartNewSlot,
    Prepare(ConsensusProposal),
    PrepareVote(ConsensusProposalHash),
    Confirm(ConsensusProposalHash, QuorumCertificate),
    ConfirmAck(ConsensusProposalHash),
    Commit(ConsensusProposalHash, QuorumCertificate),
    ValidatorCandidacy(ValidatorCandidacy),
}

#[derive(Encode, Decode, Default, Debug)]
enum Step {
    #[default]
    StartNewSlot,
    PrepareVote,
    ConfirmAck,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    SingleNodeBlockGeneration,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusEvent {
    CommitBlock { block: Block },
}

impl BusMessage for ConsensusCommand {}
impl BusMessage for ConsensusEvent {}
impl BusMessage for ConsensusNetMessage {}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ValidatorCandidacy {
    pubkey: ValidatorPublicKey,
    slot: Slot,
    previous_consensus_proposal_hash: ConsensusProposalHash,
}

// TODO: move struct to model.rs ?
#[derive(Encode, Decode, Default)]
pub struct BFTRoundState {
    consensus_proposal: ConsensusProposal,
    slot: Slot,
    view: View,
    step: Step,
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
    /// Validators for current slot
    validators: Vec<ValidatorPublicKey>, // TODO use ID instead of pubkey ?
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

impl Display for ValidatorCandidacy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Pubkey: {}, Slot: {}, Previous hash: {}",
            self.pubkey, self.slot, self.previous_consensus_proposal_hash
        )
    }
}

impl Display for ConsensusProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Hash: {}, Slot: {}, View: {}, Previous hash: {}, Previous commit hash: {}, Block: {}",
            self.hash(),
            self.slot,
            self.view,
            self.previous_consensus_proposal_hash,
            self.previous_commit_quorum_certificate.hash(),
            self.block
        )
    }
}
impl Display for ConsensusProposalHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0))
    }
}
impl Display for QuorumCertificateHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0))
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

#[derive(Encode, Decode, Default)]
pub struct ConsensusStore {
    is_next_leader: bool,
    /// Validators that asked to be part of consensus
    consensus_candidates: Vec<ValidatorPublicKey>, // TODO use ID instead of pubkey ?
    bft_round_state: BFTRoundState,
    // FIXME: pub is here for testing
    pub blocks: Vec<Block>,
    pending_batches: Vec<Vec<Transaction>>,
}

pub struct Consensus {
    metrics: ConsensusMetrics,
    validators: ValidatorRegistry,
    file: Option<PathBuf>,
    store: ConsensusStore,
    config: SharedConf,
}

impl Deref for Consensus {
    type Target = ConsensusStore;
    fn deref(&self) -> &Self::Target {
        &self.store
    }
}

impl DerefMut for Consensus {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.store
    }
}

impl Module for Consensus {
    fn name() -> &'static str {
        "Consensus"
    }

    type Context = SharedRunContext;

    async fn build(ctx: &Self::Context) -> Result<Self> {
        let file = ctx.config.data_directory.clone().join("consensus.bin");
        let store = Self::load_from_disk_or_default(file.as_path());
        Ok(Consensus::new(
            ctx.validator_registry.share(),
            Some(file),
            store,
            ConsensusMetrics::global(&ctx.config),
            ctx.config.clone(),
        ))
    }

    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        self.start(ctx.bus.new_handle(), ctx.config.clone(), ctx.crypto.clone())
    }
}

impl Consensus {
    fn new(
        validators: ValidatorRegistry,
        file: Option<PathBuf>,
        store: ConsensusStore,
        metrics: ConsensusMetrics,
        config: SharedConf,
    ) -> Self {
        Self {
            metrics,
            validators,
            file,
            store,
            config,
        }
    }

    /// Create a consensus proposal with the given previous consensus proposal
    /// includes previous validators and add the candidates
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
        let mut validators = self.bft_round_state.consensus_proposal.validators.clone();
        validators.append(&mut self.consensus_candidates.drain(..).collect::<Vec<_>>());

        // Start Consensus with following block
        self.bft_round_state.consensus_proposal = ConsensusProposal {
            slot: self.bft_round_state.slot,
            view: self.bft_round_state.view,
            previous_consensus_proposal_hash,
            previous_commit_quorum_certificate,
            validators,
            block,
        };
        Ok(())
    }

    /// Create a consensus proposal with all validators connected
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
            validators: self.validators.list_validators(),
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
        self.store
            .blocks
            .push(self.store.bft_round_state.consensus_proposal.block.clone());
        Ok(())
    }

    fn finish_round(&mut self, bus: &ConsensusBusClient) -> Result<(), Error> {
        // Add and applies new block to its NodeState through ConsensusEvent
        self.add_block(bus)?;

        // Save added block
        if let Some(file) = &self.file {
            Self::save_on_disk(file.as_path(), &self.store)?;
        }

        info!(
            "🔒 Slot {} finished",
            self.bft_round_state.consensus_proposal.slot
        );

        self.bft_round_state.step = Step::StartNewSlot;
        self.bft_round_state.slot = self.bft_round_state.consensus_proposal.slot + 1;
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

    /// Verify that quorum certificate includes only validators that are part of the consensus
    fn verify_quroum_signers_part_of_consensus(
        &self,
        quorum_certificate: &QuorumCertificate,
    ) -> bool {
        quorum_certificate.validators.iter().all(|v| {
            self.bft_round_state
                .consensus_proposal
                .validators
                .iter()
                .any(|v2| v2 == v)
        })
    }

    fn is_next_leader(&self) -> bool {
        // TODO
        self.is_next_leader
    }

    fn is_part_of_consensus(&self, pubkey: &ValidatorPublicKey) -> bool {
        self.bft_round_state
            .consensus_proposal
            .validators
            .contains(pubkey)
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

        // Verifies that previous slot received a *Commit* Quorum Certificate.
        match self
            .bft_round_state
            .commit_quorum_certificates
            .get(&(self.bft_round_state.slot.saturating_sub(1)))
        {
            Some((previous_consensus_proposal_hash, previous_commit_quorum_certificate)) => {
                info!(
                    "🚀 Starting new slot with {} existing validators and {} candidates",
                    self.bft_round_state.consensus_proposal.validators.len(),
                    self.consensus_candidates.len()
                );
                // Creates ConsensusProposal
                self.create_consensus_proposal(
                    previous_consensus_proposal_hash.clone(),
                    previous_commit_quorum_certificate.clone(),
                )?;

                self.metrics.start_new_slot("consensus_proposal");
            }
            None if self.bft_round_state.slot == 0 => {
                info!(
                    "🚀 Starting genesis slot with all known {} validators ",
                    self.validators.get_validators_count(),
                );
                self.create_genesis_consensus_proposal();
                self.metrics.start_new_slot("genesis");
            }
            None => {
                self.metrics
                    .start_new_slot_error("previous_commit_qc_not_found");
                bail!("Can't start new slot: previous commit quorum certificate not found")
            }
        };

        // Verifies that to-be-built block is large enough (?)

        // Updates its bft state
        // TODO: update view when rebellion
        // self.bft_round_state.view += 1;

        // Broadcasts Prepare message to all validators
        debug!(
            proposal_hash = %self.bft_round_state.consensus_proposal.hash(),
            "🌐 Slot {} started. Broadcasting Prepare message", self.bft_round_state.slot,
        );
        self.bft_round_state.step = Step::PrepareVote;
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
        msg: &SignedWithId<ConsensusNetMessage>,
        crypto: &BlstCrypto,
        bus: &ConsensusBusClient,
    ) -> Result<(), Error> {
        if !self.validators.check_signed(msg)? {
            self.metrics.signature_error("prepare");
            bail!("Invalid signature for message {:?}", msg);
        }

        match &msg.msg {
            // TODO: do we really get a net message for StartNewSlot ?
            ConsensusNetMessage::StartNewSlot => self.start_new_slot(bus, crypto),
            ConsensusNetMessage::Prepare(consensus_proposal) => {
                // Message received by follower.
                debug!("Received Prepare message: {}", consensus_proposal);
                // Validate message comes from the correct leader
                if !msg.validators.contains(&self.leader_id()) {
                    self.metrics.prepare_error("wrong_leader");
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
                            self.metrics.prepare_error("missing_correct_commit_qc");
                            bail!("PrepareVote consensus proposal does not contain correct previous commit quorum certificate");
                        }
                        if previous_consensus_proposal_hash
                            != &consensus_proposal.previous_consensus_proposal_hash
                        {
                            self.metrics.prepare_error("proposal_match");
                            bail!("PrepareVote previous consensus proposal hash does not match");
                        }
                        // Verify slot
                        if self.bft_round_state.slot != consensus_proposal.slot {
                            self.metrics.prepare_error("wrong_slot");
                            bail!(
                                "Consensus proposal slot is incorrect. Received {} and expected {}",
                                consensus_proposal.slot,
                                self.bft_round_state.slot
                            );
                        }
                        // Verify view
                        if self.bft_round_state.view != consensus_proposal.view {
                            self.metrics.prepare_error("wrong_view");
                            bail!(
                                "Consensus proposal view is incorrect. Received {} and expected {}",
                                consensus_proposal.view,
                                self.bft_round_state.view
                            );
                        }
                        // Verify block
                    }
                    None if self.bft_round_state.slot == 0 => {
                        if consensus_proposal.slot != 0 {
                            info!("🔄 Consensus state out of sync, need to catchup");
                        } else {
                            info!("#### Received genesis block proposal ####");
                        }
                    }
                    None => {
                        // Verify proposal hash is correct
                        if !self.verify_consensus_proposal_hash(
                            &consensus_proposal.previous_consensus_proposal_hash,
                        ) {
                            self.metrics.prepare_error("previous_commit_qc_unknown");
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
                                    self.metrics.prepare_error("previous_commit_qc_invalid");
                                    bail!("Previous Commit Quorum Certificate is invalid")
                                }
                                // Buffers the previous *Commit* Quorum Cerficiate
                                self.store
                                    .bft_round_state
                                    .commit_quorum_certificates
                                    .insert(
                                        self.store.bft_round_state.slot - 1,
                                        (
                                            consensus_proposal
                                                .previous_consensus_proposal_hash
                                                .clone(),
                                            consensus_proposal
                                                .previous_commit_quorum_certificate
                                                .clone(),
                                        ),
                                    );
                            }
                            Err(err) => {
                                self.metrics.prepare_error("bls_failure");
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
                if self.is_part_of_consensus(crypto.validator_pubkey()) {
                    info!(
                        proposal_hash = %consensus_proposal.hash(),
                        "📤 Slot {} Prepare message validated. Sending PrepareVote to leader",
                        self.bft_round_state.slot
                    );
                    _ = bus
                        .send(OutboundMessage::send(
                            self.leader_id(),
                            Self::sign_net_message(
                                crypto,
                                ConsensusNetMessage::PrepareVote(consensus_proposal.hash()),
                            )?,
                        ))
                        .context("Failed to send ConsensusNetMessage::Confirm msg on the bus")?;
                } else {
                    debug!("😥 Not part of consensus, not sending PrepareVote");
                }

                self.metrics.prepare();

                Ok(())
            }
            ConsensusNetMessage::PrepareVote(consensus_proposal_hash) => {
                // Message received by leader.
                if !matches!(self.bft_round_state.step, Step::PrepareVote) {
                    debug!(
                        "PrepareVote received at wrong step (step = {:?})",
                        self.bft_round_state.step
                    );
                    return Ok(());
                }

                // Verify that the PrepareVote is for the correct proposal
                if !self.verify_consensus_proposal_hash(consensus_proposal_hash) {
                    self.metrics.prepare_vote_error("invalid_proposal_hash");
                    bail!("PrepareVote has not received valid consensus proposal hash");
                }

                // Save vote message
                self.store.bft_round_state.prepare_votes.insert(
                    msg.with_pub_keys(self.validators.get_pub_keys_from_id(msg.validators.clone())),
                );

                self.metrics
                    .prepare_votes_gauge(self.bft_round_state.prepare_votes.len());

                // Get matching vote count
                let validated_votes = self
                    .bft_round_state
                    .prepare_votes
                    .iter()
                    .flat_map(|signed_message| signed_message.validators.clone())
                    .collect::<HashSet<ValidatorPublicKey>>();

                // Waits for at least n-f = 2f+1 matching PrepareVote messages
                let f: usize = self.bft_round_state.consensus_proposal.validators.len() / 3;
                info!(
                    "📩 Slot {} validated votes: {} / {} ({} validators)",
                    self.bft_round_state.slot,
                    validated_votes.len(),
                    2 * f,
                    self.bft_round_state.consensus_proposal.validators.len()
                );
                // TODO: Only wait for 2 * f because leader himself if the +1
                if validated_votes.len() == max(1, 2 * f) {
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

                    self.metrics.prepare_votes_aggregation();

                    // Buffers the *Prepare* Quorum Cerficiate
                    self.bft_round_state.prepare_quorum_certificate = QuorumCertificate {
                        signature: prepvote_signed_aggregation.signature.clone(),
                        validators: prepvote_signed_aggregation.validators.clone(),
                    };

                    // if fast-path ... TODO
                    // else send Confirm message to validators

                    // Broadcast the *Prepare* Quorum Certificate to all validators
                    debug!(
                        "Slot {} PrepareVote message validated. Broadcasting Confirm",
                        self.bft_round_state.slot
                    );
                    self.bft_round_state.step = Step::ConfirmAck;
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
                    self.metrics.confirm_error("invalid_proposal_hash");
                    bail!("Confirm step got invalid proposal hash.");
                }

                // Verify that validators that signed are legit
                if !self.verify_quroum_signers_part_of_consensus(prepare_quorum_certificate) {
                    bail!("Prepare quorum certificate includes validators that are not part of the consensus")
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
                            self.metrics.confirm_error("prepare_qc_invalid");
                            bail!("Prepare Quorum Certificate received is invalid")
                        }
                        // Verify enough validators signed
                        let f: usize = self.bft_round_state.consensus_proposal.validators.len() / 3;
                        if prepare_quorum_certificate.validators.len() < 2 * f + 1 {
                            self.metrics.confirm_error("prepare_qc_incomplete");
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
                if self.is_part_of_consensus(crypto.validator_pubkey()) {
                    info!(
                        proposal_hash = %consensus_proposal_hash,
                        "📤 Slot {} Confirm message validated. Sending ConfirmAck to leader",
                        self.bft_round_state.slot
                    );
                    _ = bus
                        .send(OutboundMessage::send(
                            self.leader_id(),
                            Self::sign_net_message(
                                crypto,
                                ConsensusNetMessage::ConfirmAck(consensus_proposal_hash.clone()),
                            )?,
                        ))
                        .context("Failed to send ConsensusNetMessage::ConfirmAck msg on the bus")?;
                } else {
                    debug!("😥 Not part of consensus, not sending ConfirmAck");
                }
                Ok(())
            }
            ConsensusNetMessage::ConfirmAck(consensus_proposal_hash) => {
                // Message received by leader.

                if !matches!(self.bft_round_state.step, Step::ConfirmAck) {
                    debug!(
                        "ConfirmAck received at wrong step (step ={:?})",
                        self.bft_round_state.step
                    );
                    return Ok(());
                }

                // Verify that the ConfirmAck is for the correct proposal
                if !self.verify_consensus_proposal_hash(consensus_proposal_hash) {
                    self.metrics.confirm_ack_error("invalid_proposal_hash");
                    debug!(
                        "Got {} exptected {}",
                        consensus_proposal_hash,
                        self.bft_round_state.consensus_proposal.hash()
                    );
                    bail!("ConfirmAck got invalid consensus proposal hash");
                }

                // Save ConfirmAck. Ends if the message already has been processed
                if !self.store.bft_round_state.confirm_ack.insert(
                    msg.with_pub_keys(self.validators.get_pub_keys_from_id(msg.validators.clone())),
                ) {
                    self.metrics.confirm_ack("already_processed");
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

                let f: usize = self.bft_round_state.consensus_proposal.validators.len() / 3;
                info!(
                    "✅ Slot {} confirmed acks: {} / {} ({} validators)",
                    self.bft_round_state.slot,
                    confirmed_ack_validators.len(),
                    2 * f,
                    self.bft_round_state.consensus_proposal.validators.len()
                );
                self.metrics
                    .confirmed_ack_gauge(confirmed_ack_validators.len());
                // TODO: Only waiting for 2 * f because leader himself if the +1
                if confirmed_ack_validators.len() == max(1, 2 * f) {
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

                    self.metrics.confirm_ack_commit_aggregate();

                    // Buffers the *Commit* Quorum Cerficiate
                    let commit_quorum_certificate = QuorumCertificate {
                        signature: commit_signed_aggregation.signature.clone(),
                        validators: commit_signed_aggregation.validators.clone(),
                    };
                    self.store
                        .bft_round_state
                        .commit_quorum_certificates
                        .insert(
                            self.store.bft_round_state.slot,
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
                        #[cfg(not(test))]
                        {
                            info!(
                                "⏱️  Sleeping {} seconds before starting a new slot",
                                self.config.consensus.slot_duration
                            );
                            std::thread::sleep(Duration::from_secs(
                                self.config.consensus.slot_duration,
                            ));
                        }
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
                    self.metrics.commit_error("invalid_proposal_hash");
                    bail!("Commit has not received valid consensus proposal hash");
                }

                // Verify that validators that signed are legit
                if !self.verify_quroum_signers_part_of_consensus(commit_quorum_certificate) {
                    self.metrics
                        .commit_error("invalid_validators_qorum_certificate");
                    bail!("Commit quorum certificate includes validators that are not part of the consensus")
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
                            self.metrics.commit_error("invalid_qc");
                            bail!("Commit Quorum Certificate received is invalid")
                        }
                        // Verify enough validators signed
                        let f: usize = self.bft_round_state.consensus_proposal.validators.len() / 3;
                        if commit_quorum_certificate.validators.len() < 2 * f + 1 {
                            self.metrics.commit_error("incomplete_qc");
                            bail!("Commit Quorum Certificate does not contain enough validators signatures")
                        }
                        // Verify that validators that signed are legit
                        // TODO

                        // Buffers the *Commit* Quorum Cerficiate
                        self.store
                            .bft_round_state
                            .commit_quorum_certificates
                            .insert(
                                self.store.bft_round_state.slot,
                                (
                                    consensus_proposal_hash.clone(),
                                    commit_quorum_certificate.clone(),
                                ),
                            );
                    }
                    Err(err) => {
                        self.metrics.commit_error("bls_failure");
                        bail!("Commit Quorum Certificate verification failed: {}", err);
                    }
                }

                // Finishes the bft round
                self.finish_round(bus)?;

                self.metrics.commit();

                // Start new slot
                if self.is_next_leader() {
                    // Send Prepare message to all validators
                    self.start_new_slot(bus, crypto)?;
                } else if !self.is_part_of_consensus(crypto.validator_pubkey()) {
                    // Ask to be part of consensus
                    let candidacy = ValidatorCandidacy {
                        pubkey: crypto.validator_pubkey().clone(),
                        slot: self.bft_round_state.slot,
                        previous_consensus_proposal_hash: consensus_proposal_hash.clone(),
                    };
                    info!(
                        "📝 Sending candidacy message to be part of consensus.  {}",
                        candidacy
                    );
                    _ = bus
                        .send(OutboundMessage::broadcast(Self::sign_net_message(
                            crypto,
                            ConsensusNetMessage::ValidatorCandidacy(candidacy),
                        )?))
                        .context("Failed to send candidacy msg on the bus")?;
                }
                Ok(())
            }
            ConsensusNetMessage::ValidatorCandidacy(candidacy) => {
                // Message received by leader & follower.
                info!("📝 Received candidacy message: {}", candidacy);

                debug!(
                    "Current consensus proposal: {}",
                    self.bft_round_state.consensus_proposal
                );

                // Verify that the candidacy is for the correct slot
                if candidacy.slot != self.bft_round_state.slot {
                    bail!("Candidacy is not up-to-date with current slot");
                }
                // Verify that the candidacy is for the correct previous consensus proposal hash
                if candidacy.previous_consensus_proposal_hash
                    != self
                        .bft_round_state
                        .consensus_proposal
                        .previous_consensus_proposal_hash
                {
                    bail!("Candidacy is not up-to-date with the correct previous consensus proposal hash");
                }
                // Verify that the validator is not already part of the consensus
                if self.is_part_of_consensus(&candidacy.pubkey) {
                    debug!("Validator is already part of the consensus");
                    return Ok(());
                }
                // Add validator to consensus candidates
                self.consensus_candidates.push(candidacy.pubkey.clone());
                info!(
                    "📝 Validator {} added to consensus candidates",
                    candidacy.pubkey
                );
                Ok(())
            }
        }
    }

    async fn handle_command(
        &mut self,
        msg: ConsensusCommand,
        bus: &ConsensusBusClient,
    ) -> Result<()> {
        match msg {
            ConsensusCommand::SingleNodeBlockGeneration => {
                let mut txs = vec![];
                if !self.pending_batches.is_empty() {
                    txs = self.pending_batches.remove(0);
                }
                let block = Block {
                    parent_hash: BlockHash::new(""),
                    height: BlockHeight(0),
                    timestamp: get_current_timestamp(),
                    txs,
                };
                _ = bus
                    .send(ConsensusEvent::CommitBlock {
                        block: block.clone(),
                    })
                    .context("Failed to send ConsensusEvent::CommitBlock msg on the bus")?;
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

        let interval = config.storage.interval;

        if config.id == ValidatorId("single-node".to_owned()) {
            info!(
                "No peers configured, starting as master generating blocks every {} seconds",
                interval
            );

            let bus2 = ConsensusBusClient::new_from_bus(shared_bus).await;
            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(interval)).await;

                    _ = bus2
                        .send(ConsensusCommand::SingleNodeBlockGeneration)
                        .log_error("Cannot send message over channel");
                }
            });
        }
        // FIXME: node-1 sera le leader
        if config.id == ValidatorId("node-1".to_owned()) {
            self.is_next_leader = true;
        }

        handle_messages! {
            on_bus bus,
            listen<ConsensusCommand> cmd => {
                match self.handle_command(cmd, &bus).await{
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
                match self.handle_net_message(&cmd, &crypto, &bus) {
                    Ok(_) => (),
                    Err(e) => warn!("Consensus message failed: {:#}", e),
                }
            }
            listen<ValidatorRegistryNetMessage> cmd => {
                self.validators.handle_net_message(cmd.clone());
                if self.validators.get_validators_count() == 2 && self.is_next_leader() {
                    // Start first slot
                    debug!("Got a 2nd validator, starting first slot");
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

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use super::*;
    use crate::{
        p2p::network::NetMessage,
        utils::{conf::Conf, crypto},
        validator_registry::ConsensusValidator,
    };
    use tokio::sync::broadcast::Receiver;

    struct TestCtx {
        bus: ConsensusBusClient,
        crypto: BlstCrypto,
        out_receiver: Receiver<OutboundMessage>,
        _event_receiver: Receiver<ConsensusEvent>,
        consensus: Consensus,
    }

    impl TestCtx {
        async fn new(crypto: BlstCrypto, other: ConsensusValidator) -> Self {
            let bus = SharedMessageBus::new();
            let out_receiver = get_receiver::<OutboundMessage>(&bus).await;
            let event_receiver = get_receiver::<ConsensusEvent>(&bus).await;
            let bus = ConsensusBusClient::new_from_bus(bus.new_handle()).await;

            let mut registry = ValidatorRegistry::new(crypto.as_validator());
            registry.handle_net_message(ValidatorRegistryNetMessage::NewValidator(other));
            let store = ConsensusStore::default();
            let conf = Arc::new(Conf::default());
            let consensus = Consensus::new(
                registry,
                None,
                store,
                ConsensusMetrics::global(&Default::default()),
                conf,
            );
            Self {
                bus,
                crypto,
                out_receiver,
                _event_receiver: event_receiver,
                consensus,
            }
        }

        async fn build() -> (Self, Self) {
            let crypto = crypto::BlstCrypto::new("node-1".into());
            let c_other = crypto::BlstCrypto::new("node-2".into());
            let mut leader = Self::new(crypto.clone(), c_other.as_validator()).await;
            leader.consensus.is_next_leader = true;
            (leader, Self::new(c_other, crypto.as_validator()).await)
        }

        async fn new_slave(leader: &mut TestCtx, id: &str) -> Self {
            let crypto = crypto::BlstCrypto::new(id.into());
            leader.consensus.validators.handle_net_message(
                ValidatorRegistryNetMessage::NewValidator(crypto.as_validator()),
            );
            Self::new(crypto.clone(), leader.crypto.as_validator()).await
        }

        #[track_caller]
        fn handle_msg(&mut self, msg: &SignedWithId<ConsensusNetMessage>, err: &str) {
            debug!(
                "📥 {} Handling message: {:?}",
                self.crypto.validator_id(),
                msg
            );
            self.consensus
                .handle_net_message(msg, &self.crypto, &self.bus)
                .expect(err);
        }

        fn start_new_slot(&mut self) {
            self.consensus
                .start_new_slot(&self.bus, &self.crypto)
                .expect("Failed to start slot");
        }

        #[track_caller]
        fn assert_broadcast(&mut self, err: &str) -> Signed<ConsensusNetMessage, ValidatorId> {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .out_receiver
                .try_recv()
                .expect(format!("{err}: No message broadcasted").as_str());

            if let OutboundMessage::BroadcastMessage(net_msg) = rec {
                if let NetMessage::ConsensusMessage(msg) = net_msg {
                    msg
                } else {
                    panic!("{err}: Consenus OutboundMessage message is missing");
                }
            } else {
                panic!("{err}: Broadcast OutboundMessage message is missing");
            }
        }

        #[track_caller]
        fn assert_send(
            &mut self,
            to: ValidatorId,
            err: &str,
        ) -> Signed<ConsensusNetMessage, ValidatorId> {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .out_receiver
                .try_recv()
                .expect(format!("{err}: No message sent").as_str());

            if let OutboundMessage::SendMessage {
                validator_id: dest,
                msg: net_msg,
            } = rec
            {
                assert_eq!(to, dest);
                if let NetMessage::ConsensusMessage(msg) = net_msg {
                    msg
                } else {
                    panic!("Consenus OutboundMessage message is missing");
                }
            } else {
                panic!("Send OutboundMessage message is missing");
            }
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_happy_path() {
        let (mut leader, mut slave) = TestCtx::build().await;

        leader.start_new_slot();
        for _ in 0..3 {
            let leader_proposal = leader.assert_broadcast("Leader proposal");
            slave.handle_msg(&leader_proposal, "Leader proposal");

            let slave_vote = slave.assert_send("node-1".into(), "Slave vote");
            leader.handle_msg(&slave_vote, "Slave vote");

            let leader_confirm = leader.assert_broadcast("Leader confirm");
            slave.handle_msg(&leader_confirm, "Leader confirm");

            let slave_confirm_ack = slave.assert_send("node-1".into(), "Slave confirm ack");
            leader.handle_msg(&slave_confirm_ack, "Slave confirm ack");

            let leader_commit = leader.assert_broadcast("Leader commit");
            slave.handle_msg(&leader_commit, "Leader commit");
        }

        assert_eq!(slave.consensus.blocks.len(), 3);
    }

    #[test_log::test(tokio::test)]
    async fn test_candidacy() {
        let (mut leader, mut slave) = TestCtx::build().await;

        // Slot 0
        {
            leader.start_new_slot();
            let leader_proposal = leader.assert_broadcast("Leader proposal");
            slave.handle_msg(&leader_proposal, "Leader proposal");
            let slave_vote = slave.assert_send("node-1".into(), "Slave vote");
            leader.handle_msg(&slave_vote, "Slave vote");
            let leader_confirm = leader.assert_broadcast("Leader confirm");
            slave.handle_msg(&leader_confirm, "Leader confirm");
            let slave_confirm_ack = slave.assert_send("node-1".into(), "Slave confirm ack");
            leader.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            let leader_commit = leader.assert_broadcast("Leader commit");
            slave.handle_msg(&leader_commit, "Leader commit");
        }

        let mut slave2 = TestCtx::new_slave(&mut leader, "node-3").await;
        // Slot 1: New slave catchup
        {
            info!("➡️  Leader proposal");
            let leader_proposal = leader.assert_broadcast("Leader proposal");
            slave.handle_msg(&leader_proposal, "Leader proposal");
            slave2.handle_msg(&leader_proposal, "Leader proposal");
            info!("➡️  Slave vote");
            let slave_vote = slave.assert_send("node-1".into(), "Slave vote");
            leader.handle_msg(&slave_vote, "Slave vote");
            info!("➡️  Leader confirm");
            let leader_confirm = leader.assert_broadcast("Leader confirm");
            slave.handle_msg(&leader_confirm, "Leader confirm");
            slave2.handle_msg(&leader_confirm, "Leader confirm");
            info!("➡️  Slave confirm ack");
            let slave_confirm_ack = slave.assert_send("node-1".into(), "Slave confirm ack");
            leader.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            info!("➡️  Leader commit");
            let leader_commit = leader.assert_broadcast("Leader commit");
            slave.handle_msg(&leader_commit, "Leader commit");
            slave2.handle_msg(&leader_commit, "Leader commit");
            info!("➡️  Slave 2 candidacy");
            let slave2_candidacy = slave2.assert_broadcast("Slave 2 candidacy");
            leader.handle_msg(&slave2_candidacy, "Slave 2 candidacy");
        }

        // Slot 2: Still a slot without slave 2
        {
            info!("➡️  Leader proposal");
            let leader_proposal = leader.assert_broadcast("Leader proposal");
            if let ConsensusNetMessage::Prepare(consensus_proposal) = &leader_proposal.msg {
                assert_eq!(consensus_proposal.validators.len(), 2);
            } else {
                panic!("Leader proposal is not a Prepare message");
            }
            slave.handle_msg(&leader_proposal, "Leader proposal");
            slave2.handle_msg(&leader_proposal, "Leader proposal");
            info!("➡️  Slave vote");
            let slave_vote = slave.assert_send("node-1".into(), "Slave vote");
            leader.handle_msg(&slave_vote, "Slave vote");
            info!("➡️  Leader confirm");
            let leader_confirm = leader.assert_broadcast("Leader confirm");
            slave.handle_msg(&leader_confirm, "Leader confirm");
            slave2.handle_msg(&leader_confirm, "Leader confirm");
            info!("➡️  Slave confirm ack");
            let slave_confirm_ack = slave.assert_send("node-1".into(), "Slave confirm ack");
            leader.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            info!("➡️  Leader commit");
            let leader_commit = leader.assert_broadcast("Leader commit");
            slave.handle_msg(&leader_commit, "Leader commit");
            slave2.handle_msg(&leader_commit, "Leader commit");
            info!("➡️  Slave 2 candidacy");
            let slave2_candidacy = slave2.assert_broadcast("Slave 2 candidacy");
            leader.handle_msg(&slave2_candidacy, "Slave 2 candidacy");
        }

        // Slot 3: Slave 2 joined consensus
        {
            info!("➡️  Leader proposal");
            let leader_proposal = leader.assert_broadcast("Leader proposal");
            if let ConsensusNetMessage::Prepare(consensus_proposal) = &leader_proposal.msg {
                assert_eq!(consensus_proposal.validators.len(), 3);
            } else {
                panic!("Leader proposal is not a Prepare message");
            }
            slave.handle_msg(&leader_proposal, "Leader proposal");
            slave2.handle_msg(&leader_proposal, "Leader proposal");
            info!("➡️  Slave vote");
            let slave_vote = slave.assert_send("node-1".into(), "Slave vote");
            let slave2_vote = slave2.assert_send("node-1".into(), "Slave vote");
            leader.handle_msg(&slave_vote, "Slave vote");
            leader.handle_msg(&slave2_vote, "Slave vote");
            info!("➡️  Leader confirm");
            let leader_confirm = leader.assert_broadcast("Leader confirm");
            slave.handle_msg(&leader_confirm, "Leader confirm");
            slave2.handle_msg(&leader_confirm, "Leader confirm");
            info!("➡️  Slave confirm ack");
            let slave_confirm_ack = slave.assert_send("node-1".into(), "Slave confirm ack");
            let slave2_confirm_ack = slave2.assert_send("node-1".into(), "Slave confirm ack");
            leader.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            leader.handle_msg(&slave2_confirm_ack, "Slave confirm ack");
            info!("➡️  Leader commit");
            let leader_commit = leader.assert_broadcast("Leader commit");
            slave.handle_msg(&leader_commit, "Leader commit");
            slave2.handle_msg(&leader_commit, "Leader commit");
        }

        assert_eq!(slave.consensus.bft_round_state.slot, 4);
        assert_eq!(slave2.consensus.bft_round_state.slot, 4);
    }
}
