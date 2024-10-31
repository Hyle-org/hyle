use sha3::{Digest, Sha3_256};
use std::{
    fmt::Display,
    io::Write,
    ops::{Deref, DerefMut},
};

use crate::model::Hashable;

use super::{
    Consensus, ConsensusNetMessage, ConsensusProposal, ConsensusProposalHash, ConsensusStore,
    QuorumCertificate, QuorumCertificateHash, Ticket, ValidatorCandidacy,
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
        _ = write!(hasher, "{:?}", self.cut);
        _ = write!(hasher, "{:?}", self.new_validators_to_bond);
        return ConsensusProposalHash(hasher.finalize().as_slice().to_owned());
    }
}
impl Display for ValidatorCandidacy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pubkey: {}", self.pubkey)
    }
}

impl Display for Ticket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Ticket: {:?}", self)
    }
}

impl Display for ConsensusProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Hash: {}, Slot: {}, View: {}, Cut: {:?}, new_validators_to_bond: {:?}",
            self.hash(),
            self.slot,
            self.view,
            self.cut,
            self.new_validators_to_bond,
        )
    }
}

pub const HASH_DISPLAY_SIZE: usize = 3;
impl Display for ConsensusProposalHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            &hex::encode(self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0))
        )
    }
}
impl Display for QuorumCertificateHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            hex::encode(self.0.get(..HASH_DISPLAY_SIZE).unwrap_or(&self.0))
        )
    }
}

impl Display for ConsensusNetMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let enum_variant: &'static str = self.into();

        match self {
            ConsensusNetMessage::Prepare(cp, ticket) => {
                write!(f, "{} CP: {}, ticket: {}", enum_variant, cp, ticket)
            }
            ConsensusNetMessage::PrepareVote(cphash) => {
                write!(f, "{} (CP hash: {})", enum_variant, cphash)
            }
            ConsensusNetMessage::Confirm(cert) => {
                _ = writeln!(f, "{}", enum_variant);
                _ = write!(f, "Certificate {} with validators ", cert.signature);
                for v in cert.validators.iter() {
                    _ = write!(f, "{},", v);
                }
                write!(f, "")
            }
            ConsensusNetMessage::ConfirmAck(cphash) => {
                write!(f, "{} (CP hash: {})", enum_variant, cphash)
            }
            ConsensusNetMessage::Commit(cert, cphash) => {
                _ = writeln!(f, "{} (CP hash: {})", enum_variant, cphash);
                _ = write!(f, "Certificate {} with validators ", cert.signature);
                for v in cert.validators.iter() {
                    _ = write!(f, "{},", v);
                }
                write!(f, "")
            }
            ConsensusNetMessage::ValidatorCandidacy(candidacy) => {
                write!(f, "{} (CP hash {})", enum_variant, candidacy)
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
