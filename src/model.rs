// use rand::{distributions::Alphanumeric, Rng};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::{
    fmt::Display,
    io::Write,
    ops::Deref,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct TxHash(String);

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct BlockHeight(u64);

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct BlobIndex(u32);

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct Identity(String);

#[derive(Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct ContractName(String);

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
    pub tx_hash: TxHash,
    pub contract_name: ContractName,
    pub blob_index: BlobIndex,
    pub proof: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone)]
pub struct RegisterContractTransaction {
    pub owner: String,
    pub verifier: String,
    pub program_id: Vec<u8>,
    pub state_digest: Vec<u8>,
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
    pub data: Vec<u8>,
}

impl Transaction {
    // FIXME:
    pub fn as_bytes(&self) -> &[u8] {
        self.inner.as_bytes()
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct BlockHash {
    pub inner: Vec<u8>,
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

pub trait Hashable {
    fn hash(&self) -> BlockHash;
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Block {
    pub parent_hash: BlockHash,
    pub height: usize,
    pub timestamp: u64,
    pub txs: Vec<Transaction>,
}

impl Hashable for Block {
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

impl std::default::Default for Block {
    fn default() -> Self {
        Block {
            parent_hash: BlockHash::default(),
            height: 0,
            timestamp: get_current_timestamp(),
            txs: vec![],
        }
    }
}

pub fn get_current_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("Time went backwards")
        .as_secs()
}
