use std::collections::HashSet;

use borsh::{BorshDeserialize, BorshSerialize};
use hyle_model::{Hashed, Signed, TCKind, TimeoutKind};
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
                trace!("⏲️ Cancelling timeout after it was emitted");
            }
            TimeoutState::Scheduled { timestamp } => {
                trace!("⏲️ Cancelling timeout set to trigger to {}", timestamp);
            }
            TimeoutState::Inactive => {
                trace!("⏲️ Cancelling inactive timeout");
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
                trace!("⏲️ Mark TimeoutCertificate as emitted");
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
    ) -> bool {
        if let Some((s, v, _)) = &self.highest_seen_prepare_qc {
            if slot <= *s || (slot == *s && view <= *v) {
                return false;
            }
            self.highest_seen_prepare_qc = Some((slot, view, qc));
            return true;
        }
        false
    }
}

impl Consensus {
    pub(super) fn verify_tc(
        &mut self,
        received_timeout_certificate: &QuorumCertificate,
        received_proposal_qc: &TCKind,
        received_slot: Slot,
        received_view: View,
    ) -> Result<()> {
        info!(
            "Verifying TC for {}/{}, kind: {:?}",
            received_slot, received_view, received_proposal_qc
        );

        // Two options
        match received_proposal_qc {
            TCKind::NilProposal => {
                // If this is a Nil timout certificate, then we should be receiving  2f+1 signatures of a full timeout message with nil proposal
                self.verify_quorum_certificate(
                    (
                        received_slot,
                        received_view,
                        self.bft_round_state.parent_hash.clone(),
                        (),
                    ),
                    received_timeout_certificate,
                )
                .context(format!(
                    "Verifying timeout certificate for (slot: {}, view: {})",
                    self.bft_round_state.slot, self.bft_round_state.view
                ))?;
            }
            TCKind::PrepareQC((qc, cp)) => {
                // This is a PQC timout certificate, check the 'limited' signature
                self.verify_quorum_certificate(
                    (
                        received_slot,
                        received_view,
                        self.bft_round_state.parent_hash.clone(),
                    ),
                    received_timeout_certificate,
                )
                .context(format!(
                    "Verifying timeout certificate for (slot: {}, view: {})",
                    self.bft_round_state.slot, self.bft_round_state.view
                ))?;
                // Then check the prepare quorum certificate
                self.verify_quorum_certificate(ConsensusNetMessage::PrepareVote(cp.hashed()), qc)
                    .context("Verifying PrepareQC")?;
                // Update prepare QC & local CP
                if self
                    .store
                    .bft_round_state
                    .timeout
                    .update_highest_seen_prepare_qc(received_slot, received_view, qc.clone())
                {
                    // Update our consensus proposal
                    self.bft_round_state.current_proposal = cp.clone();
                    debug!("Highest seen PrepareQC updated");
                }
            }
        }
        Ok(())
    }

