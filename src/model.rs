//! Various data structures

use bincode::{Decode, Encode};
use derive_more::Display;
use serde::{
    de::{self, Visitor},
    Deserialize, Serialize,
};
use sha3::{Digest, Sha3_256};
use std::{
    fmt,
    io::Write,
    ops::{Add, Deref},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::debug;

use crate::{
    bus::SharedMessageBus,
    utils::{conf::SharedConf, crypto::SharedBlstCrypto},
    validator_registry::ValidatorRegistry,
};

#[derive(Default, Clone, Eq, PartialEq, Hash, Encode, Decode)]
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

impl Serialize for TxHash {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(hex::encode(&self.0).as_str())
    }
}

impl std::fmt::Debug for TxHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("TxHash")
            .field(&hex::encode(&self.0))
            .finish()
    }
}

impl<'de> Deserialize<'de> for TxHash {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TxHashVisitor;

        impl<'de> Visitor<'de> for TxHashVisitor {
            type Value = TxHash;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a hex string representing a TxHash")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                let bytes = hex::decode(value).map_err(de::Error::custom)?;
                Ok(TxHash(bytes))
            }
        }

        deserializer.deserialize_str(TxHashVisitor)
    }
}

#[derive(Default, Clone, Serialize, Deserialize, Eq, PartialEq, Hash)]
pub struct BlobsHash(pub Vec<u8>);

impl BlobsHash {
    pub fn new(s: &str) -> BlobsHash {
        BlobsHash(s.into())
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
impl std::fmt::Debug for BlobsHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BlobsHash ")
            .field(&hex::encode(&self.0))
            .finish()
    }
}

#[derive(
    Default,
    Debug,
    Clone,
    Serialize,
    Deserialize,
    Eq,
    PartialEq,
    Hash,
    Display,
    Copy,
    Encode,
    Decode,
)]
pub struct BlockHeight(pub u64);

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Display, Encode, Decode,
)]
pub struct BlobIndex(pub u32);

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Display, Encode, Decode,
)]
pub struct Identity(pub String);

#[derive(
    Default, Debug, Clone, Serialize, Deserialize, Eq, PartialEq, Hash, Display, Encode, Decode,
)]
pub struct ContractName(pub String);

#[derive(Debug, Serialize, Deserialize, Default, Clone, Eq, PartialEq, Hash, Encode, Decode)]
pub struct StateDigest(pub Vec<u8>);

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct BlobData(pub Vec<u8>);

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct Transaction {
    pub version: u32,
    pub transaction_data: TransactionData,
    pub inner: String, // FIXME: to remove
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, Encode, Decode, Hash)]
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

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode, Hash)]
pub struct ProofTransaction {
    pub blobs_references: Vec<BlobReference>,
    pub proof: Vec<u8>,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct BlobReference {
    pub contract_name: ContractName,
    pub blob_tx_hash: TxHash,
    pub blob_index: BlobIndex,
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct RegisterContractTransaction {
    pub owner: String,
    pub verifier: String,
    pub program_id: Vec<u8>,
    pub state_digest: StateDigest,
    pub contract_name: ContractName,
}

#[derive(Debug, Serialize, Deserialize, Default, PartialEq, Eq, Clone, Encode, Decode, Hash)]
pub struct BlobTransaction {
    pub identity: Identity,
    pub blobs: Vec<Blob>,
    // FIXME: add a nonce or something to prevent BlobTransaction to share the same hash
}

#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq, Eq, Encode, Decode, Hash)]
pub struct Blob {
    pub contract_name: ContractName,
    pub data: BlobData,
}

impl Transaction {
    // FIXME:
    pub fn as_bytes(&self) -> &[u8] {
        self.inner.as_bytes()
    }

    pub fn wrap(data: TransactionData) -> Self {
        Transaction {
            version: 1,
            transaction_data: data,
            inner: "".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Encode, Decode, PartialEq, Eq, Hash)]
pub struct BlockHash {
    pub inner: Vec<u8>,
}

impl BlockHash {
    pub fn new(s: &str) -> BlockHash {
        BlockHash { inner: s.into() }
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
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("BlockHash ")
            .field(&hex::encode(&self.inner))
            .finish()
    }
}

pub trait Hashable<T> {
    fn hash(&self) -> T;
}

#[derive(Debug, Serialize, Deserialize, Clone, Encode, Decode, PartialEq, Eq, Hash, Display)]
#[display("")]
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
impl Hashable<TxHash> for Transaction {
    fn hash(&self) -> TxHash {
        match &self.transaction_data {
            TransactionData::Blob(tx) => tx.hash(),
            TransactionData::Proof(tx) => tx.hash(),
            TransactionData::RegisterContract(tx) => tx.hash(),
        }
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
impl Hashable<TxHash> for ProofTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        for blob_ref in self.blobs_references.iter() {
            _ = write!(hasher, "{}", blob_ref.contract_name);
            _ = write!(hasher, "{}", blob_ref.blob_tx_hash);
            _ = write!(hasher, "{}", blob_ref.blob_index);
        }
        hasher.update(self.proof.clone());
        return TxHash(hasher.finalize().as_slice().to_owned());
    }
}
impl Hashable<TxHash> for RegisterContractTransaction {
    fn hash(&self) -> TxHash {
        let mut hasher = Sha3_256::new();
        _ = write!(hasher, "{}", self.owner);
        _ = write!(hasher, "{}", self.verifier);
        hasher.update(self.program_id.clone());
        hasher.update(self.state_digest.0.clone());
        _ = write!(hasher, "{}", self.contract_name);
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

pub struct RunContext {
    pub config: SharedConf,
    pub bus: SharedMessageBus,
    pub crypto: SharedBlstCrypto,
    pub validator_registry: ValidatorRegistry,
}
pub type SharedRunContext = Arc<RunContext>;

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
