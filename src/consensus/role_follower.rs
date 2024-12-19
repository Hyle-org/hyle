use std::collections::HashSet;

use bincode::{Decode, Encode};
use tracing::{debug, info, warn};

use super::{
    Consensus, ConsensusNetMessage, ConsensusProposal, ConsensusProposalHash, QuorumCertificate,
    Ticket,
};
use crate::{
    consensus::StateTag,
    mempool::MempoolNetMessage,
    model::{get_current_timestamp, Hashable, ValidatorPublicKey},
    p2p::network::{Signed, SignedByValidator},
    utils::{
        crypto::{AggregateSignature, BlstCrypto},
        logger::LogMe,
    },
};
use anyhow::{bail, Context, Result};

#[derive(Debug, Encode, Decode, Default)]
pub(super) enum TimeoutState {
    #[default]
    // Initial state
    Inactive,
    // A new slot was created, and its timeout is scheduled
    Scheduled {
        timestamp: u64,
    },
    CertificateEmitted,
}

impl TimeoutState {
    pub const TIMEOUT_SECS: u64 = 5;
    pub fn schedule_next(&mut self, timestamp: u64) {
        match self {
            TimeoutState::Inactive => {
                info!("⏲️ Scheduling timeout");
            }
            TimeoutState::CertificateEmitted => {
                info!("⏲️ Rescheduling timeout after a certificate was emitted");
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
            TimeoutState::CertificateEmitted => {
                info!("⏲️ Cancelling timeout after it was emitted");
            }
            TimeoutState::Scheduled { timestamp } => {
                info!("⏲️ Cancelling timeout set to trigger to {}", timestamp);
            }
            TimeoutState::Inactive => {
                info!("⏲️ Cancelling inactive timeout");
            }
        }
        *self = TimeoutState::Inactive;
    }

    pub fn certificate_emitted(&mut self) {
        match self {
            TimeoutState::CertificateEmitted => {
                warn!("⏲️ Try to emit a certificate after it was already emitted");
            }
            TimeoutState::Scheduled { timestamp } => {
                warn!(
                    "⏲️ Mark TimeoutCertificate as emitted while scheduled {}",
                    timestamp
                );
            }
            TimeoutState::Inactive => {
                info!("⏲️ Mark TimeoutCertificate as emitted");
            }
        }
        *self = TimeoutState::CertificateEmitted;
    }
}

#[derive(Encode, Decode, Default)]
pub(super) struct FollowerState {
    pub(super) timeout_requests: HashSet<SignedByValidator<ConsensusNetMessage>>,
    pub(super) timeout_state: TimeoutState,
    pub(super) buffered_quorum_certificate: Option<QuorumCertificate>, // if we receive a commit before the next prepare
}

pub(super) trait FollowerRole {
    fn on_prepare(
        &mut self,
        sender: ValidatorPublicKey,
        consensus_proposal: ConsensusProposal,
        ticket: Ticket,
    ) -> Result<()>;
    fn on_confirm(&mut self, prepare_quorum_certificate: QuorumCertificate) -> Result<()>;
    fn on_commit(
        &mut self,
        commit_quorum_certificate: QuorumCertificate,
        proposal_hash_hint: ConsensusProposalHash,
    ) -> Result<()>;

