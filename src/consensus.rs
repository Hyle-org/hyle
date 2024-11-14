//! Handles all consensus logic up to block commitment.

use crate::utils::logger::LogMe;
#[cfg(not(test))]
use crate::utils::static_type_map::Pick;
use crate::{
    bus::{
        bus_client,
        command_response::{CmdRespClient, Query},
        BusMessage, SharedMessageBus,
    },
    data_availability::DataEvent,
    genesis::GenesisEvent,
    handle_messages,
    mempool::{storage::Cut, QueryNewCut},
    model::{get_current_timestamp, BlockHeight, Hashable, ValidatorPublicKey},
    p2p::{
        network::{OutboundMessage, PeerEvent, Signed, SignedByValidator},
        P2PCommand,
    },
    utils::{
        conf::SharedConf,
        crypto::{AggregateSignature, BlstCrypto, SharedBlstCrypto, ValidatorSignature},
        modules::Module,
    },
};
use anyhow::{anyhow, bail, Context, Error, Result};
use bincode::{Decode, Encode};
use metrics::ConsensusMetrics;
use serde::{Deserialize, Serialize};
use staking::{Staker, Staking, MIN_STAKE};
use std::time::Duration;
use std::{
    collections::{HashMap, HashSet},
    default::Default,
    path::PathBuf,
};
use tokio::time::interval;
#[cfg(not(test))]
use tokio::{sync::broadcast, time::sleep};
use tracing::{debug, error, info, warn};

use strum_macros::IntoStaticStr;

pub mod metrics;
pub mod module;
pub mod staking;
pub mod utils;

// -----------------------------
// ------ Consensus bus --------
// -----------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    TimeoutTick,
    NewStaker(Staker),
    NewBonded(ValidatorPublicKey),
    ProcessedBlock(BlockHeight),
    StartNewSlot,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusEvent {
    CommitCut {
        validators: Vec<ValidatorPublicKey>,
        new_bonded_validators: Vec<ValidatorPublicKey>,
        cut: Cut,
    },
}

#[derive(Clone)]
pub struct QueryConsensusInfo {}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ConsensusInfo {
    pub slot: Slot,
    pub view: View,
    pub round_leader: ValidatorPublicKey,
    pub validators: Vec<ValidatorPublicKey>,
}

impl BusMessage for ConsensusCommand {}
impl BusMessage for ConsensusEvent {}
impl BusMessage for ConsensusNetMessage {}

bus_client! {
struct ConsensusBusClient {
    sender(OutboundMessage),
    sender(ConsensusEvent),
    sender(ConsensusCommand),
    sender(P2PCommand),
    sender(Query<QueryNewCut, Cut>),
    receiver(ConsensusCommand),
    receiver(GenesisEvent),
    receiver(DataEvent),
    receiver(SignedByValidator<ConsensusNetMessage>),
    receiver(PeerEvent),
    receiver(Query<QueryConsensusInfo, ConsensusInfo>),
}
}

// -----------------------------
// --- Consensus data model ----
// -----------------------------

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ValidatorCandidacy {
    pubkey: ValidatorPublicKey,
    peer_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct NewValidatorCandidate {
    pubkey: ValidatorPublicKey, // TODO: possible optim: the pubkey is already present in the msg,
    msg: SignedByValidator<ConsensusNetMessage>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Default)]
pub struct QuorumCertificateHash(Vec<u8>);

type QuorumCertificate = AggregateSignature;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct TimeoutCertificate(ConsensusProposalHash, QuorumCertificate);

// A Ticket is necessary to send a valid prepare
#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub enum Ticket {
    // Special value for the initial Cut, needed because we don't have a quorum certificate for the genesis block.
    Genesis,
    CommitQC(QuorumCertificate),
    TimeoutQC(QuorumCertificate),
}

pub type Slot = u64;
pub type View = u64;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Default)]
pub struct ConsensusProposalHash(Vec<u8>);

#[derive(Debug, Clone, Default, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ConsensusProposal {
    // These first few items are checked when receiving the proposal from the leader.
    slot: Slot,
    view: View,
    round_leader: ValidatorPublicKey,
    // Below items aren't.
    cut: Cut,
    new_validators_to_bond: Vec<NewValidatorCandidate>,
}

type NextLeader = ValidatorPublicKey;

#[derive(
    Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, IntoStaticStr,
)]
pub enum ConsensusNetMessage {
    Prepare(ConsensusProposal, Ticket),
    PrepareVote(ConsensusProposalHash),
    Confirm(QuorumCertificate),
    ConfirmAck(ConsensusProposalHash),
    Commit(QuorumCertificate, ConsensusProposalHash),
    Timeout(ConsensusProposalHash, NextLeader),
    TimeoutCertificate(QuorumCertificate, ConsensusProposalHash),
    ValidatorCandidacy(ValidatorCandidacy),
}

#[derive(Debug, Encode, Decode, Default)]
enum TimeoutState {
    #[default]
    // Initial state
    Inactive,
    // A new slot was created, and its timeout is scheduled
    Scheduled {
        timestamp: u64,
    },
}

impl TimeoutState {
    pub const TIMEOUT_SECS: u64 = 5;
    pub fn schedule_next(&mut self, timestamp: u64) {
        match self {
            TimeoutState::Inactive => {
                info!("⏲️ Scheduling timeout");
            }
            TimeoutState::Scheduled { .. } => {
                info!("⏲️ Rescheduling timeout");
            }
        }
        *self = TimeoutState::Scheduled {
            timestamp: timestamp + TimeoutState::TIMEOUT_SECS,
        };
    }
    pub fn cancel(&mut self) {
        match self {
            TimeoutState::Scheduled { timestamp } => {
                info!("⏲️ Cancelling timeout set to trigger to {}", timestamp);
            }
            TimeoutState::Inactive => {
                info!("⏲️ Cancelling inactive timeout");
            }
        }
        *self = TimeoutState::Inactive;
    }
}

// TODO: move struct to model.rs ?
#[derive(Encode, Decode, Default)]
pub struct BFTRoundState {
    consensus_proposal: ConsensusProposal,
    staking: Staking,

    leader: LeaderState,
    follower: FollowerState,
    joining: JoiningState,
    genesis: GenesisState,
    state_tag: StateTag,
}

#[derive(Encode, Decode, Default, Debug)]
enum StateTag {
    #[default]
    Joining,
    Leader,
    Follower,
}

#[derive(Encode, Decode, Default, Debug)]
enum Step {
    #[default]
    StartNewSlot,
    PrepareVote,
    ConfirmAck,
}

#[derive(Encode, Decode, Default)]
pub struct LeaderState {
    step: Step,
    prepare_votes: HashSet<SignedByValidator<ConsensusNetMessage>>,
    confirm_ack: HashSet<SignedByValidator<ConsensusNetMessage>>,
    pending_ticket: Option<Ticket>,
}

#[derive(Encode, Decode, Default)]
pub struct FollowerState {
    timeout_requests: HashSet<SignedByValidator<ConsensusNetMessage>>,
    timeout_state: TimeoutState,
    buffered_quorum_certificate: Option<QuorumCertificate>, // if we receive a commit before the next prepare
}

#[derive(Encode, Decode, Default)]
pub struct JoiningState {
    staking_updated_to: Slot,
    buffered_prepares: Vec<ConsensusProposal>,
}

#[derive(Encode, Decode, Default)]
pub struct GenesisState {
    peer_pubkey: HashMap<String, ValidatorPublicKey>,
}

#[derive(Encode, Decode, Default)]
pub struct ConsensusStore {
    bft_round_state: BFTRoundState,
    /// Validators that asked to be part of consensus
    validator_candidates: Vec<NewValidatorCandidate>,
    last_cut: Cut,
}

pub struct Consensus {
    metrics: ConsensusMetrics,
    bus: ConsensusBusClient,
    file: Option<PathBuf>,
    store: ConsensusStore,
    #[allow(dead_code)]
    config: SharedConf,
    crypto: SharedBlstCrypto,
}

impl Consensus {
    fn next_leader(&self) -> Result<ValidatorPublicKey> {
        // Find out who the next leader will be.
        let leader_index = self
            .bft_round_state
            .staking
            .bonded()
            .iter()
            .position(|v| v == &self.bft_round_state.consensus_proposal.round_leader)
            .context(format!(
                "Leader {} not found in validators",
                &self.bft_round_state.consensus_proposal.round_leader,
            ))?;

        Ok(self
            .bft_round_state
            .staking
            .bonded()
            .get((leader_index + 1) % self.bft_round_state.staking.bonded().len())
            .context("No next leader found")?
            .clone())
    }

