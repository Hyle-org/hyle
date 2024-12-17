use std::collections::HashSet;

use crate::{
    bus::command_response::CmdRespClient,
    consensus::StateTag,
    mempool::QueryNewCut,
    model::{Hashable, ValidatorPublicKey},
    p2p::network::SignedByValidator,
};
use anyhow::{anyhow, bail, Result};
use bincode::{Decode, Encode};
use staking::state::MIN_STAKE;
use tracing::{debug, error, info, warn};

use super::{Consensus, ConsensusNetMessage, ConsensusProposalHash, Ticket};

#[derive(Encode, Decode, Default, Debug)]
pub enum Step {
    #[default]
    StartNewSlot,
    PrepareVote,
    ConfirmAck,
}

#[derive(Encode, Decode, Default)]
pub struct LeaderState {
    pub(super) step: Step,
    pub(super) prepare_votes: HashSet<SignedByValidator<ConsensusNetMessage>>,
    pub(super) confirm_ack: HashSet<SignedByValidator<ConsensusNetMessage>>,
    pub(super) pending_ticket: Option<Ticket>,
}

pub(crate) trait LeaderRole {
    fn is_round_leader(&self) -> bool;
    async fn start_round(&mut self) -> Result<()>;
    fn on_prepare_vote(
        &mut self,
        msg: SignedByValidator<ConsensusNetMessage>,
        consensus_proposal_hash: ConsensusProposalHash,
    ) -> Result<()>;
    fn on_confirm_ack(
        &mut self,
        msg: SignedByValidator<ConsensusNetMessage>,
        consensus_proposal_hash: ConsensusProposalHash,
    ) -> Result<()>;
}

impl LeaderRole for Consensus {
    async fn start_round(&mut self) -> Result<()> {
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
        match self
            .bus
            .request(QueryNewCut(self.bft_round_state.staking.clone()))
            .await
        {
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

        let votes_power = self
            .bft_round_state
            .staking
            .compute_voting_power(&validated_votes);
        let voting_power = votes_power + self.get_own_voting_power();

        self.metrics.prepare_votes_gauge(voting_power as u64); // TODO risky cast

        // Waits for at least n-f = 2f+1 matching PrepareVote messages
        let f = self.bft_round_state.staking.compute_f();

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

        let confirmed_power = self
            .bft_round_state
            .staking
            .compute_voting_power(&confirmed_ack_validators);
        let voting_power = confirmed_power + self.get_own_voting_power();

        let f = self.bft_round_state.staking.compute_f();

        info!(
            "✅ Slot {} confirmed acks: {} / {} ({} validators for a total bond = {})",
            self.bft_round_state.consensus_proposal.slot,
            voting_power,
            2 * f + 1,
            self.bft_round_state.staking.bonded().len(),
            self.bft_round_state.staking.total_bond()
        );

        self.metrics.confirmed_ack_gauge(voting_power as u64); // TODO risky cast

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
}
