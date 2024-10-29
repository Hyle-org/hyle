//! Handles all consensus logic up to block commitment.

use anyhow::{anyhow, bail, Context, Error, Result};
use bincode::{Decode, Encode};
use metrics::ConsensusMetrics;
use serde::{Deserialize, Serialize};
use staking::{Stake, Staker, Staking, MIN_STAKE};
use std::{collections::HashSet, default::Default, path::PathBuf, time::Duration};
use tokio::{sync::broadcast, time::sleep};
use tracing::{debug, info, warn};

use crate::{
    bus::{bus_client, BusMessage, SharedMessageBus},
    handle_messages,
    mempool::{Cut, MempoolEvent},
    model::{BlockHeight, Hashable, ValidatorPublicKey},
    p2p::{
        network::{OutboundMessage, PeerEvent, Signature, Signed, SignedWithKey},
        P2PCommand,
    },
    utils::{
        conf::SharedConf,
        crypto::{BlstCrypto, SharedBlstCrypto},
        logger::LogMe,
        modules::Module,
        static_type_map::Pick,
    },
};

use strum_macros::IntoStaticStr;

pub mod metrics;
pub mod module;
pub mod staking;
pub mod utils;

#[derive(
    Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, IntoStaticStr,
)]
pub enum ConsensusNetMessage {
    Prepare(ConsensusProposal, Ticket),
    PrepareVote(ConsensusProposalHash),
    // TODO: these should probably just be "SignedWithQuorumCertificate" to be consistent
    Confirm(QuorumCertificate),
    ConfirmAck(ConsensusProposalHash),
    Commit(QuorumCertificate),
    ValidatorCandidacy(ValidatorCandidacy),
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

impl BusMessage for ConsensusCommand {}
impl BusMessage for ConsensusEvent {}
impl BusMessage for ConsensusNetMessage {}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ValidatorCandidacy {
    pubkey: ValidatorPublicKey,
    peer_address: String,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct TimeoutCertificate(Slot, View, QuorumCertificate);

// A Ticket is necessary to send a valid prepare
#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub enum Ticket {
    // Special value for the initial Cut
    Genesis,
    CommitQC(QuorumCertificate),
    TC(TimeoutCertificate),
}

// TODO: move struct to model.rs ?
#[derive(Encode, Decode, Default)]
pub struct BFTRoundState {
    consensus_proposal: ConsensusProposal,
    slot: Slot,
    view: View,
    staking: Staking,

    leader: LeaderState,
    follower: FollowerState,
    joining: JoiningState,
    state_tag: StateTag,
}

#[derive(Encode, Decode, Default, Debug)]
enum StateTag {
    #[default]
    Genesis,
    Joining,
    Leader,
    Follower,
}

#[derive(Encode, Decode, Default)]
pub struct LeaderState {
    step: Step,
    prepare_votes: HashSet<Signed<ConsensusNetMessage, ValidatorPublicKey>>,
    confirm_ack: HashSet<Signed<ConsensusNetMessage, ValidatorPublicKey>>,
    pending_ticket: Option<Ticket>,
}

#[derive(Encode, Decode, Default)]
pub struct FollowerState {
    round_leader: ValidatorPublicKey,
    buffered_quorum_certificate: Option<QuorumCertificate>, // if we receive a commit before the next prepare
}

#[derive(Encode, Decode, Default)]
pub struct JoiningState {
    staking_updated_to: Slot,
}

#[derive(Serialize, Deserialize, Encode, Decode, Default, Debug, Clone, PartialEq, Eq, Hash)]
pub struct QuorumCertificate {
    signature: Signature,
    validators: Vec<ValidatorPublicKey>,
}

pub type Slot = u64;
pub type View = u64;

#[derive(Debug, Clone, Default, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ConsensusProposal {
    slot: Slot,
    view: View,
    cut: Cut,
    new_validators_to_bond: Vec<NewValidatorCandidate>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Default)]
pub struct ConsensusProposalHash(Vec<u8>);
#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Default)]
pub struct QuorumCertificateHash(Vec<u8>);

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct NewValidatorCandidate {
    pubkey: ValidatorPublicKey, // TODO: possible optim: the pubkey is already present in the msg,
    msg: SignedWithKey<ConsensusNetMessage>,
}

