//! Handles all consensus logic up to block commitment.

use crate::model::*;
use crate::module_handle_messages;
use crate::node_state::module::NodeStateEvent;
use crate::utils::modules::module_bus_client;
use crate::{bus::BusClientSender, utils::logger::LogMe};
use crate::{
    bus::{command_response::Query, BusMessage},
    genesis::GenesisEvent,
    mempool::QueryNewCut,
    model::{Cut, Hashed, StakingAction, ValidatorPublicKey},
    p2p::{network::OutboundMessage, P2PCommand},
    utils::{
        conf::SharedConf,
        crypto::{BlstCrypto, SharedBlstCrypto},
        modules::Module,
    },
};
use anyhow::{anyhow, bail, Context, Error, Result};
use borsh::{BorshDeserialize, BorshSerialize};
use hyle_model::utils::get_current_timestamp;
use hyle_model::utils::get_current_timestamp_ms;
use metrics::ConsensusMetrics;
use role_follower::{FollowerRole, FollowerState};
use role_leader::{LeaderRole, LeaderState};
use role_timeout::{TimeoutRole, TimeoutRoleState, TimeoutState};
use serde::{Deserialize, Serialize};
use staking::state::{Staking, MIN_STAKE};
use std::ops::Deref;
use std::ops::DerefMut;
use std::time::Duration;
use std::{collections::HashMap, default::Default, path::PathBuf};
use tokio::time::interval;
#[cfg(not(test))]
use tokio::{sync::broadcast, time::sleep};
use tracing::{debug, info, trace, warn};

pub mod api;
pub mod metrics;
pub mod module;
pub mod role_follower;
pub mod role_leader;
pub mod role_timeout;

// -----------------------------
// ------ Consensus bus --------
// -----------------------------

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusCommand {
    TimeoutTick,
    StartNewSlot,
}

#[derive(Debug, Clone, Deserialize, Serialize, BorshSerialize, BorshDeserialize)]
pub struct CommittedConsensusProposal {
    pub staking: Staking,
    pub consensus_proposal: ConsensusProposal,
    pub certificate: QuorumCertificate,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum ConsensusEvent {
    CommitConsensusProposal(CommittedConsensusProposal),
}

#[derive(Clone)]
pub struct QueryConsensusInfo {}

#[derive(Clone)]
pub struct QueryConsensusStakingState {}

impl BusMessage for ConsensusCommand {}
impl BusMessage for ConsensusEvent {}
impl BusMessage for ConsensusNetMessage {}

impl<T> BusMessage for SignedByValidator<T> where T: BorshSerialize + BusMessage {}

module_bus_client! {
struct ConsensusBusClient {
sender(OutboundMessage),
sender(ConsensusEvent),
sender(ConsensusCommand),
sender(P2PCommand),
sender(Query<QueryNewCut, Cut>),
receiver(ConsensusCommand),
receiver(GenesisEvent),
receiver(NodeStateEvent),
receiver(SignedByValidator<ConsensusNetMessage>),
receiver(Query<QueryConsensusInfo, ConsensusInfo>),
receiver(Query<QueryConsensusStakingState, Staking>),
}
}

// TODO: move struct to model.rs ?
#[derive(BorshSerialize, BorshDeserialize, Default)]
pub struct BFTRoundState {
    consensus_proposal: ConsensusProposal,
    last_cut: Cut,
    staking: Staking,

    leader: LeaderState,
    follower: FollowerState,
    timeout: TimeoutRoleState,
    joining: JoiningState,
    genesis: GenesisState,
    state_tag: StateTag,
}

#[derive(BorshSerialize, BorshDeserialize, Default, Debug)]
enum StateTag {
    #[default]
    Joining,
    Leader,
    Follower,
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
pub struct JoiningState {
    staking_updated_to: Slot,
    buffered_prepares: Vec<ConsensusProposal>,
}
#[derive(BorshSerialize, BorshDeserialize, Default)]
pub struct GenesisState {
    peer_pubkey: HashMap<String, ValidatorPublicKey>,
}

#[derive(BorshSerialize, BorshDeserialize, Default)]
pub struct ConsensusStore {
    bft_round_state: BFTRoundState,
    /// Validators that asked to be part of consensus
    validator_candidates: Vec<NewValidatorCandidate>,
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

        let round_proposal_hash = self.bft_round_state.consensus_proposal.hashed();
        let round_parent_hash =
            std::mem::take(&mut self.bft_round_state.consensus_proposal.parent_hash);

        let staking_actions =
            std::mem::take(&mut self.bft_round_state.consensus_proposal.staking_actions);