    fn on_timeout_certificate(
        &mut self,
        received_consensus_proposal_hash: &ConsensusProposalHash,
        received_timeout_certificate: &AggregateSignature,
    ) -> Result<()>;
    fn on_timeout(
        &mut self,
        received_msg: SignedByValidator<ConsensusNetMessage>,
        received_consensus_proposal_hash: ConsensusProposalHash,
        next_leader: ValidatorPublicKey,
    ) -> Result<()>;
    fn verify_poda(&mut self, consensus_proposal: &ConsensusProposal) -> Result<()>;
    fn verify_timestamp(&self, consensus_proposal: &ConsensusProposal) -> Result<()>;
}

impl FollowerRole for Consensus {
    fn on_prepare(
        &mut self,
        sender: ValidatorPublicKey,
        consensus_proposal: ConsensusProposal,
        ticket: Ticket,
    ) -> Result<()> {
        debug!("Received Prepare message: {}", consensus_proposal);

        if matches!(self.bft_round_state.state_tag, StateTag::Joining) {
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

        self.verify_poda(&consensus_proposal)?;

        self.verify_new_validators_to_bond(&consensus_proposal)?;

        self.verify_timestamp(&consensus_proposal)?;

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
            info!(
                "😥 Not part of consensus ({}), not sending PrepareVote",
                self.crypto.validator_pubkey()
            );
        }

        self.metrics.prepare();

        Ok(())
    }

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

        let f = self.bft_round_state.staking.compute_f();

        let timeout_validators = self
            .store
            .bft_round_state
            .follower
            .timeout_requests
            .iter()
            .map(|signed_message| signed_message.signature.validator.clone())
            .collect::<Vec<ValidatorPublicKey>>();

        let mut len = timeout_validators.len();

        let mut voting_power = self
            .bft_round_state
            .staking
            .compute_voting_power(&timeout_validators);

        info!("Got {voting_power} voting power with {len} timeout requests for the same view {}. f is {f}", self.store.bft_round_state.consensus_proposal.view);

        // Count requests and if f+1 requests, and not already part of it, join the mutiny
        if voting_power > f && !timeout_validators.contains(self.crypto.validator_pubkey()) {
            info!("Joining timeout mutiny!");

            let timeout_message = ConsensusNetMessage::Timeout(
                received_consensus_proposal_hash.clone(),
                next_leader.clone(),
            );

            self.store
                .bft_round_state
                .follower
                .timeout_requests
                .insert(self.sign_net_message(timeout_message.clone())?);

            // Broadcast a timeout message
            self.broadcast_net_message(timeout_message)
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
        if voting_power > 2 * f
            && !matches!(
                self.bft_round_state.follower.timeout_state,
                TimeoutState::CertificateEmitted
            )
        {
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

            self.bft_round_state
                .follower
                .timeout_state
                .schedule_next(get_current_timestamp());

            if &self.next_leader()? == self.crypto.validator_pubkey() {
                self.carry_on_with_ticket(Ticket::TimeoutQC(timeout_certificate))?;
            } else {
                // Broadcast the Timeout Certificate to all validators
                self.broadcast_net_message(ConsensusNetMessage::TimeoutCertificate(
                    timeout_certificate.clone(),
                    received_consensus_proposal_hash.clone(),
                ))?;
                self.bft_round_state
                    .follower
                    .timeout_state
                    .certificate_emitted();
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

    fn verify_timestamp(
        &self,
        ConsensusProposal { timestamp, .. }: &ConsensusProposal,
    ) -> Result<()> {
        let previous_timestamp = self.bft_round_state.consensus_proposal.timestamp;

        if previous_timestamp == 0 {
            warn!(
                "Previous timestamp is zero, accepting {} as next",
                timestamp
            );
            return Ok(());
        }

        let next_max_timestamp = previous_timestamp + (2 * self.config.consensus.slot_duration);

        if &previous_timestamp > timestamp {
            bail!(
                "Timestamp {} too old (should be > {}, {} ms too old)",
                timestamp,
                previous_timestamp,
                previous_timestamp - timestamp
            );
        }

        if &next_max_timestamp < timestamp {
            dbg!(self.bft_round_state.consensus_proposal.clone());
            bail!(
                "Timestamp {} too late (should be < {}, exceeded by {} ms)",
                timestamp,
                next_max_timestamp,
                timestamp - next_max_timestamp
            );
        }

        info!(
            "Consensus Proposal Timestamp verification ok {} -> {} ({} ms between the two rounds)",
            previous_timestamp,
            timestamp,
            timestamp - previous_timestamp
        );

        Ok(())
    }
}
