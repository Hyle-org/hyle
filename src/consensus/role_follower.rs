use borsh::{BorshDeserialize, BorshSerialize};
use tracing::{debug, info, trace, warn};

use super::Consensus;
use crate::{
    consensus::StateTag,
    log_error,
    mempool::MempoolNetMessage,
    model::{Hashed, Signed, ValidatorPublicKey},
    utils::crypto::BlstCrypto,
};
use anyhow::{bail, Result};
use hyle_model::{
    ConsensusNetMessage, ConsensusProposal, ConsensusProposalHash, QuorumCertificate, Ticket,
};

#[derive(BorshSerialize, BorshDeserialize, Default)]
pub(super) struct FollowerState {
    pub(super) buffered_quorum_certificate: Option<QuorumCertificate>, // if we receive a commit before the next prepare
}

pub(super) trait FollowerRole {
    fn on_prepare(
        &mut self,
        sender: ValidatorPublicKey,
        consensus_proposal: ConsensusProposal,
        ticket: Ticket,
    ) -> Result<()>;
    fn on_confirm(
        &mut self,
        sender: ValidatorPublicKey,
        prepare_quorum_certificate: QuorumCertificate,
    ) -> Result<()>;
    fn on_commit(
        &mut self,
        sender: ValidatorPublicKey,
        commit_quorum_certificate: QuorumCertificate,
        proposal_hash_hint: ConsensusProposalHash,
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
        debug!(
            sender = %sender,
            "Received Prepare message: {}", consensus_proposal
        );

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
                if log_error!(
                    self.try_process_timeout_qc(timeout_qc),
                    "Processing Timeout ticket"
                )
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

        self.verify_staking_actions(&consensus_proposal)?;

        self.verify_timestamp(&consensus_proposal)?;

        // At this point we are OK with this new consensus proposal, update locally and vote.
        self.bft_round_state.consensus_proposal = consensus_proposal.clone();

        // Responds PrepareVote message to leader with validator's vote on this proposal
        if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            debug!(
                proposal_hash = %consensus_proposal.hashed(),
                sender = %sender,
                "📤 Slot {} Prepare message validated. Sending PrepareVote to leader",
                self.bft_round_state.consensus_proposal.slot
            );
            self.send_net_message(
                self.bft_round_state.consensus_proposal.round_leader.clone(),
                ConsensusNetMessage::PrepareVote(consensus_proposal.hashed()),
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

    fn on_confirm(
        &mut self,
        sender: ValidatorPublicKey,
        prepare_quorum_certificate: QuorumCertificate,
    ) -> Result<()> {
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
        let consensus_proposal_hash = self.bft_round_state.consensus_proposal.hashed();
        self.verify_quorum_certificate(
            ConsensusNetMessage::PrepareVote(consensus_proposal_hash.clone()),
            &prepare_quorum_certificate,
        )?;

        // Responds ConfirmAck to leader
        if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            debug!(
                proposal_hash = %consensus_proposal_hash,
                sender = %sender,
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
        sender: ValidatorPublicKey,
        commit_quorum_certificate: QuorumCertificate,
        proposal_hash_hint: ConsensusProposalHash,
    ) -> Result<()> {
        match self.bft_round_state.state_tag {
            StateTag::Follower => self.try_commit_current_proposal(commit_quorum_certificate),
            StateTag::Joining => {
                self.on_commit_while_joining(commit_quorum_certificate, proposal_hash_hint)
            }
            _ => {
                debug!(
                    sender = %sender,
                    proposal_hash = %proposal_hash_hint,
                    "Commit message received while not follower"
                );
                bail!("Commit message received while not follower")
            }
        }
    }

    /// Verifies that the proposed cut in the consensus proposal is valid.
    ///
    /// For the cut to be considered valid:
    /// - Each DataProposal associated with a validator must have received sufficient signatures.
    /// - The aggregated signatures for each DataProposal must be valid.
    fn verify_poda(&mut self, consensus_proposal: &ConsensusProposal) -> Result<()> {
        let f = self.bft_round_state.staking.compute_f();

        trace!(
            "verify poda with staking: {:#?}",
            self.bft_round_state.staking
        );

        for (lane_id, data_proposal_hash, lane_size, poda_sig) in &consensus_proposal.cut {
            let voting_power = self
                .bft_round_state
                .staking
                .compute_voting_power(poda_sig.validators.as_slice());

            // Check that this is a known lane.
            // TODO: this prevents ever deleting lane which may or may not be desirable.
            if !self.bft_round_state.staking.is_known(&lane_id.0) {
                bail!("Lane {} is in cut but is not a valid lane", lane_id);
            }

            // If this same data proposal was in the last cut, ignore.
            if self
                .bft_round_state
                .last_cut
                .iter()
                .any(|(v, h, _, _)| v == lane_id && h == data_proposal_hash)
            {
                debug!(
                    "DataProposal {} from lane {} was already in the last cut, not checking PoDA",
                    data_proposal_hash, lane_id
                );
                continue;
            }

            trace!("consensus_proposal: {:#?}", consensus_proposal);
            trace!("voting_power: {voting_power} < {f} + 1");

            // Verify that DataProposal received enough votes
            if voting_power < f + 1 {
                bail!("PoDA for lane {lane_id} does not have enough validators that signed his DataProposal");
            }

            // Verify that PoDA signature is valid
            let msg = MempoolNetMessage::DataVote(data_proposal_hash.clone(), *lane_size);
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
            warn!(
                "Timestamp {} too late (should be < {}, exceeded by {} ms)",
                timestamp,
                next_max_timestamp,
                timestamp - next_max_timestamp
            );
        }

        trace!(
            "Consensus Proposal Timestamp verification ok {} -> {} ({} ms between the two rounds)",
            previous_timestamp,
            timestamp,
            timestamp - previous_timestamp
        );

        Ok(())
    }
}
