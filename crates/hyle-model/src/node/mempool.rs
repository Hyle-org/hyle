use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use staking::ValidatorPublicKey;
use std::{fmt::Display, sync::RwLock};

pub mod rwlock_borsh {
    use std::sync::RwLock;

    pub fn serialize<T: borsh::ser::BorshSerialize, W: borsh::io::Write>(
        obj: &RwLock<T>,
        writer: &mut W,
    ) -> ::core::result::Result<(), borsh::io::Error> {
        #[allow(
            clippy::unwrap_used,
            reason = "Cannot panic in sync code unless poisoned where panic is OK"
        )]
        borsh::BorshSerialize::serialize(&*obj.read().unwrap(), writer)?;
        Ok(())
    }

    pub fn deserialize<R: borsh::io::Read, T: borsh::de::BorshDeserialize>(
        reader: &mut R,
    ) -> ::core::result::Result<RwLock<T>, borsh::io::Error> {
        Ok(RwLock::new(T::deserialize_reader(reader)?))
    }
}

use crate::*;

#[derive(Debug, Default, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub struct DataProposal {
    pub parent_data_proposal_hash: Option<DataProposalHash>,
    pub txs: Vec<Transaction>,
    /// Internal cache of the hash of the transaction
    #[borsh(
        serialize_with = "rwlock_borsh::serialize",
        deserialize_with = "rwlock_borsh::deserialize"
    )]
    hash_cache: RwLock<Option<DataProposalHash>>,
}

impl DataProposal {
    pub fn new(parent_data_proposal_hash: Option<DataProposalHash>, txs: Vec<Transaction>) -> Self {
        Self {
            parent_data_proposal_hash,
            txs,
            hash_cache: RwLock::new(None),
        }
    }
}

impl Clone for DataProposal {
    fn clone(&self) -> Self {
        let mut new = Self::default();
        new.parent_data_proposal_hash = self.parent_data_proposal_hash.clone();
        new.txs = self.txs.clone();
        new.hash_cache = RwLock::new(self.hash_cache.read().unwrap().clone());
        new
    }
}

impl PartialEq for DataProposal {
    fn eq(&self, other: &Self) -> bool {
        self.hash() == other.hash()
    }
}

impl Eq for DataProposal {}

impl DataSized for DataProposal {
    fn estimate_size(&self) -> usize {
        self.txs.iter().map(|tx| tx.estimate_size()).sum()
    }
}

#[derive(
    Default,
    Serialize,
    Deserialize,
    Debug,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
    BorshDeserialize,
    BorshSerialize,
)]
pub struct TxId(pub DataProposalHash, pub TxHash);

#[derive(
    Clone,
    Debug,
    Default,
    Serialize,
    Deserialize,
    BorshSerialize,
    BorshDeserialize,
    PartialEq,
    Eq,
    Hash,
    Ord,
    PartialOrd,
)]
#[cfg_attr(feature = "full", derive(utoipa::ToSchema))]
pub struct DataProposalHash(pub String);

impl Hashable<DataProposalHash> for DataProposal {
    fn invalidate_cache(&self) {
        let mut guard = self.hash_cache.write().unwrap();
        *guard = None;
    }

    fn hash(&self) -> DataProposalHash {
        if let Some(hash) = self.hash_cache.read().unwrap().as_ref() {
            return hash.clone();
        }
        let mut hasher = Sha3_256::new();
        if let Some(ref parent_data_proposal_hash) = self.parent_data_proposal_hash {
            hasher.update(parent_data_proposal_hash.0.as_bytes());
        }
        for tx in self.txs.iter() {
            hasher.update(tx.hash().0);
        }
        let hash = DataProposalHash(hex::encode(hasher.finalize()));
        *self.hash_cache.write().unwrap() = Some(hash.clone());
        hash
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

pub type PoDA = AggregateSignature;
pub type Cut = Vec<(ValidatorPublicKey, DataProposalHash, LaneBytesSize, PoDA)>;

impl Display for TxId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", &self.0 .0, &self.1 .0)
    }
}