    /// Reset bft_round_state for the next round of consensus.
    fn finish_round(&mut self, ticket: Option<Ticket>) -> Result<(), Error> {
        match self.bft_round_state.state_tag {
            StateTag::Follower => {}
            StateTag::Leader => {}
            _ => bail!("Cannot finish_round unless synchronized to the consensus."),
        }

        let new_validators_to_bond = std::mem::take(
            &mut self
                .bft_round_state
                .consensus_proposal
                .new_validators_to_bond,
        );

        // Saving last cut
        self.last_cut = self.bft_round_state.consensus_proposal.cut.clone();

        // Reset round state, carrying over staking and current proposal.
        self.bft_round_state = BFTRoundState {
            consensus_proposal: ConsensusProposal {
                slot: self.bft_round_state.consensus_proposal.slot,
                view: self.bft_round_state.consensus_proposal.view,
                round_leader: self.next_leader()?,
                ..ConsensusProposal::default()
            },
            staking: std::mem::take(&mut self.bft_round_state.staking),
            ..BFTRoundState::default()
        };

        if self.bft_round_state.consensus_proposal.round_leader == *self.crypto.validator_pubkey() {
            self.bft_round_state.state_tag = StateTag::Leader;
        } else {
            self.bft_round_state.state_tag = StateTag::Follower;
        }

        // If we finish the round via a committed proposal, update some state
        match ticket {
            Some(Ticket::CommitQC(qc)) => {
                self.bft_round_state.consensus_proposal.slot += 1;
                self.bft_round_state.consensus_proposal.view = 0;
                self.bft_round_state.follower.buffered_quorum_certificate = Some(qc);
                // Any new validators are added to the consensus and removed from candidates.
                for new_v in new_validators_to_bond {
                    debug!("🎉 New validator bonded: {}", new_v.pubkey);
                    self.store
                        .bft_round_state
                        .staking
                        .bond(new_v.pubkey.clone())?;
                }
            }
            Some(Ticket::TimeoutQC(_)) => {
                self.bft_round_state.consensus_proposal.view += 1;
            }
            els => {
                bail!("Invalid ticket here {:?}", els);
            }
        }

        info!(
            "🥋 Ready for slot {}, view {}",
            self.bft_round_state.consensus_proposal.slot,
            self.bft_round_state.consensus_proposal.view
        );

        if self.is_round_leader() {
            info!("👑 I'm the new leader! 👑")
        } else {
            self.bft_round_state
                .follower
                .timeout_state
                .schedule_next(get_current_timestamp());
        }

        Ok(())
    }

    /// Verify that quorum certificate includes only validators that are part of the consensus
    fn verify_quorum_signers_part_of_consensus(
        &self,
        quorum_certificate: &QuorumCertificate,
    ) -> bool {
        quorum_certificate.validators.iter().all(|v| {
            self.bft_round_state
                .staking
                .bonded()
                .iter()
                .any(|v2| v2 == v)
        })
    }

    /// Verify that new validators have enough stake
    /// and have a valid signature so can be bonded.
    fn verify_new_validators_to_bond(&mut self, proposal: &ConsensusProposal) -> Result<()> {
        for new_validator in &proposal.new_validators_to_bond {
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
            if let ConsensusNetMessage::ValidatorCandidacy(ValidatorCandidacy {
                pubkey,
                peer_address,
            }) = &new_validator.msg.msg
            {
                if pubkey != &new_validator.pubkey {
                    debug!("Invalid candidacy message");
                    debug!("Got - Expected");
                    debug!("{} - {}", pubkey, new_validator.pubkey);

                    bail!("New bonded validator has an invalid candidacy message");
                }

                self.validator_candidates
                    .retain(|v| v.pubkey != new_validator.pubkey);
                self.bus.send(P2PCommand::ConnectTo {
                    peer: peer_address.clone(),
                })?;
            } else {
                bail!("New bonded validator forwarded signed message is not a candidacy message");
            }
        }
        Ok(())
    }

    fn is_part_of_consensus(&self, pubkey: &ValidatorPublicKey) -> bool {
        self.bft_round_state.staking.is_bonded(pubkey)
    }

    fn delay_start_new_round(&mut self, ticket: Ticket) -> Result<(), Error> {
        if !matches!(self.bft_round_state.state_tag, StateTag::Leader) {
            bail!(
                "Cannot delay start new round while in state {:?}",
                self.bft_round_state.state_tag
            );
        }
        self.bft_round_state.leader.pending_ticket = Some(ticket);
        #[cfg(not(test))]
        {
            let command_sender =
                Pick::<broadcast::Sender<ConsensusCommand>>::get(&self.bus).clone();
            let interval = self.config.consensus.slot_duration;
            tokio::task::Builder::new()
                .name("sleep-consensus")
                .spawn(async move {
                    info!(
                        "⏱️  Sleeping {} milliseconds before starting a new slot",
                        interval
                    );
                    sleep(Duration::from_millis(interval)).await;

                    _ = command_sender
                        .send(ConsensusCommand::StartNewSlot)
                        .log_error("Cannot send StartNewSlot message over channel");
                })?;
            Ok(())
        }
        #[cfg(test)]
        {
            Ok(())
        }
    }

    async fn start_round(&mut self) -> Result<(), Error> {
        if !matches!(self.bft_round_state.leader.step, Step::StartNewSlot) {
            bail!(
                "Cannot start a new slot while in step {:?}",
                self.bft_round_state.leader.step
            );
        }

        if !self.is_round_leader() {
            bail!("I'm not the leader for this slot");
        }

        let ticket = self
            .bft_round_state
            .leader
            .pending_ticket
            .take()
            .ok_or(anyhow!("No ticket available for this slot"))?;

        // TODO: keep candidates around?
        let mut new_validators_to_bond = std::mem::take(&mut self.validator_candidates);
        new_validators_to_bond.retain(|v| {
            self.bft_round_state
                .staking
                .get_stake(&v.pubkey)
                .map(|s| s.amount)
                .unwrap_or(0)
                > MIN_STAKE
                && !self.bft_round_state.staking.is_bonded(&v.pubkey)
        });

        info!(
            "🚀 Starting new slot {} with {} existing validators and {} candidates",
            self.bft_round_state.consensus_proposal.slot,
            self.bft_round_state.staking.bonded().len(),
            new_validators_to_bond.len()
        );

        // Creates ConsensusProposal
        // Query new cut to Mempool
        let validators = self.bft_round_state.staking.bonded().clone();
        match self.bus.request(QueryNewCut(validators)).await {
            Ok(cut) => {
                self.last_cut = cut;
            }
            Err(err) => {
                // In case of an error, we reuse the last cut to avoid being considered byzantine
                error!(
                    "Could not get a new cut from Mempool {:?}. Reusing previous one... {:?}",
                    err, self.last_cut
                );
            }
        };

        self.bft_round_state.leader.step = Step::PrepareVote;

        // Start Consensus with following cut
        self.bft_round_state.consensus_proposal.cut = self.last_cut.clone();
        self.bft_round_state
            .consensus_proposal
            .new_validators_to_bond = new_validators_to_bond;

        self.metrics.start_new_round("consensus_proposal");

        // Verifies that to-be-built block is large enough (?)

        // Broadcasts Prepare message to all validators
        debug!(
            proposal_hash = %self.bft_round_state.consensus_proposal.hash(),
            "🌐 Slot {} started. Broadcasting Prepare message", self.bft_round_state.consensus_proposal.slot,
        );
        self.broadcast_net_message(ConsensusNetMessage::Prepare(
            self.bft_round_state.consensus_proposal.clone(),
            ticket,
        ))?;

        Ok(())
    }

    fn is_round_leader(&self) -> bool {
        matches!(self.bft_round_state.state_tag, StateTag::Leader)
    }