        // Reset round state, carrying over staking and current proposal.
        self.bft_round_state = BFTRoundState {
            last_cut: std::mem::take(&mut self.bft_round_state.consensus_proposal.cut),
            consensus_proposal: ConsensusProposal {
                slot: self.bft_round_state.consensus_proposal.slot,
                view: self.bft_round_state.consensus_proposal.view,
                timestamp: self.bft_round_state.consensus_proposal.timestamp,
                round_leader: std::mem::take(
                    &mut self.bft_round_state.consensus_proposal.round_leader,
                ),
                ..ConsensusProposal::default()
            },
            staking: std::mem::take(&mut self.bft_round_state.staking),
            ..BFTRoundState::default()
        };

        // If we finish the round via a committed proposal, update some state
        match ticket {
            Some(Ticket::CommitQC(qc)) => {
                self.bft_round_state.consensus_proposal.parent_hash = round_proposal_hash;
                self.bft_round_state.consensus_proposal.slot += 1;
                self.bft_round_state.consensus_proposal.view = 0;
                self.bft_round_state.follower.buffered_quorum_certificate = Some(qc);
                let staking = &mut self.store.bft_round_state.staking;
                for action in staking_actions {
                    match action {
                        // Any new validators are added to the consensus and removed from candidates.
                        ConsensusStakingAction::Bond { candidate } => {
                            debug!("🎉 New validator bonded: {}", candidate.pubkey);
                            staking
                                .bond(candidate.pubkey)
                                .map_err(|e| anyhow::anyhow!(e))?;
                        }
                        ConsensusStakingAction::PayFeesForDaDi {
                            disseminator,
                            cumul_size,
                        } => staking
                            .pay_for_dadi(disseminator, cumul_size)
                            .map_err(|e| anyhow::anyhow!(e))?,
                    }
                }
                staking.distribute().map_err(|e| anyhow::anyhow!(e))?;
            }
            Some(Ticket::TimeoutQC(_)) => {
                self.bft_round_state.consensus_proposal.parent_hash = round_parent_hash;
                self.bft_round_state.consensus_proposal.view += 1;
            }
            els => {
                bail!("Invalid ticket here {:?}", els);
            }
        }

        debug!(
            "🥋 Ready for slot {}, view {}",
            self.bft_round_state.consensus_proposal.slot,
            self.bft_round_state.consensus_proposal.view
        );

        self.bft_round_state.consensus_proposal.round_leader = self.next_leader()?;

