//! Handles all consensus logic up to block commitment.

use anyhow::{anyhow, bail, Context, Error, Result};
use bincode::{Decode, Encode};
use metrics::ConsensusMetrics;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use staking::{Stake, Staker, Staking};
use std::{
    collections::{HashMap, HashSet},
    default::Default,
    fmt::Display,
    io::Write,
    ops::{Deref, DerefMut},
    path::PathBuf,
    time::Duration,
};
use tokio::{sync::broadcast, time::sleep};
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
        constants,
        crypto::{BlstCrypto, SharedBlstCrypto},
        logger::LogMe,
        modules::Module,
        static_type_map::Pick,
    },
    validator_registry::{
        ValidatorId, ValidatorPublicKey, ValidatorRegistry, ValidatorRegistryNetMessage,
    },
};

pub mod metrics;
pub mod staking;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub enum ConsensusNetMessage {
    StartNewSlot,
    Prepare(ConsensusProposal),
    PrepareVote(ConsensusProposalHash),
    Confirm(ConsensusProposalHash, QuorumCertificate),
    ConfirmAck(ConsensusProposalHash),
    Commit(ConsensusProposalHash, QuorumCertificate),
    ValidatorCandidacy(ValidatorCandidacy),
    CatchupRequest(),
    CatchupResponse(Staking),
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
    NewStaker(Staker),
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
    staking: Staking,
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
    new_bonded_validators: Vec<NewValidatorCandidate>,
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

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct NewValidatorCandidate {
    pubkey: ValidatorPublicKey, // TODO: possible optim: the pubkey is already present in the msg,
    msg: SignedWithId<ConsensusNetMessage>,
}

#[derive(Encode, Decode, Default)]
pub struct ConsensusStore {
    is_next_leader: bool,
    /// Validators that asked to be part of consensus
    new_validators_candidates: Vec<NewValidatorCandidate>,
    bft_round_state: BFTRoundState,
    // FIXME: pub is here for testing
    pub blocks: Vec<Block>,
    pending_batches: Vec<Vec<Transaction>>,
}

pub struct Consensus {
    metrics: ConsensusMetrics,
    bus: ConsensusBusClient,
    pubkeys: ValidatorRegistry,
    file: Option<PathBuf>,
    store: ConsensusStore,
    crypto: SharedBlstCrypto,
    #[allow(dead_code)]
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
        let metrics = ConsensusMetrics::global(ctx.config.id.clone());
        let pubkeys = ctx.validator_registry.share();
        let bus = ConsensusBusClient::new_from_bus(ctx.bus.new_handle()).await;

        Ok(Consensus {
            metrics,
            bus,
            pubkeys,
            file: Some(file),
            store,
            crypto: ctx.crypto.clone(),
            config: ctx.config.clone(),
        })
    }

    fn run(&mut self, ctx: Self::Context) -> impl futures::Future<Output = Result<()>> + Send {
        _ = self.start_master(ctx.config.clone());
        self.start(ctx.config.clone())
    }
}

impl Consensus {
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

        // add new bonded validators pubkeys to Validators
        let new_validators = self.new_validators_candidates.clone();
        for new_validator in new_validators {
            // Bond the candidate
            self.bft_round_state
                .staking
                .bond(new_validator.pubkey.clone())
                .expect("Failed to bond new validator");
            validators.push(new_validator.pubkey);
        }

        // Start Consensus with following block
        self.bft_round_state.consensus_proposal = ConsensusProposal {
            slot: self.bft_round_state.slot,
            view: self.bft_round_state.view,
            previous_consensus_proposal_hash,
            previous_commit_quorum_certificate,
            validators,
            new_bonded_validators: self.new_validators_candidates.drain(..).collect(),
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

        self.genesis_bond(self.pubkeys.list_validators().as_slice())
            .expect("Failed to bond genesis validators");

        // Start Consensus with following block
        self.bft_round_state.consensus_proposal = ConsensusProposal {
            slot: self.bft_round_state.slot,
            view: self.bft_round_state.view,
            previous_consensus_proposal_hash: ConsensusProposalHash(vec![]),
            previous_commit_quorum_certificate: QuorumCertificate::default(),
            validators: self.pubkeys.list_validators(),
            new_bonded_validators: vec![],
            block: first_block,
        };
    }