    pub(super) fn on_timeout_certificate(
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
                "🌘 Ignoring timeout certificate for slot {} view {}, am at {} {}",
                received_slot, received_view, self.bft_round_state.slot, self.bft_round_state.view
            );
            return Ok(());
        }
        if received_slot > self.bft_round_state.slot || received_view > self.bft_round_state.view {
            debug!(
                "Timeout Certificate (Slot: {}, view: {}) does not match expected (Slot: {}, view: {})",
                received_slot,
                received_view,
                self.bft_round_state.slot,
                self.bft_round_state.view,
            );
            return Ok(());
        }

        self.verify_tc(
            received_timeout_certificate,
            received_proposal_qc,
            received_slot,
            received_view,
        )?;

        // This TC is for our current slot and view, so we can leave Joining mode
        let is_next_view_leader = &self.next_view_leader()? != self.crypto.validator_pubkey();
        if is_next_view_leader && matches!(self.bft_round_state.state_tag, StateTag::Joining) {
            self.bft_round_state.state_tag = StateTag::Leader;
        }

        self.carry_on_with_ticket(Ticket::TimeoutQC(
            received_timeout_certificate.clone(),
            received_proposal_qc.clone(),
        ))
    }

    pub(super) fn on_timeout_tick(&mut self) -> Result<()> {
        match &self.bft_round_state.timeout.state {
            TimeoutState::Scheduled { timestamp } if get_current_timestamp() >= *timestamp => {
                // Trigger state transition to mutiny
                info!(
                    "⏰ Trigger timeout for slot {} and view {}",
                    self.bft_round_state.slot, self.bft_round_state.view
                );
                let timeout_message = self.get_timeout_message()?;

                // TODO: we are signing twice here
                let signed_timeout_message = self
                    .sign_net_message(timeout_message.clone())
                    .context("Signing timeout message")?;

                self.on_timeout(signed_timeout_message)?;

                self.broadcast_net_message(timeout_message)?;

                self.bft_round_state.timeout.state.cancel();

                Ok(())
            }
            _ => Ok(()),
        }
    }

    pub(super) fn on_timeout(
        &mut self,
        received_msg: SignedByValidator<ConsensusNetMessage>,
    ) -> Result<()> {
        // Only timeout if it is in consensus
        if !self.is_part_of_consensus(self.crypto.validator_pubkey()) {
            info!(
                "Received timeout message while not being part of the consensus: {}",
                self.crypto.validator_pubkey()
            );
            return Ok(());
        }

        let (received_slot, received_view, received_parent_hash) = match &received_msg {
            Signed {
                msg: ConsensusNetMessage::Timeout(Signed { msg: (s, v, p), .. }, ..),
                ..
            } => (s, v, p),
            _ => {
                bail!("Received timeout message with unexpected format");
            }
        };

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
            info!(
                "Timeout (Slot: {}, view: {}) does not match expected (Slot: {}, view: {})",
                received_slot, received_view, self.bft_round_state.slot, self.bft_round_state.view,
            );
            return Ok(());
        }

        // If there is a prepareQC along with this message, verify it (we can, it's the same slot),
        // and then potentially update our highest seen PrepareQC.
        if let Some((qc, cp)) = match &received_msg {
            Signed {
                msg: ConsensusNetMessage::Timeout(_, TimeoutKind::PrepareQC((qc, cp))),
                ..
            } => Some((qc, cp)),
            _ => None,
        } {
            if cp.slot == *received_slot {
                self.verify_quorum_certificate(ConsensusNetMessage::PrepareVote(cp.hashed()), &qc)
                    .context("Verifying PrepareQC")?;
                if self
                    .store
                    .bft_round_state
                    .timeout
                    .update_highest_seen_prepare_qc(*received_slot, *received_view, qc.clone())
                {
                    // Update our consensus proposal
                    self.bft_round_state.current_proposal = cp.clone();
                    debug!("Highest seen PrepareQC updated");
                }
            } else {
                // We actually cannot process this, or we might end up thinking we have 2f+1 timeouts but not working.
                bail!(
                    "Received timeout message with prepareQC for slot {}, but we are at {}",
                    cp.slot,
                    received_slot
                );
            }
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

            let timeout_message = self.get_timeout_message()?;

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

            let ticket: Result<_, anyhow::Error> =
                match &self.bft_round_state.timeout.highest_seen_prepare_qc {
                    Some((s, v, qc)) if s == received_slot && v == received_view => {
                        // We have a prepare QC for this round, so let's send that.
                        // Aggregate a timeout message and send the prepareQC
                        let signed_messages: Vec<_> = self
                            .bft_round_state
                            .timeout
                            .requests
                            .iter()
                            .map(|msg| match &msg.msg {
                                ConsensusNetMessage::Timeout(signed_message, _) => {
                                    Ok(signed_message)
                                }
                                // Unreachable because this is a logic error in the earlier code
                                _ => bail!("All messages should be Timeout messages"),
                            })
                            .collect::<Result<_>>()?;
                        // TODO: check current proposal matches QC.
                        Result::Ok((
                            self.crypto
                                .sign_aggregate(
                                    (
                                        self.bft_round_state.slot,
                                        self.bft_round_state.view,
                                        self.bft_round_state.parent_hash.clone(),
                                    ),
                                    signed_messages.as_slice(),
                                )?
                                .signature,
                            TCKind::PrepareQC((
                                qc.clone(),
                                self.bft_round_state.current_proposal.clone(),
                            )),
                        ))
                    }
                    _ => {
                        // Simple case - we will aggregate a 'nil' certificate. We need 2f+1 NIL signed messages
                        let signed_nil_messages: Vec<_> = self
                            .bft_round_state
                            .timeout
                            .requests
                            .iter()
                            .map(|msg| match &msg.msg {
                                ConsensusNetMessage::Timeout(
                                    _,
                                    TimeoutKind::NilProposal(signed_nil_message),
                                ) => Ok(signed_nil_message),
                                _ => bail!("All messages should be Nil Timeout messages"),
                            })
                            .collect::<Result<_>>()?;
                        Result::Ok((
                            self.crypto
                                .sign_aggregate(
                                    (
                                        self.bft_round_state.slot,
                                        self.bft_round_state.view,
                                        self.bft_round_state.parent_hash.clone(),
                                        (),
                                    ),
                                    signed_nil_messages.as_slice(),
                                )?
                                .signature,
                            TCKind::NilProposal,
                        ))
                    }
                };
            let ticket = ticket.context("Creating Timeout Certificate")?;

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

    fn get_timeout_message(&self) -> Result<ConsensusNetMessage> {
        let signed_timeout_metadata = self.crypto.sign((
            self.bft_round_state.slot,
            self.bft_round_state.view,
            self.bft_round_state.parent_hash.clone(),
        ))?;
        Ok(
            match &self.bft_round_state.timeout.highest_seen_prepare_qc {
                Some((s, v, qc))
                    if s == &self.bft_round_state.slot && v == &self.bft_round_state.view =>
                {
                    // TODO: we should check that the CP we're sending matches the prepare.
                    ConsensusNetMessage::Timeout(
                        signed_timeout_metadata,
                        TimeoutKind::PrepareQC((
                            qc.clone(),
                            self.bft_round_state.current_proposal.clone(),
                        )),
                    )
                }
                _ => ConsensusNetMessage::Timeout(
                    signed_timeout_metadata,
                    TimeoutKind::NilProposal(self.crypto.sign((
                        self.bft_round_state.slot,
                        self.bft_round_state.view,
                        self.bft_round_state.parent_hash.clone(),
                        (),
                    ))?),
                ),
            },
        )
    }
}
