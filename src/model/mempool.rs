use bincode::{Decode, Encode};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::fmt::Display;

use super::crypto::AggregateSignature;
use crate::model::{Hashable, Transaction};
use staking::model::ValidatorPublicKey;

#[derive(Clone, Debug, Default, Serialize, Deserialize, Encode, Decode, Eq, PartialEq)]
pub struct DataProposal {
    pub id: u32,
    pub parent_data_proposal_hash: Option<DataProposalHash>,
    pub txs: Vec<Transaction>,
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

pub type PoDA = AggregateSignature;
pub type Cut = Vec<(ValidatorPublicKey, DataProposalHash, PoDA)>;
