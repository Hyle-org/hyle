use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use strum_macros::IntoStaticStr;

use super::crypto::{AggregateSignature, SignedByValidator};
use super::mempool::Cut;
use super::Hashable;
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

#[derive(Debug, Clone, Serialize, Deserialize, Encode, Decode, PartialEq, Eq)]
pub struct NewValidatorCandidate {
    pub pubkey: ValidatorPublicKey, // TODO: possible optim: the pubkey is already present in the msg,
    pub msg: SignedByValidator<ConsensusNetMessage>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Default)]
pub struct QuorumCertificateHash(pub Vec<u8>);

pub type QuorumCertificate = AggregateSignature;

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq)]
pub struct TimeoutCertificate(ConsensusProposalHash, QuorumCertificate);

// A Ticket is necessary to send a valid prepare
#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub enum Ticket {
    // Special value for the initial Cut, needed because we don't have a quorum certificate for the genesis block.
    Genesis,
    CommitQC(QuorumCertificate),
    TimeoutQC(QuorumCertificate),
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Default)]
pub struct ConsensusProposalHash(pub String);

#[derive(Debug, Clone, Default, Serialize, Deserialize, Encode, Decode, PartialEq, Eq)]
pub struct ConsensusProposal {
    // These first few items are checked when receiving the proposal from the leader.
    pub slot: Slot,
    pub view: View,
    // This field is necessary for joining the network.
    // Without it, new joiners cannot safely know who the leader is
    // as we cannot verify that the sender of the Prepare message is indeed the leader.
    // Additionally, this information is not present in any other message.
    pub round_leader: ValidatorPublicKey,
    // Below items aren't.
    pub cut: Cut,
    pub new_validators_to_bond: Vec<NewValidatorCandidate>,
    pub timestamp: u64,
    pub parent_hash: ConsensusProposalHash,
}

/// This is the hash of the proposal, signed by validators
/// Any consensus-critical data should be hashed here.
impl Hashable<ConsensusProposalHash> for ConsensusProposal {
    fn hash(&self) -> ConsensusProposalHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.slot.to_le_bytes());
        hasher.update(self.view.to_le_bytes());
        hasher.update(&self.round_leader.0);
        self.cut.iter().for_each(|(pubkey, hash, _)| {
            hasher.update(&pubkey.0);
            hasher.update(hash.0.as_bytes());
        });
        self.new_validators_to_bond.iter().for_each(|val| {
            hasher.update(&val.pubkey.0);
        });
        hasher.update(self.timestamp.to_le_bytes());
        hasher.update(self.parent_hash.0.as_bytes());
        ConsensusProposalHash(hex::encode(hasher.finalize()))
    }
}

impl std::hash::Hash for ConsensusProposalHash {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        state.write(self.0.as_bytes());
    }
}

impl std::hash::Hash for ConsensusProposal {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        Hashable::<ConsensusProposalHash>::hash(self).hash(state);
    }
}

#[derive(
    Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, IntoStaticStr, Hash,
)]
pub enum ConsensusNetMessage {
    Prepare(ConsensusProposal, Ticket),
    PrepareVote(ConsensusProposalHash),
    Confirm(QuorumCertificate),
    ConfirmAck(ConsensusProposalHash),
    Commit(QuorumCertificate, ConsensusProposalHash),
    Timeout(Slot, View),
    TimeoutCertificate(QuorumCertificate, Slot, View),
    ValidatorCandidacy(ValidatorCandidacy),
}

#[cfg(test)]
mod tests {
    use crate::{
        mempool::DataProposalHash,
        model::crypto::{Signature, ValidatorSignature},
    };

    #[test]
    fn test_consensus_proposal_hash() {
        use super::*;
        let proposal = ConsensusProposal {
            slot: 1,
            view: 1,
            round_leader: ValidatorPublicKey(vec![1, 2, 3]),
            cut: Cut::default(),
            new_validators_to_bond: vec![],
            timestamp: 1,
            parent_hash: ConsensusProposalHash("".to_string()),
        };
        let hash = proposal.hash();
        assert_eq!(hash.0.len(), 64);
    }
    #[test]
    fn test_consensus_proposal_hash_ignored_fields() {
        use super::*;
        let mut a = ConsensusProposal {
            slot: 1,
            view: 1,
            round_leader: ValidatorPublicKey(vec![1, 2, 3]),
            cut: vec![(
                ValidatorPublicKey(vec![1]),
                DataProposalHash("propA".to_string()),
                AggregateSignature::default(),
            )],
            new_validators_to_bond: vec![NewValidatorCandidate {
                pubkey: ValidatorPublicKey(vec![1, 2, 3]),
                msg: SignedByValidator {
                    msg: ConsensusNetMessage::PrepareVote(ConsensusProposalHash("".to_string())),
                    signature: ValidatorSignature {
                        signature: Signature(vec![1, 2, 3]),
                        validator: ValidatorPublicKey(vec![1, 2, 3]),
                    },
                },
            }],
            timestamp: 1,
            parent_hash: ConsensusProposalHash("parent".to_string()),
        };
        let mut b = ConsensusProposal {
            slot: 1,
            view: 1,
            round_leader: ValidatorPublicKey(vec![1, 2, 3]),
            cut: vec![(
                ValidatorPublicKey(vec![1]),
                DataProposalHash("propA".to_string()),
                AggregateSignature {
                    signature: Signature(vec![1, 2, 3]),
                    validators: vec![ValidatorPublicKey(vec![1, 2, 3])],
                },
            )],
            new_validators_to_bond: vec![NewValidatorCandidate {
                pubkey: ValidatorPublicKey(vec![1, 2, 3]),
                msg: SignedByValidator {
                    msg: ConsensusNetMessage::PrepareVote(ConsensusProposalHash(
                        "differebt".to_string(),
                    )),
                    signature: ValidatorSignature {
                        signature: Signature(vec![]),
                        validator: ValidatorPublicKey(vec![]),
                    },
                },
            }],
            timestamp: 1,
            parent_hash: ConsensusProposalHash("parent".to_string()),
        };
        assert_eq!(a.hash(), b.hash());
        a.timestamp = 2;
        assert_ne!(a.hash(), b.hash());
        b.timestamp = 2;
        assert_eq!(a.hash(), b.hash());

        a.slot = 2;
        assert_ne!(a.hash(), b.hash());
        b.slot = 2;
        assert_eq!(a.hash(), b.hash());

        a.view = 2;
        assert_ne!(a.hash(), b.hash());
        b.view = 2;
        assert_eq!(a.hash(), b.hash());

        a.round_leader = ValidatorPublicKey(vec![4, 5, 6]);
        assert_ne!(a.hash(), b.hash());
        b.round_leader = ValidatorPublicKey(vec![4, 5, 6]);
        assert_eq!(a.hash(), b.hash());

        a.parent_hash = ConsensusProposalHash("different".to_string());
        assert_ne!(a.hash(), b.hash());
        b.parent_hash = ConsensusProposalHash("different".to_string());
        assert_eq!(a.hash(), b.hash());
    }
}