        if self.bft_round_state.consensus_proposal.round_leader == *self.crypto.validator_pubkey() {
            self.bft_round_state.state_tag = StateTag::Leader;
            debug!("👑 I'm the new leader! 👑")
        } else {
            self.bft_round_state.state_tag = StateTag::Follower;
            self.bft_round_state
                .timeout
                .state
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

    fn verify_staking_actions(&mut self, proposal: &ConsensusProposal) -> Result<()> {
        for action in &proposal.staking_actions {
            match action {
                ConsensusStakingAction::Bond { candidate } => {
                    self.verify_new_validators_to_bond(candidate)?;
                }
                ConsensusStakingAction::PayFeesForDaDi {
                    disseminator,
                    cumul_size,
                } => Self::verify_dadi_fees(&proposal.cut, disseminator, cumul_size)?,
            }
        }
        Ok(())
    }

    /// Verify that the fees paid by the disseminator are correct
    fn verify_dadi_fees(
        cut: &Cut,
        disseminator: &ValidatorPublicKey,
        cumul_size: &LaneBytesSize,
    ) -> Result<()> {
        cut.iter()
            .find(|l| &l.0 == disseminator && &l.2 == cumul_size)
            .map(|_| ())
            .ok_or(anyhow!(
                "Malformed PayFeesForDadi. Not found in cut: {disseminator}, {cumul_size}"
            ))
    }

    /// Verify that new validators have enough stake
    /// and have a valid signature so can be bonded.
    fn verify_new_validators_to_bond(
        &mut self,
        new_validator: &NewValidatorCandidate,
    ) -> Result<()> {
        // Verify that the new validator has enough stake
        if let Some(stake) = self
            .bft_round_state
            .staking
            .get_stake(&new_validator.pubkey)
        {
            if stake < staking::state::MIN_STAKE {
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
            let command_sender = crate::utils::static_type_map::Pick::<
                broadcast::Sender<ConsensusCommand>,
            >::get(&self.bus)
            .clone();
            let interval = self.config.consensus.slot_duration;
            tokio::task::Builder::new()
                .name("sleep-consensus")
                .spawn(async move {
                    debug!(
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

    fn get_own_voting_power(&self) -> u128 {
        if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            if let Some(my_stake) = self
                .bft_round_state
                .staking
                .get_stake(self.crypto.validator_pubkey())
            {
                my_stake
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
    fn verify_quorum_certificate<T: borsh::BorshSerialize>(
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

        trace!(
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
            ConsensusNetMessage::Timeout(slot, view) => self.on_timeout(msg, slot, view),
            ConsensusNetMessage::TimeoutCertificate(timeout_certificate, slot, view) => {
                self.on_timeout_certificate(&timeout_certificate, slot, view)
            }

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
        debug!(
            "Trying to process timeout Certificate against consensus proposal slot: {}, view: {}",
            self.bft_round_state.consensus_proposal.slot,
            self.bft_round_state.consensus_proposal.view,
        );

        self.verify_quorum_certificate(
            ConsensusNetMessage::Timeout(
                self.bft_round_state.consensus_proposal.slot,
                self.bft_round_state.consensus_proposal.view,
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
            .position(|p| p.hashed() == proposal_hash_hint)
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
        // We sucessfully joined the consensus
        info!(
            "🏁 Synchronized to slot {}",
            self.bft_round_state.consensus_proposal.slot
        );
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
            ConsensusNetMessage::ConfirmAck(self.bft_round_state.consensus_proposal.hashed()),
            &commit_quorum_certificate,
        )?;

        self.metrics.commit();

        _ = self
            .bus
            .send(ConsensusEvent::CommitConsensusProposal(
                CommittedConsensusProposal {
                    staking: self.bft_round_state.staking.clone(),
                    consensus_proposal: self.bft_round_state.consensus_proposal.clone(),
                    certificate: commit_quorum_certificate.clone(),
                },
            ))
            .log_error("Failed to send ConsensusEvent::CommittedConsensusProposal on the bus");

        debug!(
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
            if stake < staking::state::MIN_STAKE {
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

    async fn handle_node_state_event(&mut self, msg: NodeStateEvent) -> Result<()> {
        match msg {
            NodeStateEvent::NewBlock(block) => {
                let block_total_tx = block.total_txs();
                for action in block.staking_actions {
                    match action {
                        (identity, StakingAction::Stake { amount }) => {
                            self.store
                                .bft_round_state
                                .staking
                                .stake(identity, amount)
                                .map_err(|e| anyhow!(e))?;
                        }
                        (identity, StakingAction::Delegate { validator }) => {
                            self.store
                                .bft_round_state
                                .staking
                                .delegate_to(identity, validator)
                                .map_err(|e| anyhow!(e))?;
                        }
                        (_identity, StakingAction::Distribute { claim: _ }) => todo!(),
                        (_identity, StakingAction::DepositForFees { holder, amount }) => {
                            self.store
                                .bft_round_state
                                .staking
                                .deposit_for_fees(holder, amount)
                                .map_err(|e| anyhow!(e))?;
                        }
                    }
                }
                for validator in block.new_bounded_validators.iter() {
                    self.store
                        .bft_round_state
                        .staking
                        .bond(validator.clone())
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
        }
    }

    async fn handle_command(&mut self, msg: ConsensusCommand) -> Result<()> {
        match msg {
            ConsensusCommand::TimeoutTick => match &self.bft_round_state.timeout.state {
                TimeoutState::Scheduled { timestamp } if get_current_timestamp() >= *timestamp => {
                    // Trigger state transition to mutiny
                    info!(
                        "⏰ Trigger timeout for slot {} and view {}",
                        self.bft_round_state.consensus_proposal.slot,
                        self.bft_round_state.consensus_proposal.view
                    );

                    let timeout_message = ConsensusNetMessage::Timeout(
                        self.bft_round_state.consensus_proposal.slot,
                        self.bft_round_state.consensus_proposal.view,
                    );

                    let signed_timeout_message = self
                        .sign_net_message(timeout_message.clone())
                        .context("Signing timeout message")?;

                    self.on_timeout(
                        signed_timeout_message,
                        self.bft_round_state.consensus_proposal.slot,
                        self.bft_round_state.consensus_proposal.view,
                    )?;

                    self.broadcast_net_message(timeout_message)?;

                    self.bft_round_state.timeout.state.cancel();

                    Ok(())
                }
                _ => Ok(()),
            },
            ConsensusCommand::StartNewSlot => {
                self.start_round(get_current_timestamp_ms()).await?;
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
        let should_shutdown = module_handle_messages! {
            on_bus self.bus,
            listen<GenesisEvent> msg => {
                match msg {
                    GenesisEvent::GenesisBlock(signed_block) => {
                        self.bft_round_state.consensus_proposal.parent_hash = signed_block.hashed();
                        self.bft_round_state.consensus_proposal.round_leader = signed_block.consensus_proposal.round_leader.clone();

                        if self.bft_round_state.consensus_proposal.round_leader == *self.crypto.validator_pubkey() {
                            self.bft_round_state.state_tag = StateTag::Leader;
                            self.bft_round_state.consensus_proposal.slot = 1;
                            info!("👑 Starting consensus as leader");
                        } else {
                            self.bft_round_state.state_tag = StateTag::Follower;
                            self.bft_round_state.consensus_proposal.slot = 1;
                            info!(
                                "💂‍♂️ Starting consensus as follower of leader {}",
                                self.bft_round_state.consensus_proposal.round_leader
                            );
                        }
                        // Now wait until we have processed the genesis block to update our Staking.
                        module_handle_messages! {
                            on_bus self.bus,
                            listen<NodeStateEvent> event => {
                                let NodeStateEvent::NewBlock(block) = &event;
                                if block.block_height.0 != 0 {
                                    bail!("Non-genesis block received during consensus genesis");
                                }
                                match self.handle_node_state_event(event).await {
                                    Ok(_) => break,
                                    Err(e) => bail!("Error while handling Genesis block: {:#}", e),
                                }
                            }
                        };
                        // Send a CommitConsensusProposal for the genesis block
                        _ = self
                            .bus
                            .send(ConsensusEvent::CommitConsensusProposal(
                                CommittedConsensusProposal {
                                    staking: self.bft_round_state.staking.clone(),
                                    consensus_proposal: signed_block.consensus_proposal,
                                    certificate: signed_block.certificate,
                                },
                            ))
                            .log_error("Failed to send ConsensusEvent::CommittedConsensusProposal on the bus");
                        break;
                    },
                    GenesisEvent::NoGenesis => {
                        // If we deserialized, we might be a follower or a leader.
                        // There's a few possibilities: maybe we're restarting quick enough that we're still synched,
                        // maybe we actually would block consensus by having a large stake
                        // maybe we were about to be the leader and got byzantined out.
                        // Regardless, we should probably assume that we need to catch up.
                        // TODO: this logic can be improved.
                        self.bft_round_state.state_tag = StateTag::Joining;
                        break;
                    },
                }
            }
        };
        if should_shutdown {
            return Ok(());
        }

        if self.is_round_leader() {
            self.delay_start_new_round(Ticket::Genesis)?;
        }
        self.start().await
    }

    async fn start(&mut self) -> Result<()> {
        info!("🚀 Starting consensus");

        let mut timeout_ticker = interval(Duration::from_millis(100));
        timeout_ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        module_handle_messages! {
            on_bus self.bus,
            listen<NodeStateEvent> event => {
                let _ = self.handle_node_state_event(event).await.log_error("Error while handling data event");
            }
            listen<ConsensusCommand> cmd => {
                let _ = self.handle_command(cmd).await.log_error("Error while handling consensus command");
            }
            listen<SignedByValidator<ConsensusNetMessage>> cmd => {
                let _ = self.handle_net_message(cmd).log_error("Consensus message failed");
            }
            command_response<QueryConsensusInfo, ConsensusInfo> _ => {
                let slot = self.bft_round_state.consensus_proposal.slot;
                let view = self.bft_round_state.consensus_proposal.view;
                let round_leader = self.bft_round_state.consensus_proposal.round_leader.clone();
                let validators = self.bft_round_state.staking.bonded().clone();
                Ok(ConsensusInfo { slot, view, round_leader, validators })
            }
            command_response<QueryConsensusStakingState, Staking> _ => {
                Ok(self.bft_round_state.staking.clone())
            }
            _ = timeout_ticker.tick() => {
                self.bus.send(ConsensusCommand::TimeoutTick)
                    .log_error("Cannot send message over channel")?;
            }
        };

        if let Some(file) = &self.file {
            if let Err(e) = Self::save_on_disk(file.as_path(), &self.store) {
                warn!("Failed to save consensus storage on disk: {}", e);
            }
        }

        Ok(())
    }

    fn sign_net_message(
        &self,
        msg: ConsensusNetMessage,
    ) -> Result<SignedByValidator<ConsensusNetMessage>> {
        trace!("🔏 Signing message: {}", msg);
        self.crypto.sign(msg)
    }
}

#[cfg(test)]
impl Consensus {}

#[cfg(test)]
pub mod test {

    use crate::{
        bus::{bus_client, command_response::CmdRespClient},
        handle_messages,
        model::Block,
        node_state::module::NodeStateModule,
        rest::RestApi,
        utils::integration_test::NodeIntegrationCtxBuilder,
    };
    use std::sync::Arc;

    use super::*;
    use crate::{
        bus::{dont_use_this::get_receiver, metrics::BusMetrics, SharedMessageBus},
        model::DataProposalHash,
        p2p::network::NetMessage,
        tests::autobahn_testing::{
            broadcast, build_tuple, send, simple_commit_round, AutobahnBusClient, AutobahnTestCtx,
        },
        utils::{conf::Conf, crypto},
    };
    use assertables::assert_contains;
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

    impl ConsensusTestCtx {
        pub async fn build_consensus(
            shared_bus: &SharedMessageBus,
            crypto: BlstCrypto,
        ) -> Consensus {
            let store = ConsensusStore::default();
            let mut conf = Conf::default();
            conf.consensus.slot_duration = 1000;
            let bus = ConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

            Consensus {
                metrics: ConsensusMetrics::global("id".to_string()),
                bus,
                file: None,
                store,
                config: Arc::new(conf),
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
                };
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
            self.consensus
                .bft_round_state
                .consensus_proposal
                .parent_hash = ConsensusProposalHash("genesis".to_string());

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

        pub fn setup_for_joining(&mut self, nodes: &[&ConsensusTestCtx]) {
            for other_node in nodes.iter() {
                self.add_trusted_validator(other_node.consensus.crypto.validator_pubkey());
            }

            self.consensus.bft_round_state.state_tag = StateTag::Joining;
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
                    .timeout
                    .state
                    .schedule_next(get_current_timestamp() - 10);
                n.consensus
                    .handle_command(ConsensusCommand::TimeoutTick)
                    .await
                    .unwrap_or_else(|err| panic!("Timeout tick for node {}: {:?}", n.name, err));
            }
        }

        pub fn add_trusted_validator(&mut self, pubkey: &ValidatorPublicKey) {
            self.consensus
                .bft_round_state
                .staking
                .stake(hex::encode(pubkey.0.clone()).into(), 100)
                .unwrap();

            self.consensus
                .bft_round_state
                .staking
                .deposit_for_fees(pubkey.clone(), 1_000_000_000)
                .unwrap();

            self.consensus
                .bft_round_state
                .staking
                .delegate_to(hex::encode(pubkey.0.clone()).into(), pubkey.clone())
                .unwrap();

            self.consensus
                .bft_round_state
                .staking
                .bond(pubkey.clone())
                .expect("cannot bond trusted validator");
            info!("🎉 Trusted validator added: {}", pubkey);
        }

        async fn new_node(name: &str) -> Self {
            let crypto = crypto::BlstCrypto::new(name.into()).unwrap();
            Self::new(name, crypto.clone()).await
        }

        pub(crate) fn pubkey(&self) -> ValidatorPublicKey {
            self.consensus.crypto.validator_pubkey().clone()
        }

        pub(crate) fn is_joining(&self) -> bool {
            matches!(self.consensus.bft_round_state.state_tag, StateTag::Joining)
        }

        pub fn setup_for_round(
            nodes: &mut [&mut ConsensusTestCtx],
            leader: usize,
            slot: u64,
            view: u64,
        ) {
            let leader_pubkey = nodes
                .get(leader)
                .unwrap()
                .consensus
                .crypto
                .validator_pubkey()
                .clone();

            // TODO: write a real one?
            let commit_qc = AggregateSignature::default();

            for (index, node) in nodes.iter_mut().enumerate() {
                node.consensus.bft_round_state.consensus_proposal.slot = slot;
                node.consensus.bft_round_state.consensus_proposal.view = view;

                node.consensus
                    .bft_round_state
                    .follower
                    .buffered_quorum_certificate = Some(commit_qc.clone());

                if index == leader {
                    node.consensus.bft_round_state.state_tag = StateTag::Leader;
                    node.consensus.bft_round_state.leader.pending_ticket =
                        Some(Ticket::CommitQC(commit_qc.clone()));
                } else {
                    node.consensus.bft_round_state.state_tag = StateTag::Follower;
                }

                node.consensus
                    .bft_round_state
                    .consensus_proposal
                    .round_leader = leader_pubkey.clone();
            }
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

        pub(crate) async fn handle_node_state_event(&mut self, msg: NodeStateEvent) -> Result<()> {
            self.consensus.handle_node_state_event(msg).await
        }

        async fn add_staker(&mut self, staker: &Self, amount: u128, err: &str) {
            info!("➕ {} Add staker: {:?}", self.name, staker.name);
            self.consensus
                .handle_node_state_event(NodeStateEvent::NewBlock(Box::new(Block {
                    staking_actions: vec![
                        (staker.name.clone().into(), StakingAction::Stake { amount }),
                        (
                            staker.name.clone().into(),
                            StakingAction::Delegate {
                                validator: staker.pubkey(),
                            },
                        ),
                    ],
                    ..Default::default()
                })))
                .await
                .expect(err);
        }

        async fn add_bonded_staker(&mut self, staker: &Self, amount: u128, err: &str) {
            self.add_staker(staker, amount, err).await;
            self.consensus
                .handle_node_state_event(NodeStateEvent::NewBlock(Box::new(Block {
                    new_bounded_validators: vec![staker.pubkey()],
                    ..Default::default()
                })))
                .await
                .expect(err)
        }

        async fn with_stake(&mut self, amount: u128, err: &str) {
            self.consensus
                .handle_node_state_event(NodeStateEvent::NewBlock(Box::new(Block {
                    staking_actions: vec![
                        (self.name.clone().into(), StakingAction::Stake { amount }),
                        (
                            self.name.clone().into(),
                            StakingAction::Delegate {
                                validator: self.consensus.crypto.validator_pubkey().clone(),
                            },
                        ),
                    ],
                    ..Default::default()
                })))
                .await
                .expect(err)
        }

        pub async fn start_round(&mut self) {
            self.consensus
                .start_round(get_current_timestamp_ms())
                .await
                .expect("Failed to start slot");
        }

        pub async fn start_round_at(&mut self, current_timestamp: u64) {
            self.consensus
                .start_round(current_timestamp)
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
        assert_eq!(
            cp1.parent_hash,
            ConsensusProposalHash("genesis".to_string())
        );
        assert_eq!(ticket1, Ticket::Genesis);

        // Slot 1 - leader = node2
        node2.start_round().await;

        let (cp2, ticket2) = simple_commit_round! {
            leader: node2,
            followers: [node1]
        };

        assert_eq!(cp2.slot, 2);
        assert_eq!(cp2.view, 0);
        assert_eq!(cp2.parent_hash, cp1.hashed());
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

        let cp_round_hash = &cp_round.unwrap().hashed();

        send! {
            description: "Follower - PrepareVote",
            from: [
                node2; ConsensusNetMessage::PrepareVote(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); },
                node3; ConsensusNetMessage::PrepareVote(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); },
                node4; ConsensusNetMessage::PrepareVote(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); }
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
                node2; ConsensusNetMessage::ConfirmAck(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); },
                node3; ConsensusNetMessage::ConfirmAck(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); },
                node4; ConsensusNetMessage::ConfirmAck(cp_hash) => { assert_eq!(cp_hash, cp_round_hash); }
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
                    timestamp: 123,
                    round_leader: node1.pubkey(),
                    cut: vec![(
                        node2.pubkey(),
                        DataProposalHash("test".to_string()),
                        LaneBytesSize::default(),
                        AggregateSignature::default(),
                    )],
                    staking_actions: vec![],
                    parent_hash: ConsensusProposalHash("hash".into()),
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
                    timestamp: 123,
                    cut: vec![(
                        node2.pubkey(),
                        DataProposalHash("test".to_string()),
                        LaneBytesSize::default(),
                        AggregateSignature::default(),
                    )],
                    staking_actions: vec![],
                    parent_hash: ConsensusProposalHash("hash".into()),
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
                    timestamp: 123,
                    round_leader: node3.pubkey(),
                    cut: vec![(
                        node2.pubkey(),
                        DataProposalHash("test".to_string()),
                        LaneBytesSize::default(),
                        AggregateSignature::default(),
                    )],
                    staking_actions: vec![],
                    parent_hash: ConsensusProposalHash("hash".into()),
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
    async fn prepare_wrong_timestamp_too_old() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round_at(1000).await;

        let (cp, _) = simple_commit_round! {
            leader: node1,
            followers: [node2, node3, node4]
        };

        assert_eq!(cp.timestamp, 1000);

        node2.start_round_at(900).await;

        broadcast! {
            description: "Leader Node2 second round",
            from: node2, to: [],
            message_matches: ConsensusNetMessage::Prepare(next_cp, next_ticket) => {

                assert_eq!(next_cp.timestamp, 900);

                let prepare_msg = node2
                    .consensus
                    .sign_net_message(ConsensusNetMessage::Prepare(next_cp.clone(), next_ticket.clone()))
                    .unwrap();

                assert_contains!(
                    format!("{:#}", node1.handle_msg_err(&prepare_msg)),
                    "too old"
                );
                assert_contains!(
                    format!("{:#}", node3.handle_msg_err(&prepare_msg)),
                    "too old"
                );
                assert_contains!(
                    format!("{:#}", node4.handle_msg_err(&prepare_msg)),
                    "too old"
                );
            }
        };
    }

    #[ignore]
    #[test_log::test(tokio::test)]
    async fn prepare_valid_timestamp() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round_at(1000).await;

        let (cp, _) = simple_commit_round! {
            leader: node1,
            followers: [node2, node3, node4]
        };

        assert_eq!(cp.timestamp, 1000);

        node2.start_round_at(3000).await;

        assert_eq!(
            node1.consensus.bft_round_state.consensus_proposal.timestamp,
            1000
        );
        assert_eq!(
            node3.consensus.bft_round_state.consensus_proposal.timestamp,
            1000
        );
        assert_eq!(
            node4.consensus.bft_round_state.consensus_proposal.timestamp,
            1000
        );

        broadcast! {
            description: "Leader Node2 second round",
            from: node2, to: [node1, node3, node4],
            message_matches: ConsensusNetMessage::Prepare(next_cp, _) => {
                assert_eq!(next_cp.timestamp, 3000);
            }
        };

        assert_eq!(
            node1.consensus.bft_round_state.consensus_proposal.timestamp,
            3000
        );
        assert_eq!(
            node3.consensus.bft_round_state.consensus_proposal.timestamp,
            3000
        );
        assert_eq!(
            node4.consensus.bft_round_state.consensus_proposal.timestamp,
            3000
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

        // Broadcasted prepare is ignored
        node1.assert_broadcast("Lost prepare");

        // Make node2 and node3 timeout, node4 will not timeout but follow mutiny
        // , because at f+1, mutiny join
        ConsensusTestCtx::timeout(&mut [&mut node2, &mut node3]).await;

        broadcast! {
            description: "Follower - Timeout",
            from: node2, to: [node3, node4],
            message_matches: ConsensusNetMessage::Timeout(..)
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node3, to: [node2, node4],
            message_matches: ConsensusNetMessage::Timeout(..)
        };

        // node 4 should join the mutiny
        broadcast! {
            description: "Follower - Timeout",
            from: node4, to: [node2, node3],
            message_matches: ConsensusNetMessage::Timeout(..)
        };

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
        assert_eq!(cp.parent_hash, ConsensusProposalHash("genesis".into()));
    }

    #[test_log::test(tokio::test)]
    async fn test_timeout_join_mutiny_leader_4() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round().await;
        // Slot 1 - leader = node1

        // Broadcasted prepare is ignored
        node1.assert_broadcast("Lost prepare");

        ConsensusTestCtx::timeout(&mut [&mut node3]).await;

        broadcast! {
            description: "Follower - Timeout",
            from: node3, to: [node1, node4],
            message_matches: ConsensusNetMessage::Timeout(..)
        };

        ConsensusTestCtx::timeout(&mut [&mut node2]).await;

        broadcast! {
            description: "Follower - Timeout",
            from: node2, to: [node1, node3],
            message_matches: ConsensusNetMessage::Timeout(..)
        };

        // node 1:leader should join the mutiny
        broadcast! {
            description: "Follower - Timeout",
            from: node1, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Timeout(..)
        };

        // After this broadcast, every node has 2f+1 timeouts and can create a timeout certificate

        // Node 2 is next leader, but has not yet a timeout certificate
        node2.assert_no_broadcast("Timeout Certificate 2");

        broadcast! {
            description: "Leader - timeout certificate",
            from: node1, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::TimeoutCertificate(_, _, _)
        };

        // Node2 will use node1's timeout certificate

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
        assert_eq!(cp.parent_hash, ConsensusProposalHash("genesis".into()));
    }

    #[test_log::test(tokio::test)]
    async fn test_timeout_join_mutiny_when_triggering_timeout_4() {
        let (mut node1, _node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round().await;
        // Slot 1 - leader = node1

        // Broadcasted prepare is ignored
        node1.assert_broadcast("Lost prepare");

        ConsensusTestCtx::timeout(&mut [&mut node3]).await;

        broadcast! {
            description: "Follower - Timeout",
            from: node3, to: [node4],
            message_matches: ConsensusNetMessage::Timeout(slot, view) => {
                assert_eq!(slot, &1);
                assert_eq!(view, &0);
            }
        };

        ConsensusTestCtx::timeout(&mut [&mut node4]).await;

        node4.assert_broadcast("Timeout Message 4");
        node4.assert_no_broadcast("Timeout Certificate 4");
    }

    #[test_log::test(tokio::test)]
    async fn test_timeout_next_leader_build_and_use_its_timeout_certificate() {
        let (mut node1, mut node2, mut node3, mut node4): (
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
            ConsensusTestCtx,
        ) = build_nodes!(4).await;

        node1.start_round().await;
        // Slot 1 - leader = node1

        // Broadcasted prepare is ignored
        node1.assert_broadcast("Lost prepare");

        ConsensusTestCtx::timeout(&mut [&mut node3, &mut node4]).await;

        broadcast! {
            description: "Follower - Timeout",
            from: node3, to: [node1, node2],
            message_matches: ConsensusNetMessage::Timeout(slot, view) => {
                assert_eq!(slot, &1);
                assert_eq!(view, &0);
            }
        };

        // Only node1 current leader receives the timeout message from node4
        // Since it already received a timeout from node3, it enters the mutiny

        broadcast! {
            description: "Follower - Timeout",
            from: node4, to: [node1],
            message_matches: ConsensusNetMessage::Timeout(slot, view) => {
                assert_eq!(slot, &1);
                assert_eq!(view, &0);
            }
        };

        // Node 1 joined the mutiny, and sends its timeout to node2 (next leader) which already has one timeout from node3

        broadcast! {
            description: "Follower - Timeout",
            from: node1, to: [node2],
            message_matches: ConsensusNetMessage::Timeout(slot, view) => {
                assert_eq!(slot, &1);
                assert_eq!(view, &0);
            }
        };

        // Now node2 has 2 timeouts, so it joined the mutiny, and since at 4 nodes joining mutiny == timeout certificate, it is ready for round 2

        // Node 2 is next leader, and does not emits a timeout certificate since it will use it for its next Prepare
        node2.assert_broadcast("Timeout Message 2");
        node2.assert_no_broadcast("Timeout Certificate 2");

        node2.start_round().await;

        // Slot 2 view 1 (following a timeout round)
        let (cp, ticket) = simple_commit_round! {
          leader: node2,
          followers: [node1, node3, node4]
        };

        assert!(matches!(ticket, Ticket::TimeoutQC(_)));
        assert_eq!(cp.slot, 1);
        assert_eq!(cp.view, 1);
        assert_eq!(cp.parent_hash, ConsensusProposalHash("genesis".into()));
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
            message_matches: ConsensusNetMessage::Timeout(..)
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node3, to: [node2, node4, node5],
            message_matches: ConsensusNetMessage::Timeout(..)
        };

        // node 4 should join the mutiny
        broadcast! {
            description: "Follower - Timeout",
            from: node4, to: [node2, node3, node5],
            message_matches: ConsensusNetMessage::Timeout(slot, view) => {
                assert_eq!(slot, &1);
                assert_eq!(view, &0);
            }
        };

        // By receiving this, other nodes should not produce another timeout certificate
        broadcast! {
            description: "Follower - Timeout",
            from: node5, to: [node2, node3, node4],
            message_matches: ConsensusNetMessage::Timeout(..)
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
        assert_eq!(cp.parent_hash, ConsensusProposalHash("genesis".into()));
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
            message_matches: ConsensusNetMessage::Timeout(slot, view) => {
                assert_eq!(slot, &1);
                assert_eq!(view, &0);
            }
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node4, to: [node3, node5, node6, node7],
            message_matches: ConsensusNetMessage::Timeout(..)
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node5, to: [node3, node4, node6, node7],
            message_matches: ConsensusNetMessage::Timeout(..)
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node6, to: [node3, node4, node5, node7],
            message_matches: ConsensusNetMessage::Timeout(..)
        };
        broadcast! {
            description: "Follower - Timeout",
            from: node7, to: [node3, node4, node5, node6],
            message_matches: ConsensusNetMessage::Timeout(..)
        };

        // Send node5 timeout certificate to node2
        broadcast! {
            description: "Follower - Timeout Certificate to next leader",
            from: node5, to: [node2],
            message_matches: ConsensusNetMessage::TimeoutCertificate(_, slot, view) => {
                if let ConsensusNetMessage::Prepare(cp, ticket) = lost_prepare {
                    assert_eq!(&cp.slot, slot);
                    assert_eq!(&cp.view, view);
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
        assert_eq!(cp.parent_hash, ConsensusProposalHash("genesis".into()));
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

        // Slot 5: Slave 2 joined consensus, leader = node-3
        {
            info!("➡️  Leader proposal");
            node3.start_round().await;

            let (cp, _) = simple_commit_round! {
                leader: node3,
                followers: [node1, node2]
            };
            assert_eq!(cp.slot, 5);
            assert_eq!(node2.consensus.bft_round_state.staking.bonded().len(), 3);
        }

        assert_eq!(node1.consensus.bft_round_state.consensus_proposal.slot, 6);
        assert_eq!(node2.consensus.bft_round_state.consensus_proposal.slot, 6);
        assert_eq!(node3.consensus.bft_round_state.consensus_proposal.slot, 6);
    }

    bus_client! {
        struct TestBC {
            sender(Query<QueryConsensusInfo, ConsensusInfo>),
            sender(NodeStateEvent),
        }
    }

    #[test_log::test(tokio::test)]
    async fn test_consensus_starts_after_genesis_is_processed() {
        let mut node_builder = NodeIntegrationCtxBuilder::new().await;
        node_builder.conf.run_indexer = false;
        node_builder.conf.single_node = Some(false);
        node_builder = node_builder.skip::<NodeStateModule>().skip::<RestApi>();
        let mut node = node_builder.build().await.unwrap();

        let mut bc = TestBC::new_from_bus(node.bus.new_handle()).await;

        node.wait_for_genesis_event().await.unwrap();

        // Check that we haven't started the consensus yet
        // (this is awkward to do for now so assume that not receiving an answer is OK)
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            _ = bc.request(QueryConsensusInfo {}) => {
                panic!("Consensus should not have started yet");
            }
        }

        bc.send(NodeStateEvent::NewBlock(Box::default())).unwrap();

        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                panic!("Consensus should have started");
            }
            _ = bc.request(QueryConsensusInfo {}) => {
            }
        }
    }
}
