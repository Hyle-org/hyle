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
        Transaction, ValidatorPublicKey,
    },
    p2p::network::{OutboundMessage, PeerEvent, Signature, Signed, SignedWithKey},
    utils::{
        conf::SharedConf,
        crypto::{BlstCrypto, SharedBlstCrypto},
        logger::LogMe,
        modules::Module,
        static_type_map::Pick,
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
    StartNewSlot,
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
}

// TODO: move struct to model.rs ?
#[derive(Encode, Decode, Default)]
pub struct BFTRoundState {
    consensus_proposal: ConsensusProposal,
    slot: Slot,
    view: View,
    step: Step,
    leader_index: u64,
    leader_pubkey: ValidatorPublicKey,
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
    next_leader: u64,
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
        write!(f, "Pubkey: {}", self.pubkey)
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
    receiver(ConsensusCommand),
    receiver(MempoolEvent),
    receiver(SignedWithKey<ConsensusNetMessage>),
    receiver(PeerEvent),
}
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct NewValidatorCandidate {
    pubkey: ValidatorPublicKey, // TODO: possible optim: the pubkey is already present in the msg,
    msg: SignedWithKey<ConsensusNetMessage>,
}

#[derive(Encode, Decode, Default)]
pub struct ConsensusStore {
    is_next_leader: bool,
    /// Validators that asked to be part of consensus
    new_validators_candidates: Vec<NewValidatorCandidate>,
    bft_round_state: BFTRoundState,
    /// Proposals that we refuse to vote for, but that might be valid
    /// it can happen we consider invalid because we missed a slot
    /// but if we get a consensus on this proposal, we should accept it
    buffered_invalid_proposals: HashMap<ConsensusProposalHash, ConsensusProposal>,
    // FIXME: pub is here for testing
    pub blocks: Vec<Block>,
    pending_batches: Vec<Vec<Transaction>>,
}

pub struct Consensus {
    metrics: ConsensusMetrics,
    bus: ConsensusBusClient,
    genesis_pubkeys: Vec<ValidatorPublicKey>,
    file: Option<PathBuf>,
    store: ConsensusStore,
    #[allow(dead_code)]
    config: SharedConf,
    crypto: SharedBlstCrypto,
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

    async fn build(ctx: Self::Context) -> Result<Self> {
        let file = ctx
            .common
            .config
            .data_directory
            .clone()
            .join("consensus.bin");
        let mut store: ConsensusStore = Self::load_from_disk_or_default(file.as_path());
        let metrics = ConsensusMetrics::global(ctx.common.config.id.clone());
        let bus = ConsensusBusClient::new_from_bus(ctx.common.bus.new_handle()).await;

        // FIXME a bit hacky for now
        if store.bft_round_state.leader_pubkey == ValidatorPublicKey::default() {
            store.bft_round_state.leader_index = 0;
            //store.bft_round_state.leader_id = "node-1".to_string(); FIXME
            if ctx.common.config.id == "node-1" {
                store.is_next_leader = true;
            }
        }

        Ok(Consensus {
            metrics,
            bus,
            genesis_pubkeys: vec![ctx.node.crypto.validator_pubkey().clone()],
            file: Some(file),
            store,
            config: ctx.common.config.clone(),
            crypto: ctx.node.crypto.clone(),
        })
    }