#[derive(Encode, Decode, Default)]
pub struct ConsensusStore {
    bft_round_state: BFTRoundState,
    /// Validators that asked to be part of consensus
    validator_candidates: Vec<NewValidatorCandidate>,
    pending_cuts: Vec<Cut>,
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

bus_client! {
struct ConsensusBusClient {
    sender(OutboundMessage),
    sender(ConsensusEvent),
    sender(ConsensusCommand),
    sender(P2PCommand),
    receiver(ConsensusCommand),
    receiver(MempoolEvent),
    receiver(SignedWithKey<ConsensusNetMessage>),
    receiver(PeerEvent),
}
}

impl Consensus {
    /// Add a validator with the default stake to our consensus.
    /// This is trusted because it's out of the usual consensus process,
    /// either at genesis or when fast-forwarding.
    pub fn add_trusted_validator(
        &mut self,
        pubkey: &ValidatorPublicKey,
        stake: Stake,
    ) -> Result<()> {
        self.bft_round_state
            .staking
            .add_staker(Staker {
                pubkey: pubkey.clone(),
                stake,
            })
            .context("cannot add trusted staker")?;
        self.bft_round_state
            .staking
            .bond(pubkey.clone())
            .context("cannot bond trusted validator")?;
        info!("🎉 Trusted validator added: {}", pubkey);
        Ok(())
    }

