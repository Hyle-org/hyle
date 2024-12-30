use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use strum_macros::IntoStaticStr;

use super::crypto::{AggregateSignature, SignedByValidator};
use super::mempool::Cut;
use staking::model::ValidatorPublicKey;

pub type Slot = u64;
pub type View = u64;

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct ConsensusInfo {
    pub slot: Slot,
    pub view: View,
    pub round_leader: ValidatorPublicKey,
    pub validators: Vec<ValidatorPublicKey>,
}

// -----------------------------
// --- Consensus data model ----
// -----------------------------

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ValidatorCandidacy {
    pub pubkey: ValidatorPublicKey,
    pub peer_address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct NewValidatorCandidate {
    pub pubkey: ValidatorPublicKey, // TODO: possible optim: the pubkey is already present in the msg,
    pub msg: SignedByValidator<ConsensusNetMessage>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Default)]
pub struct QuorumCertificateHash(pub Vec<u8>);

pub type QuorumCertificate = AggregateSignature;

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

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Default)]
pub struct ConsensusProposalHash(pub Vec<u8>);

#[derive(Debug, Clone, Default, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct ConsensusProposal {
    // These first few items are checked when receiving the proposal from the leader.
    pub slot: Slot,
    pub view: View,
    pub round_leader: ValidatorPublicKey,
    // Below items aren't.
    pub cut: Cut,
    pub new_validators_to_bond: Vec<NewValidatorCandidate>,
    pub timestamp: u64,
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