    fn run(&mut self) -> impl futures::Future<Output = Result<()>> + Send {
        _ = self.start_master(self.config.clone());
        self.start()
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
            next_leader: (self.bft_round_state.leader_index + 1) % validators.len() as u64,
            previous_consensus_proposal_hash,
            previous_commit_quorum_certificate,
            validators,
            new_bonded_validators: self.new_validators_candidates.drain(..).collect(),
            block,
        };
        Ok(())
    }

    /// On genesis, create a consensus proposal with all validators connected to node-1
    /// will grand them a gree stake to start
    /// this genesis logic might change later
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

        let validators = self.genesis_pubkeys.clone();
        self.genesis_bond(validators.as_slice())
            .expect("Failed to bond genesis validators");

        // Start Consensus with following block
        self.bft_round_state.consensus_proposal = ConsensusProposal {
            slot: self.bft_round_state.slot,
            view: self.bft_round_state.view,
            next_leader: 1,
            previous_consensus_proposal_hash: ConsensusProposalHash(vec![]),
            previous_commit_quorum_certificate: QuorumCertificate::default(),
            validators,
            new_bonded_validators: vec![],
            block: first_block,
        };
    }

    /// On genesis slot, bond all validators
    /// see create_genesis_consensus_proposal
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
        self.bft_round_state.leader_index = self.bft_round_state.consensus_proposal.next_leader;
        self.bft_round_state.view = 0;
        self.bft_round_state.prepare_votes = HashSet::default();
        self.bft_round_state.confirm_ack = HashSet::default();

        self.bft_round_state.leader_pubkey = self
            .bft_round_state
            .consensus_proposal
            .validators
            .get(self.bft_round_state.leader_index as usize)
            .ok_or_else(|| anyhow!("wrong leader index {}", self.bft_round_state.leader_index))?
            .clone();

        self.is_next_leader = self.leader_id() == *self.crypto.validator_pubkey();
        if self.is_next_leader() {
            info!("👑 I'm the new leader! 👑");
        } else {
            info!("👑 Next leader: {}", self.leader_id());
        }

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
        quorum_certificate: &QuorumCertificate,
        consensus_proposal: &ConsensusProposal,
    ) -> bool {
        quorum_certificate
            .validators
            .iter()
            .all(|v| consensus_proposal.validators.iter().any(|v2| v2 == v))
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
                if stake.amount < staking::MIN_STAKE {
                    bail!("New bonded validator has not enough stake to be bonded");
                }
            } else {
                bail!("New bonded validator has no stake");
            }
            // Verify that the new validator has a valid signature
            if !BlstCrypto::verify(&new_validator.msg)? {
                bail!("New bonded validator has an invalid signature");
            }
            // Verify that the signed message is a matching candidacy
            if let ConsensusNetMessage::ValidatorCandidacy(ValidatorCandidacy { pubkey }) =
                &new_validator.msg.msg
            {
                if pubkey != &new_validator.pubkey {
                    debug!("Invalid candidacy message");
                    debug!("Got - Expected");
                    debug!("{} - {}", pubkey, new_validator.pubkey);

                    bail!("New bonded validator has an invalid candidacy message");
                }

                self.bft_round_state
                    .staking
                    .bond(new_validator.pubkey.clone())?;
                self.new_validators_candidates
                    .retain(|v| v.pubkey != new_validator.pubkey);
            } else {
                bail!("New bonded validator forwarded signed message is not a candidacy message");
            }
        }
        Ok(())
    }

    fn is_next_leader(&self) -> bool {
        self.is_next_leader
    }

    fn is_part_of_consensus(&self, pubkey: &ValidatorPublicKey) -> bool {
        self.bft_round_state
            .consensus_proposal
            .validators
            .contains(pubkey)
    }

    fn leader_id(&self) -> ValidatorPublicKey {
        self.bft_round_state.leader_pubkey.clone()
    }

    fn delay_start_new_slot(&mut self) -> Result<(), Error> {
        #[cfg(not(test))]
        {
            let command_sender =
                Pick::<broadcast::Sender<ConsensusCommand>>::get(&self.bus).clone();
            let interval = self.config.consensus.slot_duration;
            tokio::spawn(async move {
                info!(
                    "⏱️  Sleeping {} seconds before starting a new slot",
                    interval
                );
                sleep(Duration::from_secs(interval)).await;

                _ = command_sender
                    .send(ConsensusCommand::StartNewSlot)
                    .log_error("Cannot send message over channel");
            });
            Ok(())
        }
        #[cfg(test)]
        self.start_new_slot()
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
                    self.genesis_pubkeys.len(),
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

    /// Verify that the certificate signers are legit for proposal
    /// and update current proposal if the quorum is valid
    /// precondition: verification of voting power is already made on quorum
    fn verify_quorum_and_catchup(
        &mut self,
        consensus_proposal_hash: &ConsensusProposalHash,
        prepare_quorum_certificate: &QuorumCertificate,
    ) -> Result<(), Error> {
        // Verify that the Confirm is for the correct proposal
        let proposal: ConsensusProposal = match self
            .verify_consensus_proposal_hash(consensus_proposal_hash)
        {
            true => self.bft_round_state.consensus_proposal.clone(),
            false => {
                // If the verification failed, we might have missed a slot
                if let Some(proposal) = self
                    .buffered_invalid_proposals
                    .remove(consensus_proposal_hash)
                {
                    warn!("Confirm refers to proposal hash I didn't vote for because I might have missed a slot. Restoring this proposal as valid if quorum signers are part of the consensus.");
                    proposal
                } else {
                    bail!("Confirm refers to an unknwon proposal hash.");
                }
            }
        };

        if proposal.slot < self.bft_round_state.slot {
            bail!(
                "Quorum for a passed slot {} while we are in slot {}",
                proposal.slot,
                self.bft_round_state.slot
            );
        }
        if proposal.slot == self.bft_round_state.slot && proposal.view < self.bft_round_state.view {
            bail!(
                "Quorum for a passed view {}:{} while we are in slot {}:{}",
                proposal.slot,
                proposal.view,
                self.bft_round_state.slot,
                self.bft_round_state.view
            );
        }

        // Verify that validators that signed are legit
        if !Self::verify_quorum_signers_part_of_consensus(prepare_quorum_certificate, &proposal) {
            bail!(
                "Prepare quorum certificate includes validators that are not part of the consensus"
            )
        }

        self.bft_round_state.consensus_proposal = proposal;
        Ok(())
    }

    fn handle_net_message(
        &mut self,
        msg: &SignedWithKey<ConsensusNetMessage>,
    ) -> Result<(), Error> {
        if !BlstCrypto::verify(msg)? {
            self.metrics.signature_error("prepare");
            bail!("Invalid signature for message {:?}", msg);
        }

        match &msg.msg {
            // TODO: do we really get a net message for StartNewSlot ?
            ConsensusNetMessage::StartNewSlot => self.start_new_slot(),
            ConsensusNetMessage::Prepare(consensus_proposal) => {
                self.on_prepare(msg, consensus_proposal)
            }
            ConsensusNetMessage::PrepareVote(consensus_proposal_hash) => {
                self.on_prepare_vote(msg, consensus_proposal_hash)
            }
            ConsensusNetMessage::Confirm(consensus_proposal_hash, prepare_quorum_certificate) => {
                self.on_confirm(consensus_proposal_hash, prepare_quorum_certificate)
            }
            ConsensusNetMessage::ConfirmAck(consensus_proposal_hash) => {
                self.on_confirm_ack(msg, consensus_proposal_hash)
            }
            ConsensusNetMessage::Commit(consensus_proposal_hash, commit_quorum_certificate) => {
                self.on_commit(consensus_proposal_hash, commit_quorum_certificate)
            }
            ConsensusNetMessage::ValidatorCandidacy(candidacy) => {
                self.on_validator_candidacy(msg, candidacy)
            }
            ConsensusNetMessage::CatchupRequest() => self.on_catchup_request(msg),
            ConsensusNetMessage::CatchupResponse(staking) => self.on_catchup_response(staking),
        }
    }

    /// Message received by follower.
    fn on_prepare(
        &mut self,
        msg: &SignedWithKey<ConsensusNetMessage>,
        consensus_proposal: &ConsensusProposal,
    ) -> Result<(), Error> {
        debug!("Received Prepare message: {}", consensus_proposal);
        // if slot is 0, we are in genesis and we accept any leader
        // unless we already have one set (e.g. in tests)
        if self.bft_round_state.slot == 0
            && self.bft_round_state.leader_pubkey == ValidatorPublicKey::default()
        {
            self.bft_round_state.leader_pubkey = consensus_proposal.validators[0].clone();
        }
        // Validate message comes from the correct leader
        if !msg.validators.contains(&self.leader_id()) {
            self.metrics.prepare_error("wrong_leader");
            self.buffered_invalid_proposals
                .insert(consensus_proposal.hash(), consensus_proposal.clone());
            bail!(
                "Prepare consensus message does not come from current leader. I won't vote for it."
            );
        }

        // Verify received previous *Commit* Quorum Certificate
        // If a previous *Commit* Quorum Certificate exists, verify hashes
        // If no previous *Commit* Quorum Certificate is found, verify this one and save it if legit
        match self
            .bft_round_state
            .commit_quorum_certificates
            .get(&(self.bft_round_state.slot.saturating_sub(1)))
        {
            Some((previous_consensus_proposal_hash, previous_commit_quorum_certificate)) => {
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
                            self.sign_net_message(ConsensusNetMessage::CatchupRequest())?,
                        ))
                        .context(
                            "Failed to send ConsensusNetMessage::CatchupRequest msg on the bus",
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
                                    consensus_proposal.previous_consensus_proposal_hash.clone(),
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
                .context("Failed to send ConsensusNetMessage::PrepareVote msg on the bus")?;
        } else {
            info!("😥 Not part of consensus, not sending PrepareVote");
        }

        self.metrics.prepare();

        Ok(())
    }

    /// Message received by leader.
    fn on_prepare_vote(
        &mut self,
        msg: &SignedWithKey<ConsensusNetMessage>,
        consensus_proposal_hash: &ConsensusProposalHash,
    ) -> Result<()> {
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
        self.store.bft_round_state.prepare_votes.insert(msg.clone());

        // Get matching vote count
        let validated_votes = self
            .bft_round_state
            .prepare_votes
            .iter()
            .flat_map(|signed_message| signed_message.validators.clone())
            .collect::<HashSet<ValidatorPublicKey>>();

        let votes_power =
            self.compute_voting_power(validated_votes.into_iter().collect::<Vec<_>>().as_slice());

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
        if voting_power > 2 * f {
            // Get all received signatures
            let aggregates: &Vec<&Signed<ConsensusNetMessage, ValidatorPublicKey>> =
                &self.bft_round_state.prepare_votes.iter().collect();

            // Aggregates them into a *Prepare* Quorum Certificate
            let prepvote_signed_aggregation = self.crypto.sign_aggregate(
                ConsensusNetMessage::PrepareVote(self.bft_round_state.consensus_proposal.hash()),
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
                .context("Failed to broadcast ConsensusNetMessage::Confirm msg on the bus")?;
        }
        // TODO(?): Update behaviour when having more ?
        // else if validated_votes > 2 * f + 1 {}
        Ok(())
    }

    /// Message received by follower.
    fn on_confirm(
        &mut self,
        consensus_proposal_hash: &ConsensusProposalHash,
        prepare_quorum_certificate: &QuorumCertificate,
    ) -> Result<()> {
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

                let voting_power =
                    self.compute_voting_power(prepare_quorum_certificate.validators.as_slice());

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

                self.verify_quorum_and_catchup(
                    consensus_proposal_hash,
                    prepare_quorum_certificate,
                )?;

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

    /// Message received by leader.
    fn on_confirm_ack(
        &mut self,
        msg: &SignedWithKey<ConsensusNetMessage>,
        consensus_proposal_hash: &ConsensusProposalHash,
    ) -> Result<()> {
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
        if !self.store.bft_round_state.confirm_ack.insert(msg.clone()) {
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
        if voting_power > 2 * f {
            // Get all signatures received and change ValidatorPublicKey for ValidatorPubKey
            let aggregates: &Vec<&Signed<ConsensusNetMessage, ValidatorPublicKey>> =
                &self.bft_round_state.confirm_ack.iter().collect();

            // Aggregates them into a *Commit* Quorum Certificate
            let commit_signed_aggregation = self.crypto.sign_aggregate(
                ConsensusNetMessage::ConfirmAck(self.bft_round_state.consensus_proposal.hash()),
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
                .context("Failed to broadcast ConsensusNetMessage::Confirm msg on the bus")?;

            // Finishes the bft round
            self.finish_round()?;
            // Start new slot
            if self.is_next_leader() {
                self.delay_start_new_slot()?;
            }
        }
        // TODO(?): Update behaviour when having more ?
        Ok(())
    }

    /// Message received by follower.
    fn on_commit(
        &mut self,
        consensus_proposal_hash: &ConsensusProposalHash,
        commit_quorum_certificate: &QuorumCertificate,
    ) -> Result<()> {
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
                let voting_power =
                    self.compute_voting_power(commit_quorum_certificate.validators.as_slice());
                let f = self.compute_f();
                if voting_power < 2 * f + 1 {
                    self.metrics.commit_error("incomplete_qc");
                    bail!("Commit Quorum Certificate does not contain enough validators signatures")
                }

                self.verify_quorum_and_catchup(consensus_proposal_hash, commit_quorum_certificate)?;

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
            self.delay_start_new_slot()
        } else if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            return Ok(());
        } else if self
            .bft_round_state
            .staking
            .get_stake(self.crypto.validator_pubkey())
            .is_some()
        {
            // Ask to be part of consensus
            let candidacy = ValidatorCandidacy {
                pubkey: self.crypto.validator_pubkey().clone(),
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
            return Ok(());
        } else {
            info!(
                "😥 No stake on pubkey '{}'. Not sending candidacy.",
                self.crypto.validator_pubkey()
            );
            return Ok(());
        }
    }

    /// Message received by leader & follower.
    fn on_validator_candidacy(
        &mut self,
        msg: &SignedWithKey<ConsensusNetMessage>,
        candidacy: &ValidatorCandidacy,
    ) -> Result<()> {
        info!("📝 Received candidacy message: {}", candidacy);

        debug!(
            "Current consensus proposal: {}",
            self.bft_round_state.consensus_proposal
        );

        // Verify that the validator is not already part of the consensus
        if self.is_part_of_consensus(&candidacy.pubkey) {
            debug!("Validator is already part of the consensus");
            return Ok(());
        }

        // Verify that the candidate has enough stake
        if let Some(stake) = self.bft_round_state.staking.get_stake(&candidacy.pubkey) {
            if stake.amount < staking::MIN_STAKE {
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

    fn on_catchup_request(&mut self, msg: &SignedWithKey<ConsensusNetMessage>) -> Result<()> {
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
            .context("Failed to send ConsensusNetMessage::CatchupResponse msg on the bus")?;
        Ok(())
    }

    fn on_catchup_response(&mut self, staking: &Staking) -> Result<()> {
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
            ConsensusCommand::StartNewSlot => self.start_new_slot(),
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

    fn handle_peer_event(&mut self, msg: PeerEvent) -> Result<()> {
        match msg {
            PeerEvent::NewPeer { pubkey } => {
                if self.bft_round_state.slot != 0 {
                    return Ok(());
                }
                if !self.is_next_leader() {
                    return Ok(());
                }
                info!("New peer added to genesis: {}", pubkey);
                self.genesis_pubkeys.push(pubkey.clone());
                if self.genesis_pubkeys.len() == 2 {
                    // Start first slot
                    debug!("Got a 2nd validator, starting first slot after delay");
                    return self.delay_start_new_slot();
                }
                Ok(())
            }
        }
    }

    pub fn start_master(&mut self, config: SharedConf) -> Result<()> {
        let interval = config.storage.interval;

        // hack to avoid another bus for a specific wip case
        let command_sender = Pick::<broadcast::Sender<ConsensusCommand>>::get(&self.bus).clone();
        if config.id == "single-node" {
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

    pub async fn start(&mut self) -> Result<()> {
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
            listen<SignedWithKey<ConsensusNetMessage>> cmd => {
                match self.handle_net_message(&cmd) {
                    Ok(_) => (),
                    Err(e) => warn!("Consensus message failed: {:#}", e),
                }
            }
            listen<PeerEvent> cmd => {
                match self.handle_peer_event(cmd) {
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling peer event: {:#}", e),
                }
            }
        }
    }

    fn sign_net_message(
        &self,
        msg: ConsensusNetMessage,
    ) -> Result<SignedWithKey<ConsensusNetMessage>> {
        debug!("🔏 Signing message: {:?}", msg);
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
    };
    use assertables::assert_contains;
    use staking::Stake;
    use tokio::sync::broadcast::Receiver;

    struct TestCtx {
        out_receiver: Receiver<OutboundMessage>,
        _event_receiver: Receiver<ConsensusEvent>,
        consensus: Consensus,
        name: String,
    }

    impl TestCtx {
        async fn new(name: &str, crypto: BlstCrypto) -> Self {
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let bus = ConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

            let store = ConsensusStore::default();
            let conf = Arc::new(Conf::default());
            let consensus = Consensus {
                metrics: ConsensusMetrics::global("id".to_string()),
                bus,
                genesis_pubkeys: vec![],
                file: None,
                store,
                config: conf,
                crypto: Arc::new(crypto),
            };

            Self {
                out_receiver,
                _event_receiver: event_receiver,
                consensus,
                name: name.to_string(),
            }
        }

        async fn build() -> (Self, Self) {
            let crypto = crypto::BlstCrypto::new("node-1".into());
            info!("node 1: {}", crypto.validator_pubkey());
            let c_other = crypto::BlstCrypto::new("node-2".into());
            info!("node 2: {}", c_other.validator_pubkey());
            let mut node1 = Self::new("node-1", crypto.clone()).await;
            node1.consensus.is_next_leader = true;
            node1.consensus.genesis_pubkeys = vec![
                crypto.validator_pubkey().clone(),
                c_other.validator_pubkey().clone(),
            ];

            let mut node2 = Self::new("node-2", c_other).await;
            node2.consensus.is_next_leader = false;
            node2.consensus.bft_round_state.leader_pubkey = crypto.validator_pubkey().clone();
            node2.consensus.bft_round_state.leader_index = 0;

            (node1, node2)
        }

        async fn new_node(name: &str, leader: &Self, leader_index: u64) -> Self {
            let crypto = crypto::BlstCrypto::new(name.into());
            let mut node = Self::new(name, crypto.clone()).await;

            node.consensus.bft_round_state.leader_pubkey = leader.pubkey();
            node.consensus.bft_round_state.leader_index = leader_index;

            node
        }

        fn pubkey(&self) -> ValidatorPublicKey {
            self.consensus.crypto.validator_pubkey().clone()
        }

        #[track_caller]
        fn handle_msg(&mut self, msg: &SignedWithKey<ConsensusNetMessage>, err: &str) {
            debug!("📥 {} Handling message: {:?}", self.name, msg);
            self.consensus.handle_net_message(msg).expect(err);
        }

        #[track_caller]
        fn handle_msg_err(&mut self, msg: &SignedWithKey<ConsensusNetMessage>) -> Error {
            debug!("📥 {} Handling message expecting err: {:?}", self.name, msg);
            let err = self.consensus.handle_net_message(msg).unwrap_err();
            info!("Expected error: {:#}", err);
            err
        }

        #[track_caller]
        fn add_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            debug!("➕ {} Add staker: {:?}", self.name, staker.name);
            self.consensus
                .handle_command(ConsensusCommand::NewStaker(Staker {
                    pubkey: staker.consensus.crypto.validator_pubkey().clone(),
                    stake: Stake { amount },
                }))
                .expect(err)
        }

        #[track_caller]
        fn with_stake(&mut self, amount: u64, err: &str) {
            self.consensus
                .handle_command(ConsensusCommand::NewStaker(Staker {
                    pubkey: self.consensus.crypto.validator_pubkey().clone(),
                    stake: Stake { amount },
                }))
                .expect(err);
        }

        fn start_new_slot(&mut self) {
            self.consensus
                .start_new_slot()
                .expect("Failed to start slot");
        }

        #[track_caller]
        fn assert_broadcast(
            &mut self,
            err: &str,
        ) -> Signed<ConsensusNetMessage, ValidatorPublicKey> {
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
            to: &Self,
            err: &str,
        ) -> Signed<ConsensusNetMessage, ValidatorPublicKey> {
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
                assert_eq!(to.pubkey(), dest);
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
        let (mut node1, mut node2) = TestCtx::build().await;

        node1.start_new_slot();
        // Slot 0 - leader = node1
        let leader_proposal = node1.assert_broadcast("Leader proposal");
        node2.handle_msg(&leader_proposal, "Leader proposal");

        let slave_vote = node2.assert_send(&node1, "Slave vote");
        node1.handle_msg(&slave_vote, "Slave vote");

        let leader_confirm = node1.assert_broadcast("Leader confirm");
        node2.handle_msg(&leader_confirm, "Leader confirm");

        let slave_confirm_ack = node2.assert_send(&node1, "Slave confirm ack");
        node1.handle_msg(&slave_confirm_ack, "Slave confirm ack");

        let leader_commit = node1.assert_broadcast("Leader commit");
        node2.handle_msg(&leader_commit, "Leader commit");

        // Slot 1 - leader = node2
        let leader_proposal = node2.assert_broadcast("Leader proposal");
        node1.handle_msg(&leader_proposal, "Leader proposal");

        let slave_vote = node1.assert_send(&node2, "Slave vote");
        node2.handle_msg(&slave_vote, "Slave vote");

        let leader_confirm = node2.assert_broadcast("Leader confirm");
        node1.handle_msg(&leader_confirm, "Leader confirm");

        let slave_confirm_ack = node1.assert_send(&node2, "Slave confirm ack");
        node2.handle_msg(&slave_confirm_ack, "Slave confirm ack");

        let leader_commit = node2.assert_broadcast("Leader commit");
        node1.handle_msg(&leader_commit, "Leader commit");

        // Slot 2 - leader = node1
        let leader_proposal = node1.assert_broadcast("Leader proposal");
        node2.handle_msg(&leader_proposal, "Leader proposal");

        let slave_vote = node2.assert_send(&node1, "Slave vote");
        node1.handle_msg(&slave_vote, "Slave vote");

        let leader_confirm = node1.assert_broadcast("Leader confirm");
        node2.handle_msg(&leader_confirm, "Leader confirm");

        let slave_confirm_ack = node2.assert_send(&node1, "Slave confirm ack");
        node1.handle_msg(&slave_confirm_ack, "Slave confirm ack");

        let leader_commit = node1.assert_broadcast("Leader commit");
        node2.handle_msg(&leader_commit, "Leader commit");

        assert_eq!(node1.consensus.blocks.len(), 3);
        assert_eq!(node2.consensus.blocks.len(), 3);
    }

    #[test_log::test(tokio::test)]
    async fn test_candidacy() {
        let (mut node1, mut node2) = TestCtx::build().await;

        // Slot 0
        {
            node1.start_new_slot();
            let leader_proposal = node1.assert_broadcast("Leader proposal");
            node2.handle_msg(&leader_proposal, "Leader proposal");
            let slave_vote = node2.assert_send(&node1, "Slave vote");
            node1.handle_msg(&slave_vote, "Slave vote");
            let leader_confirm = node1.assert_broadcast("Leader confirm");
            node2.handle_msg(&leader_confirm, "Leader confirm");
            let slave_confirm_ack = node2.assert_send(&node1, "Slave confirm ack");
            node1.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            let leader_commit = node1.assert_broadcast("Leader commit");
            node2.handle_msg(&leader_commit, "Leader commit");
        }

        let mut node3 = TestCtx::new_node("node-3", &node2, 1).await;
        // Slot 1: New slave catchup - leader = node2
        {
            info!("➡️  Leader proposal");
            let leader_proposal = node2.assert_broadcast("Leader proposal");
            node1.handle_msg(&leader_proposal, "Leader proposal");
            node3.handle_msg(&leader_proposal, "Leader proposal");
            info!("➡️  Slave 2 catchup");
            let slave2_catchup_req = node3.assert_send(&node2, "Slave2 catchup");
            node2.handle_msg(&slave2_catchup_req, "Slave2 catchup");
            let leader_catchup_resp = node2.assert_send(&node3, "Salve2 catchup");
            node3.handle_msg(&leader_catchup_resp, "Slave2 catchup");
            info!("➡️  Slave vote");
            let slave_vote = node1.assert_send(&node2, "Slave vote");
            node2.handle_msg(&slave_vote, "Slave vote");
            info!("➡️  Leader confirm");
            let leader_confirm = node2.assert_broadcast("Leader confirm");
            node1.handle_msg(&leader_confirm, "Leader confirm");
            node3.handle_msg(&leader_confirm, "Leader confirm");
            info!("➡️  Slave confirm ack");
            let slave_confirm_ack = node1.assert_send(&node2, "Slave confirm ack");
            node2.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            info!("➡️  Leader commit");
            let leader_commit = node2.assert_broadcast("Leader commit");
            node1.handle_msg(&leader_commit, "Leader commit");

            node3.with_stake(100, "Add stake");
            node3.handle_msg(&leader_commit, "Leader commit");
            info!("➡️  Slave 2 candidacy");
            let slave2_candidacy = node3.assert_broadcast("Slave 2 candidacy");
            assert_contains!(
                node2.handle_msg_err(&slave2_candidacy).to_string(),
                "validator is not staking"
            );
            node1.add_staker(&node3, 100, "Add staker");
            node1.handle_msg(&slave2_candidacy, "Slave 2 candidacy");
            node2.add_staker(&node3, 100, "Add staker");
            node2.handle_msg(&slave2_candidacy, "Slave 2 candidacy");
        }

        // Slot 2: Still a slot without slave 2 - leader = node 1
        {
            info!("➡️  Leader proposal");
            let leader_proposal = node1.assert_broadcast("Leader proposal");
            if let ConsensusNetMessage::Prepare(consensus_proposal) = &leader_proposal.msg {
                assert_eq!(consensus_proposal.validators.len(), 2);
            } else {
                panic!("Leader proposal is not a Prepare message");
            }
            node2.handle_msg(&leader_proposal, "Leader proposal");
            node3.handle_msg(&leader_proposal, "Leader proposal");
            info!("➡️  Slave vote");
            let slave_vote = node2.assert_send(&node1, "Slave vote");
            node1.handle_msg(&slave_vote, "Slave vote");
            info!("➡️  Leader confirm");
            let leader_confirm = node1.assert_broadcast("Leader confirm");
            node2.handle_msg(&leader_confirm, "Leader confirm");
            node3.handle_msg(&leader_confirm, "Leader confirm");
            info!("➡️  Slave confirm ack");
            let slave_confirm_ack = node2.assert_send(&node1, "Slave confirm ack");
            node1.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            info!("➡️  Leader commit");
            let leader_commit = node1.assert_broadcast("Leader commit");
            node2.handle_msg(&leader_commit, "Leader commit");
            node3.handle_msg(&leader_commit, "Leader commit");
            info!("➡️  Slave 2 candidacy");
            let slave2_candidacy = node3.assert_broadcast("Slave 2 candidacy");
            node1.handle_msg(&slave2_candidacy, "Slave 2 candidacy");
            node2.handle_msg(&slave2_candidacy, "Slave 2 candidacy");
        }

        // Slot 3: Slave 2 joined consensus, leader = node-2
        {
            info!("➡️  Leader proposal");
            let leader_proposal = node2.assert_broadcast("Leader proposal");
            if let ConsensusNetMessage::Prepare(consensus_proposal) = &leader_proposal.msg {
                assert_eq!(consensus_proposal.validators.len(), 3);
            } else {
                panic!("Leader proposal is not a Prepare message");
            }
            node1.handle_msg(&leader_proposal, "Leader proposal");
            node3.handle_msg(&leader_proposal, "Leader proposal");
            info!("➡️  Slave vote");
            let slave_vote = node1.assert_send(&node2, "Slave vote");
            let slave2_vote = node3.assert_send(&node2, "Slave vote");
            node2.handle_msg(&slave_vote, "Slave vote");
            node2.handle_msg(&slave2_vote, "Slave vote");
            info!("➡️  Leader confirm");
            let leader_confirm = node2.assert_broadcast("Leader confirm");
            node1.handle_msg(&leader_confirm, "Leader confirm");
            node3.handle_msg(&leader_confirm, "Leader confirm");
            info!("➡️  Slave confirm ack");
            let slave_confirm_ack = node1.assert_send(&node2, "Slave confirm ack");
            let slave2_confirm_ack = node3.assert_send(&node2, "Slave confirm ack");
            node2.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            node2.handle_msg(&slave2_confirm_ack, "Slave confirm ack");
            info!("➡️  Leader commit");
            let leader_commit = node2.assert_broadcast("Leader commit");
            node1.handle_msg(&leader_commit, "Leader commit");
            node3.handle_msg(&leader_commit, "Leader commit");
        }

        assert_eq!(node1.consensus.bft_round_state.slot, 4);
        assert_eq!(node2.consensus.bft_round_state.slot, 4);
        assert_eq!(node3.consensus.bft_round_state.slot, 4);
    }
}