    fn compute_f(&self) -> u64 {
        self.bft_round_state.staking.total_bond().div_ceil(3)
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

    /// Verify that:
    ///  - the quorum certificate is for the given message.
    ///  - the signatures are above 2f+1 voting power.
    ///
    /// This ensures that we can trust the message.
    fn verify_quorum_certificate<T: bincode::Encode>(
        &self,
        message: T,
        quorum_certificate: &QuorumCertificate,
    ) -> Result<()> {
        // Construct the expected signed message
        let expected_signed_message = Signed {
            msg: message,
            signature: AggregateSignature {
                signature: quorum_certificate.signature.clone(),
                validators: quorum_certificate.validators.clone(),
            },
        };

        match (
            BlstCrypto::verify_aggregate(&expected_signed_message),
            self.verify_quorum_signers_part_of_consensus(quorum_certificate),
        ) {
            (Ok(res), true) if !res => {
                //self.metrics.confirm_error("qc_invalid"); todo
                bail!("Quorum Certificate received is invalid")
            }
            (Err(err), _) => bail!("Quorum Certificate verification failed: {}", err),
            (_, false) => {
                //self.metrics.confirm_error("qc_invalid"); todo
                bail!("Quorum Certificate received contains non-consensus validators")
            }
            _ => {}
        };

        // This helpfully ignores any signatures that would not be actually part of the consensus
        // since those would have voting power 0.
        // TODO: should we reject such messages?
        let voting_power = self.compute_voting_power(quorum_certificate.validators.as_slice());

        let f = self.compute_f();

        info!(
            "📩 Slot {} validated votes: {} / {} ({} validators for a total bond = {})",
            self.bft_round_state.consensus_proposal.slot,
            voting_power,
            2 * f + 1,
            self.bft_round_state.staking.bonded().len(),
            self.bft_round_state.staking.total_bond()
        );

        // Verify enough validators signed
        if voting_power < 2 * f + 1 {
            self.metrics.confirm_error("prepare_qc_incomplete");
            bail!("Quorum Certificate does not contain enough voting power")
        }
        Ok(())
    }

    /// Connect to all validators & ask to be part of consensus
    fn send_candidacy(&mut self) -> Result<()> {
        let candidacy = ValidatorCandidacy {
            pubkey: self.crypto.validator_pubkey().clone(),
            peer_address: self.config.host.clone(),
        };
        info!(
            "📝 Sending candidacy message to be part of consensus.  {}",
            candidacy
        );
        // TODO: it would be more optimal to send this to the next leaders only.
        self.broadcast_net_message(ConsensusNetMessage::ValidatorCandidacy(candidacy))?;
        Ok(())
    }

    fn handle_net_message(
        &mut self,
        msg: SignedByValidator<ConsensusNetMessage>,
    ) -> Result<(), Error> {
        if !BlstCrypto::verify(&msg)? {
            self.metrics.signature_error("prepare");
            bail!("Invalid signature for message {:?}", &msg);
        }

        // TODO: reduce cloning here.
        let SignedByValidator::<ConsensusNetMessage> {
            msg: net_message,
            signature: ValidatorSignature {
                validator: sender, ..
            },
            ..
        } = msg.clone();

        match net_message {
            ConsensusNetMessage::Prepare(consensus_proposal, ticket) => {
                self.on_prepare(sender, consensus_proposal, ticket)
            }
            ConsensusNetMessage::PrepareVote(consensus_proposal_hash) => {
                self.on_prepare_vote(msg, consensus_proposal_hash)
            }
            ConsensusNetMessage::Confirm(prepare_quorum_certificate) => {
                self.on_confirm(prepare_quorum_certificate)
            }
            ConsensusNetMessage::ConfirmAck(consensus_proposal_hash) => {
                self.on_confirm_ack(msg, consensus_proposal_hash)
            }
            ConsensusNetMessage::Commit(commit_quorum_certificate, proposal_hash_hint) => {
                self.on_commit(commit_quorum_certificate, proposal_hash_hint)
            }
            ConsensusNetMessage::Timeout(consensus_proposal_hash, next_leader) => {
                self.on_timeout(msg, consensus_proposal_hash, next_leader)
            }
            ConsensusNetMessage::TimeoutCertificate(
                timeout_certificate,
                consensus_proposal_hash,
            ) => self.on_timeout_certificate(&consensus_proposal_hash, &timeout_certificate),

            ConsensusNetMessage::ValidatorCandidacy(candidacy) => {
                self.on_validator_candidacy(msg, candidacy)
            }
        }
    }

    fn verify_commit_ticket(&mut self, commit_qc: QuorumCertificate) -> bool {
        // Three options:
        // - we have already received the commit message for this ticket, so we already processed the QC.
        // - we haven't, so we process it right away
        // - the CQC is invalid and we just ignore it.
        if let Some(qc) = &self.bft_round_state.follower.buffered_quorum_certificate {
            return qc == &commit_qc;
        }

        self.try_commit_current_proposal(commit_qc.clone())
            .log_error("Processing Commit Ticket")
            .is_ok()
    }

    fn try_process_timeout_qc(&mut self, timeout_qc: QuorumCertificate) -> Result<()> {
        info!(
            "Trying to process timeout Certificate against consensus proposal slot: {}, view: {}",
            self.bft_round_state.consensus_proposal.slot,
            self.bft_round_state.consensus_proposal.view
        );

        self.verify_quorum_certificate(
            ConsensusNetMessage::Timeout(
                self.bft_round_state.consensus_proposal.hash(),
                self.next_leader()?,
            ),
            &timeout_qc,
        )
        .context("Verifying Timeout Ticket")?;

        self.carry_on_with_ticket(Ticket::TimeoutQC(timeout_qc.clone()))
    }

    /// Message received by follower after start_round
    fn on_prepare(
        &mut self,
        sender: ValidatorPublicKey,
        consensus_proposal: ConsensusProposal,
        ticket: Ticket,
    ) -> Result<(), Error> {
        debug!("Received Prepare message: {}", consensus_proposal);

        match self.bft_round_state.state_tag {
            StateTag::Joining => {
                // Ignore obviously outdated messages.
                // We'll be optimistic for ones in the future and hope that
                // maybe we'll have caught up by the time the commit rolls around.
                if consensus_proposal.slot <= self.bft_round_state.joining.staking_updated_to {
                    info!(
                        "🌑 Outdated Prepare message (Slot {} / view {} while at {}) received while joining. Ignoring.",
                        consensus_proposal.slot, consensus_proposal.view, self.bft_round_state.joining.staking_updated_to
                    );
                    return Ok(());
                }
                info!(
                    "🌕 Prepare message (Slot {} / view {}) received while joining. Storing.",
                    consensus_proposal.slot, consensus_proposal.view
                );
                // Store the message until we receive a matching Commit.
                // Because we may receive old or rogue proposals, we store all of them.
                // TODO: it would be slightly DOS-safer to only save those from validators we know,
                // but I'm not sure it's an actual problem in practice.
                self.bft_round_state
                    .joining
                    .buffered_prepares
                    .push(consensus_proposal);
                return Ok(());
            }
            StateTag::Follower | StateTag::Leader => {}
        }

        // Process the ticket
        match ticket {
            Ticket::Genesis => {
                if self.bft_round_state.consensus_proposal.slot != 1 {
                    bail!("Genesis ticket is only valid for the first slot.");
                }
            }
            Ticket::CommitQC(commit_qc) => {
                if !self.verify_commit_ticket(commit_qc) {
                    bail!("Invalid commit ticket");
                }
            }
            Ticket::TimeoutQC(timeout_qc) => {
                if self
                    .try_process_timeout_qc(timeout_qc)
                    .log_error("Processing Timeout ticket")
                    .is_err()
                {
                    bail!("Invalid timeout ticket");
                }
            }
        }

        // TODO: check we haven't voted for a proposal this slot/view already.

        // After processing the ticket, we should be in the right slot/view.

        if consensus_proposal.slot != self.bft_round_state.consensus_proposal.slot {
            self.metrics.prepare_error("wrong_slot");
            bail!("Prepare message received for wrong slot");
        }
        if consensus_proposal.view != self.bft_round_state.consensus_proposal.view {
            self.metrics.prepare_error("wrong_view");
            bail!("Prepare message received for wrong view");
        }

        // Validate message comes from the correct leader
        if sender != self.bft_round_state.consensus_proposal.round_leader {
            self.metrics.prepare_error("wrong_leader");
            bail!(
                "Prepare consensus message does not come from current leader. I won't vote for it."
            );
        }

        self.verify_new_validators_to_bond(&consensus_proposal)?;

        // At this point we are OK with this new consensus proposal, update locally and vote.
        self.bft_round_state.consensus_proposal = consensus_proposal.clone();

        // Responds PrepareVote message to leader with validator's vote on this proposal
        if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            info!(
                proposal_hash = %consensus_proposal.hash(),
                "📤 Slot {} Prepare message validated. Sending PrepareVote to leader",
                self.bft_round_state.consensus_proposal.slot
            );
            self.send_net_message(
                self.bft_round_state.consensus_proposal.round_leader.clone(),
                ConsensusNetMessage::PrepareVote(consensus_proposal.hash()),
            )?;
        } else {
            info!("😥 Not part of consensus, not sending PrepareVote");
        }

        self.metrics.prepare();

        Ok(())
    }

    /// Message received by leader.
    fn on_prepare_vote(
        &mut self,
        msg: SignedByValidator<ConsensusNetMessage>,
        consensus_proposal_hash: ConsensusProposalHash,
    ) -> Result<()> {
        if !matches!(self.bft_round_state.state_tag, StateTag::Leader) {
            bail!("PrepareVote received while not leader");
        }
        if !matches!(self.bft_round_state.leader.step, Step::PrepareVote) {
            debug!(
                "PrepareVote received at wrong step (step = {:?})",
                self.bft_round_state.leader.step
            );
            return Ok(());
        }

        // Verify that the PrepareVote is for the correct proposal.
        // This also checks slot/view as those are part of the hash.
        if consensus_proposal_hash != self.bft_round_state.consensus_proposal.hash() {
            self.metrics.prepare_vote_error("invalid_proposal_hash");
            bail!("PrepareVote has not received valid consensus proposal hash");
        }

        // Save vote message
        self.store.bft_round_state.leader.prepare_votes.insert(msg);

        // Get matching vote count
        let validated_votes = self
            .bft_round_state
            .leader
            .prepare_votes
            .iter()
            .map(|signed_message| signed_message.signature.validator.clone())
            .collect::<Vec<ValidatorPublicKey>>();

        let votes_power = self.compute_voting_power(&validated_votes);
        let voting_power = votes_power + self.get_own_voting_power();

        self.metrics.prepare_votes_gauge(voting_power);

        // Waits for at least n-f = 2f+1 matching PrepareVote messages
        let f = self.compute_f();

        info!(
            "📩 Slot {} validated votes: {} / {} ({} validators for a total bond = {})",
            self.bft_round_state.consensus_proposal.slot,
            voting_power,
            2 * f + 1,
            self.bft_round_state.staking.bonded().len(),
            self.bft_round_state.staking.total_bond()
        );

        if voting_power > 2 * f {
            // Get all received signatures
            let aggregates: &Vec<&SignedByValidator<ConsensusNetMessage>> =
                &self.bft_round_state.leader.prepare_votes.iter().collect();

            // Aggregates them into a *Prepare* Quorum Certificate
            let prepvote_signed_aggregation = self.crypto.sign_aggregate(
                ConsensusNetMessage::PrepareVote(self.bft_round_state.consensus_proposal.hash()),
                aggregates,
            )?;

            self.metrics.prepare_votes_aggregation();

            // Process the Confirm message locally, then send it to peers.
            self.bft_round_state.leader.step = Step::ConfirmAck;

            // if fast-path ... TODO
            // else send Confirm message to validators

            // Broadcast the *Prepare* Quorum Certificate to all validators
            debug!(
                "Slot {} PrepareVote message validated. Broadcasting Confirm",
                self.bft_round_state.consensus_proposal.slot
            );
            self.broadcast_net_message(ConsensusNetMessage::Confirm(
                prepvote_signed_aggregation.signature,
            ))?;
        }
        // TODO(?): Update behaviour when having more ?
        // else if validated_votes > 2 * f + 1 {}
        Ok(())
    }

