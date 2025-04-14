use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use hyle_crypto::BlstCrypto;
use tracing::{debug, info, trace, warn};

use super::Consensus;
use crate::{
    bus::BusClientSender,
    consensus::StateTag,
    log_error,
    mempool::MempoolNetMessage,
    model::{Hashed, Signed, ValidatorPublicKey},
    p2p::P2PCommand,
};
use anyhow::{bail, Context, Result};
use hyle_model::{
    utils::TimestampMs, ConsensusNetMessage, ConsensusProposal, ConsensusProposalHash,
    ConsensusStakingAction, Cut, LaneBytesSize, LaneId, NewValidatorCandidate, QuorumCertificate,
    TCKind, Ticket, ValidatorCandidacy, View,
};

#[derive(BorshSerialize, BorshDeserialize, Default)]
pub(super) struct FollowerState {
    pub(super) buffered_quorum_certificate: Option<QuorumCertificate>, // if we receive a commit before the next prepare
    pub(super) buffered_prepares: BufferedPrepares, // History of seen prepares & buffer of future prepares
}

impl Consensus {
    pub(super) fn on_prepare(
        &mut self,
        sender: ValidatorPublicKey,
        consensus_proposal: ConsensusProposal,
        ticket: Ticket,
        view: View,
    ) -> Result<()> {
        debug!(
            sender = %sender,
            slot = %self.bft_round_state.slot,
            "Received Prepare message: {}", consensus_proposal
        );

        if matches!(self.bft_round_state.state_tag, StateTag::Joining) {
            // Shortcut - if this is the prepare we expected, exit joining mode.
            // Three cases: the next slot, the next view, or our current slot/view.
            let is_next_prepare = view == 0
                && consensus_proposal.slot == self.bft_round_state.slot + 1
                || view == self.bft_round_state.view + 1
                    && consensus_proposal.slot == self.bft_round_state.slot
                || consensus_proposal.slot == self.bft_round_state.slot
                    && view == self.bft_round_state.view;
            if is_next_prepare {
                info!(
                    "Received Prepare message for next slot while joining. Exiting joining mode."
                );
                self.bft_round_state.state_tag = StateTag::Follower;
            } else {
                self.follower_state().buffered_prepares.push((
                    sender.clone(),
                    consensus_proposal,
                    ticket,
                    view,
                ));
                return Ok(());
            }
        }

        // If received proposal for next slot, we continue processing and will try to fast forward
        // if received proposal is for an even further slot, we buffer it
        if consensus_proposal.slot > self.bft_round_state.slot + 1 {
            warn!(
                proposal_hash = %consensus_proposal.hashed(),
                sender = %sender,
                "🚚 Prepare message for slot {} while at slot {}. Buffering.",
                consensus_proposal.slot, self.bft_round_state.slot
            );
            return self.buffer_prepare_message_and_fetch_missing_parent(
                sender,
                consensus_proposal,
                ticket,
                view,
            );
        } else if consensus_proposal.slot < self.bft_round_state.slot {
            // Ignore outdated messages.
            info!(
                "🌑 Outdated Prepare message (Slot {} / view {} while at {}) received. Ignoring.",
                consensus_proposal.slot, view, self.bft_round_state.slot
            );
            return Ok(());
        }
        // Process the ticket
        match &ticket {
            Ticket::Genesis => {
                if self.bft_round_state.slot != 1 {
                    bail!("Genesis ticket is only valid for the first slot.");
                }
            }
            Ticket::CommitQC(commit_qc) => {
                if !self.verify_commit_ticket_or_fast_forward(commit_qc.clone()) {
                    bail!("Invalid commit ticket");
                }
            }
            Ticket::TimeoutQC(timeout_qc, tc_kind_data) => {
                self.try_process_timeout_qc(
                    timeout_qc.clone(),
                    tc_kind_data,
                    &consensus_proposal,
                    view,
                )
                .context("Processing Timeout ticket")?;
            }
            els => {
                bail!("Invalid TimedOutCommit ticket here {:?}", els);
            }
        }

        // TODO: check we haven't voted for a proposal this slot/view already.

        // Sanity check: after processing the ticket, we should be in the right slot/view.
        // TODO: these checks are almost entirely redundant at this point because we process the ticket above.
        if consensus_proposal.slot != self.bft_round_state.slot {
            self.metrics.prepare_error("wrong_slot");
            bail!("Prepare message received for wrong slot");
        }
        if view != self.bft_round_state.view {
            self.metrics.prepare_error("wrong_view");
            bail!("Prepare message received for wrong view");
        }

        // Validate message comes from the correct leader
        // (can't do this earlier as might need to process the ticket first)
        let round_leader = self.round_leader()?;
        if sender != round_leader {
            self.metrics.prepare_error("wrong_leader");
            bail!(
                "Prepare consensus message for {} {} does not come from current leader {}. I won't vote for it.",
                self.bft_round_state.slot, self.bft_round_state.view, round_leader
            );
        }

        self.verify_poda(&consensus_proposal)?;

        self.verify_staking_actions(&consensus_proposal)?;

        self.verify_timestamp(&consensus_proposal)?;

        // At this point we are OK with this new consensus proposal, update locally and vote.
        self.bft_round_state.current_proposal = consensus_proposal.clone();
        self.bft_round_state.last_cut_seen = consensus_proposal.cut.clone();
        let cp_hash = self.bft_round_state.current_proposal.hashed();

        self.follower_state().buffered_prepares.push((
            sender.clone(),
            consensus_proposal,
            ticket,
            view,
        ));

        // If we already have the next Prepare, fast-forward
        if let Some(prepare) = self
            .follower_state()
            .buffered_prepares
            .next_prepare(cp_hash.clone())
        {
            debug!("🏎️ Fast forwarding to next Prepare");
            // FIXME? In theory, we could have a stackoverflow if we need to catchup a lot of prepares
            // Note: If we want to vote on the passed proposal even if it's too late,
            // we can just remove the "return" here and continue.
            return self.on_prepare(prepare.0, prepare.1, prepare.2, 0);
        }

        // Responds PrepareVote message to leader with validator's vote on this proposal
        if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            debug!(
                proposal_hash = %cp_hash,
                sender = %sender,
                "📤 Slot {} Prepare message validated. Sending PrepareVote to leader",
                self.bft_round_state.slot
            );
            self.send_net_message(round_leader, ConsensusNetMessage::PrepareVote(cp_hash))?;
        } else {
            info!(
                "😥 Not part of consensus ({}), not sending PrepareVote",
                self.crypto.validator_pubkey()
            );
        }

