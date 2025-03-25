use std::collections::HashSet;

use borsh::{BorshDeserialize, BorshSerialize};
use hyle_model::{
    AggregateSignature, ConsensusProposal, ConsensusProposalHash, Hashed, Signed, TCKind,
};
use tracing::{debug, info, trace, warn};

use super::Consensus;
use crate::{
    consensus::StateTag,
    model::{
        utils::get_current_timestamp, ConsensusNetMessage, QuorumCertificate, SignedByValidator,
        Slot, Ticket, ValidatorPublicKey, View,
    },
};
use anyhow::{bail, Context, Result};

#[derive(Debug, BorshSerialize, BorshDeserialize, Default)]
pub(super) enum TimeoutState {
    #[default]
    // Initial state
    Inactive,
    // A new slot was created, and its (timeout) is scheduled
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
                trace!("⏲️ Scheduling timeout");
            }
            TimeoutState::CertificateEmitted => {
                trace!("⏲️ Rescheduling timeout after a certificate was emitted");
            }
            TimeoutState::Scheduled { .. } => {
                trace!("⏲️ Rescheduling timeout");
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

#[derive(BorshSerialize, BorshDeserialize, Default)]
pub(super) struct TimeoutRoleState {
    pub(super) requests: HashSet<SignedByValidator<ConsensusNetMessage>>,
    pub(super) state: TimeoutState,
    pub(super) highest_seen_prepare_qc: Option<(Slot, View, QuorumCertificate)>,
}

impl TimeoutRoleState {
    pub(super) fn update_highest_seen_prepare_qc(
        &mut self,
        slot: Slot,
        view: View,
        qc: QuorumCertificate,
    ) {
        if let Some((s, v, _)) = &self.highest_seen_prepare_qc {
            if slot <= *s || (slot == *s && view <= *v) {
                return;
            }
            self.highest_seen_prepare_qc = Some((slot, view, qc));
        }
    }
}

pub(super) trait TimeoutRole {
    fn on_timeout_certificate(
        &mut self,
        received_timeout_certificate: &QuorumCertificate,
        received_proposal_qc: &TCKind,
        received_slot: Slot,
        received_view: View,
    ) -> Result<()>;
    fn on_timeout(
        &mut self,
        received_msg: SignedByValidator<ConsensusNetMessage>,
        received_slot_view: SignedByValidator<(Slot, View, ConsensusProposalHash)>,
        received_cp: Option<(QuorumCertificate, ConsensusProposal)>,
    ) -> Result<()>;
}

impl TimeoutRole for Consensus {
    fn on_timeout_certificate(
        &mut self,
        received_timeout_certificate: &QuorumCertificate,
        received_proposal_qc: &TCKind,
        received_slot: Slot,
        received_view: View,
    ) -> Result<()> {
        if received_slot < self.bft_round_state.slot
            || received_slot == self.bft_round_state.slot
                && received_view < self.bft_round_state.view
        {
            debug!(
                "🌘 Ignoring timeout certificate for slot {}, am at {}",
                received_slot, self.bft_round_state.slot
            );
            return Ok(());
        }
        if received_slot > self.bft_round_state.slot || received_view > self.bft_round_state.view {
            bail!(
                "Timeout Certificate (Slot: {}, view: {}) does not match expected (Slot: {}, view: {})",
                received_slot,
                received_view,
                self.bft_round_state.slot,
                self.bft_round_state.view,
            );
        }

        info!(
            "Process quorum certificate {:?}",
            received_timeout_certificate
        );

        self.verify_quorum_certificate(
            ConsensusNetMessage::Timeout(received_slot, received_view),
            received_timeout_certificate,
        )
        .context(format!(
            "Verifying timeout certificate for (slot: {}, view: {})",
            self.bft_round_state.slot, self.bft_round_state.view
        ))?;

        // This TC is for our current slot and view, so we can leave Joining mode
        let is_next_view_leader = &self.next_view_leader()? != self.crypto.validator_pubkey();
        if is_next_view_leader && matches!(self.bft_round_state.state_tag, StateTag::Joining) {
            self.bft_round_state.state_tag = StateTag::Leader;
        }

        self.carry_on_with_ticket(Ticket::TimeoutQC(received_timeout_certificate.clone()))
    }

    fn on_timeout(
        &mut self,
        received_msg: SignedByValidator<ConsensusNetMessage>,
        received_slot_view: SignedByValidator<(Slot, View, ConsensusProposalHash)>,
        received_cp: Option<(QuorumCertificate, ConsensusProposal)>,
    ) -> Result<()> {
        // Only timeout if it is in consensus
        if !self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            info!(
                "Received timeout message while not being part of the consensus: {}",
                self.crypto.validator_pubkey()
            );
            return Ok(());
        }

        let Signed::<_, _> {
            msg: (received_slot, received_view, received_parent_hash),
            ..
        } = &received_slot_view;

        if received_parent_hash != &self.bft_round_state.parent_hash {
            debug!(
                "🌘 Ignoring timeout with incorrect parent hash {}, expected {}",
                received_parent_hash, self.bft_round_state.parent_hash
            );
            return Ok(());
        }
        if received_slot < &self.bft_round_state.slot {
            debug!(
                "🌘 Ignoring timeout for slot {}, am at {}",
                received_slot, self.bft_round_state.slot
            );
            return Ok(());
        }

        if received_slot != &self.bft_round_state.slot
            || received_view != &self.bft_round_state.view
        {
            bail!(
                "Timeout (Slot: {}, view: {}) does not match expected (Slot: {}, view: {})",
                received_slot,
                received_view,
                self.bft_round_state.slot,
                self.bft_round_state.view,
            );
        }

        // Insert timeout request and if already present notify
        if !self
            .store
            .bft_round_state
            .timeout
            .requests
            .insert(received_msg.clone())
        {
            // self.metrics.timeout_request("already_processed");
            info!("Timeout has already been processed");
            return Ok(());
        }

        // If there is a prepareQC along with this message, verify it (we can, it's the same slot), and then potentially update our highest seen PrepareQC
        if let Some((qc, cp)) = &received_cp {
            if cp.slot == *received_slot {
                self.verify_quorum_certificate(ConsensusNetMessage::PrepareVote(cp.hashed()), &qc)
                    .context("Verifying PrepareQC")?;
                self.store
                    .bft_round_state
                    .timeout
                    .update_highest_seen_prepare_qc(*received_slot, *received_view, qc.clone());
                debug!("Highest seen PrepareQC updated");
            }
        }

        let f = self.bft_round_state.staking.compute_f();

        let timeout_validators = self
            .store
            .bft_round_state
            .timeout
            .requests
            .iter()
            .map(|signed_message| signed_message.signature.validator.clone())
            .collect::<Vec<ValidatorPublicKey>>();

        let mut len = timeout_validators.len();

        let mut voting_power = self
            .bft_round_state
            .staking
            .compute_voting_power(&timeout_validators);

        info!("Got {voting_power} voting power with {len} timeout requests for the same view {}. f is {f}", self.store.bft_round_state.view);

        // Count requests and if f+1 requests, and not already part of it, join the mutiny
        if voting_power > f && !timeout_validators.contains(self.crypto.validator_pubkey()) {
            info!("Joining timeout mutiny!");

            let timeout_message =
                ConsensusNetMessage::Timeout(received_slot_view.clone(), received_cp.clone());

            self.store
                .bft_round_state
                .timeout
                .requests
                .insert(self.sign_net_message(timeout_message.clone())?);

            // Broadcast a timeout message
            self.broadcast_net_message(timeout_message)
                .context(format!(
                    "Sending timeout message for slot:{} view:{}",
                    self.bft_round_state.slot, self.bft_round_state.view,
                ))?;

            len += 1;
            voting_power += self.get_own_voting_power();

            self.bft_round_state
                .timeout
                .state
                .schedule_next(get_current_timestamp());
        }

        // Create TC if applicable
        if voting_power > 2 * f
            && !matches!(
                self.bft_round_state.timeout.state,
                TimeoutState::CertificateEmitted
            )
        {
            debug!("⏲️ ⏲️ Creating a timeout certificate with {len} timeout requests and {voting_power} voting power");

            let ticket = (|| -> Result<(AggregateSignature, TCKind)> {
                {
                    if let Some((s, v, qc)) = &self.bft_round_state.timeout.highest_seen_prepare_qc
                    {
                        if s == received_slot && v == received_view {
                            let signatures: Vec<_> = self
                                .bft_round_state
                                .timeout
                                .requests
                                .iter()
                                .map(|s| {let ConsensusNetMessage::Timeout(signed_slot_view, ..) = &s.msg else {
                                    unreachable!("All messages in the timeout set should be Timeout messages")
                                };
                                signed_slot_view
                            }
                            )
                                .collect();
                            return Result::Ok((self.crypto.sign_aggregate(
                                received_slot_view.msg.clone(),
                                signatures.as_slice(),
                            )?.signature, TCKind::PrepareQC(qc.clone())));
                        }
                    }
                    let signatures: Vec<_> = self.bft_round_state.timeout.requests.iter().collect();
                    Result::Ok((
                        self.crypto
                            .sign_aggregate(received_msg.msg, signatures.as_slice())?
                            .signature,
                        TCKind::NilProposal,
                    ))
                }
            })()
            .context("Creating Timeout Certificate")?;

            self.bft_round_state
                .timeout
                .state
                .schedule_next(get_current_timestamp());

            let round_leader = self.next_view_leader()?;
            if &round_leader == self.crypto.validator_pubkey() {
                // This TC is for our current slot and view (by construction), so we can leave Joining mode
                if matches!(self.bft_round_state.state_tag, StateTag::Joining) {
                    self.bft_round_state.state_tag = StateTag::Leader;
                }
                self.carry_on_with_ticket(Ticket::TimeoutQC(ticket.0, ticket.1))?;
            } else {
                // Broadcast the Timeout Certificate to all validators
                self.broadcast_net_message(ConsensusNetMessage::TimeoutCertificate(
                    ticket.0,
                    ticket.1,
                    *received_slot,
                    *received_view,
                ))?;
                self.bft_round_state.timeout.state.certificate_emitted();
            }
        }

        Ok(())
    }
}
