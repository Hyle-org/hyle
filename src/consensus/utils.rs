use sha3::{Digest, Sha3_256};
use std::{
    fmt::Display,
    io::Write,
    ops::{Deref, DerefMut},
};

use crate::model::Hashable;

use super::{
    Consensus, ConsensusNetMessage, ConsensusProposal, ConsensusProposalHash, ConsensusStore,
    QuorumCertificate, QuorumCertificateHash, ValidatorCandidacy,
};

impl Hashable<QuorumCertificateHash> for QuorumCertificate {
    fn hash(&self) -> QuorumCertificateHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{:?}", self.signature);
        _ = write!(hasher, "{:?}", self.validators);
        return QuorumCertificateHash(hasher.finalize().as_slice().to_owned());
    }
}

impl Hashable<ConsensusProposalHash> for ConsensusProposal {
    fn hash(&self) -> ConsensusProposalHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.slot);
        _ = write!(hasher, "{}", self.view);
        _ = write!(hasher, "{:?}", self.previous_consensus_proposal_hash);
        _ = write!(hasher, "{:?}", self.previous_commit_quorum_certificate);
        _ = write!(hasher, "{}", self.block);
        return ConsensusProposalHash(hasher.finalize().as_slice().to_owned());
    }
}
impl Display for ValidatorCandidacy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pubkey: {}", self.pubkey)
    }
}
impl Display for ConsensusProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Hash: {}, Slot: {}, View: {}, Previous hash: {}, Previous commit hash: {}, Block: {}",
            self.hash(),
            self.slot,
            self.view,
            self.previous_consensus_proposal_hash,
            self.previous_commit_quorum_certificate.hash(),
            self.block
        )
    }
}

pub const HASH_DISPLAY_SIZE: usize = 3;
impl Display for ConsensusProposalHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.len() > HASH_DISPLAY_SIZE {
            write!(f, "{}", hex::encode(&self.0[..HASH_DISPLAY_SIZE]))
        } else {
            write!(f, "{}", hex::encode(&self.0))
        }
    }
}
impl Display for QuorumCertificateHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.len() > HASH_DISPLAY_SIZE {
            write!(f, "{}", hex::encode(&self.0[..HASH_DISPLAY_SIZE]))
        } else {
            write!(f, "{}", hex::encode(&self.0))
        }
    }
}

impl Display for ConsensusNetMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let enum_variant: &'static str = self.into();

        match self {
            ConsensusNetMessage::StartNewSlot => {
                write!(f, "{}", enum_variant)
            }
            ConsensusNetMessage::Prepare(cp) => {
                write!(f, "{} CP {}", enum_variant, cp)
            }
            ConsensusNetMessage::PrepareVote(cphash) => {
                write!(f, "{} CP hash {}", enum_variant, cphash)
            }
            ConsensusNetMessage::Confirm(cphash, cert) => {
                _ = writeln!(f, "{} CP hash {}", enum_variant, cphash);
                _ = write!(f, "Certificate {} with validators ", cert.signature);
                for v in cert.validators.iter() {
                    _ = write!(f, "{},", v);
                }
                write!(f, "")
            }
            ConsensusNetMessage::ConfirmAck(cphash) => {
                write!(f, "{} CP hash {}", enum_variant, cphash)
            }
            ConsensusNetMessage::Commit(cphash, cert) => {
                _ = writeln!(f, "{} CP hash {}", enum_variant, cphash);
                _ = write!(f, "Certificate {} with validators ", cert.signature);
                for v in cert.validators.iter() {
                    _ = write!(f, "{},", v);
                }
                write!(f, "")
            }
            ConsensusNetMessage::ValidatorCandidacy(candidacy) => {
                write!(f, "{} CP hash {}", enum_variant, candidacy)
            }
        }
    }
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