    fn genesis_bond(&mut self, validators: &[ValidatorPublicKey]) -> Result<()> {
        // Bond all validators
        for validator in validators {
            self.bft_round_state.staking.add_staker(Staker {
                pubkey: validator.clone(),
                stake: Stake { amount: 100 },
            })?;
            self.bft_round_state.staking.bond(validator.clone())?;
        }
        Ok(())
    }

    /// Send block to internal bus
    fn add_block(&mut self) -> Result<(), Error> {
        _ = self
            .bus
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

    /// Add and applies new block to its NodeState through ConsensusEvent
    fn finish_round(&mut self) -> Result<(), Error> {
        self.add_block()?;

        info!(
            "🔒 Slot {} finished",
            self.bft_round_state.consensus_proposal.slot
        );

        self.bft_round_state.step = Step::StartNewSlot;
        self.bft_round_state.slot = self.bft_round_state.consensus_proposal.slot + 1;
        self.bft_round_state.view = 0;
        self.bft_round_state.prepare_votes = HashSet::default();
        self.bft_round_state.confirm_ack = HashSet::default();

        // Save added block
        if let Some(file) = &self.file {
            Self::save_on_disk(file.as_path(), &self.store)?;
        }

        Ok(())
    }

    /// Verify that the vote is for the correct proposal
    fn verify_consensus_proposal_hash(
        &self,
        consensus_proposal_hash: &ConsensusProposalHash,
    ) -> bool {
        &self.bft_round_state.consensus_proposal.hash() == consensus_proposal_hash
    }

    /// Verify that quorum certificate includes only validators that are part of the consensus
    fn verify_quorum_signers_part_of_consensus(
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

    /// Verify that new bonded validators
    /// are part of the consensus
    /// and have enough stake
    /// and have a valid signature
    fn verify_new_bonded_validators(&mut self, proposal: &ConsensusProposal) -> Result<()> {
        for new_validator in &proposal.new_bonded_validators {
            // Verify that the new validator is part of the consensus
            if !proposal.validators.contains(&new_validator.pubkey) {
                bail!("New bonded validator is not part of the consensus");
            }
            // Verify that the new validator has enough stake
            if let Some(stake) = self
                .bft_round_state
                .staking
                .get_stake(&new_validator.pubkey)
            {
                if stake.amount < constants::MIN_STAKE {
                    bail!("New bonded validator has not enough stake to be bonded");
                }
            } else {
                bail!("New bonded validator has no stake");
            }
            // Verify that the new validator has a valid signature
            let id = new_validator
                .msg
                .validators
                .first()
                .ok_or(anyhow!("No validator in msg"))?
                .clone();
            self.pubkeys.add(id, new_validator.pubkey.clone());
            if !self.pubkeys.check_signed(&new_validator.msg)? {
                bail!("New bonded validator has an invalid signature");
            }
            // Verify that the signed message is a matching candidacy
            if let ConsensusNetMessage::ValidatorCandidacy(ValidatorCandidacy {
                pubkey,
                slot,
                previous_consensus_proposal_hash,
            }) = &new_validator.msg.msg
            {
                if pubkey != &new_validator.pubkey
                    || slot != &self.bft_round_state.consensus_proposal.slot
                    || previous_consensus_proposal_hash
                        != &self
                            .bft_round_state
                            .consensus_proposal
                            .previous_consensus_proposal_hash
                {
                    debug!("Invalid candidacy message");
                    debug!("Got - Expected");
                    debug!(
                        "{} - {}",
                        previous_consensus_proposal_hash,
                        self.bft_round_state
                            .consensus_proposal
                            .previous_consensus_proposal_hash
                    );
                    debug!(
                        "{} - {}",
                        slot, self.bft_round_state.consensus_proposal.slot
                    );
                    debug!("{} - {}", pubkey, new_validator.pubkey);

                    bail!("New bonded validator has an invalid candidacy message");
                }
            } else {
                bail!("New bonded validator forwarded signed message is not a candidacy message");
            }
        }
        Ok(())
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

    fn start_new_slot(&mut self) -> Result<(), Error> {
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
                    self.new_validators_candidates.len()
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
                    self.pubkeys.get_validators_count(),
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
        _ = self
            .bus
            .send(OutboundMessage::broadcast(self.sign_net_message(
                ConsensusNetMessage::Prepare(self.bft_round_state.consensus_proposal.clone()),
            )?))
            .context("Failed to broadcast ConsensusNetMessage::Prepare msg on the bus")?;

        Ok(())
    }

    fn compute_f(&self) -> u64 {
        self.bft_round_state.staking.total_bond() / 3
    }

    fn get_own_voting_power(&self) -> u64 {
        if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            if let Some(my_sake) = self
                .bft_round_state
                .staking
                .get_stake(self.crypto.validator_pubkey())
            {
                my_sake.amount
            } else {
                panic!("I'm not in my own staking registry !")
            }
        } else {
            0
        }
    }

    fn compute_voting_power(&self, validators: &[ValidatorPublicKey]) -> u64 {
        validators
            .iter()
            .flat_map(|v| self.bft_round_state.staking.get_stake(v).map(|s| s.amount))
            .sum::<u64>()
    }

    fn handle_net_message(&mut self, msg: &SignedWithId<ConsensusNetMessage>) -> Result<(), Error> {
        if !self.pubkeys.check_signed(msg)? {
            self.metrics.signature_error("prepare");
            bail!("Invalid signature for message {:?}", msg);
        }

        match &msg.msg {
            // TODO: do we really get a net message for StartNewSlot ?
            ConsensusNetMessage::StartNewSlot => self.start_new_slot(),
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
                            _ = self
                                .bus
                                .send(OutboundMessage::send(
                                    self.leader_id(),
                                    self.sign_net_message(ConsensusNetMessage::CatchupRequest(
                                    ))?,
                                ))
                                .context(
                                    "Failed to send ConsensusNetMessage::PrepareVote msg on the bus",
                                )?;
                        } else {
                            info!("#### Received genesis block proposal ####");
                            self.genesis_bond(&consensus_proposal.validators)?;
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
                        self.finish_round()?;
                    }
                };

                self.verify_new_bonded_validators(consensus_proposal)?;

                // Update consensus_proposal
                self.bft_round_state.consensus_proposal = consensus_proposal.clone();

                // Responds PrepareVote message to leader with validator's vote on this proposal
                if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
                    info!(
                        proposal_hash = %consensus_proposal.hash(),
                        "📤 Slot {} Prepare message validated. Sending PrepareVote to leader",
                        self.bft_round_state.slot
                    );
                    _ = self
                        .bus
                        .send(OutboundMessage::send(
                            self.leader_id(),
                            self.sign_net_message(ConsensusNetMessage::PrepareVote(
                                consensus_proposal.hash(),
                            ))?,
                        ))
                        .context(
                            "Failed to send ConsensusNetMessage::PrepareVote msg on the bus",
                        )?;
                } else {
                    info!("😥 Not part of consensus, not sending PrepareVote");
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
                    msg.with_pub_keys(self.pubkeys.get_pub_keys_from_id(msg.validators.clone())),
                );

                // Get matching vote count
                let validated_votes = self
                    .bft_round_state
                    .prepare_votes
                    .iter()
                    .flat_map(|signed_message| signed_message.validators.clone())
                    .collect::<HashSet<ValidatorPublicKey>>();

                let votes_power = self.compute_voting_power(
                    validated_votes.into_iter().collect::<Vec<_>>().as_slice(),
                );

                let voting_power = votes_power + self.get_own_voting_power();

                self.metrics.prepare_votes_gauge(voting_power);

                // Waits for at least n-f = 2f+1 matching PrepareVote messages
                let f = self.compute_f();

                info!(
                    "📩 Slot {} validated votes: {} / {} ({} validators for a total bond = {})",
                    self.bft_round_state.slot,
                    voting_power,
                    2 * f + 1,
                    self.bft_round_state.consensus_proposal.validators.len(),
                    self.bft_round_state.staking.total_bond()
                );
                if voting_power > 2 * f + 1 {
                    // Get all received signatures
                    let aggregates: &Vec<&Signed<ConsensusNetMessage, ValidatorPublicKey>> =
                        &self.bft_round_state.prepare_votes.iter().collect();

                    // Aggregates them into a *Prepare* Quorum Certificate
                    let prepvote_signed_aggregation = self.crypto.sign_aggregate(
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
                    _ = self
                        .bus
                        .send(OutboundMessage::broadcast(self.sign_net_message(
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
                if !self.verify_quorum_signers_part_of_consensus(prepare_quorum_certificate) {
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

                        let voting_power = self
                            .compute_voting_power(prepare_quorum_certificate.validators.as_slice());

                        // Verify enough validators signed
                        let f = self.compute_f();

                        info!(
                            "📩 Slot {} validated votes: {} / {} ({} validators for a total bond = {})",
                                self.bft_round_state.slot,
                                voting_power,
                                2 * f + 1,
                                self.bft_round_state.consensus_proposal.validators.len(),
                                self.bft_round_state.staking.total_bond()
                        );

                        if voting_power < 2 * f + 1 {
                            self.metrics.confirm_error("prepare_qc_incomplete");
                            bail!("Prepare Quorum Certificate does not contain enough voting power")
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
                if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
                    info!(
                        proposal_hash = %consensus_proposal_hash,
                        "📤 Slot {} Confirm message validated. Sending ConfirmAck to leader",
                        self.bft_round_state.slot
                    );
                    _ = self
                        .bus
                        .send(OutboundMessage::send(
                            self.leader_id(),
                            self.sign_net_message(ConsensusNetMessage::ConfirmAck(
                                consensus_proposal_hash.clone(),
                            ))?,
                        ))
                        .context("Failed to send ConsensusNetMessage::ConfirmAck msg on the bus")?;
                } else {
                    info!("😥 Not part of consensus, not sending ConfirmAck");
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
                    msg.with_pub_keys(self.pubkeys.get_pub_keys_from_id(msg.validators.clone())),
                ) {
                    self.metrics.confirm_ack("already_processed");
                    info!("ConfirmAck has already been processed");

                    return Ok(());
                }

                // Get ConfirmAck count
                let confirmed_ack_validators = self
                    .bft_round_state
                    .confirm_ack
                    .iter()
                    .flat_map(|signed_message| signed_message.validators.clone())
                    .collect::<HashSet<ValidatorPublicKey>>();

                let confirmed_power = self.compute_voting_power(
                    confirmed_ack_validators
                        .into_iter()
                        .collect::<Vec<_>>()
                        .as_slice(),
                );
                let voting_power = confirmed_power + self.get_own_voting_power();
                let f = self.compute_f();
                info!(
                    "✅ Slot {} confirmed acks: {} / {} ({} validators for a total bond = {})",
                    self.bft_round_state.slot,
                    voting_power,
                    2 * f + 1,
                    self.bft_round_state.consensus_proposal.validators.len(),
                    self.bft_round_state.staking.total_bond()
                );
                self.metrics.confirmed_ack_gauge(voting_power);
                if voting_power > 2 * f + 1 {
                    // Get all signatures received and change ValidatorId for ValidatorPubKey
                    let aggregates: &Vec<&Signed<ConsensusNetMessage, ValidatorPublicKey>> =
                        &self.bft_round_state.confirm_ack.iter().collect();

                    // Aggregates them into a *Commit* Quorum Certificate
                    let commit_signed_aggregation = self.crypto.sign_aggregate(
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
                    _ = self
                        .bus
                        .send(OutboundMessage::broadcast(self.sign_net_message(
                            ConsensusNetMessage::Commit(
                                consensus_proposal_hash.clone(),
                                commit_quorum_certificate,
                            ),
                        )?))
                        .context(
                            "Failed to broadcast ConsensusNetMessage::Confirm msg on the bus",
                        )?;

                    // Finishes the bft round
                    self.finish_round()?;
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
                        self.start_new_slot()?;
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
                if !self.verify_quorum_signers_part_of_consensus(commit_quorum_certificate) {
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
                        let voting_power = self
                            .compute_voting_power(commit_quorum_certificate.validators.as_slice());
                        let f = self.compute_f();
                        if voting_power < 2 * f + 1 {
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
                self.finish_round()?;

                self.metrics.commit();

                // Start new slot
                if self.is_next_leader() {
                    // Send Prepare message to all validators
                    self.start_new_slot()?;
                } else if !self.is_part_of_consensus(self.crypto.validator_pubkey()) {
                    // Ask to be part of consensus
                    let candidacy = ValidatorCandidacy {
                        pubkey: self.crypto.validator_pubkey().clone(),
                        slot: self.bft_round_state.slot,
                        previous_consensus_proposal_hash: consensus_proposal_hash.clone(),
                    };
                    info!(
                        "📝 Sending candidacy message to be part of consensus.  {}",
                        candidacy
                    );
                    _ = self
                        .bus
                        .send(OutboundMessage::broadcast(self.sign_net_message(
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

                // Verify that the candidate has enough stake
                if let Some(stake) = self.bft_round_state.staking.get_stake(&candidacy.pubkey) {
                    if stake.amount < constants::MIN_STAKE {
                        bail!("🛑 Candidate validator does not have enough stake to be part of consensus");
                    }
                } else {
                    bail!("🛑 Candidate validator is not staking !");
                }

                // Add validator to consensus candidates
                self.new_validators_candidates.push(NewValidatorCandidate {
                    pubkey: candidacy.pubkey.clone(),
                    msg: msg.clone(),
                });
                Ok(())
            }
            ConsensusNetMessage::CatchupRequest() => {
                info!("🔄 Catchup request received");
                _ = self
                    .bus
                    .send(OutboundMessage::send(
                        msg.validators
                            .first()
                            .ok_or(anyhow!("No validator in msg"))?
                            .clone(),
                        self.sign_net_message(ConsensusNetMessage::CatchupResponse(
                            self.bft_round_state.staking.clone(),
                        ))?,
                    ))
                    .context(
                        "Failed to send ConsensusNetMessage::CatchupResponse msg on the bus",
                    )?;
                Ok(())
            }
            ConsensusNetMessage::CatchupResponse(staking) => {
                info!("🔄 Catchup response received");
                if self.bft_round_state.slot == 0
                    && self.bft_round_state.slot != self.bft_round_state.consensus_proposal.slot
                {
                    self.bft_round_state.staking = staking.clone();
                    Ok(())
                } else {
                    bail!("CatchupResponse is only valid for genesis block");
                }
            }
        }
    }

    fn handle_command(&mut self, msg: ConsensusCommand) -> Result<()> {
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
                _ = self
                    .bus
                    .send(ConsensusEvent::CommitBlock {
                        block: block.clone(),
                    })
                    .context("Failed to send ConsensusEvent::CommitBlock msg on the bus")?;
                Ok(())
            }
            ConsensusCommand::NewStaker(staker) => {
                self.store.bft_round_state.staking.add_staker(staker)
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

    pub async fn start_master(&mut self, config: SharedConf) -> Result<()> {
        let interval = config.storage.interval;

        // hack to avoid another bus for a specific wip case
        let command_sender = Pick::<broadcast::Sender<ConsensusCommand>>::get(&self.bus).clone();
        if config.id == ValidatorId("single-node".to_owned()) {
            info!(
                "No peers configured, starting as master generating blocks every {} seconds",
                interval
            );

            tokio::spawn(async move {
                loop {
                    sleep(Duration::from_secs(interval)).await;

                    _ = command_sender
                        .send(ConsensusCommand::SingleNodeBlockGeneration)
                        .log_error("Cannot send message over channel");
                }
            });
        }

        Ok(())
    }

    pub async fn start(&mut self, config: SharedConf) -> Result<()> {
        // FIXME: node-1 sera le leader
        if config.id == ValidatorId("node-1".to_owned()) {
            self.is_next_leader = true;
        }

        handle_messages! {
            on_bus self.bus,
            listen<ConsensusCommand> cmd => {
                match self.handle_command(cmd) {
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
                match self.handle_net_message(&cmd) {
                    Ok(_) => (),
                    Err(e) => warn!("Consensus message failed: {:#}", e),
                }
            }
            listen<ValidatorRegistryNetMessage> cmd => {
                self.pubkeys.handle_net_message(cmd.clone());
                if self.pubkeys.get_validators_count() == 2 && self.is_next_leader() {
                    // Start first slot
                    debug!("Got a 2nd validator, starting first slot");
                    sleep(Duration::from_secs(2)).await;
                    _ = self.start_new_slot();
                }
            }
        }
    }

    fn sign_net_message(
        &self,
        msg: ConsensusNetMessage,
    ) -> Result<SignedWithId<ConsensusNetMessage>> {
        self.crypto.sign(msg)
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
    use assertables::assert_contains;
    use staking::Stake;
    use tokio::sync::broadcast::Receiver;

    struct TestCtx {
        out_receiver: Receiver<OutboundMessage>,
        _event_receiver: Receiver<ConsensusEvent>,
        consensus: Consensus,
    }

    impl TestCtx {
        async fn new(crypto: BlstCrypto, other: ConsensusValidator) -> Self {
            let shared_bus = SharedMessageBus::new();
            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let bus = ConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

            let mut registry = ValidatorRegistry::new(crypto.as_validator());
            registry.handle_net_message(ValidatorRegistryNetMessage::NewValidator(other));
            let store = ConsensusStore::default();
            let conf = Arc::new(Conf::default());
            let consensus = Consensus {
                metrics: ConsensusMetrics::global(ValidatorId("id".to_string())),
                bus,
                pubkeys: registry,
                file: None,
                store,
                crypto: Arc::new(crypto),
                config: conf,
            };

            Self {
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

            let slave = Self::new(c_other, crypto.as_validator()).await;

            (leader, slave)
        }

        async fn new_slave(leader: &mut TestCtx, id: &str) -> Self {
            let crypto = crypto::BlstCrypto::new(id.into());
            leader
                .consensus
                .pubkeys
                .handle_net_message(ValidatorRegistryNetMessage::NewValidator(
                    crypto.as_validator(),
                ));
            Self::new(crypto.clone(), leader.consensus.crypto.as_validator()).await
        }

        #[track_caller]
        fn handle_msg(&mut self, msg: &SignedWithId<ConsensusNetMessage>, err: &str) {
            debug!(
                "📥 {} Handling message: {:?}",
                self.consensus.crypto.validator_id(),
                msg
            );
            self.consensus.handle_net_message(msg).expect(err);
        }

        #[track_caller]
        fn handle_msg_err(&mut self, msg: &SignedWithId<ConsensusNetMessage>) -> Error {
            debug!(
                "📥 {} Handling message expecting err: {:?}",
                self.consensus.crypto.validator_id(),
                msg
            );
            let err = self.consensus.handle_net_message(msg).unwrap_err();
            info!("Expected error: {:#}", err);
            err
        }

        #[track_caller]
        fn add_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            debug!(
                "➕ {} Add staker: {:?}",
                self.consensus.crypto.validator_id(),
                staker.consensus.crypto.validator_id()
            );
            self.consensus
                .pubkeys
                .handle_net_message(ValidatorRegistryNetMessage::NewValidator(
                    staker.consensus.crypto.as_validator(),
                ));
            self.consensus
                .handle_command(ConsensusCommand::NewStaker(Staker {
                    pubkey: staker.consensus.crypto.validator_pubkey().clone(),
                    stake: Stake { amount },
                }))
                .expect(err)
        }

        #[track_caller]
        fn add_bonded_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            self.add_staker(staker, amount, err);
            self.bond(staker, err);
        }

        #[track_caller]
        fn with_stake(&mut self, amount: u64, err: &str) {
            self.consensus
                .pubkeys
                .handle_net_message(ValidatorRegistryNetMessage::NewValidator(
                    self.consensus.crypto.as_validator(),
                ));
            self.consensus
                .handle_command(ConsensusCommand::NewStaker(Staker {
                    pubkey: self.consensus.crypto.validator_pubkey().clone(),
                    stake: Stake { amount },
                }))
                .expect(err);
        }

        #[track_caller]
        fn bond(&mut self, staker: &Self, err: &str) {
            debug!(
                "➕ {} Bond staker: {:?}",
                self.consensus.crypto.validator_id(),
                staker.consensus.crypto.validator_id()
            );
            self.consensus
                .bft_round_state
                .staking
                .bond(staker.consensus.crypto.validator_pubkey().clone())
                .expect(err);
        }

        fn start_new_slot(&mut self) {
            self.consensus
                .start_new_slot()
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
        slave2.with_stake(100, "Add stake");
        slave2.add_bonded_staker(&leader, 100, "Add staker");
        slave2.add_bonded_staker(&slave, 100, "Add staker");
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
            assert_contains!(
                leader.handle_msg_err(&slave2_candidacy).to_string(),
                "validator is not staking"
            );
            leader.add_staker(&slave2, 100, "Add staker");
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
            // Slave refuses to vote because it does not know slave 2 stake
            assert_contains!(
                slave.handle_msg_err(&leader_proposal).to_string(),
                "validator has no stake"
            );
            slave.add_staker(&slave2, 100, "Add staker");
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