    /// Message received by follower.
    fn on_confirm(&mut self, prepare_quorum_certificate: QuorumCertificate) -> Result<()> {
        match self.bft_round_state.state_tag {
            StateTag::Follower => {}
            StateTag::Joining => {
                return Ok(());
            }
            _ => bail!("Confirm message received while not follower"),
        }

        // Check that this is a QC for PrepareVote for the expected proposal.
        // This also checks slot/view as those are part of the hash.
        // TODO: would probably be good to make that more explicit.
        let consensus_proposal_hash = self.bft_round_state.consensus_proposal.hash();
        self.verify_quorum_certificate(
            ConsensusNetMessage::PrepareVote(consensus_proposal_hash.clone()),
            &prepare_quorum_certificate,
        )?;

        // Responds ConfirmAck to leader
        if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            info!(
                proposal_hash = %consensus_proposal_hash,
                "📤 Slot {} Confirm message validated. Sending ConfirmAck to leader",
                self.bft_round_state.consensus_proposal.slot
            );
            self.send_net_message(
                self.bft_round_state.consensus_proposal.round_leader.clone(),
                ConsensusNetMessage::ConfirmAck(consensus_proposal_hash.clone()),
            )?;
        } else {
            info!("😥 Not part of consensus, not sending ConfirmAck");
        }
        Ok(())
    }

    /// Message received by leader.
    fn on_confirm_ack(
        &mut self,
        msg: SignedByValidator<ConsensusNetMessage>,
        consensus_proposal_hash: ConsensusProposalHash,
    ) -> Result<()> {
        if !matches!(self.bft_round_state.state_tag, StateTag::Leader) {
            warn!("ConfirmAck received while not leader");
            return Ok(());
        }

        if !matches!(self.bft_round_state.leader.step, Step::ConfirmAck) {
            debug!(
                "ConfirmAck received at wrong step (step ={:?})",
                self.bft_round_state.leader.step
            );
            return Ok(());
        }

        // Verify that the ConfirmAck is for the correct proposal
        if consensus_proposal_hash != self.bft_round_state.consensus_proposal.hash() {
            self.metrics.confirm_ack_error("invalid_proposal_hash");
            debug!(
                "Got {} expected {}",
                consensus_proposal_hash,
                self.bft_round_state.consensus_proposal.hash()
            );
            bail!("ConfirmAck got invalid consensus proposal hash");
        }

        // Save ConfirmAck. Ends if the message already has been processed
        if !self.store.bft_round_state.leader.confirm_ack.insert(msg) {
            self.metrics.confirm_ack("already_processed");
            info!("ConfirmAck has already been processed");

            return Ok(());
        }

        // Compute voting power so far and hope for >= 2f+1
        let confirmed_ack_validators = self
            .bft_round_state
            .leader
            .confirm_ack
            .iter()
            .map(|signed_message| signed_message.signature.validator.clone())
            .collect::<Vec<ValidatorPublicKey>>();

        let confirmed_power = self.compute_voting_power(&confirmed_ack_validators);
        let voting_power = confirmed_power + self.get_own_voting_power();

        let f = self.compute_f();

        info!(
            "✅ Slot {} confirmed acks: {} / {} ({} validators for a total bond = {})",
            self.bft_round_state.consensus_proposal.slot,
            voting_power,
            2 * f + 1,
            self.bft_round_state.staking.bonded().len(),
            self.bft_round_state.staking.total_bond()
        );

        self.metrics.confirmed_ack_gauge(voting_power);

        if voting_power > 2 * f {
            // Get all signatures received and change ValidatorPublicKey for ValidatorPubKey
            let aggregates: &Vec<&SignedByValidator<ConsensusNetMessage>> =
                &self.bft_round_state.leader.confirm_ack.iter().collect();

            // Aggregates them into a *Commit* Quorum Certificate
            let commit_signed_aggregation = self.crypto.sign_aggregate(
                ConsensusNetMessage::ConfirmAck(self.bft_round_state.consensus_proposal.hash()),
                aggregates,
            )?;

            self.metrics.confirm_ack_commit_aggregate();

            // Buffers the *Commit* Quorum Cerficiate
            let commit_quorum_certificate = commit_signed_aggregation.signature;

            // Broadcast the *Commit* Quorum Certificate to all validators
            self.broadcast_net_message(ConsensusNetMessage::Commit(
                commit_quorum_certificate.clone(),
                consensus_proposal_hash,
            ))?;

            // Process the same locally.
            self.try_commit_current_proposal(commit_quorum_certificate)?;
        }
        // TODO(?): Update behaviour when having more ?
        Ok(())
    }

    /// Message received by follower.
    fn on_commit(
        &mut self,
        commit_quorum_certificate: QuorumCertificate,
        proposal_hash_hint: ConsensusProposalHash,
    ) -> Result<()> {
        match self.bft_round_state.state_tag {
            StateTag::Follower => self.try_commit_current_proposal(commit_quorum_certificate),
            StateTag::Joining => {
                self.on_commit_while_joining(commit_quorum_certificate, proposal_hash_hint)
            }
            _ => bail!("Commit message received while not follower"),
        }
    }

    fn on_commit_while_joining(
        &mut self,
        commit_quorum_certificate: QuorumCertificate,
        proposal_hash_hint: ConsensusProposalHash,
    ) -> Result<()> {
        // We are joining consensus, try to sync our state.
        // First find the prepare message to this commit.
        let Some(proposal_index) = self
            .bft_round_state
            .joining
            .buffered_prepares
            .iter()
            .position(|p| p.hash() == proposal_hash_hint)
        else {
            // Maybe we just missed it, carry on.
            return Ok(());
        };
        // Use that as our proposal.
        self.bft_round_state.consensus_proposal = self
            .bft_round_state
            .joining
            .buffered_prepares
            .swap_remove(proposal_index);
        self.bft_round_state.joining.buffered_prepares.clear();

        // At this point check that we're caught up enough that it's realistic to verify the QC.
        if self.bft_round_state.joining.staking_updated_to + 1
            < self.bft_round_state.consensus_proposal.slot
        {
            info!(
                "🏃Ignoring commit message, we are only caught up to {} ({} needed).",
                self.bft_round_state.joining.staking_updated_to,
                self.bft_round_state.consensus_proposal.slot - 1
            );
            return Ok(());
        }

        info!(
            "📦 Commit message received for slot {}, trying to synchronize.",
            self.bft_round_state.consensus_proposal.slot
        );

        // Try to commit the proposal
        self.bft_round_state.state_tag = StateTag::Follower;
        if self
            .try_commit_current_proposal(commit_quorum_certificate)
            .is_err()
        {
            self.bft_round_state.state_tag = StateTag::Joining;
            bail!("⛑️ Failed to synchronize, retrying soon.");
        }
        Ok(())
    }

    fn on_timeout(
        &mut self,
        received_msg: SignedByValidator<ConsensusNetMessage>,
        received_consensus_proposal_hash: ConsensusProposalHash,
        next_leader: ValidatorPublicKey,
    ) -> Result<()> {
        // Leader does not care about timeouts, his role is to rebroadcast messages to generate a commit
        if matches!(self.bft_round_state.state_tag, StateTag::Leader) {
            debug!("Leader does not process timeout messages");
            return Ok(());
        }

        // Only timeout if it is in consensus
        if !self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            info!(
                "Received timeout message while not being part of the consensus: {}",
                self.crypto.validator_pubkey()
            );
        }

        if received_consensus_proposal_hash != self.bft_round_state.consensus_proposal.hash() {
            bail!(
                "Consensus proposal (Slot: {}, view: {}) {} does not match {}",
                self.bft_round_state.consensus_proposal.slot,
                self.bft_round_state.consensus_proposal.view,
                self.bft_round_state.consensus_proposal.hash(),
                received_consensus_proposal_hash
            );
        }

        // Validates received next leader will be the same as the locally computed one
        if self.next_leader()? != next_leader {
            bail!(
                "Received next leader {} does not match the locally computed one {}",
                next_leader,
                self.next_leader()?
            )
        }

        // In the paper, a replica returns a commit if present
        // TODO ?

        // Insert timeout request and if already present notify
        if !self
            .store
            .bft_round_state
            .follower
            .timeout_requests
            .insert(received_msg.clone())
        {
            // self.metrics.timeout_request("already_processed");
            info!("Timeout has already been processed");
            return Ok(());
        }

        let f = self.compute_f();

        let timeout_validators = self
            .store
            .bft_round_state
            .follower
            .timeout_requests
            .iter()
            .map(|signed_message| signed_message.signature.validator.clone())
            .collect::<Vec<ValidatorPublicKey>>();

        let mut len = timeout_validators.len();
        let mut voting_power = self.compute_voting_power(&timeout_validators);

        info!("Got {voting_power} voting power with {len} timeout requests for the same view {}. f is {f}", self.store.bft_round_state.consensus_proposal.view);

        // Count requests and if f+1 requests, and not already part of it, join the mutiny
        if voting_power > f && !timeout_validators.contains(self.crypto.validator_pubkey()) {
            info!("Joining timeout mutiny!");

            // Broadcast a timeout message
            self.broadcast_net_message(ConsensusNetMessage::Timeout(
                received_consensus_proposal_hash.clone(),
                next_leader.clone(),
            ))
            .context(format!(
                "Sending timeout message for slot:{} view:{}",
                self.bft_round_state.consensus_proposal.slot,
                self.bft_round_state.consensus_proposal.view,
            ))?;

            len += 1;
            voting_power += self.get_own_voting_power();

            self.bft_round_state
                .follower
                .timeout_state
                .schedule_next(get_current_timestamp());
        }

        // Create TC if applicable
        if voting_power > 2 * f {
            debug!("⏲️ ⏲️ Creating a timeout certificate with {len} timeout requests and {voting_power} voting power");
            // Get all signatures received and change ValidatorId for ValidatorPubKey
            let aggregates: &Vec<&SignedByValidator<ConsensusNetMessage>> = &self
                .bft_round_state
                .follower
                .timeout_requests
                .iter()
                .collect();

            // Aggregates them into a Timeout Certificate
            let timeout_signed_aggregation = self.crypto.sign_aggregate(
                ConsensusNetMessage::Timeout(received_consensus_proposal_hash.clone(), next_leader),
                aggregates.as_slice(),
            )?;

            // self.metrics.timeout_certificate_aggregate();

            let timeout_certificate = timeout_signed_aggregation.signature;

            // Broadcast the Timeout Certificate to all validators
            self.broadcast_net_message(ConsensusNetMessage::TimeoutCertificate(
                timeout_certificate.clone(),
                received_consensus_proposal_hash.clone(),
            ))?;

            self.bft_round_state
                .follower
                .timeout_state
                .schedule_next(get_current_timestamp());

            if &self.next_leader()? == self.crypto.validator_pubkey() {
                self.carry_on_with_ticket(Ticket::TimeoutQC(timeout_certificate))?;
            }
        }

        Ok(())
    }

    fn on_timeout_certificate(
        &mut self,
        received_consensus_proposal_hash: &ConsensusProposalHash,
        received_timeout_certificate: &AggregateSignature,
    ) -> Result<()> {
        if *received_consensus_proposal_hash != self.bft_round_state.consensus_proposal.hash() {
            bail!(
                "Wrong consensus proposal (CP hash: {}, view: {})",
                received_consensus_proposal_hash,
                self.bft_round_state.consensus_proposal.view
            );
        }

        if &self.next_leader()? != self.crypto.validator_pubkey() {
            return Ok(());
        }

        info!(
            "Process quorum certificate {:?}",
            received_timeout_certificate
        );

        self.verify_quorum_certificate(
            ConsensusNetMessage::Timeout(
                received_consensus_proposal_hash.clone(),
                self.next_leader()?,
            ),
            received_timeout_certificate,
        )
        .context(format!(
            "Verifying timeout certificate for (slot: {}, view: {})",
            self.bft_round_state.consensus_proposal.slot,
            self.bft_round_state.consensus_proposal.view
        ))?;

        self.carry_on_with_ticket(Ticket::TimeoutQC(received_timeout_certificate.clone()))
    }

    fn carry_on_with_ticket(&mut self, ticket: Ticket) -> Result<()> {
        self.finish_round(Some(ticket.clone()))?;

        if self.is_round_leader() {
            // Setup our ticket for the next round
            // Send Prepare message to all validators
            self.delay_start_new_round(ticket)
        } else if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            Ok(())
        } else if self
            .bft_round_state
            .staking
            .get_stake(self.crypto.validator_pubkey())
            .map(|s| s.amount)
            .unwrap_or(0)
            > MIN_STAKE
        {
            self.send_candidacy()
        } else {
            info!(
                "😥 No stake on pubkey '{}'. Not sending candidacy.",
                self.crypto.validator_pubkey()
            );
            Ok(())
        }
    }

    fn try_commit_current_proposal(
        &mut self,
        commit_quorum_certificate: QuorumCertificate,
    ) -> Result<()> {
        // Check that this is a QC for ConfirmAck for the expected proposal.
        // This also checks slot/view as those are part of the hash.
        // TODO: would probably be good to make that more explicit.
        self.verify_quorum_certificate(
            ConsensusNetMessage::ConfirmAck(self.bft_round_state.consensus_proposal.hash()),
            &commit_quorum_certificate,
        )?;

        self.metrics.commit();

        _ = self
            .bus
            .send(ConsensusEvent::CommitCut {
                // TODO: investigate if those are necessary here
                validators: self.bft_round_state.staking.bonded().clone(),
                cut: self.bft_round_state.consensus_proposal.cut.clone(),
                new_bonded_validators: self
                    .bft_round_state
                    .consensus_proposal
                    .new_validators_to_bond
                    .iter()
                    .map(|v| v.pubkey.clone())
                    .collect(),
            })
            .expect("Failed to send ConsensusEvent::CommitCut on the bus");

        // Save added cut TODO: remove ? (data availability)
        if let Some(file) = &self.file {
            if let Err(e) = Self::save_on_disk(
                self.config.data_directory.as_path(),
                file.as_path(),
                &self.store,
            ) {
                warn!("Failed to save consensus state on disk: {}", e);
            }
        }

        info!(
            "📈 Slot {} committed",
            &self.bft_round_state.consensus_proposal.slot
        );

        self.carry_on_with_ticket(Ticket::CommitQC(commit_quorum_certificate.clone()))
    }

    /// Message received by leader & follower.
    fn on_validator_candidacy(
        &mut self,
        msg: SignedByValidator<ConsensusNetMessage>,
        candidacy: ValidatorCandidacy,
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

        if self.bft_round_state.staking.is_bonded(&candidacy.pubkey) {
            debug!("Validator is already bonded. Ignoring candidacy");
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
        self.validator_candidates.push(NewValidatorCandidate {
            pubkey: candidacy.pubkey.clone(),
            msg,
        });
        Ok(())
    }

    async fn handle_command(&mut self, msg: ConsensusCommand) -> Result<()> {
        match msg {
            ConsensusCommand::TimeoutTick => match &self.bft_round_state.follower.timeout_state {
                TimeoutState::Scheduled { timestamp } if get_current_timestamp() >= *timestamp => {
                    // Trigger state transition to mutiny
                    info!(
                        "⏰ Trigger timeout for slot {} and view {}",
                        self.bft_round_state.consensus_proposal.slot,
                        self.bft_round_state.consensus_proposal.view
                    );
                    let timeout_message = ConsensusNetMessage::Timeout(
                        self.bft_round_state.consensus_proposal.hash(),
                        self.next_leader()?,
                    );

                    let signed_timeout_message = self
                        .sign_net_message(timeout_message.clone())
                        .context("Signing timeout message")?;

                    self.bft_round_state
                        .follower
                        .timeout_requests
                        .insert(signed_timeout_message);

                    self.broadcast_net_message(timeout_message)?;

                    self.bft_round_state.follower.timeout_state.cancel();

                    Ok(())
                }
                _ => Ok(()),
            },
            ConsensusCommand::NewStaker(staker) => {
                self.store.bft_round_state.staking.add_staker(staker)?;
                Ok(())
            }
            ConsensusCommand::NewBonded(validator) => {
                self.store.bft_round_state.staking.bond(validator)?;
                Ok(())
            }
            ConsensusCommand::ProcessedBlock(block_height) => {
                if let StateTag::Joining = self.bft_round_state.state_tag {
                    if self.store.bft_round_state.joining.staking_updated_to < block_height.0 {
                        info!("🚪 Processed block {}", block_height.0);
                        self.store.bft_round_state.joining.staking_updated_to = block_height.0;
                    }
                }
                Ok(())
            }
            ConsensusCommand::StartNewSlot => {
                self.start_round().await?;
                Ok(())
            }
        }
    }

    #[inline(always)]
    fn broadcast_net_message(&mut self, net_message: ConsensusNetMessage) -> Result<()> {
        let signed_msg = self.sign_net_message(net_message)?;
        let enum_variant_name: &'static str = (&signed_msg.msg).into();
        _ = self
            .bus
            .send(OutboundMessage::broadcast(signed_msg))
            .context(format!(
                "Failed to broadcast {} msg on the bus",
                enum_variant_name
            ))?;
        Ok(())
    }

    #[inline(always)]
    fn send_net_message(
        &mut self,
        to: ValidatorPublicKey,
        net_message: ConsensusNetMessage,
    ) -> Result<()> {
        let signed_msg = self.sign_net_message(net_message)?;
        let enum_variant_name: &'static str = (&signed_msg.msg).into();
        _ = self
            .bus
            .send(OutboundMessage::send(to, signed_msg))
            .context(format!(
                "Failed to send {} msg on the bus",
                enum_variant_name
            ))?;
        Ok(())
    }

    async fn wait_genesis(&mut self) -> Result<()> {
        handle_messages! {
            on_bus self.bus,
            listen<GenesisEvent> msg => {
                match msg {
                    GenesisEvent::GenesisBlock { initial_validators, ..} => {
                        self.bft_round_state.consensus_proposal.round_leader =
                            initial_validators.first().unwrap().clone();

                        if self.bft_round_state.consensus_proposal.round_leader == *self.crypto.validator_pubkey() {
                            self.bft_round_state.state_tag = StateTag::Leader;
                            self.bft_round_state.consensus_proposal.slot = 1;
                            info!("👑 Starting consensus as leader");
                        } else {
                            self.bft_round_state.state_tag = StateTag::Follower;
                            self.bft_round_state.consensus_proposal.slot = 1;
                            info!(
                                "👑 Starting consensus as follower of leader {}",
                                self.bft_round_state.consensus_proposal.round_leader
                            );
                        }
                        break;
                    },
                    GenesisEvent::NoGenesis => {
                        // We are in state Joining by default, DA will fetch blocks and we will move to Follower
                        break;
                    },
                }
            }
        }

        if self.is_round_leader() {
            self.delay_start_new_round(Ticket::Genesis)?;
        }
        self.start().await
    }

    async fn start(&mut self) -> Result<()> {
        info!("🚀 Starting consensus");

        let mut timeout_ticker = interval(Duration::from_millis(100));

        handle_messages! {
            on_bus self.bus,
            listen<ConsensusCommand> cmd => {
                match self.handle_command(cmd).await {
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling consensus command: {:#}", e),
                }
            }
            listen<SignedByValidator<ConsensusNetMessage>> cmd => {
                match self.handle_net_message(cmd) {
                    Ok(_) => (),
                    Err(e) => warn!("Consensus message failed: {:#}", e),
                }
            }
            command_response<QueryConsensusInfo, ConsensusInfo> _ => {
                let slot = self.bft_round_state.consensus_proposal.slot;
                let view = self.bft_round_state.consensus_proposal.view;
                let round_leader = self.bft_round_state.consensus_proposal.round_leader.clone();
                let validators = self.bft_round_state.staking.bonded().clone();
                Ok(ConsensusInfo { slot, view, round_leader, validators })
            }
            _ = timeout_ticker.tick() => {
                self.bus.send(ConsensusCommand::TimeoutTick)
                    .log_error("Cannot send message over channel")?;
            }
        }
    }

    fn sign_net_message(
        &self,
        msg: ConsensusNetMessage,
    ) -> Result<SignedByValidator<ConsensusNetMessage>> {
        debug!("🔏 Signing message: {}", msg);
        self.crypto.sign(msg)
    }
}

#[cfg(test)]
mod test {
    use std::sync::Arc;

    use super::*;
    use crate::{
        bus::SharedMessageBus,
        p2p::network::NetMessage,
        utils::{conf::Conf, crypto},
    };
    use assertables::assert_contains;
    use staking::Stake;
    use tokio::sync::broadcast::Receiver;

    struct TestCtx {
        out_receiver: Receiver<OutboundMessage>,
        _event_receiver: Receiver<ConsensusEvent>,
        _p2p_receiver: Receiver<P2PCommand>,
        pub consensus: Consensus,
        name: String,
    }

    bus_client!(
        struct TestBusClient {
            receiver(Query<QueryNewCut, Cut>),
        }
    );
    macro_rules! build_tuple {
        ($nodes:expr, 1) => {
            ($nodes)
        };
        ($nodes:expr, 2) => {
            ($nodes, $nodes)
        };
        ($nodes:expr, 3) => {
            ($nodes, $nodes, $nodes)
        };
        ($nodes:expr, 4) => {
            ($nodes, $nodes, $nodes, $nodes)
        };
        ($nodes:expr, 5) => {
            ($nodes, $nodes, $nodes, $nodes, $nodes)
        };
        ($nodes:expr, 6) => {
            ($nodes, $nodes, $nodes, $nodes, $nodes, $nodes)
        };
        ($nodes:expr, 7) => {
            ($nodes, $nodes, $nodes, $nodes, $nodes, $nodes, $nodes)
        };
        ($nodes:expr, $count:expr) => {
            panic!("Le nombre de nœuds {} n'est pas supporté", $count)
        };
    }
    macro_rules! build_nodes {
        ($count:tt) => {{
            async {
                let cryptos: Vec<BlstCrypto> = (0..$count)
                    .map(|i| {
                        let crypto = crypto::BlstCrypto::new(format!("node-{i}").into());
                        info!("node {}: {}", i, crypto.validator_pubkey());
                        crypto
                    })
                    .collect();

                let mut nodes = vec![];

                for i in 0..$count {
                    let mut node = TestCtx::new(
                        format!("node-{i}").as_ref(),
                        cryptos.get(i).unwrap().clone(),
                    )
                    .await;

                    for other_crypto in cryptos.iter() {
                        node.add_trusted_validator(other_crypto.validator_pubkey());
                    }

                    node.consensus.bft_round_state.consensus_proposal.slot = 1;

                    if i == 0 {
                        node.consensus.bft_round_state.state_tag = StateTag::Leader;
                        node.consensus.bft_round_state.leader.pending_ticket =
                            Some(Ticket::Genesis);
                    } else {
                        node.consensus.bft_round_state.state_tag = StateTag::Follower;
                    }

                    node.consensus
                        .bft_round_state
                        .consensus_proposal
                        .round_leader = cryptos.get(0).unwrap().validator_pubkey().clone();

                    nodes.push(node);
                }

                // Appel à une macro récursive pour construire le tuple
                build_tuple!(nodes.remove(0), $count)
            }
        }};
    }

    macro_rules! broadcast {
        (description: $description:literal, from: $sender:ident, to: [$($node:ident),+]$(, message_matches: $pattern:pat $(=> $asserts:block)? )?) => {
            {
                // Construct the broadcast message with sender information
                let message = $sender.assert_broadcast(format!("[broadcast from: {}] {}", stringify!($sender), $description).as_str());

                $({
                    let msg_variant_name: &'static str = message.msg.clone().into();
                    if let $pattern = &message.msg {
                        $($asserts)?
                    } else {
                        panic!("[broadcast from: {}] {}: Message {} did not match {}", stringify!($sender), $description, msg_variant_name, stringify!($pattern));
                    }
                })?

                // Distribute the message to each specified node
                $(
                    $node.handle_msg(&message, (format!("[handling broadcast message from: {} at: {}] {}", stringify!($sender), stringify!($node), $description).as_str()));
                )+

                message
            }
        };
    }
    macro_rules! send {
    (
        description: $description:literal,
        from: [$($node:ident),+],
        to: $to:ident,
        message_matches: $pattern:pat
    ) => {
        // Distribute the message to the target node from all specified nodes
        $(
            let answer = $node.assert_send(
                &$to,
                format!("[send from: {} to: {}] {}", stringify!($node), stringify!($to), $description).as_str()
            );

            // If `message_matches` is provided, perform the pattern match
            if let $pattern = &answer.msg {
                // Execute optional assertions if provided
            } else {
                let msg_variant_name: &'static str = answer.msg.clone().into();
                panic!(
                    "[send from: {}] {}: Message {} did not match {}",
                    stringify!($node),
                    $description,
                    msg_variant_name,
                    stringify!($pattern)
                );
            }

            // Handle the message
            $to.handle_msg(
                &answer,
                format!("[handling sent message from: {} to: {}] {}", stringify!($node), stringify!($to), $description).as_str()
            );
        )+
    };

    (
        description: $description:literal,
        from: [$($node:ident: $pattern:pat $(=> $asserts:block)?)+],
        to: $to:ident
    ) => {
        // Distribute the message to the target node from all specified nodes
        $(
            let answer = $node.assert_send(
                &$to,
                format!("[send from: {} to: {}] {}", stringify!($node), stringify!($to), $description).as_str()
            );

            // If `message_matches` is provided, perform the pattern match
            if let $pattern = &answer.msg {
                // Execute optional assertions if provided
                $($asserts)?
            } else {
                let msg_variant_name: &'static str = answer.msg.clone().into();
                panic!(
                    "[send from: {}] {}: Message {} did not match {}",
                    stringify!($node),
                    $description,
                    msg_variant_name,
                    stringify!($pattern)
                );
            }

            // Handle the message
            $to.handle_msg(
                &answer,
                format!("[handling sent message from: {} to: {}] {}", stringify!($node), stringify!($to), $description).as_str()
            );
        )+
    };
}

    macro_rules! timeout {
        ($($node:ident),+) => {
            // Make all nodes timeout (schedule timeout to now + tick timeout to trigger it)
            $(
                $node.consensus
                    .bft_round_state
                    .follower
                    .timeout_state
                    .schedule_next(get_current_timestamp() - 10);
                $node.consensus
                    .handle_command(ConsensusCommand::TimeoutTick)
                    .await
                    .expect(format!("Timeout tick for node {}", stringify!($node)).as_str());

            )+
        }
    }

    impl TestCtx {
        async fn new(name: &str, crypto: BlstCrypto) -> Self {
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let p2p_receiver = get_receiver::<P2PCommand>(&shared_bus).await;
            let bus = ConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

            let mut new_cut_query_receiver = TestBusClient::new_from_bus(shared_bus).await;
            tokio::spawn(async move {
                handle_messages! {
                    on_bus new_cut_query_receiver,
                    command_response<QueryNewCut, Cut> _ => {
                        Ok(Cut::default())
                    }
                }
            });

            let store = ConsensusStore::default();
            let conf = Arc::new(Conf::default());
            let consensus = Consensus {
                metrics: ConsensusMetrics::global("id".to_string()),
                bus,
                file: None,
                store,
                config: conf,
                crypto: Arc::new(crypto),
            };

            Self {
                out_receiver,
                _event_receiver: event_receiver,
                _p2p_receiver: p2p_receiver,
                consensus,
                name: name.to_string(),
            }
        }

        pub fn add_trusted_validator(&mut self, pubkey: &ValidatorPublicKey) {
            self.consensus
                .bft_round_state
                .staking
                .add_staker(Staker {
                    pubkey: pubkey.clone(),
                    stake: Stake { amount: 100 },
                })
                .expect("cannot add trusted staker");
            self.consensus
                .bft_round_state
                .staking
                .bond(pubkey.clone())
                .expect("cannot bond trusted validator");
            info!("🎉 Trusted validator added: {}", pubkey);
        }

        async fn new_node(name: &str) -> Self {
            let crypto = crypto::BlstCrypto::new(name.into());
            Self::new(name, crypto.clone()).await
        }

        fn pubkey(&self) -> ValidatorPublicKey {
            self.consensus.crypto.validator_pubkey().clone()
        }

        #[track_caller]
        fn handle_msg(&mut self, msg: &SignedByValidator<ConsensusNetMessage>, err: &str) {
            debug!("📥 {} Handling message: {:?}", self.name, msg);
            self.consensus.handle_net_message(msg.clone()).expect(err);
        }

        #[track_caller]
        fn handle_msg_err(&mut self, msg: &SignedByValidator<ConsensusNetMessage>) -> Error {
            debug!("📥 {} Handling message expecting err: {:?}", self.name, msg);
            let err = self.consensus.handle_net_message(msg.clone()).unwrap_err();
            info!("Expected error: {:#}", err);
            err
        }

        #[cfg(test)]
        #[track_caller]
        fn handle_block(&mut self, msg: &SignedByValidator<ConsensusNetMessage>) {
            match &msg.msg {
                ConsensusNetMessage::Prepare(_, _) => {}
                _ => panic!("Block message is not a Prepare message"),
            }
        }

        async fn add_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            info!("➕ {} Add staker: {:?}", self.name, staker.name);
            self.consensus
                .handle_command(ConsensusCommand::NewStaker(Staker {
                    pubkey: staker.consensus.crypto.validator_pubkey().clone(),
                    stake: Stake { amount },
                }))
                .await
                .expect(err)
        }

        async fn add_bonded_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            self.add_staker(staker, amount, err).await;
            self.consensus
                .handle_command(ConsensusCommand::NewBonded(staker.pubkey()))
                .await
                .expect(err);
        }

        async fn with_stake(&mut self, amount: u64, err: &str) {
            self.consensus
                .handle_command(ConsensusCommand::NewStaker(Staker {
                    pubkey: self.consensus.crypto.validator_pubkey().clone(),
                    stake: Stake { amount },
                }))
                .await
                .expect(err);
        }

        async fn start_round(&mut self) {
            self.consensus
                .start_round()
                .await
                .expect("Failed to start slot");
        }

        #[track_caller]
        fn assert_broadcast(
            &mut self,
            description: &str,
        ) -> SignedByValidator<ConsensusNetMessage> {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .out_receiver
                .try_recv()
                .expect(format!("{description}: No message broadcasted").as_str());

            if let OutboundMessage::BroadcastMessage(net_msg) = rec {
                if let NetMessage::ConsensusMessage(msg) = net_msg {
                    msg
                } else {
                    panic!("{description}: NetMessage::ConsensusMessage message is missing");
                }
            } else {
                panic!("{description}: OutboundMessage::BroadcastMessage message is missing");
            }
        }

        #[track_caller]
        fn assert_send(
            &mut self,
            to: &Self,
            description: &str,
        ) -> SignedByValidator<ConsensusNetMessage> {
            #[allow(clippy::expect_fun_call)]
            let rec = self
                .out_receiver
                .try_recv()
                .expect(format!("{description}: No message sent").as_str());
            if let OutboundMessage::SendMessage {
                validator_id: dest,
                msg: net_msg,
            } = rec
            {
                assert_eq!(to.pubkey(), dest);
                if let NetMessage::ConsensusMessage(msg) = net_msg {
                    msg
                } else {
                    panic!("{description}: NetMessage::ConsensusMessage message is missing");
                }
            } else {
                panic!("{description}: OutboundMessage::Send message is missing");
            }
        }
    }
    #[test_log::test(tokio::test)]
    async fn test_happy_path() {
        let (mut node1, mut node2): (TestCtx, TestCtx) = build_nodes!(2).await;

        node1.start_round().await;
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
        node2.start_round().await;
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
        node1.start_round().await;
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

    #[test_log::test(tokio::test)]
    async fn basic_commit_4() {
        let (mut node1, mut node2, mut node3, mut node4): (TestCtx, TestCtx, TestCtx, TestCtx) =
            build_nodes!(4).await;

        node1.start_round().await;
        // Slot 1 - leader = node1
        // Ensuring one slot commits correctly before a timeout
        let cp_round;
        broadcast! {
            description: "Leader - Prepare",
            from: node1, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Prepare(cp, ticket) => {
                cp_round = Some(cp.clone());
                assert_eq!(cp.slot, 1);
                assert_eq!(cp.view, 0);
                assert_eq!(ticket, &Ticket::Genesis);
            }
        };

        let cp_round_hash = &cp_round.unwrap().hash();

        send! {
            description: "Follower - PrepareVote",
            from: [
                node2: ConsensusNetMessage::PrepareVote(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); }
                node3: ConsensusNetMessage::PrepareVote(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); }
                node4: ConsensusNetMessage::PrepareVote(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); }
            ], to: node1
        };

        broadcast! {
            description: "Leader - Confirm",
            from: node1, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Confirm(_)
        };

        send! {
            description: "Follower - Confirm Ack",
            from: [
                node2: ConsensusNetMessage::ConfirmAck(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); }
                node3: ConsensusNetMessage::ConfirmAck(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); }
                node4: ConsensusNetMessage::ConfirmAck(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); }
            ], to: node1
        };

        broadcast! {
            description: "Leader - Commit",
            from: node1, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Commit(_, cp_hash) => {
                assert_eq!(cp_hash, cp_round_hash);
            }
        };
    }

    #[test_log::test(tokio::test)]
    async fn timeout_only_one_4() {
        let (mut node1, mut node2, mut node3, mut node4): (TestCtx, TestCtx, TestCtx, TestCtx) =
            build_nodes!(4).await;

        node1.start_round();

        timeout!(node2);

        node2.assert_broadcast("Timeout message");

        // Slot 1 - leader = node1
        // Ensuring one slot commits correctly before a timeout
        broadcast! {
            description: "Leader - Prepare",
            from: node1, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Prepare(_, _)
        };

        send! {
            description: "Follower - PrepareVote",
            from: [node2, node3, node4], to: node1,
            message_matches: ConsensusNetMessage::PrepareVote(_)
        };

        broadcast! {
            description: "Leader - Confirm",
            from: node1, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Confirm(_)
        };

        send! {
            description: "Follower - Confirm Ack",
            from: [node2, node3, node4], to: node1,
            message_matches: ConsensusNetMessage::ConfirmAck(_)
        };

        broadcast! {
            description: "Leader - Commit",
            from: node1, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Commit(_, _)
        };
    }

    #[test_log::test(tokio::test)]
    async fn test_timeout_join_mutiny_4() {
        let (mut node1, mut node2, mut node3, mut node4): (TestCtx, TestCtx, TestCtx, TestCtx) =
            build_nodes!(4).await;

        node1.start_round().await;
        // Slot 1 - leader = node1

        node1.assert_broadcast("Lost prepare");

        // Make node2 and node3 timeout, node4 will not timeout but follow mutiny
        // , because at f+1, mutiny join
        timeout!(node2, node3);

        broadcast! {
            description: "Follower - Timeout",
            from: node2, to: [node3, node4],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node3, to: [node2, node4],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };

        // node 4 should join the mutiny
        broadcast! {
            description: "Follower - Timeout",
            from: node4, to: [node2, node3],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };

        // After this broadcast, every node has 2f+1 timeouts and can create a timeout certificate

        // Node 2 is next leader, and emits a timeout certificate it will use to broadcast the next Prepare
        let timeout_certificate2 = node2.assert_broadcast("Timeout Certificate 2");
        node3.assert_broadcast("Timeout Certificate 3");
        node4.assert_broadcast("Timeout Certificate 4");

        node2.start_round().await;

        // Slot 2 view 1 (following a timeout round)
        broadcast! {
            description: "Leader - Prepare",
            from: node2, to: [node1, node3, node4],
            message_matches: ConsensusNetMessage::Prepare(cp, Ticket::TimeoutQC(qc)) => {
                if let ConsensusNetMessage::TimeoutCertificate(agg_signature, cp_hash) =
                    timeout_certificate2.msg
                {
                    assert_eq!(&agg_signature, qc);
                    assert_eq!(cp.slot, 1);
                    assert_eq!(cp.view, 1);
                    // Make sure the ticket maches the consensus proposal held by the prepare
                    // but in the previous view
                    let mut cloned_cp = cp.clone();
                    cloned_cp.view -= 1;
                    assert_eq!(cp_hash, cloned_cp.hash());
                } else {
                    panic!("timeout certificate 2 should be a timeout certificate messages but was {:?}", timeout_certificate2);
                }
            }
        };
    }

    #[test_log::test(tokio::test)]
    async fn timeout_next_leader_receive_timeout_certificate_without_timeouting() {
        let (mut node1, mut node2, mut node3, mut node4, mut node5, mut node6, mut node7): (
            TestCtx,
            TestCtx,
            TestCtx,
            TestCtx,
            TestCtx,
            TestCtx,
            TestCtx,
        ) = build_nodes!(7).await;

        node1.start_round();

        let lost_prepare = node1.assert_broadcast("Lost Prepare slot 1/view 0").msg;

        // node2 is the next leader, let the others timeout and create a certificate and send it to node2.
        // It should be able to build a prepare message with it

        timeout!(node3, node4, node5, node6, node7);

        broadcast! {
            description: "Follower - Timeout",
            from: node3, to: [node4, node5, node6, node7],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node4, to: [node3, node5, node6, node7],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node5, to: [node3, node4, node6, node7],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node6, to: [node3, node4, node5, node7],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node7, to: [node3, node4, node5, node6],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };

        // Send node5 timeout certificate to node2
        broadcast! {
            description: "Follower - Timeout Certificate to next leader",
            from: node5, to: [node2],
            message_matches: ConsensusNetMessage::TimeoutCertificate(_, cp_hash) => {
                if let ConsensusNetMessage::Prepare(cp, ticket) = lost_prepare {
                    assert_eq!(&cp.hash(), cp_hash);
                    assert_eq!(ticket, Ticket::Genesis);
                }
            }
        };

        // Clean timeout certificates
        node3.assert_broadcast("Timeout certificate 3");
        node4.assert_broadcast("Timeout certificate 4");
        node6.assert_broadcast("Timeout certificate 6");
        node7.assert_broadcast("Timeout certificate 7");

        node2.start_round();

        broadcast! {
            description: "Next Leader - Prepare with Timeout ticket",
            from: node2, to: [node1, node3, node4, node5, node6, node7],
            message_matches: ConsensusNetMessage::Prepare(cp, Ticket::TimeoutQC(_)) => {
                assert_eq!(cp.slot, 1);
                assert_eq!(cp.view, 1);
            }
        };

        send! {
            description: "Follower - PrepareVote",
            from: [node1, node3, node4, node5, node6, node7], to: node2,
            message_matches: ConsensusNetMessage::PrepareVote(_)
        };

        broadcast! {
            description: "Leader - Confirm",
            from: node2, to: [node1, node3, node4, node5, node6, node7],
            message_matches: ConsensusNetMessage::Confirm(_)
        };

        send! {
            description: "Follower - Confirm Ack",
            from: [node1, node3, node4, node5, node6, node7], to: node2,
            message_matches: ConsensusNetMessage::ConfirmAck(_)
        };

        broadcast! {
            description: "Leader - Commit",
            from: node2, to: [node1, node3, node4, node5, node6, node7],
            message_matches: ConsensusNetMessage::Commit(_, _)
        };
    }

    #[test_log::test(tokio::test)]
    async fn test_candidacy() {
        let (mut node1, mut node2): (TestCtx, TestCtx) = build_nodes!(2).await;

        // Slot 0
        {
            node1.start_round().await;
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

            info!("➡️  Handle block");
            node1.handle_block(&leader_proposal);
            node2.handle_block(&leader_proposal);
        }

        let mut node3 = TestCtx::new_node("node-3").await;
        node3.consensus.bft_round_state.state_tag = StateTag::Joining;
        node3.consensus.bft_round_state.joining.staking_updated_to = 1;
        node3.add_bonded_staker(&node1, 100, "Add staker").await;
        node3.add_bonded_staker(&node2, 100, "Add staker").await;

        // Slot 1: Node3 synchronizes its consensus to the others. - leader = node2
        {
            info!("➡️  Leader proposal");
            node2.start_round().await;
            let leader_proposal = node2.assert_broadcast("Leader proposal");
            node1.handle_msg(&leader_proposal, "Leader proposal");
            node3.handle_msg(&leader_proposal, "Leader proposal");
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
            node3.handle_msg(&leader_commit, "Leader commit");
            info!("➡️  Handle block");
            node1.handle_block(&leader_proposal);
            node2.handle_block(&leader_proposal);
            node3.handle_block(&leader_proposal);
        }

        // Slot 2: New slave candidates - leader = node1
        {
            info!("➡️  Leader proposal");
            node1.start_round().await;
            let leader_proposal = node1.assert_broadcast("Leader proposal");
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

            info!("➡️  Slave 2 candidacy");
            node3.with_stake(100, "Add stake").await;
            // This should trigger send_candidacy as we now have stake.
            node3.handle_msg(&leader_commit, "Leader commit");
            let slave2_candidacy = node3.assert_broadcast("Slave 2 candidacy");
            assert_contains!(
                node1.handle_msg_err(&slave2_candidacy).to_string(),
                "validator is not staking"
            );
            node1.add_staker(&node3, 100, "Add staker").await;
            node1.handle_msg(&slave2_candidacy, "Slave 2 candidacy");
            node2.add_staker(&node3, 100, "Add staker").await;
            node2.handle_msg(&slave2_candidacy, "Slave 2 candidacy");

            info!("➡️  Handle block");
            node1.handle_block(&leader_proposal);
            node2.handle_block(&leader_proposal);
            node3.handle_block(&leader_proposal);
        }

        // Slot 3: Still a slot without slave 2 - leader = node 2
        {
            info!("➡️  Leader proposal - Slot 3");
            node2.start_round().await;
            let leader_proposal = node2.assert_broadcast("Leader proposal");
            if let ConsensusNetMessage::Prepare(_, _) = &leader_proposal.msg {
                assert_eq!(node2.consensus.bft_round_state.staking.bonded().len(), 2);
            } else {
                panic!("Leader proposal is not a Prepare message");
            }
            node1.handle_msg(&leader_proposal, "Leader proposal");
            node3.handle_msg(&leader_proposal, "Leader proposal");
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
            node3.handle_msg(&leader_commit, "Leader commit");

            info!("➡️  Handle block");
            node1.handle_block(&leader_proposal);
            node2.handle_block(&leader_proposal);
            node3.handle_block(&leader_proposal);
        }

        // Slot 4: Slave 2 joined consensus, leader = node-1
        {
            info!("➡️  Leader proposal");
            node1.start_round().await;
            let leader_proposal = node1.assert_broadcast("Leader proposal");
            if let ConsensusNetMessage::Prepare(_, _) = &leader_proposal.msg {
                assert_eq!(node2.consensus.bft_round_state.staking.bonded().len(), 3);
            } else {
                panic!("Leader proposal is not a Prepare message");
            }
            node2.handle_msg(&leader_proposal, "Leader proposal");
            node3.handle_msg(&leader_proposal, "Leader proposal");
            info!("➡️  Slave vote");
            let slave_vote = node2.assert_send(&node1, "Slave vote");
            let slave2_vote = node3.assert_send(&node1, "Slave vote");
            node1.handle_msg(&slave_vote, "Slave vote");
            node1.handle_msg(&slave2_vote, "Slave vote");
            info!("➡️  Leader confirm");
            let leader_confirm = node1.assert_broadcast("Leader confirm");
            node2.handle_msg(&leader_confirm, "Leader confirm");
            node3.handle_msg(&leader_confirm, "Leader confirm");
            info!("➡️  Slave confirm ack");
            let slave_confirm_ack = node2.assert_send(&node1, "Slave confirm ack");
            let slave2_confirm_ack = node3.assert_send(&node1, "Slave confirm ack");
            node1.handle_msg(&slave_confirm_ack, "Slave confirm ack");
            node1.handle_msg(&slave2_confirm_ack, "Slave confirm ack");
            info!("➡️  Leader commit");
            let leader_commit = node1.assert_broadcast("Leader commit");
            node2.handle_msg(&leader_commit, "Leader commit");
            node3.handle_msg(&leader_commit, "Leader commit");
            info!("➡️  Handle block");
            node1.handle_block(&leader_proposal);
            node2.handle_block(&leader_proposal);
            node3.handle_block(&leader_proposal);
        }

        assert_eq!(node1.consensus.bft_round_state.consensus_proposal.slot, 6);
        assert_eq!(node2.consensus.bft_round_state.consensus_proposal.slot, 6);
        assert_eq!(node3.consensus.bft_round_state.consensus_proposal.slot, 6);
    }
}