    // Reset bft_round_state for the next round of consensus.
    fn finish_round(
        &mut self,
        commit: Option<(ConsensusProposal, QuorumCertificate)>,
    ) -> Result<(), Error> {
        match self.bft_round_state.state_tag {
            StateTag::Follower => {}
            StateTag::Leader => {}
            _ => bail!("Invalid state tag"),
        }

        let view = self.bft_round_state.view;
        let staking = std::mem::take(&mut self.bft_round_state.staking);

        let leader_index = staking
            .bonded()
            .iter()
            .position(|v| {
                v == match self.bft_round_state.state_tag {
                    StateTag::Leader => self.crypto.validator_pubkey(),
                    StateTag::Follower => &self.bft_round_state.follower.round_leader,
                    _ => unreachable!(),
                }
            })
            .context("Leader not found in validators")?;

        // Reset round state.
        self.bft_round_state = BFTRoundState::default();

        // Carry over existing staking
        self.bft_round_state.staking = staking;
        let validators = self.bft_round_state.staking.bonded();

        // Find out who the next leader will be.
        self.bft_round_state.follower.round_leader = validators
            .get((leader_index + 1) % validators.len())
            .context("No next leader found")?
            .clone();

        if self.bft_round_state.follower.round_leader == *self.crypto.validator_pubkey() {
            self.bft_round_state.state_tag = StateTag::Leader;
        } else {
            self.bft_round_state.state_tag = StateTag::Follower;
        }

        // If we finish the round via a committed proposal, update some state
        if let Some((committed_proposal, qc)) = commit {
            self.bft_round_state.slot = committed_proposal.slot + 1;
            self.bft_round_state.view = 0;
            self.bft_round_state.follower.buffered_quorum_certificate = Some(qc);
            // Any new validators are added to the consensus and removed from candidates.
            for new_v in &committed_proposal.new_validators_to_bond {
                warn!("🎉 New validator bonded: {}", new_v.pubkey);
                self.bft_round_state.staking.bond(new_v.pubkey.clone())?;
            }
        } else {
            self.bft_round_state.view = view + 1;
            self.bft_round_state.follower.buffered_quorum_certificate = None;
        }

        info!(
            "🥋 Ready for slot {}, view {}",
            self.bft_round_state.slot, self.bft_round_state.view
        );

        if self.is_round_leader() {
            info!("👑 I'm the new leader! 👑")
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
        {
            Ok(())
        }
    }

    fn start_round(&mut self) -> Result<(), Error> {
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
            self.bft_round_state.slot,
            self.bft_round_state.staking.bonded().len(),
            new_validators_to_bond.len()
        );

        // Creates ConsensusProposal
        let cut = self.next_cut().unwrap_or_default();

        self.bft_round_state.leader.step = Step::PrepareVote;

        // Start Consensus with following cut
        self.bft_round_state.consensus_proposal = ConsensusProposal {
            slot: self.bft_round_state.slot,
            view: self.bft_round_state.view,
            cut,
            new_validators_to_bond,
        };

        self.metrics.start_new_round("consensus_proposal");

        // Verifies that to-be-built block is large enough (?)

        // Broadcasts Prepare message to all validators
        debug!(
            proposal_hash = %self.bft_round_state.consensus_proposal.hash(),
            "🌐 Slot {} started. Broadcasting Prepare message", self.bft_round_state.slot,
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

    fn next_cut(&mut self) -> Option<Cut> {
        if self.pending_cuts.is_empty() {
            None
        } else {
            Some(self.pending_cuts.remove(0))
        }
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
        let expected_signed_message = SignedWithKey {
            msg: message,
            signature: quorum_certificate.signature.clone(),
            validators: quorum_certificate.validators.clone(),
        };

        match (
            BlstCrypto::verify(&expected_signed_message),
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
            self.bft_round_state.slot,
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
        self.broadcast_net_message(ConsensusNetMessage::ValidatorCandidacy(candidacy))?;
        Ok(())
    }

    fn handle_net_message(&mut self, msg: SignedWithKey<ConsensusNetMessage>) -> Result<(), Error> {
        if !BlstCrypto::verify(&msg)? {
            self.metrics.signature_error("prepare");
            bail!("Invalid signature for message {:?}", &msg);
        }

        // TODO: reduce cloning here.
        let SignedWithKey::<ConsensusNetMessage> {
            msg: net_message,
            validators,
            ..
        } = msg.clone();

        match net_message {
            ConsensusNetMessage::Prepare(consensus_proposal, ticket) => {
                self.on_prepare(validators, consensus_proposal, ticket)
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
            ConsensusNetMessage::Commit(commit_quorum_certificate) => {
                self.on_commit(commit_quorum_certificate)
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
        self.on_commit(commit_qc.clone()).is_ok()
    }

    /// Message received by follower after start_round
    fn on_prepare(
        &mut self,
        validators: Vec<ValidatorPublicKey>,
        consensus_proposal: ConsensusProposal,
        ticket: Ticket,
    ) -> Result<(), Error> {
        debug!("Received Prepare message: {}", consensus_proposal);

        match self.bft_round_state.state_tag {
            StateTag::Joining => {
                info!(
                    "🚪 Slot {} / view {} Prepare message received while joining. Storing proposal.",
                    consensus_proposal.slot, consensus_proposal.view
                );
                // TODO: we probably should process this differently,
                // as we are susceptible to rogue proposals not sent by the leader.
                self.bft_round_state.follower.round_leader = validators[0].clone();
                self.bft_round_state.slot = consensus_proposal.slot;
                self.bft_round_state.view = consensus_proposal.view;
                self.bft_round_state.consensus_proposal = consensus_proposal;
                return Ok(());
            }
            StateTag::Follower => {}
            _ => {
                bail!("Prepare message received while not follower");
            }
        }

        // Process the ticket
        match ticket {
            Ticket::Genesis => {
                if self.bft_round_state.slot != 0 {
                    bail!("Genesis ticket is only valid for the first slot.");
                }
            }
            Ticket::CommitQC(commit_qc) => {
                if !self.verify_commit_ticket(commit_qc) {
                    bail!("Invalid commit ticket");
                }
            }
            Ticket::TC(_timeout_certificate) => {
                todo!()
            }
        }

        // After processing the ticket, we should be in the right slot/view.

        if consensus_proposal.slot != self.bft_round_state.slot {
            self.metrics.prepare_error("wrong_slot");
            bail!("Prepare message received for wrong slot");
        }
        if consensus_proposal.view != self.bft_round_state.view {
            self.metrics.prepare_error("wrong_view");
            bail!("Prepare message received for wrong view");
        }

        // Validate message comes from the correct leader
        if !validators.contains(&self.bft_round_state.follower.round_leader) {
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
                self.bft_round_state.slot
            );
            self.send_net_message(
                self.bft_round_state.follower.round_leader.clone(),
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
        msg: SignedWithKey<ConsensusNetMessage>,
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
            .flat_map(|signed_message| signed_message.validators.clone())
            .collect::<Vec<ValidatorPublicKey>>();

        let votes_power = self.compute_voting_power(&validated_votes);
        let voting_power = votes_power + self.get_own_voting_power();

        self.metrics.prepare_votes_gauge(voting_power);

        // Waits for at least n-f = 2f+1 matching PrepareVote messages
        let f = self.compute_f();

        info!(
            "📩 Slot {} validated votes: {} / {} ({} validators for a total bond = {})",
            self.bft_round_state.slot,
            voting_power,
            2 * f + 1,
            self.bft_round_state.staking.bonded().len(),
            self.bft_round_state.staking.total_bond()
        );

        if voting_power > 2 * f {
            // Get all received signatures
            let aggregates: &Vec<&Signed<ConsensusNetMessage, ValidatorPublicKey>> =
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
                self.bft_round_state.slot
            );
            self.broadcast_net_message(ConsensusNetMessage::Confirm(QuorumCertificate {
                signature: prepvote_signed_aggregation.signature,
                validators: prepvote_signed_aggregation.validators,
            }))?;
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
                self.bft_round_state.slot
            );
            self.send_net_message(
                self.bft_round_state.follower.round_leader.clone(),
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
        msg: SignedWithKey<ConsensusNetMessage>,
        consensus_proposal_hash: ConsensusProposalHash,
    ) -> Result<()> {
        if !matches!(self.bft_round_state.state_tag, StateTag::Leader) {
            bail!("ConfirmAck received while not leader");
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
            .flat_map(|signed_message| signed_message.validators.clone())
            .collect::<Vec<ValidatorPublicKey>>();

        let confirmed_power = self.compute_voting_power(&confirmed_ack_validators);
        let voting_power = confirmed_power + self.get_own_voting_power();

        let f = self.compute_f();

        info!(
            "✅ Slot {} confirmed acks: {} / {} ({} validators for a total bond = {})",
            self.bft_round_state.slot,
            voting_power,
            2 * f + 1,
            self.bft_round_state.staking.bonded().len(),
            self.bft_round_state.staking.total_bond()
        );

        self.metrics.confirmed_ack_gauge(voting_power);

        if voting_power > 2 * f {
            // Get all signatures received and change ValidatorPublicKey for ValidatorPubKey
            let aggregates: &Vec<&Signed<ConsensusNetMessage, ValidatorPublicKey>> =
                &self.bft_round_state.leader.confirm_ack.iter().collect();

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

            // Broadcast the *Commit* Quorum Certificate to all validators
            self.broadcast_net_message(ConsensusNetMessage::Commit(
                commit_quorum_certificate.clone(),
            ))?;

            // Process the same locally.
            let consensus_proposal = std::mem::take(&mut self.bft_round_state.consensus_proposal);
            self.commit_proposal(consensus_proposal, commit_quorum_certificate)?;
        }
        // TODO(?): Update behaviour when having more ?
        Ok(())
    }

    /// Message received by follower.
    fn on_commit(&mut self, commit_quorum_certificate: QuorumCertificate) -> Result<()> {
        if matches!(self.bft_round_state.state_tag, StateTag::Joining) {
            if self.bft_round_state.joining.staking_updated_to + 1 >= self.bft_round_state.slot {
                info!(
                    "📦 Commit message received for slot {}, trying to synchronize.",
                    self.bft_round_state.slot
                );
                // Update the bonding state accordingly
                let mut to_unbond = vec![];
                for validator in self.bft_round_state.staking.bonded() {
                    if !commit_quorum_certificate.validators.contains(validator) {
                        to_unbond.push(validator.clone());
                    }
                }
                for validator in to_unbond {
                    self.bft_round_state.staking.unbond(&validator)?;
                }
                for validator in &commit_quorum_certificate.validators {
                    if !self.bft_round_state.staking.is_bonded(validator) {
                        self.bft_round_state.staking.bond(validator.clone())?;
                    }
                }
                self.bft_round_state.state_tag = StateTag::Follower;
                if self.on_commit(commit_quorum_certificate).is_err() {
                    self.bft_round_state.state_tag = StateTag::Joining;
                    bail!("⛑️ Failed to synchronize, retrying soon.");
                }
            } else {
                info!(
                    "🏃‍♀️ Ignoring commit message, we are only caught up to {} ({} needed).",
                    self.bft_round_state.joining.staking_updated_to,
                    self.bft_round_state.slot - 1
                );
            }
            return Ok(());
        }
        // Check that this is a QC for ConfirmAck for the expected proposal.
        // This also checks slot/view as those are part of the hash.
        // TODO: would probably be good to make that more explicit.
        self.verify_quorum_certificate(
            ConsensusNetMessage::ConfirmAck(self.bft_round_state.consensus_proposal.hash()),
            &commit_quorum_certificate,
        )?;

        let consensus_proposal = std::mem::take(&mut self.bft_round_state.consensus_proposal);
        self.commit_proposal(consensus_proposal, commit_quorum_certificate)
    }

    /// This assumes we've checked the CQC
    fn commit_proposal(
        &mut self,
        consensus_proposal: ConsensusProposal,
        commit_quorum_certificate: QuorumCertificate,
    ) -> Result<()> {
        self.metrics.commit();

        _ = self
            .bus
            .send(ConsensusEvent::CommitCut {
                // TODO: investigate if those are necessary here
                validators: self.bft_round_state.staking.bonded().clone(),
                cut: consensus_proposal.cut.clone(),
                new_bonded_validators: consensus_proposal
                    .new_validators_to_bond
                    .iter()
                    .map(|v| v.pubkey.clone())
                    .collect(),
            })
            .context("Failed to send ConsensusEvent::CommitCut on the bus");

        // Save added cut TODO: remove ? (data availability)
        if let Some(file) = &self.file {
            Self::save_on_disk(file.as_path(), &self.store)?;
        }

        info!("📈 Slot {} committed", &consensus_proposal.slot);

        // Prepare our state for the next round.
        self.finish_round(Some((
            consensus_proposal,
            commit_quorum_certificate.clone(),
        )))?;

        if self.is_round_leader() {
            // Setup our ticket for the next round
            // Send Prepare message to all validators
            self.delay_start_new_round(Ticket::CommitQC(commit_quorum_certificate))
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

    /// Message received by leader & follower.
    fn on_validator_candidacy(
        &mut self,
        msg: SignedWithKey<ConsensusNetMessage>,
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

    fn handle_command(&mut self, msg: ConsensusCommand) -> Result<()> {
        match msg {
            ConsensusCommand::SingleNodeBlockGeneration => {
                if let Some(cut) = self.next_cut() {
                    self.bus
                        .send(ConsensusEvent::CommitCut {
                            validators: vec![self.crypto.validator_pubkey().clone()],
                            new_bonded_validators: vec![self.crypto.validator_pubkey().clone()],
                            cut,
                        })
                        .expect("Failed to send ConsensusEvent::CommitCut msg on the bus");
                }
                Ok(())
            }
            ConsensusCommand::NewStaker(staker) => {
                match self.bft_round_state.state_tag {
                    StateTag::Genesis => {} // Ignore at genesis, we take a different path
                    _ => {
                        self.store.bft_round_state.staking.add_staker(staker)?;
                    }
                }
                Ok(())
            }
            ConsensusCommand::NewBonded(validator) => {
                match self.bft_round_state.state_tag {
                    StateTag::Joining => {
                        self.store.bft_round_state.staking.bond(validator)?;
                    }
                    _ => {
                        // Ignore in all other cases:
                        // - For genesis, we know all validators
                        // - For the rest, we are updating as part of consensus logic.
                    }
                }
                Ok(())
            }
            ConsensusCommand::ProcessedBlock(block_height) => {
                if matches!(self.bft_round_state.state_tag, StateTag::Joining)
                    && self.store.bft_round_state.joining.staking_updated_to < block_height.0
                {
                    info!(
                        "🚪 Slot {} Block message received while joining. Setting up for next round.",
                        block_height.0
                    );
                    self.store.bft_round_state.joining.staking_updated_to = block_height.0;
                }
                Ok(())
            }
            ConsensusCommand::StartNewSlot => {
                self.start_round()?;
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

    async fn handle_mempool_event(&mut self, msg: MempoolEvent) -> Result<()> {
        match msg {
            MempoolEvent::CommitBlock(..) => Ok(()),
            MempoolEvent::NewCut(cut) => {
                if let Some(last_cut) = self.pending_cuts.last() {
                    if last_cut == &cut {
                        return Ok(());
                    }
                }
                debug!("Received a new Cut");
                self.pending_cuts.push(cut);
                Ok(())
            }
        }
    }

    fn handle_peer_event(&mut self, msg: PeerEvent) -> Result<()> {
        match msg {
            PeerEvent::NewPeer { name, pubkey } => {
                debug!("New peer: {} ({})", &name, &pubkey);
                match self.bft_round_state.state_tag {
                    StateTag::Genesis => {}
                    StateTag::Joining => {}
                    _ => {
                        return Ok(());
                    }
                };
                let Some(stake) = self.config.consensus.genesis_stakers.get(&name) else {
                    return Ok(());
                };
                info!("New peer {} added to genesis: {}", &name, &pubkey);
                self.add_trusted_validator(&pubkey, Stake { amount: *stake })?;

                if matches!(self.bft_round_state.state_tag, StateTag::Joining) {
                    return Ok(());
                }

                if name == self.config.consensus.genesis_leader {
                    // Mild hack: set this directly in the right struct
                    info!("👑 Setting {}({}) as leader for genesis", &name, &pubkey);
                    self.bft_round_state.follower.round_leader = pubkey;
                }

                if self.bft_round_state.staking.bonded().len()
                    == self.config.consensus.genesis_stakers.len()
                {
                    // Genesis logic: we rely on all peers connecting with each other
                    // before the first round starts, or the validators list will mismatch.
                    // TODO: this is hacky & duplicates logic in the module building.
                    if self.config.consensus.genesis_leader == self.config.id {
                        self.bft_round_state.state_tag = StateTag::Leader;
                        info!("👑 Starting consensus as leader");
                        self.delay_start_new_round(Ticket::Genesis)?;
                    } else {
                        self.bft_round_state.state_tag = StateTag::Follower;
                        info!("👑 Starting consensus as follower");
                    }
                }
                Ok(())
            }
        }
    }

    pub fn start_master(&mut self, config: SharedConf) -> Result<()> {
        let interval = config.consensus.slot_duration;

        // hack to avoid another bus for a specific wip case
        let command_sender = Pick::<broadcast::Sender<ConsensusCommand>>::get(&self.bus).clone();
        if config.id == "single-node" {
            info!(
                "No peers configured, starting as master generating cuts every {} seconds",
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
                match self.handle_net_message(cmd) {
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
        debug!("🔏 Signing message: {}", msg);
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
        _p2p_receiver: Receiver<P2PCommand>,
        consensus: Consensus,
        name: String,
    }

    impl TestCtx {
        async fn new(name: &str, crypto: BlstCrypto) -> Self {
            let shared_bus = SharedMessageBus::new(BusMetrics::global("global".to_string()));
            let out_receiver = get_receiver::<OutboundMessage>(&shared_bus).await;
            let event_receiver = get_receiver::<ConsensusEvent>(&shared_bus).await;
            let p2p_receiver = get_receiver::<P2PCommand>(&shared_bus).await;
            let bus = ConsensusBusClient::new_from_bus(shared_bus.new_handle()).await;

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

        async fn build() -> (Self, Self) {
            let crypto = crypto::BlstCrypto::new("node-1".into());
            info!("node 1: {}", crypto.validator_pubkey());
            let c_other = crypto::BlstCrypto::new("node-2".into());
            info!("node 2: {}", c_other.validator_pubkey());
            let mut node1 = Self::new("node-1", crypto.clone()).await;
            let mut node2 = Self::new("node-2", c_other.clone()).await;

            _ = node1
                .consensus
                .add_trusted_validator(&crypto.validator_pubkey().clone(), Stake { amount: 100 });
            _ = node1
                .consensus
                .add_trusted_validator(&c_other.validator_pubkey().clone(), Stake { amount: 100 });
            _ = node2
                .consensus
                .add_trusted_validator(&crypto.validator_pubkey().clone(), Stake { amount: 100 });
            _ = node2
                .consensus
                .add_trusted_validator(&c_other.validator_pubkey().clone(), Stake { amount: 100 });

            node1.consensus.bft_round_state.state_tag = StateTag::Leader;
            node2.consensus.bft_round_state.state_tag = StateTag::Follower;
            node2.consensus.bft_round_state.follower.round_leader =
                crypto.validator_pubkey().clone();

            node1.consensus.bft_round_state.leader.pending_ticket = Some(Ticket::Genesis);

            (node1, node2)
        }

        async fn new_node(name: &str) -> Self {
            let crypto = crypto::BlstCrypto::new(name.into());
            Self::new(name, crypto.clone()).await
        }

        fn pubkey(&self) -> ValidatorPublicKey {
            self.consensus.crypto.validator_pubkey().clone()
        }

        #[track_caller]
        fn handle_msg(&mut self, msg: &SignedWithKey<ConsensusNetMessage>, err: &str) {
            debug!("📥 {} Handling message: {:?}", self.name, msg);
            self.consensus.handle_net_message(msg.clone()).expect(err);
        }

        #[track_caller]
        fn handle_msg_err(&mut self, msg: &SignedWithKey<ConsensusNetMessage>) -> Error {
            debug!("📥 {} Handling message expecting err: {:?}", self.name, msg);
            let err = self.consensus.handle_net_message(msg.clone()).unwrap_err();
            info!("Expected error: {:#}", err);
            err
        }

        #[cfg(test)]
        #[track_caller]
        fn handle_block(&mut self, msg: &SignedWithKey<ConsensusNetMessage>) {
            match &msg.msg {
                ConsensusNetMessage::Prepare(_, _) => {}
                _ => panic!("Block message is not a Prepare message"),
            }
        }

        #[track_caller]
        fn add_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            info!("➕ {} Add staker: {:?}", self.name, staker.name);
            self.consensus
                .handle_command(ConsensusCommand::NewStaker(Staker {
                    pubkey: staker.consensus.crypto.validator_pubkey().clone(),
                    stake: Stake { amount },
                }))
                .expect(err)
        }

        #[track_caller]
        fn add_bonded_staker(&mut self, staker: &Self, amount: u64, err: &str) {
            self.add_staker(staker, amount, err);
            self.consensus
                .handle_command(ConsensusCommand::NewBonded(staker.pubkey()))
                .expect(err);
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

        #[track_caller]
        fn start_round(&mut self) {
            self.consensus.start_round().expect("Failed to start slot");
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

        node1.start_round();
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
        node2.start_round();
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
        node1.start_round();
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
    async fn test_candidacy() {
        let (mut node1, mut node2) = TestCtx::build().await;

        // Slot 0
        {
            node1.start_round();
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
        node3.consensus.bft_round_state.state_tag = StateTag::Follower;
        node3.consensus.bft_round_state.follower.round_leader = node1.pubkey();
        node3.consensus.bft_round_state.slot = 1;
        node3.add_bonded_staker(&node1, 100, "Add staker");
        node3.add_bonded_staker(&node2, 100, "Add staker");

        // Slot 1: New slave candidates - leader = node2
        {
            info!("➡️  Leader proposal");
            node2.start_round();
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

            info!("➡️  Slave 2 candidacy");
            node3.with_stake(100, "Add stake");
            // This should trigger send_candidacy as we now have stake.
            node3.handle_msg(&leader_commit, "Leader commit");
            let slave2_candidacy = node3.assert_broadcast("Slave 2 candidacy");
            assert_contains!(
                node2.handle_msg_err(&slave2_candidacy).to_string(),
                "validator is not staking"
            );
            node1.add_staker(&node3, 100, "Add staker");
            node1.handle_msg(&slave2_candidacy, "Slave 2 candidacy");
            node2.add_staker(&node3, 100, "Add staker");
            node2.handle_msg(&slave2_candidacy, "Slave 2 candidacy");

            info!("➡️  Handle block");
            node1.handle_block(&leader_proposal);
            node2.handle_block(&leader_proposal);
            node3.handle_block(&leader_proposal);
        }

        // Slot 2: Still a slot without slave 2 - leader = node 1
        {
            info!("➡️  Leader proposal - Slot 2");
            node1.start_round();
            let leader_proposal = node1.assert_broadcast("Leader proposal");
            if let ConsensusNetMessage::Prepare(_, _) = &leader_proposal.msg {
                assert_eq!(node1.consensus.bft_round_state.staking.bonded().len(), 2);
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

            info!("➡️  Handle block");
            node1.handle_block(&leader_proposal);
            node2.handle_block(&leader_proposal);
            node3.handle_block(&leader_proposal);
        }

        // Slot 3: Slave 2 joined consensus, leader = node-2
        {
            info!("➡️  Leader proposal");
            node2.start_round();
            let leader_proposal = node2.assert_broadcast("Leader proposal");
            if let ConsensusNetMessage::Prepare(_, _) = &leader_proposal.msg {
                assert_eq!(node1.consensus.bft_round_state.staking.bonded().len(), 3);
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
            info!("➡️  Handle block");
            node1.handle_block(&leader_proposal);
            node2.handle_block(&leader_proposal);
            node3.handle_block(&leader_proposal);
        }

        assert_eq!(node1.consensus.bft_round_state.slot, 4);
        assert_eq!(node2.consensus.bft_round_state.slot, 4);
        assert_eq!(node3.consensus.bft_round_state.slot, 4);
    }
}
