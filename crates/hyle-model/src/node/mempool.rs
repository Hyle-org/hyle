use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use staking::ValidatorPublicKey;
use std::fmt::Display;

use crate::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize, Encode, Decode, Eq, PartialEq)]
pub struct DataProposal {
    pub parent_data_proposal_hash: Option<DataProposalHash>,
    pub txs: Vec<Transaction>,
}

impl DataSized for DataProposal {
    fn estimate_size(&self) -> usize {
        self.txs.iter().map(|tx| tx.estimate_size()).sum()
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, Encode, Decode, PartialEq, Eq, Hash)]
pub struct DataProposalHash(pub String);

impl Hashable<DataProposalHash> for DataProposal {
    fn hash(&self) -> DataProposalHash {
        let mut hasher = Sha3_256::new();
        if let Some(ref parent_data_proposal_hash) = self.parent_data_proposal_hash {
            hasher.update(parent_data_proposal_hash.0.as_bytes());
        }
        for tx in self.txs.iter() {
            hasher.update(tx.hash().0);
        }
        DataProposalHash(hex::encode(hasher.finalize()))
    }
}
impl Display for DataProposalHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl Display for DataProposal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hash())
    }
}

// Cumulative size of the lane from the beginning
#[derive(Debug, Clone, Copy, Default, Encode, Decode, Eq, PartialEq, Serialize, Deserialize)]
pub struct LaneBytesSize(pub u64); // 16M Terabytes, is it enough ?

impl std::ops::Add<usize> for LaneBytesSize {
    type Output = Self;
    fn add(self, other: usize) -> Self {
        LaneBytesSize(self.0 + other as u64)
    }
}

impl Display for LaneBytesSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0 < 1024 {
            write!(f, "{} B", self.0)
        } else if self.0 < 1024 * 1024 {
            write!(f, "{} KB", self.0 / 1024)
        } else if self.0 < 1024 * 1024 * 1024 {
            write!(f, "{} MB", self.0 / (1024 * 1024))
        } else if self.0 < 1024 * 1024 * 1024 * 1024 {
            write!(f, "{} GB", self.0 / (1024 * 1024 * 1024))
        } else {
            write!(f, "{} TB", self.0 / (1024 * 1024 * 1024 * 1024))
        }
    }
}

pub type PoDA = AggregateSignature;
pub type Cut = Vec<(ValidatorPublicKey, DataProposalHash, LaneBytesSize, PoDA)>;
