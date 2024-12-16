//! Handles all consensus logic up to block commitment.

use crate::mempool::MempoolNetMessage;
use crate::module_handle_messages;
use crate::utils::modules::module_bus_client;
#[cfg(not(test))]
use crate::utils::static_type_map::Pick;
use crate::{bus::BusClientSender, utils::logger::LogMe};
use crate::{
    bus::{command_response::Query, BusMessage},
    data_availability::DataEvent,
    genesis::GenesisEvent,
    handle_messages,
    mempool::{storage::Cut, QueryNewCut},
    model::{get_current_timestamp, Hashable, ValidatorPublicKey},
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
use role_follower::{FollowerRole, FollowerState, TimeoutState};
use role_leader::{LeaderRole, LeaderState};
use serde::{Deserialize, Serialize};
use staking::{Staking, MIN_STAKE};
use std::time::Duration;
use std::{collections::HashMap, default::Default, path::PathBuf};
use tokio::time::interval;
#[cfg(not(test))]
use tokio::{sync::broadcast, time::sleep};
use tracing::{debug, info, warn};

use strum_macros::IntoStaticStr;

pub mod api;
pub mod metrics;
pub mod module;
pub mod role_follower;
pub mod role_leader;
pub mod utils;

// -----------------------------
// ------ Consensus bus --------
// -----------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    TimeoutTick,
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

module_bus_client! {
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
                        .bond(new_v.pubkey.clone())
                        .map_err(|e| anyhow::anyhow!(e))?;
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

        if self.bft_round_state.consensus_proposal.round_leader == *self.crypto.validator_pubkey() {
            self.bft_round_state.state_tag = StateTag::Leader;
            info!("👑 I'm the new leader! 👑")
        } else {
            self.bft_round_state.state_tag = StateTag::Follower;
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

    /// Verifies that the proposed cut in the consensus proposal is valid.
    ///
    /// For the cut to be considered valid:
    /// - Each DataProposal associated with a validator must have received sufficient signatures.
    /// - The aggregated signatures for each DataProposal must be valid.
    fn verify_poda(&mut self, consensus_proposal: &ConsensusProposal) -> Result<()> {
        let f = self.bft_round_state.staking.compute_f();

        let accepted_validators = self.bft_round_state.staking.bonded();
        for (validator, data_proposal_hash, poda_sig) in &consensus_proposal.cut {
            let voting_power = self
                .bft_round_state
                .staking
                .compute_voting_power(poda_sig.validators.as_slice());

            // Verify that the validator is part of the consensus
            if !accepted_validators.contains(validator) {
                bail!(
                    "Validator {} is in cut but is not part of the consensus",
                    validator
                );
            }

            // Verify that DataProposal received enough votes
            if voting_power < f + 1 {
                bail!("PoDA for validator {validator} does not have enough validators that signed his DataProposal");
            }

            // Verify that PoDA signature is valid
            let msg = MempoolNetMessage::DataVote(data_proposal_hash.clone());
            match BlstCrypto::verify_aggregate(&Signed {
                msg,
                signature: poda_sig.clone(),
            }) {
                Ok(valid) => {
                    if !valid {
                        bail!("Failed to aggregate signatures into valid one. Messages might be different.");
                    }
                }
                Err(err) => bail!("Failed to verify PoDA: {}", err),
            };
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
        let voting_power = self
            .bft_round_state
            .staking
            .compute_voting_power(quorum_certificate.validators.as_slice());

        let f = self.bft_round_state.staking.compute_f();

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

    async fn handle_data_event(&mut self, msg: DataEvent) -> Result<()> {
        match msg {
            DataEvent::NewBlock(block) => {
                let block_total_tx = block.total_txs();
                for staker in block.stakers {
                    self.store
                        .bft_round_state
                        .staking
                        .add_staker(staker)
                        .map_err(|e| anyhow!(e))?;
                }
                for validator in block.new_bounded_validators {
                    self.store
                        .bft_round_state
                        .staking
                        .bond(validator)
                        .map_err(|e| anyhow!(e))?;
                }

                if let StateTag::Joining = self.bft_round_state.state_tag {
                    if self.store.bft_round_state.joining.staking_updated_to < block.block_height.0
                    {
                        info!(
                            "🚪 Processed block {} with {} txs",
                            block.block_height.0, block_total_tx
                        );
                        self.store.bft_round_state.joining.staking_updated_to =
                            block.block_height.0;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
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

        module_handle_messages! {
            on_bus self.bus,
            listen<DataEvent> cmd => {
                match self.handle_data_event(cmd).await {
                    Ok(_) => (),
                    Err(e) => warn!("Error while handling data event: {:#}", e),
                }
            }
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

        if let Some(file) = &self.file {
            if let Err(e) = Self::save_on_disk(
                self.config.data_directory.as_path(),
                file.as_path(),
                &self.store,
            ) {
                warn!("Failed to save consensus storage on disk: {}", e);
            }
        }

        Ok(())
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
impl Consensus {}

#[cfg(test)]
impl ConsensusProposal {
    pub fn get_cut(&self) -> Cut {
        self.cut.clone()
    }
}

#[cfg(test)]
pub mod test {

    use crate::model::Block;
    use std::sync::Arc;

    use super::*;
    use crate::{
        autobahn_testing::test::{
            broadcast, build_tuple, send, AutobahnBusClient, AutobahnTestCtx,
        },
        bus::{dont_use_this::get_receiver, metrics::BusMetrics, SharedMessageBus},
        mempool::storage::DataProposalHash,
        p2p::network::NetMessage,
        utils::{conf::Conf, crypto},
    };
    use assertables::assert_contains;
    use staking::Stake;
    use staking::Staker;
    use tokio::sync::broadcast::Receiver;
    use tracing::error;

    pub struct ConsensusTestCtx {
        pub out_receiver: Receiver<OutboundMessage>,
        pub _event_receiver: Receiver<ConsensusEvent>,
        pub _p2p_receiver: Receiver<P2PCommand>,
        pub consensus: Consensus,
        pub name: String,
    }
    macro_rules! build_nodes {
        ($count:tt) => {{
            async {
                let cryptos: Vec<BlstCrypto> = AutobahnTestCtx::generate_cryptos($count);

                let mut nodes = vec![];

                for i in 0..$count {
                    let mut node = ConsensusTestCtx::new(
                        format!("node-{i}").as_ref(),
                        cryptos.get(i).unwrap().clone(),
                    )
                    .await;

                    node.setup_node(i, &cryptos);

                    nodes.push(node);
                }

                build_tuple!(nodes.remove(0), $count)
            }
        }};
    }

    macro_rules! simple_commit_round {
        (leader: $leader:ident, followers: [$($follower:ident),+]) => {{
            let round_consensus_proposal;
            let round_ticket;
            broadcast! {
                description: "Leader - Prepare",
                from: $leader, to: [$($follower),+],
                message_matches: ConsensusNetMessage::Prepare(cp, ticket) => {
                    round_consensus_proposal = cp.clone();
                    round_ticket = ticket.clone();
                }
            };

            send! {
                description: "Follower - PrepareVote",
                from: [$($follower),+], to: $leader,
                message_matches: ConsensusNetMessage::PrepareVote(_)
            };

            broadcast! {
                description: "Leader - Confirm",
                from: $leader, to: [$($follower),+],
                message_matches: ConsensusNetMessage::Confirm(_)
            };

            send! {
                description: "Follower - Confirm Ack",
                from: [$($follower),+], to: $leader,
                message_matches: ConsensusNetMessage::ConfirmAck(_)
            };

            broadcast! {
                description: "Leader - Commit",
                from: $leader, to: [$($follower),+],
                message_matches: ConsensusNetMessage::Commit(_, _)
            };

            (round_consensus_proposal, round_ticket)
        }};
    }

    impl ConsensusTestCtx {
        pub async fn build_consensus(
            shared_bus: &SharedMessageBus,
            crypto: BlstCrypto,
        ) -> Consensus {
            let store = ConsensusStore::default();
            let conf = Arc::new(Conf::default());
            let bus = ConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

            Consensus {
                metrics: ConsensusMetrics::global("id".to_string()),
                bus,
                file: None,
                store,
                config: conf,
                crypto: Arc::new(crypto),
            }
        }

        async fn new(name: &str, crypto: BlstCrypto) -> Self {
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let p2p_receiver = get_receiver::<P2PCommand>(&shared_bus).await;

            let mut new_cut_query_receiver =
                AutobahnBusClient::new_from_bus(shared_bus.new_handle()).await;
            tokio::spawn(async move {
                handle_messages! {
                    on_bus new_cut_query_receiver,
                    command_response<QueryNewCut, Cut> _ => {
                        Ok(Cut::default())
                    }
                }
            });

            let consensus = Self::build_consensus(&shared_bus, crypto).await;
            Self {
                out_receiver,
                _event_receiver: event_receiver,
                _p2p_receiver: p2p_receiver,
                consensus,
                name: name.to_string(),
            }
        }

        pub fn setup_node(&mut self, index: usize, cryptos: &[BlstCrypto]) {
            for other_crypto in cryptos.iter() {
                self.add_trusted_validator(other_crypto.validator_pubkey());
            }

            self.consensus.bft_round_state.consensus_proposal.slot = 1;

            if index == 0 {
                self.consensus.bft_round_state.state_tag = StateTag::Leader;
                self.consensus.bft_round_state.leader.pending_ticket = Some(Ticket::Genesis);
            } else {
                self.consensus.bft_round_state.state_tag = StateTag::Follower;
            }

            self.consensus
                .bft_round_state
                .consensus_proposal
                .round_leader = cryptos.first().unwrap().validator_pubkey().clone();
        }

        pub fn validator_pubkey(&self) -> ValidatorPublicKey {
            self.consensus.crypto.validator_pubkey().clone()
        }

        pub fn staking(&self) -> Staking {
            self.consensus.bft_round_state.staking.clone()
        }

        pub async fn timeout(nodes: &mut [&mut ConsensusTestCtx]) {
            for n in nodes {
                n.consensus
                    .bft_round_state
                    .follower
                    .timeout_state
                    .schedule_next(get_current_timestamp() - 10);
                n.consensus
                    .handle_command(ConsensusCommand::TimeoutTick)
                    .await
                    .unwrap_or_else(|_| panic!("Timeout tick for node {}", n.name));
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

        pub(crate) fn pubkey(&self) -> ValidatorPublicKey {
            self.consensus.crypto.validator_pubkey().clone()
        }

        #[track_caller]
        pub(crate) fn handle_msg(
            &mut self,
            msg: &SignedByValidator<ConsensusNetMessage>,
            err: &str,
        ) {
            debug!("📥 {} Handling message: {:?}", self.name, msg);
            self.consensus.handle_net_message(msg.clone()).expect(err);
        }

        #[track_caller]
        pub(crate) fn handle_msg_err(
            &mut self,
            msg: &SignedByValidator<ConsensusNetMessage>,
        ) -> Error {
            debug!("📥 {} Handling message expecting err: {:?}", self.name, msg);
            let err = self.consensus.handle_net_message(msg.clone()).unwrap_err();
            info!("Expected error: {:#}", err);
            err
        }

        async fn add_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            info!("➕ {} Add staker: {:?}", self.name, staker.name);
            self.consensus
                .handle_data_event(DataEvent::NewBlock(Box::new(Block {
                    stakers: vec![Staker {
                        pubkey: staker.pubkey(),
                        stake: Stake { amount },
                    }],
                    ..Default::default()
                })))
                .await
                .expect(err)
        }

        async fn add_bonded_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            self.add_staker(staker, amount, err).await;
            self.consensus
                .handle_data_event(DataEvent::NewBlock(Box::new(Block {
                    new_bounded_validators: vec![staker.pubkey()],
                    ..Default::default()
                })))
                .await
                .expect(err)
        }

        async fn with_stake(&mut self, amount: u64, err: &str) {
            self.consensus
                .handle_data_event(DataEvent::NewBlock(Box::new(Block {
                    stakers: vec![Staker {
                        pubkey: self.consensus.crypto.validator_pubkey().clone(),
                        stake: Stake { amount },
                    }],
                    ..Default::default()
                })))
                .await
                .expect(err)
        }

        pub async fn start_round(&mut self) {
            self.consensus
                .start_round()
                .await
                .expect("Failed to start slot");
        }

        #[track_caller]
        pub(crate) fn assert_broadcast(
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
                    error!(
                        "{description}: NetMessage::ConsensusMessage message is missing, found {}",
                        net_msg
                    );
                    self.assert_broadcast(description)
                }
            } else {
                error!(
                    "{description}: OutboundMessage::BroadcastMessage message is missing, found {:?}",
                    rec
                );
                self.assert_broadcast(description)
            }
        }

        #[track_caller]
        pub(crate) fn assert_no_broadcast(&mut self, description: &str) {
            #[allow(clippy::expect_fun_call)]
            let rec = self.out_receiver.try_recv();

            match rec {
                Ok(OutboundMessage::BroadcastMessage(net_msg)) => {
                    panic!("{description}: Broadcast message found: {:?}", net_msg)
                }
                els => {
                    info!("{description}: Got {:?}", els);
                }
            }
        }

        #[track_caller]
        pub(crate) fn assert_send(
            &mut self,
            to: &ValidatorPublicKey,
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
                assert_eq!(to, &dest);
                if let NetMessage::ConsensusMessage(msg) = net_msg {
                    msg
                } else {
                    error!(
                        "{description}: NetMessage::ConsensusMessage message is missing, found {}",
                        net_msg
                    );
                    self.assert_send(to, description)
                }
            } else {
                error!(
                    "{description}: OutboundMessage::Send message is missing, found {:?}",
                    rec
                );
                self.assert_send(to, description)
            }
        }
    }
    #[test_log::test(tokio::test)]
    async fn test_happy_path() {
        let (mut node1, mut node2): (ConsensusTestCtx, ConsensusTestCtx) = build_nodes!(2).await;

        node1.start_round().await;
        // Slot 0 - leader = node1

        let (cp1, ticket1) = simple_commit_round! {
            leader: node1,
            followers: [node2]
        };

        assert_eq!(cp1.slot, 1);
        assert_eq!(cp1.view, 0);
        assert_eq!(ticket1, Ticket::Genesis);

        // Slot 1 - leader = node2
        node2.start_round().await;

        let (cp2, ticket2) = simple_commit_round! {
            leader: node2,
            followers: [node1]
        };

        assert_eq!(cp2.slot, 2);
        assert_eq!(cp2.view, 0);
        assert!(matches!(ticket2, Ticket::CommitQC(_)));

        // Slot 2 - leader = node1
        node1.start_round().await;

        let (cp3, ticket3) = simple_commit_round! {
            leader: node1,
            followers: [node2]
        };

        assert_eq!(cp3.slot, 3);
        assert_eq!(cp3.view, 0);
        assert!(matches!(ticket3, Ticket::CommitQC(_)));
    }

    #[test_log::test(tokio::test)]
    async fn basic_commit_4() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

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
    async fn prepare_wrong_slot() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round().await;

        // Create wrong prepare
        let prepare_msg = node1
            .consensus
            .sign_net_message(ConsensusNetMessage::Prepare(
                ConsensusProposal {
                    slot: 2,
                    view: 0,
                    round_leader: node1.pubkey(),
                    cut: vec![(
                        node2.pubkey(),
                        DataProposalHash("test".to_string()),
                        AggregateSignature::default(),
                    )],
                    new_validators_to_bond: vec![],
                },
                Ticket::Genesis,
            ))
            .expect("Error while signing");

        // Slot 1 - leader = node1
        // Ensuring one slot commits correctly before a timeout

        assert_contains!(node2.handle_msg_err(&prepare_msg).to_string(), "wrong slot");
        assert_contains!(node3.handle_msg_err(&prepare_msg).to_string(), "wrong slot");
        assert_contains!(node4.handle_msg_err(&prepare_msg).to_string(), "wrong slot");
    }

    #[test_log::test(tokio::test)]
    async fn prepare_wrong_signature() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round().await;

        // Create wrong prepare signed by other node than leader
        let prepare_msg = node2
            .consensus
            .sign_net_message(ConsensusNetMessage::Prepare(
                ConsensusProposal {
                    slot: 1,
                    view: 0,
                    round_leader: node1.pubkey(),
                    cut: vec![(
                        node2.pubkey(),
                        DataProposalHash("test".to_string()),
                        AggregateSignature::default(),
                    )],
                    new_validators_to_bond: vec![],
                },
                Ticket::Genesis,
            ))
            .expect("Error while signing");

        // Slot 1 - leader = node1
        // Ensuring one slot commits correctly before a timeout

        assert_contains!(
            node2.handle_msg_err(&prepare_msg).to_string(),
            "does not come from current leader"
        );
        assert_contains!(
            node3.handle_msg_err(&prepare_msg).to_string(),
            "does not come from current leader"
        );
        assert_contains!(
            node4.handle_msg_err(&prepare_msg).to_string(),
            "does not come from current leader"
        );
    }

    #[test_log::test(tokio::test)]
    async fn prepare_wrong_leader() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round().await;

        // Create prepare with wrong round leader (node3) and signed accordingly
        let prepare_msg = node3
            .consensus
            .sign_net_message(ConsensusNetMessage::Prepare(
                ConsensusProposal {
                    slot: 1,
                    view: 0,
                    round_leader: node3.pubkey(),
                    cut: vec![(
                        node2.pubkey(),
                        DataProposalHash("test".to_string()),
                        AggregateSignature::default(),
                    )],
                    new_validators_to_bond: vec![],
                },
                Ticket::Genesis,
            ))
            .expect("Error while signing");

        // Slot 1 - leader = node1
        // Ensuring one slot commits correctly before a timeout

        assert_contains!(
            node2.handle_msg_err(&prepare_msg).to_string(),
            "does not come from current leader"
        );
        assert_contains!(
            node1.handle_msg_err(&prepare_msg).to_string(),
            "does not come from current leader"
        );
        assert_contains!(
            node3.handle_msg_err(&prepare_msg).to_string(),
            "does not come from current leader"
        );
        assert_contains!(
            node4.handle_msg_err(&prepare_msg).to_string(),
            "does not come from current leader"
        );
    }

    #[test_log::test(tokio::test)]
    async fn timeout_only_one_4() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round().await;

        ConsensusTestCtx::timeout(&mut [&mut node2]).await;

        node2.assert_broadcast("Timeout message");

        // Slot 1 - leader = node1
        // Ensuring one slot commits correctly before a timeout

        let (cp, ticket) = simple_commit_round! {
            leader: node1,
            followers: [node2, node3, node4]
        };

        assert_eq!(cp.slot, 1);
        assert_eq!(cp.view, 0);
        assert!(matches!(ticket, Ticket::Genesis));
    }

    #[test_log::test(tokio::test)]
    async fn test_timeout_join_mutiny_4() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round().await;
        // Slot 1 - leader = node1

        node1.assert_broadcast("Lost prepare");

        // Make node2 and node3 timeout, node4 will not timeout but follow mutiny
        // , because at f+1, mutiny join
        ConsensusTestCtx::timeout(&mut [&mut node2, &mut node3]).await;

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

        // TestCtx::broadcast(
        //     "Follower - Timeout",
        //     &mut node3,
        //     &mut [&mut node2, &mut node4],
        //     &mut Some(|m: &SignedByValidator<ConsensusNetMessage>| {
        //         message_matches!(m, ConsensusNetMessage::Timeout(_, next_leader) => {
        //             assert_eq!(&node2key, next_leader);
        //         });
        //     }),
        // )
        // .await

        // After this broadcast, every node has 2f+1 timeouts and can create a timeout certificate

        // Node 2 is next leader, and does not emits a timeout certificate since it will broadcast the next Prepare with it
        node2.assert_no_broadcast("Timeout Certificate 2");

        node3.assert_broadcast("Timeout Certificate 3");
        node4.assert_broadcast("Timeout Certificate 4");

        node2.start_round().await;

        // Slot 2 view 1 (following a timeout round)
        let (cp, ticket) = simple_commit_round! {
          leader: node2,
          followers: [node1, node3, node4]
        };

        assert!(matches!(ticket, Ticket::TimeoutQC(_)));
        assert_eq!(cp.slot, 1);
        assert_eq!(cp.view, 1);
    }

    #[test_log::test(tokio::test)]
    async fn timeout_only_emit_certificate_once() {
        let (mut node1, mut node2, mut node3, mut node4, mut node5): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(5).await;

        node1.start_round().await;
        // Slot 1 - leader = node1

        node1.assert_broadcast("Lost prepare");

        // Make node2 and node3 timeout, node4 will not timeout but follow mutiny,
        // because at f+1, mutiny join
        ConsensusTestCtx::timeout(&mut [&mut node2, &mut node3]).await;

        broadcast! {
            description: "Follower - Timeout",
            from: node2, to: [node3, node4, node5],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node3, to: [node2, node4, node5],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };

        // node 4 should join the mutiny
        broadcast! {
            description: "Follower - Timeout",
            from: node4, to: [node2, node3, node5],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };

        // By receiving this, other nodes should not produce another timeout certificate
        broadcast! {
            description: "Follower - Timeout",
            from: node5, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Timeout(_, next_leader) => {
                assert_eq!(&node2.pubkey(), next_leader);
            }
        };

        // After this broadcast, every node has 2f+1 timeouts and can create a timeout certificate

        // Node 2 is next leader, and emits a timeout certificate it will use to broadcast the next Prepare
        node2.assert_no_broadcast("Timeout Certificate 2");
        node3.assert_broadcast("Timeout Certificate 3");
        node4.assert_broadcast("Timeout Certificate 4");
        node5.assert_broadcast("Timeout Certificate 5");

        // No
        node2.assert_no_broadcast("Timeout certificate 2");
        node3.assert_no_broadcast("Timeout certificate 3");
        node4.assert_no_broadcast("Timeout certificate 4");

        node2.start_round().await;

        // Slot 2 view 1 (following a timeout round)
        let (cp, ticket) = simple_commit_round! {
          leader: node2,
          followers: [node1, node3, node4, node5]
        };

        assert!(matches!(ticket, Ticket::TimeoutQC(_)));
        assert_eq!(cp.slot, 1);
        assert_eq!(cp.view, 1);
    }

    #[test_log::test(tokio::test)]
    async fn timeout_next_leader_receive_timeout_certificate_without_timeouting() {
        let (mut node1, mut node2, mut node3, mut node4, mut node5, mut node6, mut node7): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(7).await;

        node1.start_round().await;

        let lost_prepare = node1.assert_broadcast("Lost Prepare slot 1/view 0").msg;

        // node2 is the next leader, let the others timeout and create a certificate and send it to node2.
        // It should be able to build a prepare message with it

        ConsensusTestCtx::timeout(&mut [
            &mut node3, &mut node4, &mut node5, &mut node6, &mut node7,
        ])
        .await;

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

        node2.start_round().await;

        let (cp, ticket) = simple_commit_round! {
            leader: node2,
            followers: [node1, node3, node4, node5, node6, node7]
        };

        assert_eq!(cp.slot, 1);
        assert_eq!(cp.view, 1);
        assert!(matches!(ticket, Ticket::TimeoutQC(_)));
    }

    // TODO:
    // - fake Prepare
    // - Wrong leader
    // - Cut is valid

    #[test_log::test(tokio::test)]
    async fn test_candidacy() {
        let (mut node1, mut node2): (ConsensusTestCtx, ConsensusTestCtx) = build_nodes!(2).await;

        // Slot 1
        {
            node1.start_round().await;

            let (cp, ticket) = simple_commit_round! {
                leader: node1,
                followers: [node2]
            };

            assert_eq!(cp.slot, 1);
            assert_eq!(cp.view, 0);
            assert!(matches!(ticket, Ticket::Genesis));
        }

        let mut node3 = ConsensusTestCtx::new_node("node-3").await;
        node3.consensus.bft_round_state.state_tag = StateTag::Joining;
        node3.consensus.bft_round_state.joining.staking_updated_to = 1;
        node3.add_bonded_staker(&node1, 100, "Add staker").await;
        node3.add_bonded_staker(&node2, 100, "Add staker").await;

        // Slot 2: Node3 synchronizes its consensus to the others. - leader = node2
        {
            info!("➡️  Leader proposal");
            node2.start_round().await;

            broadcast! {
                description: "Leader Proposal",
                from: node2, to: [node1, node3]
            };
            send! {
                description: "Prepare Vote",
                from: [node1], to: node2,
                message_matches: ConsensusNetMessage::PrepareVote(_)
            };
            broadcast! {
                description: "Leader Confirm",
                from: node2, to: [node1, node3]
            };
            send! {
                description: "Confirm Ack",
                from: [node1], to: node2,
                message_matches: ConsensusNetMessage::ConfirmAck(_)
            };
            broadcast! {
                description: "Leader Commit",
                from: node2, to: [node1, node3]
            };
        }

        // Slot 3: New slave candidates - leader = node1
        {
            info!("➡️  Leader proposal");
            node1.start_round().await;
            let leader_proposal = node1.assert_broadcast("Leader proposal");
            node2.handle_msg(&leader_proposal, "Leader proposal");
            node3.handle_msg(&leader_proposal, "Leader proposal");
            info!("➡️  Slave vote");
            let slave_vote = node2.assert_send(&node1.validator_pubkey(), "Slave vote");
            node1.handle_msg(&slave_vote, "Slave vote");
            info!("➡️  Leader confirm");
            let leader_confirm = node1.assert_broadcast("Leader confirm");
            node2.handle_msg(&leader_confirm, "Leader confirm");
            node3.handle_msg(&leader_confirm, "Leader confirm");
            info!("➡️  Slave confirm ack");
            let slave_confirm_ack =
                node2.assert_send(&node1.validator_pubkey(), "Slave confirm ack");
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
        }

        // Slot 4: Still a slot without slave 2 - leader = node 2
        {
            info!("➡️  Leader proposal - Slot 3");
            node2.start_round().await;

            broadcast! {
                description: "Leader Proposal",
                from: node2, to: [node1, node3],
                message_matches: ConsensusNetMessage::Prepare(_, _) => {
                    assert_eq!(node2.consensus.bft_round_state.staking.bonded().len(), 2);
                }
            };
            send! {
                description: "Prepare Vote",
                from: [node1], to: node2,
                message_matches: ConsensusNetMessage::PrepareVote(_)
            };
            broadcast! {
                description: "Leader Confirm",
                from: node2, to: [node1, node3]
            };
            send! {
                description: "Confirm Ack",
                from: [node1], to: node2,
                message_matches: ConsensusNetMessage::ConfirmAck(_)
            };
            broadcast! {
                description: "Leader Commit",
                from: node2, to: [node1, node3]
            };
        }

        // Slot 5: Slave 2 joined consensus, leader = node-1
        {
            info!("➡️  Leader proposal");
            node1.start_round().await;

            let (cp, _) = simple_commit_round! {
                leader: node1,
                followers: [node2, node3]
            };
            assert_eq!(cp.slot, 5);
            assert_eq!(node2.consensus.bft_round_state.staking.bonded().len(), 3);
        }

        assert_eq!(node1.consensus.bft_round_state.consensus_proposal.slot, 6);
        assert_eq!(node2.consensus.bft_round_state.consensus_proposal.slot, 6);
        assert_eq!(node3.consensus.bft_round_state.consensus_proposal.slot, 6);
    }
}