        self.metrics.prepare();

        Ok(())
    }

    pub(super) fn on_confirm(
        &mut self,
        sender: ValidatorPublicKey,
        prepare_quorum_certificate: QuorumCertificate,
        proposal_hash_hint: ConsensusProposalHash,
    ) -> Result<()> {
        match self.bft_round_state.state_tag {
            StateTag::Follower => {}
            StateTag::Joining => {
                return Ok(());
            }
            _ => {
                debug!(
                    sender = %sender,
                    proposal_hash = %proposal_hash_hint,
                    "Confirm message received while not follower. Ignoring."
                );
                return Ok(());
            }
        }

        if proposal_hash_hint != self.bft_round_state.current_proposal.hashed() {
            warn!(
                proposal_hash = %proposal_hash_hint,
                sender = %sender,
                "Confirm message received for wrong proposal. Ignoring."
            );
            // Note: We might want to buffer it to be able to vote on it if we catchup the proposal
            return Ok(());
        }

        // Check that this is a QC for PrepareVote for the expected proposal.
        // This also checks slot/view as those are part of the hash.
        // TODO: would probably be good to make that more explicit.
        self.verify_quorum_certificate(
            ConsensusNetMessage::PrepareVote(proposal_hash_hint.clone()),
            &prepare_quorum_certificate,
        )?;

        let slot = self.bft_round_state.slot;
        self.bft_round_state
            .timeout
            .update_highest_seen_prepare_qc(slot, prepare_quorum_certificate.clone());

        // Responds ConfirmAck to leader
        if self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            debug!(
                proposal_hash = %proposal_hash_hint,
                sender = %sender,
                "📤 Slot {} Confirm message validated. Sending ConfirmAck to leader",
                self.bft_round_state.current_proposal.slot
            );
            self.send_net_message(
                self.round_leader()?,
                ConsensusNetMessage::ConfirmAck(proposal_hash_hint),
            )?;
        } else {
            info!("😥 Not part of consensus, not sending ConfirmAck");
        }
        Ok(())
    }

    pub(super) fn on_commit(
        &mut self,
        sender: ValidatorPublicKey,
        commit_quorum_certificate: QuorumCertificate,
        proposal_hash_hint: ConsensusProposalHash,
    ) -> Result<()> {
        match self.bft_round_state.state_tag {
            StateTag::Follower => {
                self.try_commit_current_proposal(commit_quorum_certificate, proposal_hash_hint)
            }
            StateTag::Joining => {
                self.on_commit_while_joining(commit_quorum_certificate, proposal_hash_hint)
            }
            _ => {
                debug!(
                    sender = %sender,
                    proposal_hash = %proposal_hash_hint,
                    "Commit message received while not follower. Ignoring."
                );
                Ok(())
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
                .last_cut_seen
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

    pub(super) fn verify_timestamp(
        &self,
        ConsensusProposal { timestamp, .. }: &ConsensusProposal,
    ) -> Result<()> {
        let previous_timestamp = self.bft_round_state.current_proposal.timestamp.clone();

        if previous_timestamp == TimestampMs::ZERO {
            warn!(
                "Previous timestamp is zero, accepting {} as next",
                timestamp
            );
            return Ok(());
        }

        let next_max_timestamp =
            previous_timestamp.clone() + (2 * self.config.consensus.slot_duration);

        if &previous_timestamp > timestamp {
            bail!(
                "Timestamp {} too old (should be > {}, {} ms too old)",
                timestamp,
                previous_timestamp,
                (previous_timestamp.clone() - timestamp.clone()).as_millis()
            );
        }

        if &next_max_timestamp < timestamp {
            warn!(
                "Timestamp {} too late (should be < {}, exceeded by {} ms)",
                timestamp,
                next_max_timestamp,
                (timestamp.clone() - next_max_timestamp.clone()).as_millis()
            );
        }

        trace!(
            "Consensus Proposal Timestamp verification ok {} -> {} ({} ms between the two rounds)",
            previous_timestamp,
            timestamp,
            (timestamp.clone() - previous_timestamp.clone()).as_millis()
        );

        Ok(())
    }
}

impl Consensus {
    fn try_process_timeout_qc(
        &mut self,
        timeout_qc: QuorumCertificate,
        tc_kind_data: &TCKind,
        consensus_proposal: &ConsensusProposal,
        prepare_view: View,
    ) -> Result<()> {
        let prepare_slot = consensus_proposal.slot;
        debug!(
            "Trying to process timeout Certificate against consensus proposal slot: {}, view: {}",
            prepare_slot, prepare_view,
        );

        // Check the ticket matches the CP
        if let TCKind::PrepareQC((_, cp)) = tc_kind_data {
            if cp != consensus_proposal {
                bail!(
                    "Timeout Certificate does not match consensus proposal. Expected {}, got {}",
                    cp.hashed(),
                    consensus_proposal.hashed()
                );
            }
        }

        // If ticket for next slot && correct parent hash, fast forward
        if prepare_slot == self.bft_round_state.slot + 1
            && consensus_proposal.parent_hash == self.bft_round_state.current_proposal.hashed()
        {
            // Try to commit our current prepare & fast-forward.

            // Safety assumption: we can't actually verify a TC for the next slot, but since it matches our hash,
            // since we have no staking actions in the prepare we're good.
            if !self
                .bft_round_state
                .current_proposal
                .staking_actions
                .is_empty()
            {
                bail!("Timeout Certificate slot {} view {} is for the next slot, but we have staking actions in our prepare", prepare_slot, prepare_view);
            }

            // We have received a timeout certificate for the next slot,
            // and it matches our know prepare for this slot, so try and commit that one then the TC.
            self.carry_on_with_ticket(Ticket::ForcedCommitQc(timeout_qc.clone()))?;

            info!(
                "🔀 Fast forwarded to slot {} view 0",
                &self.bft_round_state.slot
            );
        }
        if prepare_slot != self.bft_round_state.slot {
            bail!(
                "Timeout Certificate slot {} view {} is not the current slot {}",
                prepare_slot,
                prepare_view,
                self.bft_round_state.slot
            );
        }
        self.verify_tc(
            &timeout_qc,
            tc_kind_data,
            self.bft_round_state.slot,
            prepare_view - 1,
        )?;
        if prepare_view == self.bft_round_state.view + 1 {
            // Process it
            debug!(
                "Timeout Certificate for next view {} received, processing it",
                prepare_view
            );
            self.carry_on_with_ticket(Ticket::TimeoutQC(timeout_qc, tc_kind_data.clone()))?;
        }
        Ok(())
    }

    fn on_commit_while_joining(
        &mut self,
        commit_quorum_certificate: QuorumCertificate,
        proposal_hash_hint: ConsensusProposalHash,
    ) -> Result<()> {
        // We are joining consensus, try to sync our state.
        let Some((_, potential_proposal, _, _)) = self
            .bft_round_state
            .follower
            .buffered_prepares
            .get(&proposal_hash_hint)
        else {
            // Maybe we just missed it, carry on.
            return Ok(());
        };

        // Check it's actually in the future
        if potential_proposal.slot <= self.bft_round_state.slot {
            info!(
                "🏃Ignoring commit message, we are at slot {}, already beyond {}",
                self.bft_round_state.slot, potential_proposal.slot
            );
            return Ok(());
        }

        // At this point check that we're caught up enough that it's realistic to verify the QC.
        if self.bft_round_state.joining.staking_updated_to + 1 < potential_proposal.slot {
            info!(
                "🏃Ignoring commit message, we are only caught up to {} ({} needed).",
                self.bft_round_state.joining.staking_updated_to,
                potential_proposal.slot - 1
            );
            return Ok(());
        }

        self.bft_round_state.current_proposal = potential_proposal.clone();

        info!(
            "📦 Commit message received for slot {}, trying to synchronize.",
            self.bft_round_state.current_proposal.slot
        );

        // Try to commit the proposal
        let old_slot = self.bft_round_state.slot;
        let old_view = self.bft_round_state.view;

        self.bft_round_state.slot = self.bft_round_state.current_proposal.slot;
        self.bft_round_state.view = 0; // TODO
        self.bft_round_state.state_tag = StateTag::Follower;
        if self
            .try_commit_current_proposal(
                commit_quorum_certificate,
                self.bft_round_state.current_proposal.hashed(),
            )
            .is_err()
        {
            // Swap back
            self.bft_round_state.state_tag = StateTag::Joining;
            self.bft_round_state.slot = old_slot;
            self.bft_round_state.view = old_view;
            bail!("⛑️ Failed to synchronize, retrying soon.");
        }
        // We sucessfully joined the consensus
        info!("🏁 Synchronized to slot {}", self.bft_round_state.slot);
        Ok(())
    }

    #[inline]
    pub(super) fn follower_state(&mut self) -> &mut FollowerState {
        &mut self.store.bft_round_state.follower
    }

    pub(super) fn has_no_buffered_children(&self) -> bool {
        self.store
            .bft_round_state
            .follower
            .buffered_prepares
            .next_prepare(self.bft_round_state.current_proposal.hashed())
            .is_none()
    }

    fn buffer_prepare_message_and_fetch_missing_parent(
        &mut self,
        sender: ValidatorPublicKey,
        consensus_proposal: ConsensusProposal,
        ticket: Ticket,
        view: View,
    ) -> Result<()> {
        if self
            .follower_state()
            .buffered_prepares
            .contains(&consensus_proposal.hashed())
        {
            // As we broadcast the SyncRequest, we will get a duplicated response
            // We could here check that all responses are identical
            return Ok(());
        }

        if !self
            .follower_state()
            .buffered_prepares
            .contains(&consensus_proposal.parent_hash)
        {
            debug!(
                proposal_hash = %consensus_proposal.hashed(),
                to = %sender,
                "🔉 Requesting missing parent proposal {}",
                consensus_proposal.parent_hash
            );
            // TODO: use send & retry in case of no response instead of broadcast
            // It's not supposed to occur often, so it's fine for now
            self.broadcast_net_message(ConsensusNetMessage::SyncRequest(
                consensus_proposal.parent_hash.clone(),
            ))
            .context("Sending SyncRequest")?;
        }

        let prepare_message = (sender, consensus_proposal, ticket, view);
        self.follower_state()
            .buffered_prepares
            .push(prepare_message);

        Ok(())
    }

    fn verify_commit_ticket_or_fast_forward(&mut self, commit_qc: QuorumCertificate) -> bool {
        // Three options:
        // - we have already received the commit message for this ticket, so we already processed the QC.
        // - we haven't, so we process it right away
        // - the CQC is invalid and we just ignore it.
        if let Some(qc) = &self.bft_round_state.follower.buffered_quorum_certificate {
            if qc == &commit_qc {
                return true;
            }
        }

        // Edge case: we have already committed a different CQC. We are kinda stuck.
        if self.bft_round_state.current_proposal.slot != self.bft_round_state.slot {
            warn!(
                "Received an unknown commit QC for slot {}. This is unsafe to verify as we have updated staking. Proceeding with current staking anyways.",
                self.bft_round_state.slot
            );
            // To still sorta make this work, verify the CQC with our current staking and hope for the best.
            return self
                .verify_quorum_certificate(
                    ConsensusNetMessage::ConfirmAck(self.bft_round_state.parent_hash.clone()),
                    &commit_qc,
                )
                .is_ok();
        }

        let commited_current_proposal = log_error!(
            self.try_commit_current_proposal(
                commit_qc,
                self.bft_round_state.current_proposal.hashed()
            ),
            "Processing Commit Ticket"
        );

        if commited_current_proposal.is_ok() {
            info!("🔀 Fast forwarded to slot {}", &self.bft_round_state.slot);
        }
        commited_current_proposal.is_ok()
    }

    fn verify_staking_actions(&mut self, proposal: &ConsensusProposal) -> Result<()> {
        for action in &proposal.staking_actions {
            match action {
                ConsensusStakingAction::Bond { candidate } => {
                    self.verify_new_validators_to_bond(candidate)?;
                }
                ConsensusStakingAction::PayFeesForDaDi {
                    lane_id,
                    cumul_size,
                } => Self::verify_dadi_fees(&proposal.cut, lane_id, cumul_size)?,
            }
        }
        Ok(())
    }

    /// Verify that the fees paid by the disseminator are correct
    fn verify_dadi_fees(cut: &Cut, lane_id: &LaneId, cumul_size: &LaneBytesSize) -> Result<()> {
        cut.iter()
            .find(|l| &l.0 == lane_id && &l.2 == cumul_size)
            .map(|_| ())
            .ok_or(anyhow::anyhow!(
                "Malformed PayFeesForDadi. Not found in cut: {lane_id}, {cumul_size}"
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
}

pub type Prepare = (ValidatorPublicKey, ConsensusProposal, Ticket, View);

#[derive(BorshSerialize, BorshDeserialize, Default, Debug)]
pub(super) struct BufferedPrepares {
    prepares: BTreeMap<ConsensusProposalHash, Prepare>,
    children: BTreeMap<ConsensusProposalHash, ConsensusProposalHash>,
}

impl BufferedPrepares {
    fn contains(&self, proposal_hash: &ConsensusProposalHash) -> bool {
        self.prepares.contains_key(proposal_hash)
    }

    pub(super) fn push(&mut self, prepare_message: Prepare) {
        trace!(
            proposal_hash = %prepare_message.1.hashed(),
            "Buffering Prepare message"
        );
        let proposal_hash = prepare_message.1.hashed();
        let parent_hash = prepare_message.1.parent_hash.clone();
        // If a children exists for the parent we update it
        // TODO: make this more trustless, if we receive a prepare from a byzantine validator
        // we might get stuck
        self.children
            .entry(parent_hash)
            .and_modify(|h| {
                *h = proposal_hash.clone();
            })
            .or_insert(proposal_hash.clone());
        self.prepares.insert(proposal_hash, prepare_message);
    }

    pub(super) fn get(&self, proposal_hash: &ConsensusProposalHash) -> Option<&Prepare> {
        self.prepares.get(proposal_hash)
    }

    fn next_prepare(&self, proposal_hash: ConsensusProposalHash) -> Option<Prepare> {
        self.children
            .get(&proposal_hash)
            .and_then(|children| self.prepares.get(children).cloned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::{ConsensusProposal, Ticket, ValidatorPublicKey};

    #[test]
    fn test_contains() {
        let mut buffered_prepares = BufferedPrepares::default();
        let proposal = ConsensusProposal::default();
        let proposal_hash = proposal.hashed();
        assert!(!buffered_prepares.contains(&proposal_hash));

        let prepare_message = (
            ValidatorPublicKey::default(),
            proposal,
            Ticket::CommitQC(QuorumCertificate::default()),
            0,
        );
        buffered_prepares.push(prepare_message);
        assert!(buffered_prepares.contains(&proposal_hash));
    }

    #[test]
    fn test_push_and_get() {
        let mut buffered_prepares = BufferedPrepares::default();
        let proposal = ConsensusProposal::default();
        let proposal_hash = proposal.hashed();

        let prepare_message = (
            ValidatorPublicKey::default(),
            proposal.clone(),
            Ticket::CommitQC(QuorumCertificate::default()),
            0,
        );
        buffered_prepares.push(prepare_message);

        let retrieved_message = buffered_prepares.get(&proposal_hash);
        assert!(retrieved_message.is_some());
        let retrieved_message = retrieved_message.unwrap();
        assert_eq!(retrieved_message.1, proposal);
    }
}
