// use rand::{distributions::Alphanumeric, Rng};
use derive_more::Display;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    io::Write,
    ops::{Add, Deref},
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::debug;

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct TxHash(pub Vec<u8>);

impl TxHash {
    pub fn new(s: &str) -> TxHash {
        TxHash(s.into())
    }
}

impl Display for TxHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0))
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct BlobsHash(pub Vec<u8>);

impl BlobsHash {
    pub fn new(s: &str) -> BlobsHash {
        BlobsHash(s.as_bytes().to_vec())
    }

    pub fn from_vec(vec: &Vec<Blob>) -> BlobsHash {
        debug!("From vec {:?}", vec);
        let concatenated = vec
            .iter()
            .flat_map(|b| b.data.0.clone())
            .collect::<Vec<u8>>();
        Self::from_concatenated(&concatenated)
    }

    pub fn from_concatenated(vec: &Vec<u8>) -> BlobsHash {
        debug!("From concatenated {:?}", vec);
        let mut hasher = Sha3_256::new();
        hasher.update(vec.as_slice());
        BlobsHash(hasher.finalize().as_slice().to_owned())
    }
}

impl Display for BlobsHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.0))
    }
}

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Display, Copy)]
pub struct BlockHeight(pub u64);

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Display)]
pub struct BlobIndex(pub u32);

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Display)]
pub struct Identity(pub String);

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Display)]
pub struct ContractName(pub String);

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct StateDigest(pub Vec<u8>);

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct BlobData(pub Vec<u8>);

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Transaction {
    pub version: u32,
    pub transaction_data: TransactionData,
    pub inner: String, // FIXME: to remove
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub enum TransactionData {
    Blob(BlobTransaction),
    Proof(ProofTransaction),
    RegisterContract(RegisterContractTransaction),
}

impl Default for TransactionData {
    fn default() -> Self {
        TransactionData::Blob(BlobTransaction::default())
    }
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct ProofTransaction {
    pub blobs_references: Vec<BlobReference>,
    pub proof: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct BlobReference {
    pub contract_name: ContractName,
    pub blob_tx_hash: TxHash,
    pub blob_index: BlobIndex,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct RegisterContractTransaction {
    pub owner: String,
    pub verifier: String,
    pub program_id: Vec<u8>,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct BlobTransaction {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct Blob {
    pub contract_name: ContractName,
    pub data: BlobData,
}

impl Transaction {
    // FIXME:
    pub fn as_bytes(&self) -> &[u8] {
        self.inner.as_bytes()
    }
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct BlockHash {
    pub inner: Vec<u8>,
}

impl BlockHash {
    pub fn new(s: &str) -> BlockHash {
        BlockHash {
            inner: s.as_bytes().to_vec(),
        }
    }
}

impl Deref for BlockHash {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        return self.inner.deref();
    }
}

impl Display for BlockHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(&self.inner))
    }
}

impl std::fmt::Debug for BlockHash {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:x?}", &self.inner)
    }
}

pub trait Hashable<T> {
    fn hash(&self) -> T;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Block {
    pub parent_hash: BlockHash,
    pub height: BlockHeight,
    pub timestamp: u64,
    pub txs: Vec<Transaction>,
}

impl Hashable<BlockHash> for Block {
    fn hash(&self) -> BlockHash {
        let mut hasher = Sha3_256::new();
        hasher.update(self.parent_hash.deref());
        _ = write!(hasher, "{}", self.height);
        _ = write!(hasher, "{}", self.timestamp);
        for tx in self.txs.iter() {
            // FIXME:
            hasher.update(tx.inner.as_bytes());
        }
        return BlockHash {
            inner: hasher.finalize().as_slice().to_owned(),
        };
    }
}

impl Hashable<TxHash> for BlobTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.identity.0);
        hasher.update(self.blobs_hash().0);
        return TxHash(hasher.finalize().as_slice().to_owned());
    }
}

impl BlobTransaction {
    pub fn blobs_hash(&self) -> BlobsHash {
        BlobsHash::from_vec(&self.blobs)
    }
}

impl std::default::Default for Block {
    fn default() -> Self {
        Block {
            parent_hash: BlockHash::default(),
            height: BlockHeight(0),
            timestamp: get_current_timestamp(),
            txs: vec![],
        }
    }
}

impl Add<BlockHeight> for u64 {
    type Output = BlockHeight;
    fn add(self, other: BlockHeight) -> BlockHeight {
        BlockHeight(self + other.0)
    }
}

impl Add<u64> for BlockHeight {
    type Output = BlockHeight;
    fn add(self, other: u64) -> BlockHeight {
        BlockHeight(self.0 + other)
    }
}

impl Add<BlockHeight> for BlockHeight {
    type Output = BlockHeight;
    fn add(self, other: BlockHeight) -> BlockHeight {
        BlockHeight(self.0 + other.0)
    }
}

pub fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use crate::model::BlockHeight;

    proptest! {
        #[test]
        fn block_height_add(x in 0u64..10000, y in 0u64..10000) {
            let b = BlockHeight(x);
            let c = BlockHeight(y);

            assert_eq!((b + c).0, (c + b).0);
            assert_eq!((b + y).0, x + y);
            assert_eq!((y + b).0, x + y);
        }
    }
}
